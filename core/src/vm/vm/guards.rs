use std::{cell::Cell, ptr};

use crate::vm::context::VmContext;

use super::Vm;

thread_local! {
    static CURRENT_VM: Cell<*mut Vm> = const { Cell::new(ptr::null_mut()) };
    static CURRENT_VM_CTX: Cell<*mut VmContext> = const { Cell::new(ptr::null_mut()) };
}

pub(crate) struct VmCurrentGuard {
    prev_vm: *mut Vm,
    prev_ctx: *mut VmContext,
}

impl VmCurrentGuard {
    pub(super) fn new(vm: *mut Vm, ctx: *mut VmContext) -> Self {
        let prev_vm = CURRENT_VM.with(|cell| {
            let prev = cell.get();
            cell.set(vm);
            prev
        });
        let prev_ctx = CURRENT_VM_CTX.with(|cell| {
            let prev = cell.get();
            cell.set(ctx);
            prev
        });
        Self { prev_vm, prev_ctx }
    }
}

impl Drop for VmCurrentGuard {
    fn drop(&mut self) {
        CURRENT_VM.with(|cell| cell.set(self.prev_vm));
        CURRENT_VM_CTX.with(|cell| cell.set(self.prev_ctx));
    }
}

pub(crate) fn with_current_vm<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Vm) -> R,
{
    CURRENT_VM.with(|cell| {
        let ptr = cell.get();
        if ptr.is_null() {
            None
        } else {
            // SAFETY: pointer is valid while VmCurrentGuard is active and provides exclusive access.
            Some(unsafe { f(&mut *ptr) })
        }
    })
}

#[cfg(test)]
pub(crate) fn with_current_vm_ctx<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut VmContext) -> R,
{
    CURRENT_VM_CTX.with(|cell| {
        let ptr = cell.get();
        if ptr.is_null() {
            None
        } else {
            // SAFETY: pointer is valid while VmCurrentGuard is active and provides exclusive access.
            Some(unsafe { f(&mut *ptr) })
        }
    })
}
