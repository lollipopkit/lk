use crate::util::fast_map::fast_hash_map_new;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::{
    stmt::import::ImportStmt,
    val::{HeapRef, RuntimeMapKey, ShortStr},
};

use super::{
    ConstHeapValue, ConstPool, ConstRuntimeValue, Function, GlobalSlot, Instr, Module, analysis::PerformanceFacts,
};

pub const MODULE_ARTIFACT_VERSION: u32 = 3;

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
        serde_json::to_string_pretty(self).map_err(Into::into)
    }

    pub fn from_json_str(input: &str) -> Result<Self> {
        let artifact: Self = serde_json::from_str(input)?;
        artifact.validate()?;
        Ok(artifact)
    }

    pub fn into_module(self) -> Result<Module> {
        self.validate()?;
        self.module.into_module()
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
        Ok(Module {
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
    #[serde(skip)]
    pub performance: PerformanceFacts,
    pub register_count: u16,
    pub param_count: u16,
    pub positional_param_count: u16,
    pub param_names: Vec<String>,
    pub capture_count: u16,
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
}
