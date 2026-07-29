#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::util::fast_map::fast_hash_map_new;
use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::{
    stmt::import::ImportStmt,
    val::{HeapRef, RuntimeMapKey, ShortStr},
};

use super::{
    ConstHeapValue, ConstPool, ConstRuntimeValue, Function, GlobalSlot, Instr, Module, analysis::PerformanceFacts,
};

// Version 4: `FunctionData.performance` is serialized (previously `#[serde(skip)]`).
// Facts are semantically required by the executor (`ForLoopI`, `GetIndexStrI` /
// `SetIndexStrI`, compare-test branch targets), so version-3 artifacts containing
// those opcodes hung or failed at runtime and are rejected instead of half-working.
// Version 5: the per-pc fact tables use a sparse `(len, [(index, value)])`
// encoding and the JSON is compact (a size fix — dense null tables made even
// tiny artifacts kilobytes large).
// Version 6: the `CallMethodK` opcode (boxing-free positional method calls)
// joins the instruction encoding; older runtimes would mis-decode it.
// Version 7: `FunctionData.debug_name` carries the source function name for
// diagnostics/tracebacks (serde-defaulted, but the version bump keeps stale
// artifacts cleanly rejected rather than silently name-less).
// Version 8: the `Yield` opcode (coroutines) joins the instruction encoding;
// older runtimes would mis-decode it, same reasoning as `CallMethodK`.
// Version 9: `Yield` removed again (v2 direction: coroutines/`yield` dropped
// in favor of Go-style go/spawn concurrency) — v8 artifacts may contain an
// opcode this runtime no longer decodes.
// Version 10: `ModuleData.type_info` carries the compiler's `trait`/`impl`
// declarations (see `super::TypeInfo`). Back ends read them instead of
// reconstructing them from bytecode, so a v9 artifact would leave a v10
// consumer with an empty table rather than a wrong one — still a semantic
// difference, hence the bump.
// Version 11: `ModuleData.type_scope` carries the identity of the module as a
// declarer of types (see `super::TypeScope`). A v10 artifact has no scope, so a
// v11 consumer would file every one of its declared types under the anonymous
// scope and collide them with the host program's — exactly the wrong-dispatch
// bug the scope exists to close, hence a rejection rather than a default.
// Version 12: `ImplMethod.writes_globals` decides whether a method may be
// dispatched from another module's frame. It defaults to `false` on decode, and
// `false` is the *permissive* answer — a v11 artifact would let a
// global-writing method run against a temporary copy of its module's globals
// and silently drop the write, so this one cannot degrade quietly either.
// Version 15: `TypeInfo.structs` carries each `struct`'s field names in
// declaration order, which is what `display` prints an instance's fields in. It
// decodes to empty, and empty means "fall back to sorting by name" — so a v14
// artifact would print its structs in a different order than the source it was
// built from. Cosmetic, but a golden-output comparison is not.
pub const MODULE_ARTIFACT_VERSION: u32 = 15;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModuleArtifact {
    pub format: String,
    pub version: u32,
    pub imports: Vec<ImportStmt>,
    pub module: ModuleData,
}

impl ModuleArtifact {
    pub fn new(imports: Vec<ImportStmt>, module: &Module) -> Result<Self> {
        if !module.natives.is_empty() {
            bail!("Module artifact cannot encode inline native entries");
        }
        Ok(Self {
            format: "lk.module".to_string(),
            version: MODULE_ARTIFACT_VERSION,
            imports,
            module: ModuleData::from_module(module),
        })
    }

    pub fn to_json_string(&self) -> Result<String> {
        serde_json::to_string(self).map_err(Into::into)
    }

    pub fn from_json_str(input: &str) -> Result<Self> {
        let artifact: Self = serde_json::from_str(input)?;
        artifact.validate()?;
        Ok(artifact)
    }

    pub fn into_module(self) -> Result<Module> {
        self.validate()?;
        let module = self.module.into_module()?;
        // Artifacts are untrusted external input. Instruction words are already
        // opcode-validated by `Instr::try_from_raw`, but register indices, jump
        // targets, const-pool/function/global indices, and deserialized
        // performance facts must also be proven in-bounds before the executor's
        // release-mode unchecked paths run over them.
        super::verify::verify_module(&module)?;
        Ok(module)
    }

