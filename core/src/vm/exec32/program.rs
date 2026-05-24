use std::sync::Arc;

use anyhow::Result;

use crate::{
    stmt::{
        Program,
        import::{collect_program_imports, execute_imports},
    },
    val::{HeapStore, RuntimeVal},
    vm::{Compiler32, GlobalSlot32, Module32Artifact, VmContext},
};

use super::{Executor32, Program32Result, execute_module32, imports::import_runtime_export};

pub fn execute_program32(program: &Program) -> Result<Program32Result> {
    let mut ctx = VmContext::new_without_core_vm_builtins();
    execute_program32_with_ctx(program, &mut ctx)
}

pub fn compile_program32_module_with_ctx(program: &Program, ctx: &mut VmContext) -> Result<Arc<crate::vm::Module32>> {
    let imports = collect_program_imports(program);
    let resolver = ctx.resolver().clone();
    execute_imports(&imports, resolver.as_ref(), ctx)?;

    let mut external_globals = Vec::new();
    for (name, _) in ctx.runtime_globals_iter() {
        external_globals.push(name.clone());
    }

    Ok(Arc::new(Compiler32::compile_module_with_natives_and_globals(
        program,
        Vec::new(),
        external_globals,
    )?))
}

pub fn execute_program32_with_ctx(program: &Program, ctx: &mut VmContext) -> Result<Program32Result> {
    let module = compile_program32_module_with_ctx(program, ctx)?;
    execute_compiled_module32_with_ctx(module, ctx)
}

pub fn execute_module32_artifact_with_ctx(artifact: Module32Artifact, ctx: &mut VmContext) -> Result<Program32Result> {
    let imports = artifact.imports.clone();
    let resolver = ctx.resolver().clone();
    execute_imports(&imports, resolver.as_ref(), ctx)?;
    let module = Arc::new(artifact.into_module()?);
    execute_compiled_module32_with_ctx(module, ctx)
}

pub fn execute_compiled_module32_with_ctx(
    module: Arc<crate::vm::Module32>,
    ctx: &mut VmContext,
) -> Result<Program32Result> {
    let mut seed_heap = HeapStore::new();
    let globals = seed_module_globals(&module.globals, ctx, &mut seed_heap)?;
    let register_count = module
        .entry_function()
        .map(|function| function.register_count)
        .unwrap_or_default();
    let result = Executor32::new(register_count).run_shared_module_with_globals_and_heap_and_ctx(
        Arc::clone(&module),
        globals,
        seed_heap,
        ctx,
    )?;
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

fn seed_module_globals(slots: &[GlobalSlot32], ctx: &VmContext, heap: &mut HeapStore) -> Result<Vec<RuntimeVal>> {
    slots
        .iter()
        .map(|slot| match ctx.get_runtime_global(slot.name.as_ref()) {
            Some(export) => import_runtime_export(export, heap),
            None => Ok(RuntimeVal::Nil),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        val::{HeapStore, HeapValue, RuntimeVal},
        vm::{GlobalSlot32, RuntimeExport32, RuntimeModuleState32, VmContext},
    };

    use super::seed_module_globals;

    #[test]
    fn seed_module_globals_imports_by_module_slot_order_without_name_map() {
        let mut source_heap = HeapStore::new();
        let source_string = source_heap.alloc(HeapValue::String(Arc::<str>::from("external")));
        let mut ctx = VmContext::new_without_core_vm_builtins();
        ctx.define_runtime_global(
            "external",
            RuntimeExport32::new(
                RuntimeVal::Obj(source_string),
                Arc::new(std::sync::Mutex::new(RuntimeModuleState32::new(
                    source_heap,
                    Vec::new(),
                ))),
                Arc::new(crate::vm::Module32::default()),
            ),
        );
        let slots = vec![
            GlobalSlot32 {
                name: Arc::<str>::from("missing"),
            },
            GlobalSlot32 {
                name: Arc::<str>::from("external"),
            },
        ];
        let mut dest_heap = HeapStore::new();

        let globals = seed_module_globals(&slots, &ctx, &mut dest_heap).expect("seed globals");

        assert_eq!(globals[0], RuntimeVal::Nil);
        let RuntimeVal::Obj(imported) = globals[1] else {
            panic!("external global should import as heap object");
        };
        assert!(matches!(dest_heap.get(imported), Some(HeapValue::String(value)) if value.as_ref() == "external"));
    }
}
