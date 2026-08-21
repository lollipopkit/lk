#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::compat::sync::Mutex;
use alloc::collections::BTreeSet;
use alloc::sync::Arc;

use anyhow::{Result, anyhow};

use crate::{
    expr::{Expr, Pattern},
    stmt::{Program, Stmt},
    typ::TypeChecker,
    val::RuntimeVal,
    vm::{Module, RuntimeExport, RuntimeModuleState, VmContext},
};

/// Persistent VM state for interactive REPL execution.
///
/// Each input is still compiled as its own module, but top-level REPL bindings
/// are synchronized through `VmContext` runtime globals so later inputs can
/// resolve and use them.
#[derive(Debug)]
pub struct ReplVmSession {
    ctx: VmContext,
    type_checker: TypeChecker,
    persistent_names: BTreeSet<String>,
    /// Every `struct` the session has declared, in the order first seen — see
    /// [`ReplVmSession::carry_struct_declarations`].
    struct_decls: Vec<crate::vm::StructDecl>,
    /// Default method bodies from every `trait` the session has declared, so a
    /// later input's `impl` gets them — see `apply_carried_trait_defaults`.
    trait_defaults: crate::compat::collections::HashMap<String, Vec<Stmt>>,
}

impl ReplVmSession {
    pub fn new(ctx: VmContext, type_checker: TypeChecker) -> Self {
        Self {
            ctx,
            type_checker,
            persistent_names: BTreeSet::new(),
            struct_decls: Vec::new(),
            trait_defaults: crate::compat::collections::HashMap::new(),
        }
    }

    pub fn ctx(&self) -> &VmContext {
        &self.ctx
    }

    pub fn ctx_mut(&mut self) -> &mut VmContext {
        &mut self.ctx
    }

    pub fn execute_program(&mut self, program: &Program) -> Result<ReplExecutionResult> {
        // A trait's default bodies are copied into the impls that leave them out
        // during parsing, over one program's statements. An input that declares
        // the `impl` without the `trait` beside it never saw them.
        let carried_defaults;
        let program = if self.trait_defaults.is_empty() {
            program
        } else {
            let mut owned = program.clone();
            crate::stmt::trait_defaults::apply_carried_trait_defaults(&mut owned.statements, &self.trait_defaults);
            carried_defaults = owned;
            &carried_defaults
        };
        let mut next_type_checker = self.type_checker.clone();
        let carried = self.carried_function_types(program);
        program.type_check(&mut next_type_checker)?;
        restore_carried_function_types(&mut next_type_checker, carried);

        let (runtime_program, declared_names) = repl_runtime_program(program, &self.persistent_names)?;
        // The session's own bindings are user data, not module objects: without
        // saying so, `xs` from an earlier line is indistinguishable from an
        // imported `math`, and `xs.len()` compiles to an index read keyed by
        // `"len"` (`compile_program_module_with_ctx_and_data_globals`).
        let data_globals = self.persistent_names.iter().cloned().collect::<Vec<_>>();
        let module = crate::vm::compile_program_module_with_ctx_and_data_globals(
            &runtime_program,
            &mut self.ctx,
            &data_globals,
        )?;
        let module = self.carry_struct_declarations(module);
        let result = crate::vm::execute_compiled_module_with_ctx(module, &mut self.ctx)?;

        self.type_checker = next_type_checker;
        self.persistent_names.extend(declared_names);
        self.trait_defaults
            .extend(crate::stmt::trait_defaults::trait_defaults_of(&program.statements));
        self.record_struct_declarations(&result.module);
        self.sync_result_globals(result)
    }

    /// Give this input's module the `struct` declarations earlier inputs made.
    ///
    /// A field's *declaration order* travels with the type, and both paths that
    /// build an instance read it from the module being executed
    /// (`exec::container::declared_type` and `__lk_make_struct`). Every REPL
    /// input is its own module, so a struct declared on one line and built on
    /// the next had no declaration to order by and fell back to the field map's
    /// own iteration:
    ///
    /// ```text
    /// > struct Reading { zebra: Int, apple: Int, mango: Int, … }
    /// > Reading { zebra: 1, apple: 2, mango: 3, … }
    /// Reading{apple:2,fig:6,kiwi:4,mango:3,pear:5,zebra:1}
    /// ```
    ///
    /// Written on one line, or in a file, or across a real `use`, the same
    /// value prints in declaration order. This makes the session behave like
    /// the file: a declaration stays visible to the inputs after it.
    fn carry_struct_declarations(&self, module: Arc<crate::vm::Module>) -> Arc<crate::vm::Module> {
        // A redeclaration in *this* input wins — it is what the source being run
        // says, exactly as a later `struct` in a file would.
        let missing: Vec<crate::vm::StructDecl> = self
            .struct_decls
            .iter()
            .filter(|decl| !module.type_info.structs.iter().any(|own| own.name == decl.name))
            .cloned()
            .collect();
        if missing.is_empty() {
            return module;
        }
        let mut module = module;
        Arc::make_mut(&mut module).type_info.structs.extend(missing);
        module
    }

