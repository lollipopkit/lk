use crate::compat::path::Path;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use anyhow::Result;

use crate::vm::execute_imports;
use crate::{
    stmt::{Program, import::collect_program_imports},
    syntax::{ParseOptions, parse_program_source},
    val::{HeapStore, RuntimeVal},
    vm::{Compiler, GlobalSlot, ModuleArtifact, VmContext},
};

use super::{Executor, ProgramResult, imports::import_runtime_export};

/// Running a program from its AST.
///
/// These used to be inherent methods on `Program`, which made the AST layer
/// (`stmt`) depend on the execution layer (`vm`) — a cycle that existed only
/// for call-site convenience. As an extension trait the convenience is kept
/// while the dependency points the right way (`vm` → `stmt`).
pub trait ProgramExec {
    /// Type-checks and runs the program in a fresh context.
    fn execute(&self) -> Result<ProgramResult>;
    /// Type-checks and runs the program in `ctx`.
    fn execute_with_ctx(&self, ctx: &mut VmContext) -> Result<ProgramResult>;
    /// As [`Self::execute_with_ctx`], with the directory the program was loaded
    /// from so its own imports can be seeded into the checker.
    fn execute_with_ctx_from(&self, ctx: &mut VmContext, base_dir: Option<&Path>) -> Result<ProgramResult>;
}

impl ProgramExec for Program {
    fn execute(&self) -> Result<ProgramResult> {
        let mut ctx = VmContext::new();
        self.execute_with_ctx(&mut ctx)
    }

    fn execute_with_ctx(&self, ctx: &mut VmContext) -> Result<ProgramResult> {
        self.execute_with_ctx_from(ctx, None)
    }

    /// As [`ProgramExec::execute_with_ctx`], with the directory this program
    /// was loaded from so its own imports can be seeded.
    ///
    /// Without the directory the checker cannot open the files this program
    /// imports, so a name that crosses a module boundary — a `struct` returned
    /// by a function in another file — is unknown to it. That was invisible
    /// while an unknown name silently became `Type::Named`: the annotation
    /// checked against nothing. The entry file has always been seeded (the CLI
    /// does it); a module *loaded as an import* had not been, so it was the one
    /// place where cross-file calls went unchecked entirely.
    fn execute_with_ctx_from(&self, ctx: &mut VmContext, base_dir: Option<&Path>) -> Result<ProgramResult> {
        let mut type_checker = crate::typ::TypeChecker::new();
        #[cfg(feature = "std")]
        if let Some(base_dir) = base_dir {
            crate::typ::seed_imported_signatures(self, base_dir, &mut type_checker);
        }
        #[cfg(not(feature = "std"))]
        let _ = base_dir;
        self.type_check(&mut type_checker)?;
        execute_program_with_ctx(self, ctx)
    }
}

pub fn execute_program(program: &Program) -> Result<ProgramResult> {
    let mut ctx = VmContext::new();
    execute_program_with_ctx(program, &mut ctx)
}

pub fn compile_program_module_with_ctx(program: &Program, ctx: &mut VmContext) -> Result<Arc<crate::vm::Module>> {
    let imports = collect_program_imports(program);
    let resolver = ctx.resolver().clone();
    execute_imports(&imports, resolver.as_ref(), ctx)?;

    let mut external_globals = Vec::new();
    for (name, _) in ctx.runtime_globals_iter() {
        external_globals.push(name.clone());
    }

    let mut module = Compiler::compile_module_with_natives_and_globals(program, Vec::new(), external_globals)?;
    // The compiler has no idea which file it is compiling; the loader does, and
    // it put that on the context before handing the program over. Stamping here
    // is what gives this module's declared types an identity distinct from an
    // identically-named type in any other module (`vm::TypeScope`).
    module.type_scope = ctx.type_scope().clone();
    Ok(Arc::new(module))
}

pub fn execute_program_with_ctx(program: &Program, ctx: &mut VmContext) -> Result<ProgramResult> {
    let module = compile_program_module_with_ctx(program, ctx)?;
    execute_compiled_module_with_ctx(module, ctx)
}

pub fn execute_program_with_ctx_and_budget(
    program: &Program,
    ctx: &mut VmContext,
    instruction_budget: u64,
) -> Result<ProgramResult> {
    let module = compile_program_module_with_ctx(program, ctx)?;
    execute_compiled_module_with_ctx_and_budget(module, ctx, instruction_budget)
}

pub fn execute_module_artifact_with_ctx(artifact: ModuleArtifact, ctx: &mut VmContext) -> Result<ProgramResult> {
    let imports = artifact.imports.clone();
    let resolver = ctx.resolver().clone();
    execute_imports(&imports, resolver.as_ref(), ctx)?;
    let module = Arc::new(artifact.into_module()?);
    execute_compiled_module_with_ctx(module, ctx)
}

/// Execute with optional sandbox limits (instruction budget / heap-object cap).
/// Both are zero-cost when `None` (plan M2.6).
pub fn execute_program_with_ctx_and_limits(
    program: &Program,
    ctx: &mut VmContext,
    instruction_budget: Option<u64>,
    heap_object_limit: Option<usize>,
) -> Result<ProgramResult> {
    let module = compile_program_module_with_ctx(program, ctx)?;
    execute_compiled_module_with_ctx_inner(module, ctx, instruction_budget, heap_object_limit)
}

pub fn execute_compiled_module_with_ctx(module: Arc<crate::vm::Module>, ctx: &mut VmContext) -> Result<ProgramResult> {
    execute_compiled_module_with_ctx_inner(module, ctx, None, None)
}

/// Execute with the heap's GC threshold pinned low so (nearly) every safepoint
/// collects — the deterministic in-process twin of `LK_GC_STRESS=1`. Test-only
/// surface for host-root regression tests (core and stdlib crates); not part
/// of the public API.
#[doc(hidden)]
pub fn execute_program_with_ctx_and_gc_threshold(
    program: &Program,
    ctx: &mut VmContext,
    gc_threshold: u32,
) -> Result<ProgramResult> {
    let module = compile_program_module_with_ctx(program, ctx)?;
    execute_compiled_module_with_ctx_full(module, ctx, None, None, Some(gc_threshold))
}

fn execute_compiled_module_with_ctx_and_budget(
    module: Arc<crate::vm::Module>,
    ctx: &mut VmContext,
    instruction_budget: u64,
) -> Result<ProgramResult> {
    execute_compiled_module_with_ctx_inner(module, ctx, Some(instruction_budget), None)
}

fn execute_compiled_module_with_ctx_inner(
    module: Arc<crate::vm::Module>,
    ctx: &mut VmContext,
    instruction_budget: Option<u64>,
    heap_object_limit: Option<usize>,
) -> Result<ProgramResult> {
    execute_compiled_module_with_ctx_full(module, ctx, instruction_budget, heap_object_limit, None)
}

