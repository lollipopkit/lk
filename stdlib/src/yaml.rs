use anyhow::Result;
use lk_core::{
    module::{self, Module},
    val::{RuntimeVal, Val, de},
    vm::{NativeArgs32, NativeFunction32, NativeRuntime32},
};
use std::collections::HashMap;

#[derive(Debug)]
pub struct YamlModule {
    functions: HashMap<String, Val>,
}

impl Default for YamlModule {
    fn default() -> Self {
        Self::new()
    }
}

impl YamlModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();
        functions.insert(
            "parse".to_string(),
            Val::runtime_native32(NativeFunction32::Plain(parse32), 1),
        );
        YamlModule { functions }
    }
}

impl Module for YamlModule {
    fn name(&self) -> &str {
        "yaml"
    }

    fn register(&self, _registry: &mut module::ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}

fn parse32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    crate::runtime_native::parse_format32(args, runtime, "yaml.parse", de::Format::Yaml)
}

#[cfg(test)]
mod tests {
    use lk_core::{
        module::Module,
        val::{CallableValue, HeapValue, RuntimeVal, Val},
        vm::{NativeArgs32, NativeFunction32, NativeRuntime32, RuntimeModuleState32},
    };

    use crate::runtime_native::runtime_string_value;

    use super::YamlModule;

    #[test]
    fn yaml_parse_exports_runtime_native32() {
        let exports = YamlModule::new().exports();
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
    fn yaml_parse32_decodes_into_runtime_values() {
        let exports = YamlModule::new().exports();
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

        let mut state = RuntimeModuleState32::default();
        let input = runtime_string_value("answer: 42", &mut state.heap);
        let mut runtime = NativeRuntime32::new(&mut state, None, None);
        let result = function(NativeArgs32::new(&[input]), &mut runtime).expect("parse");

        let RuntimeVal::Obj(handle) = result else {
            panic!("yaml.parse should return runtime object");
        };
        let Some(HeapValue::Map(map)) = runtime.heap().get(handle) else {
            panic!("yaml.parse should return runtime map");
        };
        assert_eq!(map.get_str("answer"), Some(RuntimeVal::Int(42)));
    }
}
