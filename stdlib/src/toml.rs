use anyhow::Result;
use lk_core::{
    module::{self, Module},
    val::{RuntimeVal, Val, de},
    vm::{NativeArgs32, NativeFunction32, NativeRuntime32},
};
use std::collections::HashMap;

#[derive(Debug)]
pub struct TomlModule {
    functions: HashMap<String, Val>,
}

impl Default for TomlModule {
    fn default() -> Self {
        Self::new()
    }
}

impl TomlModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();
        functions.insert(
            "parse".to_string(),
            Val::runtime_native32(NativeFunction32::Plain(parse32), 1),
        );
        TomlModule { functions }
    }
}

impl Module for TomlModule {
    fn name(&self) -> &str {
        "toml"
    }

    fn register(&self, _registry: &mut module::ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}

fn parse32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    crate::runtime_native::parse_format32(args, runtime, "toml.parse", de::Format::Toml)
}

#[cfg(test)]
mod tests {
    use lk_core::{
        module::Module,
        val::{CallableValue, HeapStore, HeapValue, RuntimeVal, Val},
        vm::{NativeArgs32, NativeFunction32, NativeRuntime32, RuntimeModuleState32},
    };

    use crate::runtime_native::runtime_string_value;

    use super::TomlModule;

    #[test]
    fn toml_parse_exports_runtime_native32() {
        let exports = TomlModule::new().exports();
        let parse = exports.get("parse").expect("parse export");

        assert!(matches!(
            parse,
            Val::Obj(object)
                if matches!(
                    object.as_ref(),
                    HeapValue::Callable(CallableValue::RuntimeNative32 { arity: 1, .. })
                )
        ));
    }

    #[test]
    fn toml_parse32_decodes_into_runtime_values() {
        let exports = TomlModule::new().exports();
        let parse = exports.get("parse").expect("parse export");
        let Val::Obj(object) = parse else {
            panic!("parse must be heap callable");
        };
        let HeapValue::Callable(CallableValue::RuntimeNative32 {
            function: NativeFunction32::Plain(function),
            ..
        }) = object.as_ref()
        else {
            panic!("parse must be plain RuntimeNative32");
        };

        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        let input = runtime_string_value("answer = 42", &mut state.heap);
        let mut runtime = NativeRuntime32 {
            state: &mut state,
            ctx: None,
            module: None,
        };
        let result = function(NativeArgs32::new(&[input]), &mut runtime).expect("parse");

        let RuntimeVal::Obj(handle) = result else {
            panic!("toml.parse should return runtime object");
        };
        let Some(HeapValue::Map(map)) = runtime.state.heap.get(handle) else {
            panic!("toml.parse should return runtime map");
        };
        assert_eq!(map.get_str("answer"), Some(RuntimeVal::Int(42)));
    }
}
