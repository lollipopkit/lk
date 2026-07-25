#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::stmt::{Program, Stmt};
use serde::{Deserialize, Serialize};

/// Import system for LK - supports various `use` syntaxes and plugin-style module resolution
///
/// Supported use syntaxes:
/// 1. `use math;` - imports stdlib module 'math' with all exports
/// 2. `use "path/to/file.lk";` - imports file with all exports
/// 3. `use { abs, sqrt } from math;` - imports specific items from stdlib module
/// 4. `use { func as alias } from "file.lk";` - imports with alias
/// 5. `use * as math from math;` - imports all as namespace
/// 6. `use math as m;` - imports entire module with alias
///
/// Use statement variants
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ImportStmt {
    /// `use module;` - use entire module
    Module { module: String },
    /// `use "path";` - use from file path
    File { path: String },
    /// `use { items } from source;` - use specific items
    Items {
        items: Vec<ImportItem>,
        source: ImportSource,
    },
    /// `use * as alias from source;` - use all as namespace
    Namespace { alias: String, source: ImportSource },
    /// `use module as alias;` - use module with alias
    ModuleAlias { module: String, alias: String },
}

/// Import source - either stdlib module or file path
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ImportSource {
    Module(String),
    File(String),
}

/// Individual use item with optional alias
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportItem {
    pub name: String,
    pub alias: Option<String>,
}

// Note: The Module trait and registry live in module.rs; this file owns source use resolution.

pub fn serialize_imports(imports: &[ImportStmt]) -> serde_json::Result<String> {
    serde_json::to_string(imports)
}

pub fn deserialize_imports(json: &str) -> serde_json::Result<Vec<ImportStmt>> {
    serde_json::from_str(json)
}

pub fn collect_program_imports(program: &Program) -> Vec<ImportStmt> {
    fn visit(stmt: &Stmt, acc: &mut Vec<ImportStmt>) {
        match stmt {
            Stmt::Import(import_stmt) => acc.push(import_stmt.clone()),
            Stmt::Block { statements } => {
                for stmt in statements {
                    visit(stmt, acc);
                }
            }
            Stmt::If {
                then_stmt, else_stmt, ..
            } => {
                visit(then_stmt, acc);
                if let Some(else_stmt) = else_stmt {
                    visit(else_stmt, acc);
                }
            }
            Stmt::IfLet {
                then_stmt, else_stmt, ..
            } => {
                visit(then_stmt, acc);
                if let Some(else_stmt) = else_stmt {
                    visit(else_stmt, acc);
                }
            }
            Stmt::While { body, .. } | Stmt::WhileLet { body, .. } | Stmt::For { body, .. } => visit(body, acc),
            Stmt::Function { body, .. } => visit(body, acc),
            Stmt::Impl { methods, .. } => {
                for method in methods {
                    visit(method, acc);
                }
            }
            _ => {}
        }
    }

    let mut imports = Vec::new();
    for stmt in &program.statements {
        visit(stmt, &mut imports);
    }
    imports
}

// Standard library module implementations have been moved to module.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_import_stmt_variants() {
        let import = ImportStmt::Module {
            module: "math".to_string(),
        };
        assert!(matches!(import, ImportStmt::Module { .. }));

        let import = ImportStmt::Items {
            items: vec![ImportItem {
                name: "abs".to_string(),
                alias: None,
            }],
            source: ImportSource::Module("math".to_string()),
        };
        assert!(matches!(import, ImportStmt::Items { .. }));
    }
}
