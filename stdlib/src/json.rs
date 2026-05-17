use anyhow::Result;
use lk_core::module::Module;
use lk_core::val::Val;
use lk_core::val::de;
use lk_core::vm::VmContext;
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
        functions.insert("parse".to_string(), Val::RustFunction(parse));
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

fn parse(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow::anyhow!("json.parse(data) requires 1 argument"));
    }
    let s = args[0]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| args[0].to_string());
    de::parse_with_format(&s, Some(de::Format::Json))
}