    fn validate(&self) -> Result<()> {
        if self.format != "lk.module" {
            bail!("unsupported LK module artifact format `{}`", self.format);
        }
        if self.version != MODULE_ARTIFACT_VERSION {
            bail!(
                "unsupported LK module artifact version {}, expected {}",
                self.version,
                MODULE_ARTIFACT_VERSION
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModuleData {
    pub entry: u32,
    pub globals: Vec<String>,
    pub functions: Vec<FunctionData>,
    /// Static `trait`/`impl` declarations from the compiler; see
    /// [`super::TypeInfo`]. Defaulted so a module without declarations costs
    /// nothing in the encoding.
    #[serde(default, skip_serializing_if = "super::TypeInfo::is_empty")]
    pub type_info: super::TypeInfo,
    /// Identity of this module as a declarer of types; see
    /// [`super::TypeScope`]. Not skippable — an absent scope would silently
    /// mean "anonymous", which is a *different* type identity, not a missing
    /// one.
    #[serde(default)]
    pub type_scope: super::TypeScope,
}

impl ModuleData {
    fn from_module(module: &Module) -> Self {
        let mut globals = Vec::with_capacity(module.globals.len());
        for slot in &module.globals {
            globals.push(slot.name.to_string());
        }
        let mut functions = Vec::with_capacity(module.functions.len());
        for function in &module.functions {
            functions.push(FunctionData::from_function(function));
        }
        Self {
            entry: module.entry,
            globals,
            functions,
            type_info: module.type_info.clone(),
            type_scope: module.type_scope.clone(),
        }
    }

    fn into_module(self) -> Result<Module> {
        let mut functions = Vec::with_capacity(self.functions.len());
        for function in self.functions {
            functions.push(function.into_function()?);
        }
        if self.entry as usize >= functions.len() {
            bail!(
                "Module artifact entry {} out of bounds for {} functions",
                self.entry,
                functions.len()
            );
        }
        // Impl-method indices are the runtime dispatch table's only link to a
        // function body: an out-of-range one from a corrupt or hand-edited
        // artifact would surface as a panic (or a call to the wrong body) at
        // the first method dispatch, far from the decode that admitted it.
        for decl in &self.type_info.impls {
            for method in &decl.methods {
                if method.function as usize >= functions.len() {
                    bail!(
                        "Module artifact impl method '{}::{}' function {} out of bounds for {} functions",
                        decl.type_name,
                        method.name,
                        method.function,
                        functions.len()
                    );
                }
            }
        }
        Ok(Module {
            type_info: self.type_info,
            type_scope: self.type_scope,
            functions,
            natives: Vec::new(),
            globals: {
                let mut globals = Vec::with_capacity(self.globals.len());
                for name in self.globals {
                    globals.push(GlobalSlot {
                        name: Arc::<str>::from(name),
                    });
                }
                globals
            },
            entry: self.entry,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionData {
    pub consts: ConstPoolData,
    pub code: Vec<u32>,
    #[serde(default)]
    pub performance: PerformanceFacts,
    pub register_count: u16,
    pub param_count: u16,
    pub positional_param_count: u16,
    pub param_names: Vec<String>,
    pub capture_count: u16,
    #[serde(default)]
    pub debug_name: Option<String>,
    /// `#[export("name")]`'s symbol, carried across the bytecode boundary so a
    /// precompiled artifact still tells the native backend what to export.
    /// `#[serde(default)]`: an artifact written before this field simply has
    /// no exports, which is what it meant.
    #[serde(default)]
    pub export_name: Option<String>,
    /// `#[extern("name")]`'s symbol. `#[serde(default)]`: an artifact written
    /// before this field had no external implementations, which is what its
    /// absence means.
    #[serde(default)]
    pub extern_name: Option<String>,
}

impl FunctionData {
    fn from_function(function: &Function) -> Self {
        let mut code = Vec::with_capacity(function.code.len());
        for instr in &function.code {
            code.push(instr.raw());
        }
        let mut param_names = Vec::with_capacity(function.param_names.len());
        for name in &function.param_names {
            param_names.push(name.to_string());
        }
        Self {
            consts: ConstPoolData::from_pool(&function.consts),
            code,
            performance: function.performance.clone(),
            register_count: function.register_count,
            param_count: function.param_count,
            positional_param_count: function.positional_param_count,
            param_names,
            capture_count: function.capture_count,
            debug_name: function.debug_name.as_ref().map(|name| name.to_string()),
            export_name: function.export_name.as_ref().map(|name| name.to_string()),
            extern_name: function.extern_name.as_ref().map(|name| name.to_string()),
        }
    }

    fn into_function(self) -> Result<Function> {
        Ok(Function {
            consts: self.consts.into_pool()?,
            code: {
                let mut code = Vec::with_capacity(self.code.len());
                for raw in self.code {
                    code.push(Instr::try_from_raw(raw)?);
                }
                code
            },
            analyses: Vec::new(),
            performance: self.performance,
            register_count: self.register_count,
            param_count: self.param_count,
            positional_param_count: self.positional_param_count,
            param_names: {
                let mut names = Vec::with_capacity(self.param_names.len());
                for name in self.param_names {
                    names.push(Arc::<str>::from(name));
                }
                names
            },
            capture_count: self.capture_count,
            debug_name: self.debug_name.map(Arc::<str>::from),
            export_name: self.export_name.map(Arc::<str>::from),
            extern_name: self.extern_name.map(Arc::<str>::from),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConstPoolData {
    pub ints: Vec<i64>,
    pub floats: Vec<f64>,
    pub strings: Vec<String>,
    pub heap_values: Vec<ConstHeapValueData>,
}

impl ConstPoolData {
    fn from_pool(pool: &ConstPool) -> Self {
        let mut heap_values = Vec::with_capacity(pool.heap_values.len());
        for value in &pool.heap_values {
            heap_values.push(ConstHeapValueData::from_heap_value(value));
        }
        Self {
            ints: pool.ints.clone(),
            floats: pool.floats.clone(),
            strings: pool.strings.clone(),
            heap_values,
        }
    }

    fn into_pool(self) -> Result<ConstPool> {
        Ok(ConstPool {
            ints: self.ints,
            floats: self.floats,
            strings: self.strings,
            heap_values: {
                let mut values = Vec::with_capacity(self.heap_values.len());
                for value in self.heap_values {
                    values.push(value.into_heap_value()?);
                }
                values
            },
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ConstRuntimeValueData {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    ShortStr(String),
    Heap(Box<ConstHeapValueData>),
}

impl ConstRuntimeValueData {
    fn from_runtime_value(value: &ConstRuntimeValue) -> Self {
        match value {
            ConstRuntimeValue::Nil => Self::Nil,
            ConstRuntimeValue::Bool(value) => Self::Bool(*value),
            ConstRuntimeValue::Int(value) => Self::Int(*value),
            ConstRuntimeValue::Float(value) => Self::Float(*value),
            ConstRuntimeValue::ShortStr(value) => Self::ShortStr(value.as_str().to_string()),
            ConstRuntimeValue::Heap(value) => Self::Heap(Box::new(ConstHeapValueData::from_heap_value(value))),
        }
    }

    fn into_runtime_value(self) -> Result<ConstRuntimeValue> {
        Ok(match self {
            Self::Nil => ConstRuntimeValue::Nil,
            Self::Bool(value) => ConstRuntimeValue::Bool(value),
            Self::Int(value) => ConstRuntimeValue::Int(value),
            Self::Float(value) => ConstRuntimeValue::Float(value),
            Self::ShortStr(value) => ConstRuntimeValue::ShortStr(
                ShortStr::new(&value).ok_or_else(|| anyhow!("artifact short string exceeds inline limit"))?,
            ),
            Self::Heap(value) => ConstRuntimeValue::Heap(Box::new(value.into_heap_value()?)),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ConstHeapValueData {
    LongString(String),
    List(Vec<ConstRuntimeValueData>),
    Map(Vec<(RuntimeMapKeyData, ConstRuntimeValueData)>),
    UpvalCell(Box<ConstRuntimeValueData>),
}

impl ConstHeapValueData {
    fn from_heap_value(value: &ConstHeapValue) -> Self {
        match value {
            ConstHeapValue::LongString(value) => Self::LongString(value.to_string()),
            ConstHeapValue::List(values) => {
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    out.push(ConstRuntimeValueData::from_runtime_value(value));
                }
                Self::List(out)
            }
            ConstHeapValue::Map(values) => {
                let mut out = Vec::with_capacity(values.len());
                for (key, value) in values {
                    out.push((
                        RuntimeMapKeyData::from_runtime_key(key),
                        ConstRuntimeValueData::from_runtime_value(value),
                    ));
                }
                Self::Map(out)
            }
            ConstHeapValue::UpvalCell(value) => {
                Self::UpvalCell(Box::new(ConstRuntimeValueData::from_runtime_value(value)))
            }
        }
    }

    fn into_heap_value(self) -> Result<ConstHeapValue> {
        Ok(match self {
            Self::LongString(value) => ConstHeapValue::LongString(Arc::<str>::from(value)),
            Self::List(values) => {
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    out.push(value.into_runtime_value()?);
                }
                ConstHeapValue::List(out)
            }
            Self::Map(values) => {
                let mut map = fast_hash_map_new();
                for (key, value) in values {
                    map.insert(key.into_runtime_key()?, value.into_runtime_value()?);
                }
                ConstHeapValue::Map(map)
            }
            Self::UpvalCell(value) => ConstHeapValue::UpvalCell(Box::new(value.into_runtime_value()?)),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RuntimeMapKeyData {
    Nil,
    Bool(bool),
    Int(i64),
    ShortStr(String),
    String(String),
    Obj(u32),
}

impl RuntimeMapKeyData {
    fn from_runtime_key(key: &RuntimeMapKey) -> Self {
        match key {
            RuntimeMapKey::Nil => Self::Nil,
            RuntimeMapKey::Bool(value) => Self::Bool(*value),
            RuntimeMapKey::Int(value) => Self::Int(*value),
            RuntimeMapKey::ShortStr(value) => Self::ShortStr(value.as_str().to_string()),
            RuntimeMapKey::String(value) => Self::String(value.to_string()),
            RuntimeMapKey::Obj(value) => Self::Obj(value.index()),
        }
    }

    fn into_runtime_key(self) -> Result<RuntimeMapKey> {
        Ok(match self {
            Self::Nil => RuntimeMapKey::Nil,
            Self::Bool(value) => RuntimeMapKey::Bool(value),
            Self::Int(value) => RuntimeMapKey::Int(value),
            Self::ShortStr(value) => RuntimeMapKey::ShortStr(
                ShortStr::new(&value).ok_or_else(|| anyhow!("artifact short string key exceeds inline limit"))?,
            ),
            Self::String(value) => RuntimeMapKey::String(Arc::<str>::from(value)),
            Self::Obj(value) => RuntimeMapKey::Obj(HeapRef::new(value)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{stmt::import::ImportSource, vm::Compiler};

    #[test]
    fn module_artifact_round_trips_compiled_module() {
        let source = "fn f(x) { return x + 1; }\nreturn f(3);\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let module = Compiler::compile_module(&program).expect("compile");
        let imports = vec![ImportStmt::Items {
            items: vec![crate::stmt::import::ImportItem {
                name: "abs".to_string(),
                alias: None,
            }],
            source: ImportSource::Module("math".to_string()),
        }];

        let artifact = ModuleArtifact::new(imports.clone(), &module).expect("artifact");
        let json = artifact.to_json_string().expect("json");
        let decoded = ModuleArtifact::from_json_str(&json).expect("decode");
        assert_eq!(decoded.imports, imports);
        let decoded_module = decoded.into_module().expect("module");

        assert_eq!(decoded_module.entry, module.entry);
        assert_eq!(decoded_module.globals, module.globals);
        assert_eq!(decoded_module.functions.len(), module.functions.len());
        assert_eq!(decoded_module.functions[0].code, module.functions[0].code);
    }

    /// The compiler's `trait`/`impl` knowledge must survive into the artifact
    /// in structured form — that is the whole point of `TypeInfo`. Before this,
    /// the only encoding was string literals inside a registration call, which
    /// every back end had to decode again.
    #[test]
    fn module_artifact_carries_trait_impl_declarations() {
        let source = "\
trait Show { fn show(self) -> String; }\n\
struct Point { x: Int }\n\
impl Show for Point { fn show(self) -> String { return \"p\"; } }\n\
return 1;\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let module = Compiler::compile_module(&program).expect("compile");

        let info = &module.type_info;
        assert_eq!(info.traits.len(), 1, "the trait declaration is recorded");
        assert_eq!(info.traits[0].name, "Show");
        assert_eq!(info.impls.len(), 1, "the impl block is recorded");
        assert_eq!(info.impls[0].trait_name, "Show");
        assert_eq!(info.impls[0].type_name, "Point");
        assert_eq!(info.impls[0].methods.len(), 1);
        assert_eq!(info.impls[0].methods[0].name, "show");
        // The method points at a real compiled body, by index — not a runtime
        // value, which is what makes this serializable.
        let target = info.impls[0].methods[0].function as usize;
        assert!(
            target < module.functions.len(),
            "method index addresses a compiled body"
        );
        assert_eq!(info.impl_method("Point", "show"), Some(target as u32));
        assert_eq!(info.impl_method("Point", "missing"), None);

        // And it round-trips through the serialized boundary unchanged.
        let artifact = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
        let json = artifact.to_json_string().expect("json");
        let decoded = ModuleArtifact::from_json_str(&json).expect("decode");
        assert_eq!(decoded.module.type_info, module.type_info);
    }

    #[test]
    fn module_artifact_rejects_previous_version() {
        assert_eq!(MODULE_ARTIFACT_VERSION, 15);
        let source = "return 1;\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let module = Compiler::compile_module(&program).expect("compile");
        let mut artifact = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
        artifact.version = MODULE_ARTIFACT_VERSION - 1;

        let json = artifact.to_json_string().expect("json");
        let err = ModuleArtifact::from_json_str(&json).expect_err("previous-version artifact should be rejected");
        // Derived, not spelled out: the literal silently went stale on the last
        // bump (it still said 10 while `- 1` had become 11) and only failed on
        // the bump after that.
        assert!(
            err.to_string().contains(&format!(
                "unsupported LK module artifact version {}",
                MODULE_ARTIFACT_VERSION - 1
            )),
            "unexpected rejection message: {err}"
        );
    }

    #[test]
    fn function_debug_name_survives_compile_and_artifact_round_trip() {
        // A named `fn` carries its source name into the bytecode (diagnostics /
        // tracebacks) and that name survives the `.lkm` serialization round trip.
        let source = "fn greet() { return 1; }\nreturn greet();\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let module = Compiler::compile_module(&program).expect("compile");

        let named = |module: &Module| {
            module
                .functions
                .iter()
                .any(|function| function.debug_name.as_deref() == Some("greet"))
        };
        assert!(named(&module), "compiled module should tag `greet` with its debug name");

        let artifact = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
        let json = artifact.to_json_string().expect("json");
        let decoded = ModuleArtifact::from_json_str(&json).expect("decode");
        let decoded_module = decoded.into_module().expect("into module");
        assert!(
            named(&decoded_module),
            "debug name should survive the artifact round trip"
        );
    }

    #[test]
    fn module_artifact_round_trips_performance_facts() {
        // For-loop facts are semantically required by `ForLoopI`; a facts-less
        // round trip previously made compiled `.lkm` for-loops fail at runtime.
        let source = "let total = 0;\nfor i in 0..10 {\n    total += i;\n}\nreturn total;\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let module = Compiler::compile_module(&program).expect("compile");
        let has_for_loop_fact = |module: &Module| {
            module
                .functions
                .iter()
                .any(|function| function.performance.for_loops.iter().any(Option::is_some))
        };
        assert!(has_for_loop_fact(&module), "probe program should compile to ForLoopI");

        let artifact = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
        let json = artifact.to_json_string().expect("json");
        let decoded = ModuleArtifact::from_json_str(&json).expect("decode");
        let decoded_module = decoded.into_module().expect("module");

        assert!(
            has_for_loop_fact(&decoded_module),
            "performance facts must survive the artifact round trip"
        );
        let result = crate::vm::execute_module(&decoded_module).expect("decoded module runs");
        assert_eq!(
            result.returns.first(),
            Some(&crate::val::RuntimeVal::Int(45)),
            "decoded module must execute the for loop correctly"
        );
    }
}