fn execute_compiled_module_with_ctx_full(
    module: Arc<crate::vm::Module>,
    ctx: &mut VmContext,
    instruction_budget: Option<u64>,
    heap_object_limit: Option<usize>,
    gc_threshold: Option<u32>,
) -> Result<ProgramResult> {
    // Start each top-level run with an empty traceback so a reused context
    // (REPL / embedded `Vm`) does not carry frames from a previous error.
    ctx.truncate_call_stack(0);
    // Trait/impl declarations come from the artifact, not from executing
    // registration calls, so the method table is ready before any user code
    // runs (`VmContext::register_module_types`).
    ctx.register_module_types(&module)?;
    let mut seed_heap = HeapStore::new();
    if let Some(gc_threshold) = gc_threshold {
        seed_heap.set_gc_threshold(gc_threshold);
    }
    let globals = seed_module_globals(&module.globals, ctx, &mut seed_heap)?;
    let register_count = module
        .entry_function()
        .map(|function| function.register_count)
        .unwrap_or_default();
    let mut executor = Executor::new(register_count);
    if let Some(instruction_budget) = instruction_budget {
        executor = executor.with_instruction_budget(instruction_budget);
    }
    if let Some(heap_object_limit) = heap_object_limit {
        executor = executor.with_heap_object_limit(heap_object_limit);
    }
    let result =
        executor.run_shared_module_with_globals_and_heap_and_ctx(Arc::clone(&module), globals, seed_heap, ctx)?;
    Ok(ProgramResult {
        returns: result.returns,
        state: result.state,
        module,
    })
}

pub fn execute_source(source: &str) -> Result<ProgramResult> {
    let program = parse_program_source(source, ParseOptions::default())?;
    execute_program(&program)
}

pub(super) fn seed_module_globals(
    slots: &[GlobalSlot],
    ctx: &VmContext,
    heap: &mut HeapStore,
) -> Result<Vec<RuntimeVal>> {
    let mut globals = Vec::with_capacity(slots.len());
    for slot in slots {
        globals.push(match ctx.get_runtime_global(slot.name.as_ref()) {
            Some(export) => import_runtime_export(export, heap),
            None => Ok(RuntimeVal::Nil),
        }?);
    }
    Ok(globals)
}

