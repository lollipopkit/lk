use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::{
    stmt::import::ImportStmt,
    val::{HeapRef, RuntimeMapKey, ShortStr},
};

use super::{ConstHeapValue32, ConstPool32, ConstRuntimeValue32, Function32, GlobalSlot32, Instr32, Module32};

pub const MODULE32_ARTIFACT_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Module32Artifact {
    pub format: String,
    pub version: u32,
    pub imports: Vec<ImportStmt>,
    pub module: Module32Data,
}

impl Module32Artifact {
    pub fn new(imports: Vec<ImportStmt>, module: &Module32) -> Result<Self> {
        if !module.natives.is_empty() {
            bail!("Module32 artifact cannot encode inline native entries");
        }
        Ok(Self {
            format: "lk.module32".to_string(),
            version: MODULE32_ARTIFACT_VERSION,
            imports,
            module: Module32Data::from_module(module),
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

    pub fn into_module(self) -> Result<Module32> {
        self.validate()?;
        self.module.into_module()
    }

    fn validate(&self) -> Result<()> {
        if self.format != "lk.module32" {
            bail!("unsupported LK module artifact format `{}`", self.format);
        }
        if self.version != MODULE32_ARTIFACT_VERSION {
            bail!(
                "unsupported LK module artifact version {}, expected {}",
                self.version,
                MODULE32_ARTIFACT_VERSION
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Module32Data {
    pub entry: u32,
    pub globals: Vec<String>,
    pub functions: Vec<Function32Data>,
}

impl Module32Data {
    fn from_module(module: &Module32) -> Self {
        let mut globals = Vec::with_capacity(module.globals.len());
        for slot in &module.globals {
            globals.push(slot.name.to_string());
        }
        let mut functions = Vec::with_capacity(module.functions.len());
        for function in &module.functions {
            functions.push(Function32Data::from_function(function));
        }
        Self {
            entry: module.entry,
            globals,
            functions,
        }
    }

    fn into_module(self) -> Result<Module32> {
        let mut functions = Vec::with_capacity(self.functions.len());
        for function in self.functions {
            functions.push(function.into_function()?);
        }
        if self.entry as usize >= functions.len() {
            bail!(
                "Module32 artifact entry {} out of bounds for {} functions",
                self.entry,
                functions.len()
            );
        }
        Ok(Module32 {
            functions,
            natives: Vec::new(),
            globals: {
                let mut globals = Vec::with_capacity(self.globals.len());
                for name in self.globals {
                    globals.push(GlobalSlot32 {
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
pub struct Function32Data {
    pub consts: ConstPool32Data,
    pub code: Vec<u32>,
    pub register_count: u16,
    pub param_count: u16,
    pub positional_param_count: u16,
    pub param_names: Vec<String>,
    pub capture_count: u16,
}

impl Function32Data {
    fn from_function(function: &Function32) -> Self {
        let mut code = Vec::with_capacity(function.code.len());
        for instr in &function.code {
            code.push(instr.raw());
        }
        let mut param_names = Vec::with_capacity(function.param_names.len());
        for name in &function.param_names {
            param_names.push(name.to_string());
        }
        Self {
            consts: ConstPool32Data::from_pool(&function.consts),
            code,
            register_count: function.register_count,
            param_count: function.param_count,
            positional_param_count: function.positional_param_count,
            param_names,
            capture_count: function.capture_count,
        }
    }

    fn into_function(self) -> Result<Function32> {
        Ok(Function32 {
            consts: self.consts.into_pool()?,
            code: {
                let mut code = Vec::with_capacity(self.code.len());
                for raw in self.code {
                    code.push(Instr32::try_from_raw(raw)?);
                }
                code
            },
            analyses: Vec::new(),
            performance: Default::default(),
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
pub struct ConstPool32Data {
    pub ints: Vec<i64>,
    pub floats: Vec<f64>,
    pub strings: Vec<String>,
    pub heap_values: Vec<ConstHeapValue32Data>,
}

impl ConstPool32Data {
    fn from_pool(pool: &ConstPool32) -> Self {
        let mut heap_values = Vec::with_capacity(pool.heap_values.len());
        for value in &pool.heap_values {
            heap_values.push(ConstHeapValue32Data::from_heap_value(value));
        }
        Self {
            ints: pool.ints.clone(),
            floats: pool.floats.clone(),
            strings: pool.strings.clone(),
            heap_values,
        }
    }

    fn into_pool(self) -> Result<ConstPool32> {
        Ok(ConstPool32 {
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
pub enum ConstRuntimeValue32Data {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    ShortStr(String),
    Heap(Box<ConstHeapValue32Data>),
}

impl ConstRuntimeValue32Data {
    fn from_runtime_value(value: &ConstRuntimeValue32) -> Self {
        match value {
            ConstRuntimeValue32::Nil => Self::Nil,
            ConstRuntimeValue32::Bool(value) => Self::Bool(*value),
            ConstRuntimeValue32::Int(value) => Self::Int(*value),
            ConstRuntimeValue32::Float(value) => Self::Float(*value),
            ConstRuntimeValue32::ShortStr(value) => Self::ShortStr(value.as_str().to_string()),
            ConstRuntimeValue32::Heap(value) => Self::Heap(Box::new(ConstHeapValue32Data::from_heap_value(value))),
        }
    }

    fn into_runtime_value(self) -> Result<ConstRuntimeValue32> {
        Ok(match self {
            Self::Nil => ConstRuntimeValue32::Nil,
            Self::Bool(value) => ConstRuntimeValue32::Bool(value),
            Self::Int(value) => ConstRuntimeValue32::Int(value),
            Self::Float(value) => ConstRuntimeValue32::Float(value),
            Self::ShortStr(value) => ConstRuntimeValue32::ShortStr(
                ShortStr::new(&value).ok_or_else(|| anyhow!("artifact short string exceeds inline limit"))?,
            ),
            Self::Heap(value) => ConstRuntimeValue32::Heap(Box::new(value.into_heap_value()?)),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ConstHeapValue32Data {
    LongString(String),
    List(Vec<ConstRuntimeValue32Data>),
    Map(Vec<(RuntimeMapKeyData, ConstRuntimeValue32Data)>),
    UpvalCell(Box<ConstRuntimeValue32Data>),
}

impl ConstHeapValue32Data {
    fn from_heap_value(value: &ConstHeapValue32) -> Self {
        match value {
            ConstHeapValue32::LongString(value) => Self::LongString(value.to_string()),
            ConstHeapValue32::List(values) => {
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    out.push(ConstRuntimeValue32Data::from_runtime_value(value));
                }
                Self::List(out)
            }
            ConstHeapValue32::Map(values) => {
                let mut out = Vec::with_capacity(values.len());
                for (key, value) in values {
                    out.push((
                        RuntimeMapKeyData::from_runtime_key(key),
                        ConstRuntimeValue32Data::from_runtime_value(value),
                    ));
                }
                Self::Map(out)
            }
            ConstHeapValue32::UpvalCell(value) => {
                Self::UpvalCell(Box::new(ConstRuntimeValue32Data::from_runtime_value(value)))
            }
        }
    }

    fn into_heap_value(self) -> Result<ConstHeapValue32> {
        Ok(match self {
            Self::LongString(value) => ConstHeapValue32::LongString(Arc::<str>::from(value)),
            Self::List(values) => {
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    out.push(value.into_runtime_value()?);
                }
                ConstHeapValue32::List(out)
            }
            Self::Map(values) => {
                let mut map = BTreeMap::new();
                for (key, value) in values {
                    map.insert(key.into_runtime_key()?, value.into_runtime_value()?);
                }
                ConstHeapValue32::Map(map)
            }
            Self::UpvalCell(value) => ConstHeapValue32::UpvalCell(Box::new(value.into_runtime_value()?)),
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
    use crate::{stmt::import::ImportSource, vm::Compiler32};

    #[test]
    fn module32_artifact_round_trips_compiled_module() {
        let source = "fn f(x) { return x + 1; }\nreturn f(3);\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let module = Compiler32::compile_module(&program).expect("compile");
        let imports = vec![ImportStmt::Items {
            items: vec![crate::stmt::import::ImportItem {
                name: "abs".to_string(),
                alias: None,
            }],
            source: ImportSource::Module("math".to_string()),
        }];

        let artifact = Module32Artifact::new(imports.clone(), &module).expect("artifact");
        let json = artifact.to_json_string().expect("json");
        let decoded = Module32Artifact::from_json_str(&json).expect("decode");
        assert_eq!(decoded.imports, imports);
        let decoded_module = decoded.into_module().expect("module");

        assert_eq!(decoded_module.entry, module.entry);
        assert_eq!(decoded_module.globals, module.globals);
        assert_eq!(decoded_module.functions.len(), module.functions.len());
        assert_eq!(decoded_module.functions[0].code, module.functions[0].code);
    }
}