    /// Keep what this input declared, for the inputs after it.
    fn record_struct_declarations(&mut self, module: &crate::vm::Module) {
        for decl in &module.type_info.structs {
            match self.struct_decls.iter_mut().find(|held| held.name == decl.name) {
                Some(held) => *held = decl.clone(),
                None => self.struct_decls.push(decl.clone()),
            }
        }
    }

    /// The recorded types of functions this program does *not* declare.
    ///
    /// After checking a program the checker applies the solved substitutions to
    /// everything it has recorded — right for one program, and wrong for a
    /// sequence of them. An unannotated parameter's type is a derivation from
    /// the body rather than a claim by the source (see `FunctionSig::annotated`),
    /// and this turned one input's derivation into a claim binding every later
    /// input: `fn f(x) { return x; }` then `f(1)` left `f` as `(Int) -> Int`, so
    /// `f("a")` on the next line answered "Cannot unify Int with String". The
    /// same three lines in a file are fine, because there the substitution pass
    /// runs once with every call site already contributing to it.
    ///
    /// A function the program *does* declare is left alone: its definition and
    /// this input's uses were checked together, exactly as in a file.
    fn carried_function_types(
        &self,
        program: &Program,
    ) -> Vec<(String, crate::typ::FunctionSig, Option<crate::val::Type>)> {
        let declared_here = declared_function_names(program);
        self.type_checker
            .declared_function_names()
            .into_iter()
            .filter(|name| !declared_here.contains(name.as_str()))
            .filter_map(|name| {
                let sig = self.type_checker.get_function_sig(&name)?.clone();
                let local = self.type_checker.get_local_type(&name).cloned();
                Some((name, sig, local))
            })
            .collect()
    }

    fn sync_result_globals(&mut self, result: crate::vm::ProgramResult) -> Result<ReplExecutionResult> {
        let display_first_return = (!result.first_return_is_nil()).then(|| result.display_first_return());
        let returns = result.returns;
        let module = result.module;
        let state = Arc::new(Mutex::new(result.state));
        let exports = exports_from_state(&self.persistent_names, Arc::clone(&module), Arc::clone(&state))?;

        for (name, export) in exports {
            self.ctx.define_runtime_global(name, export);
        }

        Ok(ReplExecutionResult {
            returns,
            state,
            module,
            display_first_return,
        })
    }
}

#[derive(Debug)]
pub struct ReplExecutionResult {
    pub returns: Vec<RuntimeVal>,
    pub state: Arc<Mutex<RuntimeModuleState>>,
    pub module: Arc<Module>,
    display_first_return: Option<String>,
}

impl ReplExecutionResult {
    pub fn first_return(&self) -> &RuntimeVal {
        self.returns.first().unwrap_or(&RuntimeVal::Nil)
    }

    pub fn first_return_is_nil(&self) -> bool {
        matches!(self.first_return(), RuntimeVal::Nil)
    }

    pub fn display_first_return(&self) -> String {
        self.display_first_return.clone().unwrap_or_else(|| "nil".to_string())
    }
}

fn restore_carried_function_types(
    checker: &mut TypeChecker,
    carried: Vec<(String, crate::typ::FunctionSig, Option<crate::val::Type>)>,
) {
    for (name, sig, local) in carried {
        checker.add_function_sig(name.clone(), sig);
        if let Some(local) = local {
            checker.add_local_type(name, local);
        }
    }
}

