use super::*;
use crate::compat::sync::Mutex;
use alloc::sync::Arc;

use crate::{
    val::{CallableValue, HeapRef, HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, ShortStr, TypedList, TypedMap},
    vm::{
        ConstHeapValue, ConstPool, Instr, NativeArgs, NativeEntry, NativeFunction, NativeRuntime, Opcode,
        RuntimeCallable, VmContext,
    },
};

mod attributes;
mod basic;
mod calls;
mod cast;
mod container;
mod cross_heap;
mod gc_cell_error;
mod gc_host_roots;
mod native;

/// Run `source` against a context carrying `natives`, the way a program reaches
/// a native for real.
///
/// Every stdlib native arrives as a **global** holding a
/// `CallableValue::RuntimeNative` — `VmContext::install_runtime_builtin` puts
/// it there, and the compiler resolves the name to a global slot the loader
/// seeds. Several tests in here instead built a `Module` with an inline
/// `NativeEntry` and a `LoadNative` instruction, which no binary can produce:
/// every production caller of `compile_module_with_natives*` passes an empty
/// table, so `LoadNative` is never emitted outside these tests.
///
/// The hand-built route cannot be pointed at an installed native either —
/// `execute_module_with_globals_and_ctx` does not seed the module's global
/// vector from the context, so it answers `module expected 1 globals, got 0`.
/// Hence a helper rather than a one-line substitution.
#[cfg(test)]
pub(crate) fn execute_source_with_natives(
    source: &str,
    natives: &[(&str, NativeFunction, u16)],
) -> anyhow::Result<crate::vm::ProgramResult> {
    execute_source_with_natives_and_gc(source, natives, None)
}

/// As [`execute_source_with_natives`], collecting after every `threshold`
/// allocations — what a hand-built module got by seeding its own `HeapStore`.
#[cfg(test)]
pub(crate) fn execute_source_with_natives_and_gc(
    source: &str,
    natives: &[(&str, NativeFunction, u16)],
    gc_threshold: Option<u32>,
) -> anyhow::Result<crate::vm::ProgramResult> {
    use crate::vm::ProgramExec;

    let mut ctx = VmContext::new();
    for (name, function, arity) in natives {
        ctx.install_runtime_builtin(name, function.clone(), *arity);
    }
    let program = crate::syntax::parse_program_source(source, Default::default())?;
    match gc_threshold {
        Some(threshold) => crate::vm::execute_program_with_ctx_and_gc_threshold(&program, &mut ctx, threshold),
        None => program.execute_with_ctx(&mut ctx),
    }
}
