use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;

use crate::{
    stmt::{
        Program,
        import::{collect_program_imports, execute_imports},
    },
    val::{HeapStore, RuntimeVal},
    vm::{Compiler32, GlobalSlot32, VmContext},
};

use super::{
    Program32Result, execute_module32, execute_module32_with_globals_heap_and_ctx, imports::import_runtime_export,
};

pub fn execute_program32(program: &Program) -> Result<Program32Result> {
    let mut ctx = VmContext::new_without_core_vm_builtins();
    execute_program32_raw_with_ctx(program, &mut ctx)
}

pub fn execute_program32_raw_with_ctx(program: &Program, ctx: &mut VmContext) -> Result<Program32Result> {
    let imports = collect_program_imports(program);
    let resolver = ctx.resolver().clone();
    execute_imports(&imports, resolver.as_ref(), ctx)?;

    let mut seed_heap = HeapStore::new();
    let mut external_globals = Vec::new();
    let mut external_values = BTreeMap::new();
    for (name, value) in ctx.runtime_globals_iter() {
        external_globals.push(name.clone());
        let value = import_runtime_export(value, &mut seed_heap)?;
        external_values.insert(name.clone(), value);
    }

    let module = Arc::new(Compiler32::compile_module_with_natives_and_globals(
        program,
        Vec::new(),
        external_globals,
    )?);
    let globals = seed_module_globals(&module.globals, external_values);
    let result = execute_module32_with_globals_heap_and_ctx(module.as_ref(), globals, seed_heap, ctx)?;
    Ok(Program32Result {
        returns: result.returns,
        state: result.state,
        module,
    })
}

pub fn execute_source32(source: &str) -> Result<Program32Result> {
    let module = Compiler32::compile_source_module(source)?;
    let result = execute_module32(&module)?;
    Ok(Program32Result {
        returns: result.returns,
        state: result.state,
        module: Arc::new(module),
    })
}

fn seed_module_globals(slots: &[GlobalSlot32], values: BTreeMap<Arc<str>, RuntimeVal>) -> Vec<RuntimeVal> {
    slots
        .iter()
        .map(|slot| values.get(&slot.name).cloned().unwrap_or(RuntimeVal::Nil))
        .collect()
}