/// The names of functions a program declares at its top level, including the
/// constructor a `struct` brings with it.
fn declared_function_names(program: &Program) -> BTreeSet<String> {
    fn item(stmt: &Stmt) -> &Stmt {
        match stmt {
            Stmt::Attributed { item, .. } => item,
            other => other,
        }
    }
    program
        .statements
        .iter()
        .filter_map(|stmt| match item(stmt.as_ref()) {
            Stmt::Function { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn repl_runtime_program(program: &Program, existing_names: &BTreeSet<String>) -> Result<(Program, BTreeSet<String>)> {
    let mut active_names = existing_names.clone();
    let mut declared_names = BTreeSet::new();
    let mut statements = Vec::with_capacity(program.statements.len() * 2);

    for stmt in &program.statements {
        let mut flush_names = BTreeSet::new();
        collect_repl_statement_bindings(stmt.as_ref(), &mut active_names, &mut declared_names, &mut flush_names);
        statements.push(stmt.clone());
        for name in flush_names {
            statements.push(Box::new(flush_global_stmt(name)));
        }
    }

    Ok((Program::new(statements)?, declared_names))
}

fn collect_repl_statement_bindings(
    stmt: &Stmt,
    active_names: &mut BTreeSet<String>,
    declared_names: &mut BTreeSet<String>,
    flush_names: &mut BTreeSet<String>,
) {
    match stmt {
        Stmt::Attributed { item, .. } => {
            collect_repl_statement_bindings(item, active_names, declared_names, flush_names);
        }
        Stmt::Let { pattern, .. } => {
            for name in pattern_binding_names(pattern) {
                active_names.insert(name.clone());
                declared_names.insert(name.clone());
                flush_names.insert(name);
            }
        }
        Stmt::Define { name, .. } | Stmt::Function { name, .. } => {
            active_names.insert(name.clone());
            declared_names.insert(name.clone());
            flush_names.insert(name.clone());
        }
        Stmt::Assign { name, .. } | Stmt::CompoundAssign { name, .. } if active_names.contains(name) => {
            flush_names.insert(name.clone());
        }
        _ => {}
    }
}

fn pattern_binding_names(pattern: &Pattern) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    collect_pattern_binding_names(pattern, &mut names);
    names
}

fn collect_pattern_binding_names(pattern: &Pattern, names: &mut BTreeSet<String>) {
    match pattern {
        Pattern::Variable(name) => {
            names.insert(name.clone());
        }
        Pattern::List { patterns, rest } => {
            for pattern in patterns {
                collect_pattern_binding_names(pattern, names);
            }
            if let Some(rest) = rest {
                names.insert(rest.clone());
            }
        }
        Pattern::Map { patterns, rest } => {
            for (_, pattern) in patterns {
                collect_pattern_binding_names(pattern, names);
            }
            if let Some(rest) = rest {
                names.insert(rest.clone());
            }
        }
        Pattern::Or(patterns) => {
            for pattern in patterns {
                collect_pattern_binding_names(pattern, names);
            }
        }
        Pattern::Guard { pattern, .. } => {
            collect_pattern_binding_names(pattern, names);
        }
        Pattern::Literal(_) | Pattern::Range { .. } | Pattern::Wildcard => {}
    }
}

fn flush_global_stmt(name: String) -> Stmt {
    Stmt::Define {
        name: name.clone(),
        value: Box::new(Expr::Var(name)),
        // Synthesised to flush a REPL global; it corresponds to no source text.
        span: None,
    }
}

fn exports_from_state(
    persistent_names: &BTreeSet<String>,
    module: Arc<Module>,
    state: Arc<Mutex<RuntimeModuleState>>,
) -> Result<Vec<(String, RuntimeExport)>> {
    let state_guard = state.lock().map_err(|_| anyhow!("REPL result state lock poisoned"))?;
    let mut exports = Vec::new();
    for (index, slot) in module.globals.iter().enumerate() {
        let name = slot.name.as_ref();
        if !persistent_names.contains(name) {
            continue;
        }
        let Some(value) = state_guard.globals().get(index).cloned() else {
            continue;
        };
        exports.push((
            name.to_string(),
            RuntimeExport::new(value, Arc::clone(&state), Arc::clone(&module)),
        ));
    }
    Ok(exports)
}

#[cfg(test)]
mod tests {
    #[cfg(not(feature = "std"))]
    use crate::compat::prelude::*;
    use crate::{
        stmt::StmtParser,
        token::Tokenizer,
        typ::TypeChecker,
        val::RuntimeVal,
        vm::{ReplVmSession, VmContext, execute_program_with_ctx},
    };

    fn parse(source: &str) -> crate::stmt::Program {
        let tokens = Tokenizer::tokenize(source).expect("tokenize");
        StmtParser::new(&tokens).parse_program().expect("parse")
    }

    fn execute(session: &mut ReplVmSession, source: &str) -> anyhow::Result<crate::vm::ReplExecutionResult> {
        session.execute_program(&parse(source))
    }

    fn new_session() -> ReplVmSession {
        ReplVmSession::new(VmContext::new(), TypeChecker::new())
    }

    #[test]
    fn repl_persists_top_level_let_bindings() {
        let mut session = new_session();
        execute(&mut session, "let a = 1;").expect("define a");
        let result = execute(&mut session, "let b = a + 2; return b;").expect("use a");

        assert_eq!(result.returns, vec![RuntimeVal::Int(3)]);
        assert!(matches!(
            session.ctx().get_runtime_global("b").map(|export| export.value()),
            Some(RuntimeVal::Int(3))
        ));
    }

    #[test]
    fn repl_persists_assignment_to_current_input_binding_before_return() {
        let mut session = new_session();
        let result = execute(&mut session, "let a = 1; a = 2; return a;").expect("execute");

        assert_eq!(result.returns, vec![RuntimeVal::Int(2)]);
        assert!(matches!(
            session.ctx().get_runtime_global("a").map(|export| export.value()),
            Some(RuntimeVal::Int(2))
        ));
    }

    #[test]
    fn repl_persists_destructured_top_level_bindings() {
        let mut session = new_session();
        execute(&mut session, "let [a, b] = [1, 2];").expect("destructure");
        let result = execute(&mut session, "let c = a + b; return c;").expect("use destructured names");

        assert_eq!(result.returns, vec![RuntimeVal::Int(3)]);
    }

    /// A binding from an earlier input is an *external* global to the module
    /// compiled for this one — indistinguishable from an imported `math` unless
    /// the session says otherwise. Without that, `xs.len()` compiled to an
    /// index read keyed by `"len"` and every method call on a REPL binding
    /// failed: `xs.push(1)`, `s.upper()`, `m.get(k)`.
    #[test]
    fn repl_method_call_on_earlier_binding_dispatches_as_method() {
        let mut session = new_session();
        execute(&mut session, "xs := [1, 2];").expect("define list");
        let result = execute(&mut session, "return xs.len();").expect("method call across inputs");

        assert_eq!(result.returns, vec![RuntimeVal::Int(2)]);
    }

    #[test]
    fn repl_preserves_heap_backed_values() {
        let mut session = new_session();
        execute(&mut session, "let xs = [1, 2];").expect("define list");
        let result = execute(&mut session, "return xs.0;").expect("index list");

        assert_eq!(result.returns, vec![RuntimeVal::Int(1)]);
    }

    #[test]
    fn repl_functions_can_read_previous_bindings() {
        let mut session = new_session();
        execute(&mut session, "let seed = 40;").expect("define seed");
        execute(&mut session, "fn answer() { return seed + 2; }").expect("define function");
        let result = execute(&mut session, "return answer();").expect("call function");

        assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    }

    #[test]
    fn repl_const_bindings_remain_const_across_inputs() {
        let mut session = new_session();
        execute(&mut session, "const a = 1;").expect("define const");

        let err = execute(&mut session, "a = 2;").expect_err("const assignment should fail");
        assert!(err.to_string().contains("Cannot assign to const variable 'a'"));

        let result = execute(&mut session, "return a;").expect("read const");
        assert_eq!(result.returns, vec![RuntimeVal::Int(1)]);
    }

    #[test]
    fn normal_execute_program_with_ctx_still_does_not_sync_locals_back() {
        let mut ctx = VmContext::new();
        ctx.define_runtime_value("seed", RuntimeVal::Int(39), crate::val::HeapStore::new());
        let program = parse(
            r#"
            total := seed + 2;
            seed = total + 1;
            return seed;
            "#,
        );

        let result = execute_program_with_ctx(&program, &mut ctx).expect("execute");

        assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
        assert!(matches!(
            ctx.get_runtime_global("seed").map(|export| export.value()),
            Some(RuntimeVal::Int(39))
        ));
        assert!(ctx.get_runtime_global("total").is_none());
    }
}
