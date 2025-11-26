use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use lkr_core::{
    stmt::stmt_parser::StmtParser,
    stmt::{ImportSource, ImportStmt, ModuleResolver, Program, Stmt, serialize_imports},
    token::Tokenizer,
    vm::{BundledModule, BytecodeModule, ModuleFlags, ModuleMeta, compile_program},
};

/// Collects file-based imports and compiles them into bundled LKRB modules.
pub struct ModuleBundler {
    resolver: ModuleResolver,
    visited: HashSet<PathBuf>,
    modules: Vec<(PathBuf, BytecodeModule)>,
}

pub fn extract_import_statements(program: &Program) -> Vec<ImportStmt> {
    let mut imports = Vec::new();
    for stmt in &program.statements {
        collect_import_statements_from_stmt(stmt, &mut imports);
    }
    imports
}

impl ModuleBundler {
    pub fn new(base_dir: Option<&Path>) -> Self {
        let mut resolver = ModuleResolver::new();
        if let Some(dir) = base_dir
            && !dir.as_os_str().is_empty()
        {
            resolver.add_search_path(dir.to_path_buf());
        }
        Self {
            resolver,
            visited: HashSet::new(),
            modules: Vec::new(),
        }
    }

    /// Traverse the program and enqueue any file imports for bundling.
    pub fn bundle_program(&mut self, program: &Program) -> Result<()> {
        let imports = collect_file_imports(program);
        for spec in imports {
            self.bundle_import(&spec)?;
        }
        Ok(())
    }

    /// Finalise and return all bundled modules in deterministic order.
    pub fn into_bundled(mut self) -> Vec<BundledModule> {
        self.modules.sort_by(|a, b| a.0.cmp(&b.0));
        self.modules
            .into_iter()
            .map(|(path, module)| BundledModule {
                path: path.to_string_lossy().into_owned(),
                module,
            })
            .collect()
    }

    fn bundle_import(&mut self, spec: &str) -> Result<()> {
        let resolved = self
            .resolver
            .resolve_file_path(spec)
            .with_context(|| format!("Failed to resolve import '{}'", spec))?;

        if !self.visited.insert(resolved.clone()) {
            return Ok(());
        }

        let source =
            fs::read_to_string(&resolved).with_context(|| format!("Failed to read module '{}'", resolved.display()))?;

        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(&source).map_err(|e| anyhow!(e.to_string()))?;
        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        let program = parser
            .parse_program_with_enhanced_errors(&source)
            .map_err(|e| anyhow!(e.to_string()))?;

        // Recursively process nested imports before emitting this module.
        self.bundle_program(&program)?;

        let func = compile_program(&program);
        let mut module = BytecodeModule::new(func);
        module.flags.insert(ModuleFlags::CONST_FOLDED);

        let mut meta = ModuleMeta {
            source: Some(resolved.to_string_lossy().into_owned()),
            ..Default::default()
        };
        meta.tags.insert("entry_kind".to_string(), "module".to_string());
        if !meta.is_empty() {
            module.meta = Some(meta);
        }

        let import_stmts = extract_import_statements(&program);
        if !import_stmts.is_empty() {
            let json = serialize_imports(&import_stmts).context("serialize module imports")?;
            module
                .meta
                .get_or_insert_with(Default::default)
                .tags
                .insert("imports".to_string(), json);
        }

        self.modules.push((resolved, module));
        Ok(())
    }
}

fn collect_file_imports(program: &Program) -> BTreeSet<String> {
    let mut imports = BTreeSet::new();
    for stmt in &program.statements {
        collect_imports_from_stmt(stmt, &mut imports);
    }
    imports
}

fn collect_imports_from_stmt(stmt: &Stmt, imports: &mut BTreeSet<String>) {
    match stmt {
        Stmt::Import(import_stmt) => match import_stmt {
            ImportStmt::File { path } => {
                imports.insert(path.clone());
            }
            ImportStmt::Items { source, .. } | ImportStmt::Namespace { source, .. } => {
                if let ImportSource::File(path) = source {
                    imports.insert(path.clone());
                }
            }
            ImportStmt::Module { .. } | ImportStmt::ModuleAlias { .. } => {}
        },
        Stmt::Block { statements } => {
            for stmt in statements {
                collect_imports_from_stmt(stmt, imports);
            }
        }
        Stmt::If {
            then_stmt, else_stmt, ..
        } => {
            collect_imports_from_stmt(then_stmt, imports);
            if let Some(else_stmt) = else_stmt {
                collect_imports_from_stmt(else_stmt, imports);
            }
        }
        Stmt::IfLet {
            then_stmt, else_stmt, ..
        } => {
            collect_imports_from_stmt(then_stmt, imports);
            if let Some(else_stmt) = else_stmt {
                collect_imports_from_stmt(else_stmt, imports);
            }
        }
        Stmt::While { body, .. } | Stmt::WhileLet { body, .. } | Stmt::For { body, .. } => {
            collect_imports_from_stmt(body, imports)
        }
        Stmt::Function { body, .. } => collect_imports_from_stmt(body, imports),
        Stmt::Impl { methods, .. } => {
            for method in methods {
                collect_imports_from_stmt(method, imports);
            }
        }
        Stmt::Return { .. } | Stmt::Expr(_) => {}
        // Statements without nested statements do not contribute additional imports.
        Stmt::Let { .. }
        | Stmt::Assign { .. }
        | Stmt::CompoundAssign { .. }
        | Stmt::Define { .. }
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Struct { .. }
        | Stmt::TypeAlias { .. }
        | Stmt::Trait { .. }
        | Stmt::Empty => {}
    }
}

fn collect_import_statements_from_stmt(stmt: &Stmt, imports: &mut Vec<ImportStmt>) {
    match stmt {
        Stmt::Import(import_stmt) => imports.push(import_stmt.clone()),
        Stmt::Block { statements } => {
            for stmt in statements {
                collect_import_statements_from_stmt(stmt, imports);
            }
        }
        Stmt::If {
            then_stmt, else_stmt, ..
        } => {
            collect_import_statements_from_stmt(then_stmt, imports);
            if let Some(else_stmt) = else_stmt {
                collect_import_statements_from_stmt(else_stmt, imports);
            }
        }
        Stmt::IfLet {
            then_stmt, else_stmt, ..
        } => {
            collect_import_statements_from_stmt(then_stmt, imports);
            if let Some(else_stmt) = else_stmt {
                collect_import_statements_from_stmt(else_stmt, imports);
            }
        }
        Stmt::While { body, .. } | Stmt::WhileLet { body, .. } | Stmt::For { body, .. } => {
            collect_import_statements_from_stmt(body, imports)
        }
        Stmt::Function { body, .. } => collect_import_statements_from_stmt(body, imports),
        Stmt::Impl { methods, .. } => {
            for method in methods {
                collect_import_statements_from_stmt(method, imports);
            }
        }
        _ => {}
    }
}
