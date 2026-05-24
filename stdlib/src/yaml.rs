use anyhow::Result;
use lk_core::{
    module::{self, Module, RuntimeNativeExport32, runtime_export_from_plain_native_entries},
    val::{RuntimeVal, de},
    vm::{NativeArgs32, NativeRuntime32, RuntimeExport32},
};

#[derive(Debug)]
pub struct YamlModule;

impl Default for YamlModule {
    fn default() -> Self {
        Self::new()
    }
}

impl YamlModule {
    pub fn new() -> Self {
        Self
    }
}

impl Module for YamlModule {
    fn name(&self) -> &str {
        "yaml"
    }

    fn register(&self, _registry: &mut module::ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport32> {
        Ok(runtime_export_from_plain_native_entries(
            &[RuntimeNativeExport32::plain("parse", parse32, 1)],
            &[],
        ))
    }
}

fn parse32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    crate::runtime_native::parse_format32(args, runtime, "yaml.parse", de::Format::Yaml)
}

#[cfg(test)]
mod tests {
    use lk_core::{
        val::{HeapValue, RuntimeVal},
        vm::{NativeArgs32, NativeFunction32, NativeRuntime32, RuntimeModuleState32},
    };

    use crate::runtime_native::runtime_string_value;

    use super::YamlModule;

    #[test]
    fn yaml_parse_exports_runtime_native32() {
        let (arity, _) =
            crate::runtime_native::runtime_native_export(&YamlModule::new(), "parse").expect("parse export");
        assert_eq!(arity, 1);
    }

    #[test]
    fn yaml_parse32_decodes_into_runtime_values() {
        let (_, function) =
            crate::runtime_native::runtime_native_export(&YamlModule::new(), "parse").expect("parse export");
        let NativeFunction32::Plain(function) = function else {
            panic!("parse must be plain RuntimeNative32");
        };

        let mut state = RuntimeModuleState32::default();
        let input = runtime_string_value("answer: 42", state.heap_mut());
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
