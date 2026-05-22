use anyhow::Result;
use lk_core::module::Module;
use lk_core::val::{RuntimeVal, Val, de};
use lk_core::vm::{NativeArgs32, NativeFunction32, NativeRuntime32};
use std::collections::HashMap;

#[derive(Debug)]
pub struct JsonModule {
    functions: HashMap<String, Val>,
}

impl Default for JsonModule {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();
        functions.insert(
            "parse".to_string(),
            Val::runtime_native32(NativeFunction32::Plain(parse32), 1),
        );
        JsonModule { functions }
    }
}

impl Module for JsonModule {
    fn name(&self) -> &str {
        "json"
    }

    fn register(&self, _registry: &mut lk_core::module::ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}

fn parse32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    crate::runtime_native::parse_format32(args, runtime, "json.parse", de::Format::Json)
}

#[cfg(test)]
mod tests {
    use lk_core::{
        module::Module,
        val::{CallableValue, HeapStore, HeapValue, RuntimeVal, Val, runtime_val_to_val},
        vm::{NativeArgs32, NativeFunction32, NativeRuntime32, RuntimeModuleState32},
    };

    use crate::runtime_native::runtime_string_value;

    use super::JsonModule;

    #[test]
    fn json_parse_exports_runtime_native32() {
        let exports = JsonModule::new().exports();
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
    fn json_parse32_decodes_into_runtime_values() {
        let exports = JsonModule::new().exports();
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
        let input = runtime_string_value(r#"{"answer": 42}"#, &mut state.heap);
        let mut runtime = NativeRuntime32 {
            state: &mut state,
            ctx: None,
            module: None,
        };
        let result = function(NativeArgs32::new(&[input]), &mut runtime).expect("parse");

        let value = runtime_val_to_val(&result, &runtime.state.heap).expect("runtime to val");
        let map = value.as_map().expect("json object");
        assert_eq!(map.get("answer"), Some(&Val::Int(42)));
        assert!(matches!(result, RuntimeVal::Obj(_)));
    }
}
