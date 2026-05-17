use anyhow::Result;
use lk_core::{
    module::{self, Module},
    val::{Val, de},
    vm::VmContext,
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
        functions.insert("parse".to_string(), Val::RustFunction(parse));
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

fn parse(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow::anyhow!("yaml.parse(data) requires 1 argument"));
    }
    let s = args[0]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| args[0].to_string());
    de::parse_with_format(&s, Some(de::Format::Yaml))
}