/// A scalar argument for [`call_module_function_with_ctx`]. The Tier 1 hybrid
/// bridge marshals native scalars into VM values with these tags — containers
/// and closures are deliberately absent (see `docs/aot/tier1-hybrid.md`).
#[derive(Debug, Clone, PartialEq)]
pub enum ModuleFunctionArg {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

/// The outcome of a bridge call whose result must outlive the call: `value`
/// may reference `state`'s heap (lists, maps, long strings). The v2 return
/// bridge walks `value` against `state.heap()` to marshal a deep copy into
/// native memory before dropping both — returning `value` alone would leave
/// heap-backed results dangling (the v1 discard bridge masked this).
pub struct ModuleFunctionOutcome {
    pub value: RuntimeVal,
    pub state: crate::vm::RuntimeModuleState,
}

/// A bridge call either returns or raises. `Err` on the outer `Result` stays
/// reserved for infrastructure failures (bad artifact, bad index) — a *raise*
/// is a language-level outcome the bridge re-raises natively so an enclosing
/// native `try` observes it exactly like the VM would (v2 C6).
pub enum ModuleFunctionCall {
    Return(ModuleFunctionOutcome),
    /// An uncaught raise: the first-class error value (readable against
    /// `state`) plus the display rendered at raise time (the uncaught-error
    /// message).
    Raise {
        value: RuntimeVal,
        rendered: alloc::string::String,
        state: crate::vm::RuntimeModuleState,
    },
}

/// Discarding variant of [`call_module_function_with_ctx_keep_state`] — the
/// v1 bridge entry (`lk_hybrid_call_v`): the returned value is only
/// meaningful for scalars, because the per-call state drops here. A raise
/// comes back as `Err` carrying the rendered message.
pub fn call_module_function_with_ctx(
    module: &crate::vm::Module,
    function_index: u32,
    args: &[ModuleFunctionArg],
    ctx: &mut VmContext,
) -> Result<RuntimeVal> {
    match call_module_function_with_ctx_keep_state(module, function_index, args, ctx)? {
        ModuleFunctionCall::Return(outcome) => Ok(outcome.value),
        ModuleFunctionCall::Raise { rendered, .. } => Err(anyhow::anyhow!(rendered)),
    }
}

/// Call one function of a compiled module with positional scalar arguments —
/// the Tier 1 hybrid bridge entry (`docs/aot/tier1-hybrid.md`): globals and
/// builtins are seeded exactly like a module run, but `function_index` is
/// invoked instead of the entry, against a fresh per-call state. Bridge-eligible
/// functions touch no user globals (the lowering proves it), so per-call state
/// is semantically invisible. The state rides along in the outcome so callers
/// can read heap-backed results before dropping it.
///
/// **The caller registers `module`'s trait/impl declarations once**, via
/// [`VmContext::register_module_types`], before the first call — this entry is
/// a per-call hot path (a native loop can reach it millions of times) and one
/// `ctx` outlives the whole process, so registering here re-registered the
/// module's impls on every single call.
pub fn call_module_function_with_ctx_keep_state(
    module: &crate::vm::Module,
    function_index: u32,
    args: &[ModuleFunctionArg],
    ctx: &mut VmContext,
) -> Result<ModuleFunctionCall> {
    use crate::val::{CallableValue, HeapValue, ShortStr};

    if module.functions.get(function_index as usize).is_none() {
        anyhow::bail!(
            "hybrid bridge: function index {} out of bounds for {} functions",
            function_index,
            module.functions.len()
        );
    }
    ctx.truncate_call_stack(0);
    let mut seed_heap = HeapStore::new();
    let globals = seed_module_globals(&module.globals, ctx, &mut seed_heap)?;
    let mut state = crate::vm::RuntimeModuleState::new(seed_heap, globals);
    let callee = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Callable(CallableValue::Closure {
        function_index,
        captures: Arc::new(Vec::new()),
    })));
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        values.push(match arg {
            ModuleFunctionArg::Nil => RuntimeVal::Nil,
            ModuleFunctionArg::Bool(value) => RuntimeVal::Bool(*value),
            ModuleFunctionArg::Int(value) => RuntimeVal::Int(*value),
            ModuleFunctionArg::Float(value) => RuntimeVal::Float(*value),
            ModuleFunctionArg::Str(value) => match ShortStr::new(value) {
                Some(short) => RuntimeVal::ShortStr(short),
                None => RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::String(Arc::from(value.as_str())))),
            },
        });
    }
    match super::call_runtime_value_runtime(callee, &values, &mut state, Some(module), Some(ctx)) {
        Ok(value) => Ok(ModuleFunctionCall::Return(ModuleFunctionOutcome { value, state })),
        Err(err) => {
            // A language-level raise carries its first-class value (heap refs
            // resolve against the per-call state, which rides along) — the
            // native bridge re-raises it so `try` semantics match the VM.
            if let Some(raised) = err.downcast_ref::<super::handler::LkRaisedValue>() {
                return Ok(ModuleFunctionCall::Raise {
                    value: raised.value,
                    rendered: raised.rendered.as_ref().to_string(),
                    state,
                });
            }
            // A message-only runtime raise: the VM's catch binds the message
            // *string* (`try { 1/0 } catch e` → `typeof(e) == "String"`).
            if let Some(raise) = err.downcast_ref::<super::handler::LanguageRaise>() {
                let message = raise.message.clone();
                let value = match ShortStr::new(message.as_ref()) {
                    Some(short) => RuntimeVal::ShortStr(short),
                    None => RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::String(Arc::from(message.as_ref())))),
                };
                return Ok(ModuleFunctionCall::Raise {
                    value,
                    rendered: message.as_ref().to_string(),
                    state,
                });
            }
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(feature = "std"))]
    use crate::compat::prelude::*;
    use alloc::sync::Arc;

    use crate::{
        val::{HeapStore, HeapValue, RuntimeVal},
        vm::{Function, GlobalSlot, Instr, Module, Opcode, RuntimeExport, RuntimeModuleState, VmContext},
    };

    use super::{execute_compiled_module_with_ctx_and_budget, seed_module_globals};

    #[test]
    fn seed_module_globals_imports_by_module_slot_order_without_name_map() {
        let mut source_heap = HeapStore::new();
        let source_string = source_heap.alloc(HeapValue::String(Arc::<str>::from("external")));
        let mut ctx = VmContext::new_without_core_vm_builtins();
        ctx.define_runtime_global(
            "external",
            RuntimeExport::new(
                RuntimeVal::Obj(source_string),
                Arc::new(crate::compat::sync::Mutex::new(RuntimeModuleState::new(
                    source_heap,
                    Vec::new(),
                ))),
                Arc::new(crate::vm::Module::default()),
            ),
        );
        let slots = vec![
            GlobalSlot {
                name: Arc::<str>::from("missing"),
            },
            GlobalSlot {
                name: Arc::<str>::from("external"),
            },
        ];
        let mut dest_heap = HeapStore::new();

        let globals = seed_module_globals(&slots, &ctx, &mut dest_heap).expect("seed globals");

        assert_eq!(globals[0], RuntimeVal::Nil);
        let RuntimeVal::Obj(imported) = globals[1] else {
            panic!("external global should use as heap object");
        };
        assert!(matches!(dest_heap.get(imported), Some(HeapValue::String(value)) if value.as_ref() == "external"));
    }

    /// `-x` — negation of anything that is not a literal.
    ///
    /// The language had no negation operator at all: `UnaryOp` held only
    /// `Not`, and only the *lexer* could produce a negative number, by folding
    /// `-5` into an `Int(-5)` token where it could tell an operand was
    /// expected. So `-5` worked and `-x` was a syntax error in every position,
    /// with `0 - x` as the workaround. That workaround is also not a
    /// substitute: `0.0 - 0.0` is `+0.0` where `-(0.0)` is `-0.0`.
    #[test]
    fn negation_works_on_values_and_not_only_literals() {
        let source = "let i = 7;\n\
                      let f = 2.5;\n\
                      let z = 0.0;\n\
                      let neg = |v| -v;\n\
                      return [-i, -f, -(-i), neg(i), -z, -9223372036854775808];\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let outcome = super::execute_program(&program).expect("run");

        let RuntimeVal::Obj(handle) = *outcome.first_return() else {
            panic!("expected a list of results");
        };
        let Some(HeapValue::List(list)) = outcome.state.heap().get(handle) else {
            panic!("expected a heap list");
        };
        let items = list.collect_owned().expect("scalars only");
        assert_eq!(items[0], RuntimeVal::Int(-7));
        assert_eq!(items[1], RuntimeVal::Float(-2.5));
        assert_eq!(items[2], RuntimeVal::Int(7));
        assert_eq!(items[3], RuntimeVal::Int(-7));
        // The zero's *sign* survives, which is the whole reason this is a real
        // negation and not `0 - x`: the latter answers `+0.0` here. `==` cannot
        // see the difference, so ask for the sign bit.
        let RuntimeVal::Float(negative_zero) = items[4] else {
            panic!("expected a float");
        };
        assert!(
            negative_zero == 0.0 && negative_zero.is_sign_negative(),
            "-0.0 should keep its sign, got {negative_zero}"
        );
        // `i64::MIN`'s magnitude does not fit an `i64`, so the lexer still owns
        // this one; it has to keep agreeing with the operator.
        assert_eq!(items[5], RuntimeVal::Int(i64::MIN));
    }

    /// `if` produces a value, the way `match` always has.
    ///
    /// `let a = match c { … };` parsed and `let a = if c { … } else { … };` did
    /// not, so the only way to *choose* a value was the C-style ternary — the
    /// operator a language whose `if` is an expression does not need. Both now
    /// lower to the same node, so they cannot drift apart.
    #[test]
    fn if_is_an_expression_that_yields_its_branch() {
        let source = "let x = 5;\n\
                      let size = if x > 3 { \"big\" } else if x > 1 { \"mid\" } else { \"small\" };\n\
                      let doubled = if true { let t = x; t * 2 } else { 0 };\n\
                      let missing = if false { 1 };\n\
                      let pick = |v| if v > 0 { 1 } else { -1 };\n\
                      // Truthiness, not `Bool`: `0` is truthy, only nil and false are not.\n\
                      let zero_is_truthy = if 0 { \"yes\" } else { \"no\" };\n\
                      return [size, doubled, missing, pick(-9), zero_is_truthy];\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let outcome = super::execute_program(&program).expect("run");

        let RuntimeVal::Obj(handle) = *outcome.first_return() else {
            panic!("expected a list of results");
        };
        let Some(HeapValue::List(list)) = outcome.state.heap().get(handle) else {
            panic!("expected a heap list");
        };
        let items = list.collect_owned().expect("results are heap objects");
        let text = |value: &RuntimeVal| -> String {
            match value {
                RuntimeVal::ShortStr(s) => s.as_str().to_string(),
                RuntimeVal::Obj(h) => match outcome.state.heap().get(*h) {
                    Some(HeapValue::String(s)) => s.to_string(),
                    other => panic!("expected a string, got {other:?}"),
                },
                other => panic!("expected a string, got {other:?}"),
            }
        };
        assert_eq!(text(&items[0]), "big");
        assert_eq!(items[1], RuntimeVal::Int(10));
        // No `else` means no value: `nil`, not a parse error.
        assert_eq!(items[2], RuntimeVal::Nil);
        assert_eq!(items[3], RuntimeVal::Int(-1));
        assert_eq!(text(&items[4]), "yes");
    }

    /// `?.` calls a method, which is most of what it is for.
    ///
    /// `OptionalAccess` is a *read*, and the compiler lowers it as an index —
    /// so `s?.len()` indexed the string with the string `"len"` and failed at
    /// runtime with "String index must be Int". The null-safe operator did not
    /// work on the values it exists for; only field access on a struct or map
    /// went through. It is rewritten at parse time now, the way postfix `!`
    /// is, so the checker and the compiler both see ordinary constructs.
    #[test]
    fn optional_chaining_reaches_methods_and_stops_at_nil() {
        let source = "let present = \"abcd\";\n\
                      let m = {\"a\": \"xy\"};\n\
                      let missing = if false { \"abc\" };\n\
                      return [\n\
                        present?.len(), m.get(\"a\")?.len(), m.get(\"z\")?.len(),\n\
                        missing?.len(), missing?.len() ?? 0,\n\
                      ];\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let outcome = super::execute_program(&program).expect("run");

        let RuntimeVal::Obj(handle) = *outcome.first_return() else {
            panic!("expected a list of results");
        };
        let Some(HeapValue::List(list)) = outcome.state.heap().get(handle) else {
            panic!("expected a heap list");
        };
        let items = list.collect_owned().expect("scalars only");
        assert_eq!(items[0], RuntimeVal::Int(4));
        assert_eq!(items[1], RuntimeVal::Int(2));
        // The call does not happen at all when the receiver is nil.
        assert_eq!(items[2], RuntimeVal::Nil);
        assert_eq!(items[3], RuntimeVal::Nil);
        assert_eq!(items[4], RuntimeVal::Int(0));
    }

    /// An `if` with no `else` is a value that may be nil, not a contradiction.
    ///
    /// The missing branch is a synthesised `nil`, and the two arms were
    /// constrained to be *equal* — so `let r = if c { "a" };` reported "Cannot
    /// unify String with Nil" and the expression form could not do what the
    /// statement form does. `c ? "a" : nil` reads the same way and now gets the
    /// same answer: `String?`.
    #[test]
    fn an_if_without_else_is_optional_not_a_conflict() {
        fn check(source: &str) -> Result<(), String> {
            let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
            let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
            program
                .type_check(&mut crate::typ::TypeChecker::new())
                .map_err(|e| e.to_string())
        }

        for source in [
            "let c = true;\nlet r = if c { \"a\" };\n",
            "let c = true;\nlet r: String? = if c { \"a\" };\n",
            "let c = true;\nlet r = c ? \"a\" : nil;\n",
            "let c = true;\nlet r = c ? nil : \"a\";\n",
            // Both branches present and agreeing keeps the bare type.
            "let c = true;\nlet r: String = if c { \"a\" } else { \"b\" };\n",
        ] {
            check(source).unwrap_or_else(|e| panic!("{source} should check, said: {e}"));
        }

        let error = check("let c = true;\nlet r: String = if c { \"a\" };\n")
            .expect_err("a branch that may not run makes the value optional");
        assert!(error.contains("String?"), "should say String?, said: {error}");
    }

    /// A `match` that can miss is typed as able to miss.
    ///
    /// LK's rule is that an unmatched `match` evaluates to `nil` — deliberate,
    /// and tested. The *type* ignored it: the expression was typed as its
    /// arms' type, so
    ///
    /// ```text
    /// let r: String = match x { 1 => "one" };   // checked, held nil
    /// r.len()                                   // approved, failed at runtime
    /// ```
    ///
    /// A binding annotated `String` holding nil is the type system saying
    /// something untrue. It says `String?` now, and the shapes that cannot
    /// miss — a catch-all arm, or a `Bool` with both literals — keep the bare
    /// type so the common cases do not grow a `?`.
    #[test]
    fn a_match_that_can_miss_is_typed_as_nullable() {
        fn check(source: &str) -> Result<(), String> {
            let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
            let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
            program
                .type_check(&mut crate::typ::TypeChecker::new())
                .map_err(|e| e.to_string())
        }

        for source in [
            "let x = 5;\nlet r: String = match x { 1 => \"one\" };\n",
            "fn f(x: Int) -> String { return match x { 1 => \"one\" }; }\n",
        ] {
            let error = check(source).expect_err("a match that can miss is not a bare String");
            assert!(error.contains("String?"), "{source} should say String?, said: {error}");
        }

        for source in [
            // A catch-all arm always matches.
            "let x = 5;\nlet r: String = match x { 1 => \"a\", _ => \"b\" };\n",
            // A binding pattern is a catch-all too.
            "let x = 5;\nlet r: String = match x { 1 => \"a\", other => \"b\" };\n",
            // Both `Bool` literals cover every value of the type.
            "let b = true;\nlet r: Int = match b { true => 1, false => 2 };\n",
            // And the nullable type is writable when the miss is intended.
            "let x = 5;\nlet r: String? = match x { 1 => \"one\" };\n",
        ] {
            check(source).unwrap_or_else(|e| panic!("{source} should check, said: {e}"));
        }
    }

    /// A type declaration's position in the file does not matter.
    ///
    /// Function signatures were hoisted and type declarations were not, which
    /// nobody noticed while an undeclared name silently became `Type::Named`:
    /// the annotation checked against nothing either way. The moment unknown
    /// names became an error, `fn f() -> Point { … }` written above
    /// `struct Point { … }` — the ordinary way to put the interesting function
    /// first — started failing.
    #[test]
    fn a_type_declaration_can_come_after_its_use() {
        for source in [
            "fn f() -> Point { return Point { a: 1 }; }\nstruct Point { a: Int }\nreturn f().a;\n",
            "fn f(v: Point) -> Int { return v.a; }\nstruct Point { a: Int }\nreturn f(Point { a: 2 });\n",
            "fn f(v: Int) -> U { return v; }\ntype U = Int;\nreturn f(1);\n",
            "let s: Shown = 1;\ntype Shown = Int;\nreturn s;\n",
        ] {
            let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
            let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
            let mut checker = crate::typ::TypeChecker::new();
            program
                .type_check(&mut checker)
                .unwrap_or_else(|e| panic!("{source} should check, said: {e}"));
        }

        // A name nothing declares is still an error, wherever it appears. LK
        // has no generic parameters — `fn f<T>(…)` does not parse — so a bare
        // `T` is an undeclared name like any other.
        for source in ["fn f(v: T) -> T { return v; }\n", "let x: Nope = 1;\n"] {
            let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
            let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
            let error = program
                .type_check(&mut crate::typ::TypeChecker::new())
                .expect_err("an undeclared type name is an error");
            assert!(error.to_string().contains("Unknown type"), "got: {error}");
        }
    }

    /// A `type` alias works in every position, including across a module
    /// boundary.
    ///
    /// It is a second *spelling*, not a second type. It worked in a binding
    /// (`let x: U = 5`) and in a parameter (`fn f(v: U)`) and broke in exactly
    /// one place — the return type — with "Cannot unify U with Int", because
    /// the declared type went to the solver unresolved and the solver has no
    /// registry to look a name up in. Aliases also never crossed a module
    /// boundary at all: only `struct`s and `trait`s were seeded from an
    /// imported file.
    #[test]
    fn a_type_alias_is_a_spelling_not_a_type() {
        for source in [
            "type U = Int;\nlet x: U = 5;\nreturn x;\n",
            "type U = Int;\nfn f(v: U) -> Int { return v; }\nreturn f(3);\n",
            "type U = Int;\nfn f(v: Int) -> U { return v; }\nreturn f(3);\n",
            "type U = Int;\nfn f(v: Int) -> U { return v; }\nfn g(v: Int) -> U { return f(v); }\nreturn g(3);\n",
            "type Pair = List<Int>;\nfn f() -> Pair { return [1, 2]; }\nreturn f().len();\n",
        ] {
            let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
            let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
            let mut checker = crate::typ::TypeChecker::new();
            program
                .type_check(&mut checker)
                .unwrap_or_else(|e| panic!("{source} should check, said: {e}"));
        }

        // …and a genuine mismatch is still one.
        let source = "type U = Int;\nfn f(v: Int) -> U { return \"x\"; }\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let error = program
            .type_check(&mut crate::typ::TypeChecker::new())
            .expect_err("a String is not an Int by another name");
        assert!(error.to_string().contains("Return type mismatch"), "got: {error}");
    }

    /// A misspelled type name is reported where it is written.
    ///
    /// `Type::Named` is the parser's answer for any identifier in type
    /// position, so a typo became a type nothing declares — and the complaint
    /// landed on the *value*: `let x: Strng = "a";` said "expected Strng, but
    /// expression has type String", pointing away from the misspelling. A
    /// signature was worse: `fn f(v: Nonexistent)` made the function
    /// uncallable and blamed every caller.
    #[test]
    fn an_unknown_type_name_is_reported_at_the_annotation() {
        fn check_error(source: &str) -> String {
            let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
            let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
            let mut checker = crate::typ::TypeChecker::new();
            program
                .type_check(&mut checker)
                .expect_err("an undeclared type name is an error")
                .to_string()
        }

        // Every position an annotation can appear in. The bug repeated itself
        // one position at a time — binding, then parameter, then return, then
        // impl target, then trait method, then struct field — so the list is
        // the point of the test.
        for (source, expected) in [
            ("let x: Strng = \"a\";\n", "Unknown type 'Strng'"),
            ("fn f(v: Nonexistent) { return 1; }\n", "Unknown type 'Nonexistent'"),
            ("fn f() -> Bogus { return 1; }\n", "Unknown type 'Bogus'"),
            ("let x: List<Nope> = [1];\n", "Unknown type 'Nope'"),
            ("let x: Map<String, Nope> = {};\n", "Unknown type 'Nope'"),
            ("struct P { a: Nope }\n", "Unknown type 'Nope'"),
            ("trait T { fn f(self) -> Missing; }\n", "Unknown type 'Missing'"),
            ("trait T { fn f(self, v: Bogus) -> Int; }\n", "Unknown type 'Bogus'"),
            (
                "trait T { fn f(self) -> Int; }\nimpl T for Nonexistent { fn f(self) -> Int { return 1; } }\n",
                "Unknown type 'Nonexistent'",
            ),
        ] {
            let message = check_error(source);
            assert!(
                message.contains(expected),
                "{source} should name the type, said: {message}"
            );
        }

        // Declared names, builtins and documented runtime handles all pass.
        for source in [
            "let x: Int = 1;\n",
            "struct P { a: Int }\nlet p: P = P { a: 1 };\n",
            "trait T { fn f(self) -> Int; }\nfn g(v: T) -> Int { return 1; }\n",
            "let x: List<String> = [];\n",
            "let x: Map<String, Int> = {};\n",
        ] {
            let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
            let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
            let mut checker = crate::typ::TypeChecker::new();
            program
                .type_check(&mut checker)
                .unwrap_or_else(|e| panic!("{source} should check, said: {e}"));
        }
    }

    /// The declared arity is the arity — one source, not two.
    ///
    /// Each dispatcher stated its own in a `bail!` guard, so a method could
    /// accept a shape the checker rejected (or the reverse) and nothing said
    /// so. Three had drifted by the time anyone compared them by hand:
    /// `bytes.slice` (checker computed the wrong count for a named call),
    /// `map.get` (runtime took a default, the table declared one parameter),
    /// and `str.slice` (declared `end` required where every other sequence has
    /// it optional). Dispatch checks the declaration now, so a guard that
    /// disagrees is unreachable rather than quietly authoritative.
    #[test]
    fn a_methods_optional_arguments_are_the_declared_ones() {
        let source = "let m = {\"a\": 1};\n\
                      let text = \"abcd\";\n\
                      let xs = [10, 20, 30];\n\
                      return [\n\
                        m.get(\"z\", 9), m.get(\"a\", 9),\n\
                        text.slice(1).len(), text.slice(1, 3).len(), xs.slice(1).len(),\n\
                      ];\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let outcome = super::execute_program(&program).expect("run");

        let RuntimeVal::Obj(handle) = *outcome.first_return() else {
            panic!("expected a list of results");
        };
        let Some(HeapValue::List(list)) = outcome.state.heap().get(handle) else {
            panic!("expected a heap list");
        };
        let items = list.collect_owned().expect("ints only");
        assert_eq!(items[0], RuntimeVal::Int(9), "an absent key takes the default");
        assert_eq!(items[1], RuntimeVal::Int(1), "a present key ignores it");
        assert_eq!(items[2], RuntimeVal::Int(3), "slice without an end runs to the end");
        assert_eq!(items[3], RuntimeVal::Int(2));
        assert_eq!(items[4], RuntimeVal::Int(2));
    }

    /// A `String` is a sequence, and reads like one.
    ///
    /// `List`, `Slice` and `Bytes` were unified on `first`/`last`/`get`/
    /// `slice`/`take`/`skip`/`index_of`; `String` — a sequence of characters,
    /// which is what `len()` counts and `[i]` indexes — was left out. It had
    /// `substring(start, length)` and `find` instead, and `substring` is the
    /// reason this is more than tidiness: it takes a *length* where every
    /// `slice` takes an *end*, so `xs.slice(1, 3)` and `s.substring(1, 3)`
    /// cut different windows from the same numbers.
    #[test]
    fn a_string_reads_like_every_other_sequence() {
        let source = "let s = \"h\u{e9}llo\";\n\
                      return [\n\
                        s.slice(1, 3), s.take(2), s.skip(2),\n\
                        s.first(), s.last(), s.get(1),\n\
                        s.substring(1, 3),\n\
                      ];\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let outcome = super::execute_program(&program).expect("run");

        let RuntimeVal::Obj(handle) = *outcome.first_return() else {
            panic!("expected a list of results");
        };
        let Some(HeapValue::List(list)) = outcome.state.heap().get(handle) else {
            panic!("expected a heap list");
        };
        let items = list.collect_owned().expect("strings only");
        let text = |value: &RuntimeVal| -> String {
            match value {
                RuntimeVal::ShortStr(s) => s.as_str().to_string(),
                RuntimeVal::Obj(h) => match outcome.state.heap().get(*h) {
                    Some(HeapValue::String(s)) => s.to_string(),
                    other => panic!("expected a string, got {other:?}"),
                },
                other => panic!("expected a string, got {other:?}"),
            }
        };
        // `slice` counts to an *end*, so this is two characters — the same
        // window `[10, 20, 30, 40].slice(1, 3)` takes.
        assert_eq!(text(&items[0]), "él");
        assert_eq!(text(&items[1]), "hé");
        assert_eq!(text(&items[2]), "llo");
        assert_eq!(text(&items[3]), "h");
        assert_eq!(text(&items[4]), "o");
        assert_eq!(text(&items[5]), "é");
        // …and `substring` still counts a *length*, which is why it is on its
        // way out.
        assert_eq!(text(&items[6]), "éll");
    }

    /// `==`, `in`, and the constant folder answer the same question the same
    /// way.
    ///
    /// There were three answers to "is `1` equal to `1.0`":
    ///
    /// ```text
    /// println(1 == 1.0);                     → false   (constant folder)
    /// let a = 1; let b = 1.0; a == b;        → true    (runtime)
    /// 1 in [1.0];                            → false   (typed-list `in`)
    /// ```
    ///
    /// The folder used `LiteralVal`'s derived `PartialEq` — structural, so two
    /// variants are never equal — while contradicting its *own* ordering rule,
    /// which promotes: `1 <= 1.0 && 1 >= 1.0` folded to `true`. And `in`
    /// matched on the element's variant, so the answer depended on the list's
    /// internal representation, which no program can see.
    #[test]
    fn equality_answers_the_same_whoever_asks() {
        let source = "let a = 1;\n\
                      let b = 1.0;\n\
                      let ints = [1, 2];\n\
                      let floats = [1.0, 2.0];\n\
                      return [\n\
                        1 == 1.0, a == b, 1 <= 1.0 && 1 >= 1.0,\n\
                        a in floats, b in ints, 1 in floats, 1.0 in ints,\n\
                        1.5 in ints, a in ints,\n\
                      ];\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let outcome = super::execute_program(&program).expect("run");

        let RuntimeVal::Obj(handle) = *outcome.first_return() else {
            panic!("expected a list of answers");
        };
        let Some(HeapValue::List(list)) = outcome.state.heap().get(handle) else {
            panic!("expected a heap list");
        };
        let answers = list.collect_owned().expect("bools only");
        let expected = [true, true, true, true, true, true, true, false, true];
        for (index, want) in expected.iter().enumerate() {
            assert_eq!(
                answers[index],
                RuntimeVal::Bool(*want),
                "answer {index} disagrees with the others"
            );
        }
    }

    /// A container is a container for `in`, whatever inferred it.
    ///
    /// `in`'s type check listed `List`/`Map`/`Set` and nothing else, so a
    /// `String` (which contains substrings) and a `Tuple` (what a heterogeneous
    /// list *literal* infers to) were rejected — while indexing, `len()` and
    /// method dispatch took both. `"a" in "abc"` therefore worked as a folded
    /// literal and was a type error one line later through a variable.
    #[test]
    fn in_accepts_every_container_the_rest_of_the_language_does() {
        let source = "let text = \"abc\";\n\
                      let mixed = [1, \"a\"];\n\
                      return [\"b\" in text, \"z\" in text, \"a\" in mixed, 1 in mixed];\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let mut checker = crate::typ::TypeChecker::new();
        program
            .type_check(&mut checker)
            .expect("a String and a Tuple are containers");
        let outcome = super::execute_program(&program).expect("run");

        let RuntimeVal::Obj(handle) = *outcome.first_return() else {
            panic!("expected a list of answers");
        };
        let Some(HeapValue::List(list)) = outcome.state.heap().get(handle) else {
            panic!("expected a heap list");
        };
        let answers = list.collect_owned().expect("bools only");
        assert_eq!(answers[0], RuntimeVal::Bool(true));
        assert_eq!(answers[1], RuntimeVal::Bool(false));
        assert_eq!(answers[2], RuntimeVal::Bool(true));
        assert_eq!(answers[3], RuntimeVal::Bool(true));
    }

    /// The braced constructs agree on punctuation and on parentheses.
    ///
    /// Three rules used to differ for no reason any of them could explain:
    /// `while` *required* parentheses around its condition while `if` and
    /// `for` did not; and `match x { … }` / `unsafe { … }` as statements
    /// *required* a trailing `;` while `if c { … }` refused one. Same shape on
    /// the page, different punctuation.
    #[test]
    fn braced_constructs_agree_on_parentheses_and_semicolons() {
        let source = "let seen = [];\n\
                      let i = 0;\n\
                      while i < 3 { i = i + 1; }\n\
                      while (i < 6) { i = i + 1; }\n\
                      match i { 6 => { seen = seen.concat([\"matched\"]); }, _ => {} }\n\
                      unsafe { seen = seen.concat([\"unsafe\"]); }\n\
                      if i == 6 { seen = seen.concat([\"if\"]); }\n\
                      return [i, seen];\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let outcome = super::execute_program(&program).expect("run");

        let RuntimeVal::Obj(handle) = *outcome.first_return() else {
            panic!("expected a list of results");
        };
        let Some(HeapValue::List(list)) = outcome.state.heap().get(handle) else {
            panic!("expected a heap list");
        };
        let items = list.collect_owned().expect("results are heap objects");
        assert_eq!(items[0], RuntimeVal::Int(6), "both `while` forms should have run");
        let RuntimeVal::Obj(seen) = items[1] else {
            panic!("expected the marker list");
        };
        let Some(HeapValue::List(seen)) = outcome.state.heap().get(seen) else {
            panic!("expected the marker list");
        };
        assert_eq!(seen.len(), 3, "each statement after a closing brace should have run");
    }

    /// A braced construct ends a *statement*, never an operand.
    ///
    /// `match x { … } println("next");` is two statements; `return match x
    /// { … } == nil;` is one comparison. Stopping at the brace in both places
    /// would silently drop the `== nil` — an answer, not a syntax error.
    #[test]
    fn a_block_ends_a_statement_but_not_an_operand() {
        let source = "let compared = match 99 { 1 => \"one\", _ => nil } == nil;\n\
                      return compared;\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let outcome = super::execute_program(&program).expect("run");
        assert_eq!(*outcome.first_return(), RuntimeVal::Bool(true));
    }

    /// An `if` *statement* keeps working, and an `else` that belongs to one is
    /// still its own.
    ///
    /// The statement parser slices an expression up to the next top-level
    /// `else`, which was correct while `else` could only close a statement.
    /// Now it has to hand the `else` to an unmatched `if` inside the slice
    /// instead — and only a genuinely dangling one ends the expression.
    #[test]
    fn an_if_statement_still_owns_its_own_else() {
        let source = "let seen = [];\n\
                      if 1 > 2 { seen = seen.concat([\"then\"]); } else { seen = seen.concat([\"else\"]); }\n\
                      let nested = if true { if false { 1 } else { 2 } } else { 3 };\n\
                      return [seen, nested];\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let outcome = super::execute_program(&program).expect("run");

        let RuntimeVal::Obj(handle) = *outcome.first_return() else {
            panic!("expected a list of results");
        };
        let Some(HeapValue::List(list)) = outcome.state.heap().get(handle) else {
            panic!("expected a heap list");
        };
        let items = list.collect_owned().expect("results are heap objects");
        let RuntimeVal::Obj(branch) = items[0] else {
            panic!("expected the branch list");
        };
        let Some(HeapValue::List(branch)) = outcome.state.heap().get(branch) else {
            panic!("expected the branch list");
        };
        assert_eq!(branch.len(), 1, "exactly one branch should have run");
        assert_eq!(items[1], RuntimeVal::Int(2));
    }

    #[test]
    fn negating_a_non_number_is_a_type_error() {
        let tokens = crate::token::Tokenizer::tokenize("-\"text\"").expect("tokenize");
        let expr = crate::ast::Parser::new(&tokens).parse().expect("parse");
        let error = crate::typ::TypeChecker::new()
            .check_expr(&expr)
            .expect_err("negating a String has no answer");
        assert!(
            error.to_string().contains("numeric"),
            "the error should say the operand is not numeric, said: {error}"
        );
    }

    #[test]
    fn long_string_elements_survive_every_read_path() {
        // `ShortStr` inlines up to seven bytes. Every path that reads an
        // element out of a `TypedList::String` used to assume that was always
        // enough: the index fast path answered `Nil` for a longer element —
        // making `xs[0]` disagree with `xs.first()` about the same list — and
        // the slice path called `ShortStr::new(..).unwrap()` in the branch
        // reached exactly when it returns `None`, so `xs[0..2]` panicked.
        let source = "let xs = [\"aaaaaaaaaaaaaaaaaaaa\", \"bb\"];\n\
                      let seen = [];\n\
                      for x in xs { seen = seen.concat([x]); }\n\
                      return [xs[0], xs.get(0), xs.first(), xs[0..1], seen];\n";
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        let outcome = super::execute_program(&program).expect("run");

        let RuntimeVal::Obj(handle) = *outcome.first_return() else {
            panic!("expected a list of results");
        };
        let Some(HeapValue::List(list)) = outcome.state.heap().get(handle) else {
            panic!("expected a heap list");
        };
        let items = list
            .collect_owned()
            .expect("results are heap objects, not inline strings");

        let long = |value: &RuntimeVal| -> String {
            match value {
                RuntimeVal::Obj(handle) => match outcome.state.heap().get(*handle) {
                    Some(HeapValue::String(text)) => text.to_string(),
                    other => panic!("expected a heap string, got {other:?}"),
                },
                other => panic!("expected a heap string, got {other:?}"),
            }
        };
        assert_eq!(long(&items[0]), "aaaaaaaaaaaaaaaaaaaa", "xs[0]");
        assert_eq!(long(&items[1]), "aaaaaaaaaaaaaaaaaaaa", "xs.get(0)");
        assert_eq!(long(&items[2]), "aaaaaaaaaaaaaaaaaaaa", "xs.first()");
    }

    fn compile_source(source: &str) -> crate::vm::Module {
        let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
        let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
        crate::vm::Compiler::compile_module(&program).expect("compile")
    }

    fn function_index(module: &crate::vm::Module, name: &str) -> u32 {
        module
            .functions
            .iter()
            .position(|function| function.debug_name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("function `{name}` present")) as u32
    }

    #[test]
    fn call_module_function_keep_state_returns_live_heap_containers() {
        let module = compile_source("fn make(n) { return [n, \"a-long-string-over-7-bytes\", 2.5]; }\nreturn 0;\n");
        let index = function_index(&module, "make");
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let super::ModuleFunctionCall::Return(outcome) = super::call_module_function_with_ctx_keep_state(
            &module,
            index,
            &[super::ModuleFunctionArg::Int(7)],
            &mut ctx,
        )
        .expect("bridge call") else {
            panic!("expected a returning call");
        };

        let RuntimeVal::Obj(handle) = outcome.value else {
            panic!("expected a heap-backed list result");
        };
        let Some(HeapValue::List(list)) = outcome.state.heap().get(handle) else {
            panic!("result handle must stay live in the outcome state");
        };
        let items = list
            .collect_owned()
            .expect("the result list holds no inline-limited strings");
        assert_eq!(items[0], RuntimeVal::Int(7));
        let RuntimeVal::Obj(text) = items[1] else {
            panic!("expected the long string element on the heap");
        };
        assert!(matches!(
            outcome.state.heap().get(text),
            Some(HeapValue::String(value)) if value.as_ref() == "a-long-string-over-7-bytes"
        ));
        assert_eq!(items[2], RuntimeVal::Float(2.5));
    }

    #[test]
    fn call_module_function_keep_state_returns_live_map_in_iteration_order() {
        let module = compile_source("fn make() { return {\"alpha\": 1, \"beta\": 2, \"gamma\": 3}; }\nreturn 0;\n");
        let index = function_index(&module, "make");
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let super::ModuleFunctionCall::Return(outcome) =
            super::call_module_function_with_ctx_keep_state(&module, index, &[], &mut ctx).expect("bridge call")
        else {
            panic!("expected a returning call");
        };

        let RuntimeVal::Obj(handle) = outcome.value else {
            panic!("expected a heap-backed map result");
        };
        let Some(HeapValue::Map(map)) = outcome.state.heap().get(handle) else {
            panic!("result handle must stay live in the outcome state");
        };
        // The v2 bridge replays entries in this iteration order to reproduce
        // the VM's map layout natively. The exact order is the Fx layout's
        // (not insertion order; pinned end-to-end by the differential gates)
        // — what this walk must guarantee is completeness and a *stable*
        // full (key, value) sequence across repeated iterations.
        let entries = map.entries_iter();
        assert_eq!(entries, map.entries_iter(), "repeated walks must agree exactly");
        let mut pairs: alloc::vec::Vec<(String, RuntimeVal)> = entries
            .iter()
            .map(|(key, value)| (format!("{key:?}"), *value))
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let expected: alloc::vec::Vec<(String, RuntimeVal)> = [("alpha", 1), ("beta", 2), ("gamma", 3)]
            .into_iter()
            .map(|(key, value)| (format!("String({key:?})"), RuntimeVal::Int(value)))
            .collect();
        assert_eq!(pairs, expected);
    }

    #[test]
    fn call_module_function_invokes_named_function_with_args() {
        let module = compile_source("fn add(a, b) { return a + b; }\nreturn add(1, 2);\n");
        let index = function_index(&module, "add");
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let result = super::call_module_function_with_ctx(
            &module,
            index,
            &[super::ModuleFunctionArg::Int(2), super::ModuleFunctionArg::Int(40)],
            &mut ctx,
        )
        .expect("bridge call");
        assert_eq!(result, RuntimeVal::Int(42));
    }

    #[test]
    fn call_module_function_marshals_long_string_args_through_the_heap() {
        // 40 chars exceeds the inline short-string limit, forcing the
        // heap-string marshaling path.
        let module = compile_source("fn slen(s) { return s.len(); }\nreturn slen(\"x\");\n");
        let index = function_index(&module, "slen");
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let long = "a".repeat(40);
        let result =
            super::call_module_function_with_ctx(&module, index, &[super::ModuleFunctionArg::Str(long)], &mut ctx)
                .expect("bridge call");
        assert_eq!(result, RuntimeVal::Int(40));
    }

    #[test]
    fn call_module_function_propagates_runtime_errors() {
        // `% 0` is the VM's catchable arithmetic error — the bridge must
        // surface it as `Err`, matching an uncaught raise.
        let module = compile_source("fn boom(a) { return a % 0; }\nreturn 0;\n");
        let index = function_index(&module, "boom");
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let err = super::call_module_function_with_ctx(&module, index, &[super::ModuleFunctionArg::Int(1)], &mut ctx)
            .expect_err("mod-zero must error");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn deep_lk_recursion_grows_the_stack_instead_of_overflowing() {
        // Before segmented-stack growth, ~150 frames overflowed the Rust stack
        // in debug (test threads: 2MiB) and aborted the whole process; 30k
        // recursion now completes and stays under the call-depth cap.
        //
        // The depth tracks `DEFAULT_MAX_CALL_DEPTH`, which is deliberately far
        // lower under no_std: LK frames are a heap commitment, and an MCU heap
        // cannot absorb 30k of them.
        #[cfg(feature = "std")]
        let depth = 30000;
        #[cfg(not(feature = "std"))]
        let depth = 512;
        let module = compile_source(&format!(
            "fn f(n) {{ if (n == 0) {{ return 0; }} return f(n - 1); }}\nreturn f({depth});\n"
        ));
        let result = crate::vm::execute_module(&module).expect("deep recursion completes");
        assert_eq!(result.returns.first(), Some(&RuntimeVal::Int(0)));
    }

    #[test]
    fn call_depth_cap_raises_a_catchable_error() {
        let module = compile_source("fn f(n) { if (n == 0) { return 0; } return f(n - 1); }\nreturn f(100);\n");
        let module = alloc::sync::Arc::new(module);
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let register_count = module.entry_function().map(|f| f.register_count).unwrap_or_default();
        let err = crate::vm::Executor::new(register_count)
            .with_max_call_depth(50)
            .run_shared_module_with_globals_and_heap_and_ctx(
                alloc::sync::Arc::clone(&module),
                vec![RuntimeVal::Nil; module.globals.len()],
                HeapStore::new(),
                &mut ctx,
            )
            .expect_err("recursion beyond the cap must error, not abort");
        assert!(
            err.to_string().contains("call depth limit exceeded"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn call_module_function_rejects_out_of_bounds_index() {
        let module = compile_source("return 0;\n");
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let err = super::call_module_function_with_ctx(&module, 99, &[], &mut ctx)
            .expect_err("index out of bounds must error");
        assert!(err.to_string().contains("out of bounds"), "unexpected error: {err}");
    }

    #[test]
    fn move_batch_consumes_budget_per_move() {
        let mut function = Function {
            register_count: 4,
            ..Function::default()
        };
        let int_index = function.consts.push_int(7).expect("push int");
        function.code = vec![
            Instr::abx(Opcode::LoadInt, 0, int_index),
            Instr::abc(Opcode::Move, 1, 0, 0),
            Instr::abc(Opcode::Move, 2, 1, 0),
            Instr::abc(Opcode::Move, 3, 2, 0),
            Instr::abc(Opcode::Return, 3, 1, 0),
        ];
        let module = Arc::new(Module::single(function));

        let mut limited_ctx = VmContext::new_without_core_vm_builtins();
        let error = execute_compiled_module_with_ctx_and_budget(Arc::clone(&module), &mut limited_ctx, 3)
            .expect_err("three-instruction budget should not cover three moves after load");
        assert!(
            error.to_string().contains("execution step limit exceeded"),
            "unexpected error: {error}"
        );

        let mut enough_ctx = VmContext::new_without_core_vm_builtins();
        let result = execute_compiled_module_with_ctx_and_budget(module, &mut enough_ctx, 5)
            .expect("budget should count each batched Move and complete");
        assert_eq!(result.returns.first(), Some(&RuntimeVal::Int(7)));
    }
}

/// Test helpers for running a parsed program.
///
/// They live in the VM layer (and are re-exported from `stmt` for the existing
/// call sites) because running a program is execution: keeping them in `stmt`
/// meant the AST module depended on the executor even in test builds, which is
/// exactly the cycle this move removes.
#[cfg(test)]
pub mod test_support {
    use super::{ProgramExec, ProgramResult, VmContext};
    use crate::stmt::Program;
    use anyhow::Result;

    pub fn run_program(program: &Program, ctx: &mut VmContext) -> Result<ProgramResult> {
        program.execute_with_ctx(ctx)
    }

    pub fn run_program_default(program: &Program) -> Result<ProgramResult> {
        let mut ctx = VmContext::new();
        run_program(program, &mut ctx)
    }
}
