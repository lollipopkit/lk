use anyhow::Result;
use lk_core::module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries};
use lk_core::val::{RuntimeVal, de};
use lk_core::vm::{NativeArgs, NativeRuntime, RuntimeExport};

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

#[derive(Debug)]
pub struct JsonModule;

impl Default for JsonModule {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleProvider for JsonModule {
    fn name(&self) -> &str {
        "json"
    }

    fn register(&self, _registry: &mut lk_core::module::ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[RuntimeNativeExport::plain("parse", parse, 1)],
            &[],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("json", Box::new(JsonModule::new()))
}

fn parse(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    crate::runtime_native::parse_format(args, runtime, "json.parse", de::Format::Json)
}

#[cfg(test)]
mod tests {
    use lk_core::{
        val::{HeapValue, RuntimeVal},
        vm::{NativeArgs, NativeFunction, NativeRuntime, RuntimeModuleState},
    };

    use crate::runtime_native::runtime_string_value;

    use super::JsonModule;

    #[test]
    fn json_parse_exports_runtime_native() {
        let (arity, _) =
            crate::runtime_native::runtime_native_export(&JsonModule::new(), "parse").expect("parse export");
        assert_eq!(arity, 1);
    }

    #[test]
    fn json_parse_decodes_into_runtime_values() {
        let (_, function) =
            crate::runtime_native::runtime_native_export(&JsonModule::new(), "parse").expect("parse export");
        let NativeFunction::Plain(function) = function else {
            panic!("parse must be plain RuntimeNative");
        };

        let mut state = RuntimeModuleState::default();
        let input = runtime_string_value(r#"{"answer": 42}"#, state.heap_mut());
        let mut runtime = NativeRuntime::new(&mut state, None, None);
        let result = function(NativeArgs::new(&[input]), &mut runtime).expect("parse");

        let RuntimeVal::Obj(handle) = result else {
            panic!("json.parse should return runtime object");
        };
        let Some(HeapValue::Map(map)) = runtime.heap().get(handle) else {
            panic!("json.parse should return runtime map");
        };
        assert_eq!(map.get_str("answer"), Some(RuntimeVal::Int(42)));
    }
}
