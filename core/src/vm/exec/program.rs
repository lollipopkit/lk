#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use anyhow::Result;

use crate::{
    stmt::{
        Program,
        import::{collect_program_imports, execute_imports},
    },
    syntax::{ParseOptions, parse_program_source},
    val::{HeapStore, RuntimeVal},
    vm::{Compiler, GlobalSlot, ModuleArtifact, VmContext},
};

use super::{Executor, ProgramResult, imports::import_runtime_export};

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

    Ok(Arc::new(Compiler::compile_module_with_natives_and_globals(
        program,
        Vec::new(),
        external_globals,
    )?))
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
    ctx.truncate_call_stack(0);
    let mut seed_heap = HeapStore::new();
    seed_heap.set_gc_threshold(gc_threshold);
    let globals = seed_module_globals(&module.globals, ctx, &mut seed_heap)?;
    let register_count = module
        .entry_function()
        .map(|function| function.register_count)
        .unwrap_or_default();
    let result = Executor::new(register_count).run_shared_module_with_globals_and_heap_and_ctx(
        Arc::clone(&module),
        globals,
        seed_heap,
        ctx,
    )?;
    Ok(ProgramResult {
        returns: result.returns,
        state: result.state,
        module,
    })
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
    // Start each top-level run with an empty traceback so a reused context
    // (REPL / embedded `Vm`) does not carry frames from a previous error.
    ctx.truncate_call_stack(0);
    let mut seed_heap = HeapStore::new();
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

fn seed_module_globals(slots: &[GlobalSlot], ctx: &VmContext, heap: &mut HeapStore) -> Result<Vec<RuntimeVal>> {
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
/// and closures are deliberately absent (see `docs/llvm/tier1-hybrid.md`).
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
/// the Tier 1 hybrid bridge entry (`docs/llvm/tier1-hybrid.md`): globals and
/// builtins are seeded exactly like a module run, but `function_index` is
/// invoked instead of the entry, against a fresh per-call state. Bridge-eligible
/// functions touch no user globals (the lowering proves it), so per-call state
/// is semantically invisible. The state rides along in the outcome so callers
/// can read heap-backed results before dropping it.
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
        let items = list.collect_owned();
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
        // the VM's map layout natively — the walk itself must be stable.
        let entries = map.entries_iter();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries.iter().map(|(_, value)| *value).collect::<alloc::vec::Vec<_>>(),
            map.entries_iter()
                .iter()
                .map(|(_, value)| *value)
                .collect::<alloc::vec::Vec<_>>(),
        );
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
        let module = compile_source("fn f(n) { if (n == 0) { return 0; } return f(n - 1); }\nreturn f(30000);\n");
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
