use anyhow::{Result, bail};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdlibExportKind {
    Function,
    Module,
    Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdlibArity {
    Fixed(u16),
    Variadic,
}

impl StdlibArity {
    pub fn display(self) -> String {
        match self {
            Self::Fixed(value) => format!("{value} args"),
            Self::Variadic => "...".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StdlibConstValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdlibReturnKind {
    Nil,
    Bool,
    Int,
    IntOrFloat,
    Float,
    String,
    RuntimeValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StdlibCallableMetadata {
    pub path: &'static str,
    pub lowering_key: &'static str,
    pub return_kind: StdlibReturnKind,
    pub signature: Option<&'static str>,
    pub docs: Option<&'static str>,
}

impl StdlibCallableMetadata {
    pub const fn new(
        path: &'static str,
        lowering_key: &'static str,
        return_kind: StdlibReturnKind,
        signature: Option<&'static str>,
        docs: Option<&'static str>,
    ) -> Self {
        Self {
            path,
            lowering_key,
            return_kind,
            signature,
            docs,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StdlibModuleMetadata {
    pub name: &'static str,
    pub docs: Option<&'static str>,
    pub callables: &'static [StdlibCallableMetadata],
}

impl StdlibModuleMetadata {
    pub const fn new(
        name: &'static str,
        docs: Option<&'static str>,
        callables: &'static [StdlibCallableMetadata],
    ) -> Self {
        Self { name, docs, callables }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StdlibGlobalMetadata {
    pub name: &'static str,
    pub lowering_key: &'static str,
    pub return_kind: StdlibReturnKind,
    pub signature: Option<&'static str>,
    pub docs: Option<&'static str>,
}

impl StdlibGlobalMetadata {
    pub const fn new(name: &'static str, lowering_key: &'static str, return_kind: StdlibReturnKind) -> Self {
        Self {
            name,
            lowering_key,
            return_kind,
            signature: None,
            docs: None,
        }
    }
}

#[macro_export]
macro_rules! stdlib_module_metadata {
    ($module:ident, [$($($path:ident).+ => $return_kind:ident),* $(,)?]) => {{
        const CALLABLES: &[$crate::metadata::StdlibCallableMetadata] = &[
            $(
                $crate::metadata::StdlibCallableMetadata::new(
                    concat!(stringify!($module), $(".", stringify!($path)),+),
                    concat!(stringify!($module), $(".", stringify!($path)),+),
                    $crate::metadata::StdlibReturnKind::$return_kind,
                    None,
                    None,
                ),
            )*
        ];
        $crate::metadata::StdlibModuleMetadata::new(stringify!($module), None, CALLABLES)
    }};
}

#[macro_export]
macro_rules! stdlib_register_module_metadata {
    ($module:ident, [$($($path:ident).+ => $return_kind:ident),* $(,)?]) => {
        $crate::metadata::register_stdlib_module_metadata(
            $crate::stdlib_module_metadata!($module, [$($($path).+ => $return_kind),*])
        )
    };
}

#[macro_export]
macro_rules! stdlib_global_metadata {
    ($name:ident => $first:ident $(. $rest:ident)* : $return_kind:ident) => {
        $crate::metadata::StdlibGlobalMetadata::new(
            stringify!($name),
            concat!(stringify!($first), $(".", stringify!($rest))*),
            $crate::metadata::StdlibReturnKind::$return_kind,
        )
    };
}

#[macro_export]
macro_rules! stdlib_register_global_metadata {
    ($name:ident => $first:ident $(. $rest:ident)* : $return_kind:ident) => {
        $crate::metadata::register_stdlib_global_metadata(
            $crate::stdlib_global_metadata!($name => $first $(. $rest)* : $return_kind)
        )
    };
}

#[derive(Debug, Default)]
struct StdlibMetadataRegistry {
    modules: Vec<StdlibModuleMetadata>,
    globals: Vec<StdlibGlobalMetadata>,
}

static STDLIB_METADATA_REGISTRY: OnceLock<Mutex<StdlibMetadataRegistry>> = OnceLock::new();

pub fn register_stdlib_module_metadata(metadata: StdlibModuleMetadata) -> Result<()> {
    let mut registry = metadata_registry().lock().expect("stdlib metadata registry poisoned");
    if let Some(existing) = registry.modules.iter().find(|existing| existing.name == metadata.name) {
        if existing == &metadata {
            return Ok(());
        }
        bail!("conflicting stdlib metadata for module '{}'", metadata.name);
    }
    for callable in metadata.callables {
        if let Some(existing) = registry
            .modules
            .iter()
            .flat_map(|module| module.callables.iter())
            .find(|existing| existing.path == callable.path)
            && existing != callable
        {
            bail!("conflicting stdlib metadata for export '{}'", callable.path);
        }
        if let Some(existing) = registry
            .modules
            .iter()
            .flat_map(|module| module.callables.iter())
            .find(|existing| existing.lowering_key == callable.lowering_key)
            && existing != callable
        {
            bail!("conflicting stdlib lowering_key for export '{}'", callable.lowering_key);
        }
    }
    registry.modules.push(metadata);
    Ok(())
}

pub fn register_stdlib_global_metadata(metadata: StdlibGlobalMetadata) -> Result<()> {
    let mut registry = metadata_registry().lock().expect("stdlib metadata registry poisoned");
    if let Some(existing) = registry.globals.iter().find(|existing| existing.name == metadata.name) {
        if existing == &metadata {
            return Ok(());
        }
        bail!("conflicting stdlib metadata for global '{}'", metadata.name);
    }
    if let Some(existing) = registry
        .globals
        .iter()
        .find(|existing| existing.lowering_key == metadata.lowering_key)
    {
        if existing == &metadata {
            return Ok(());
        }
        bail!("conflicting stdlib lowering_key for global '{}'", metadata.lowering_key);
    }
    registry.globals.push(metadata);
    Ok(())
}

pub fn registered_stdlib_export_metadata(path: &str) -> Option<StdlibCallableMetadata> {
    let registry = metadata_registry().lock().expect("stdlib metadata registry poisoned");
    registry
        .modules
        .iter()
        .flat_map(|module| module.callables.iter())
        .find(|metadata| metadata.path == path)
        .copied()
}

pub fn registered_stdlib_module_metadata(name: &str) -> Option<StdlibModuleMetadata> {
    let registry = metadata_registry().lock().expect("stdlib metadata registry poisoned");
    registry.modules.iter().find(|metadata| metadata.name == name).copied()
}

pub fn registered_stdlib_global_metadata(name: &str) -> Option<StdlibGlobalMetadata> {
    let registry = metadata_registry().lock().expect("stdlib metadata registry poisoned");
    registry.globals.iter().find(|metadata| metadata.name == name).copied()
}

fn metadata_registry() -> &'static Mutex<StdlibMetadataRegistry> {
    STDLIB_METADATA_REGISTRY.get_or_init(|| Mutex::new(StdlibMetadataRegistry::default()))
}

#[derive(Debug, Clone)]
pub struct StdlibExportSpec {
    pub name: String,
    pub kind: StdlibExportKind,
    pub arity: Option<StdlibArity>,
    pub detail: String,
    pub display: String,
    pub lowering_key: Option<&'static str>,
    pub return_kind: Option<StdlibReturnKind>,
    pub signature: Option<String>,
    pub docs: Option<String>,
    pub const_value: Option<StdlibConstValue>,
    pub children: Vec<StdlibExportSpec>,
}

impl StdlibExportSpec {
    pub fn child(&self, name: &str) -> Option<&StdlibExportSpec> {
        self.children.iter().find(|export| export.name == name)
    }

    pub fn export_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.children.iter().map(|export| export.name.clone()).collect();
        names.sort();
        names
    }
}

#[derive(Debug, Clone)]
pub struct StdlibModuleSpec {
    pub name: String,
    pub detail: String,
    pub display: String,
    pub docs: Option<String>,
    pub exports: Vec<StdlibExportSpec>,
}

impl StdlibModuleSpec {
    pub fn export(&self, name: &str) -> Option<&StdlibExportSpec> {
        self.exports.iter().find(|export| export.name == name)
    }

    pub fn export_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.exports.iter().map(|export| export.name.clone()).collect();
        names.sort();
        names
    }
}

#[derive(Debug, Clone)]
pub struct StdlibGlobalSpec {
    pub name: String,
    pub arity: StdlibArity,
    pub detail: String,
    pub lowering_key: Option<&'static str>,
    pub return_kind: Option<StdlibReturnKind>,
    pub signature: Option<String>,
    pub docs: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StdlibCatalog {
    pub modules: Vec<StdlibModuleSpec>,
    pub globals: Vec<StdlibGlobalSpec>,
}

impl StdlibCatalog {
    pub fn module(&self, name: &str) -> Option<&StdlibModuleSpec> {
        self.modules.iter().find(|module| module.name == name)
    }

    pub fn global(&self, name: &str) -> Option<&StdlibGlobalSpec> {
        self.globals.iter().find(|global| global.name == name)
    }

    pub fn module_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.modules.iter().map(|module| module.name.clone()).collect();
        names.sort();
        names
    }

    pub fn global_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.globals.iter().map(|global| global.name.clone()).collect();
        names.sort();
        names
    }

    pub fn export_path(&self, path: &[&str]) -> Option<&StdlibExportSpec> {
        let (module_name, rest) = path.split_first()?;
        let mut export = self.module(module_name)?.export(rest.first().copied()?)?;
        for part in &rest[1..] {
            export = export.child(part)?;
        }
        Some(export)
    }

    pub fn export_names_at_path(&self, path: &[&str]) -> Option<Vec<String>> {
        if path.len() == 1 {
            return Some(self.module(path[0])?.export_names());
        }
        Some(self.export_path(path)?.export_names())
    }

    pub fn global_by_lowering_key(&self, key: &str) -> Option<&StdlibGlobalSpec> {
        self.globals.iter().find(|global| global.lowering_key == Some(key))
    }

    pub fn export_by_lowering_key(&self, key: &str) -> Option<&StdlibExportSpec> {
        self.modules
            .iter()
            .flat_map(|module| module.exports.iter())
            .find_map(|export| export_by_lowering_key(export, key))
    }
}

fn export_by_lowering_key<'a>(export: &'a StdlibExportSpec, key: &str) -> Option<&'a StdlibExportSpec> {
    if export.lowering_key == Some(key) {
        return Some(export);
    }
    export
        .children
        .iter()
        .find_map(|child| export_by_lowering_key(child, key))
}

#[cfg(test)]
mod tests {
    use super::StdlibReturnKind;

    #[test]
    fn module_metadata_macro_builds_nested_export_paths() {
        let metadata = crate::stdlib_module_metadata!(
            io,
            [
                std.read_to_string => String,
                std.flush => Nil,
            ]
        );

        assert_eq!(metadata.name, "io");
        assert_eq!(metadata.callables.len(), 2);
        assert_eq!(metadata.callables[0].path, "io.std.read_to_string");
        assert_eq!(metadata.callables[0].lowering_key, "io.std.read_to_string");
        assert_eq!(metadata.callables[0].return_kind, StdlibReturnKind::String);
        assert_eq!(metadata.callables[1].path, "io.std.flush");
        assert_eq!(metadata.callables[1].lowering_key, "io.std.flush");
        assert_eq!(metadata.callables[1].return_kind, StdlibReturnKind::Nil);
    }

    #[test]
    fn global_metadata_macro_builds_lowering_key() {
        let metadata = crate::stdlib_global_metadata!(assert_eq => core.assert_eq: Nil);

        assert_eq!(metadata.name, "assert_eq");
        assert_eq!(metadata.lowering_key, "core.assert_eq");
        assert_eq!(metadata.return_kind, StdlibReturnKind::Nil);
    }
}
