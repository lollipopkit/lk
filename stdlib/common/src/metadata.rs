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

#[derive(Debug, Clone)]
pub struct StdlibExportSpec {
    pub name: String,
    pub kind: StdlibExportKind,
    pub arity: Option<StdlibArity>,
    pub detail: String,
    pub display: String,
    pub lowering_key: Option<&'static str>,
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
}
