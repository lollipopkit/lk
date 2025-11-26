use std::{cell::Cell, mem, ptr};

use crate::vm::context::VmContext;

use super::Vm;
use super::frame::RegisterWindowRef;

pub(crate) struct VmNestedCallGuard {
    vm: *mut Vm,
    parent_window: RegisterWindowRef,
}

impl VmNestedCallGuard {
    pub(super) fn new(vm: &mut Vm) -> Self {
        let parent_regs = mem::take(&mut vm.regs);
        vm.reg_stack.push(parent_regs);
        let next_regs = vm.reg_pool.pop().unwrap_or_default();
        vm.regs = next_regs;
        vm.regs.clear();
        let stack_index = vm
            .reg_stack
            .len()
            .checked_sub(1)
            .expect("reg_stack should contain parent registers after push");
        let parent_window = RegisterWindowRef::StackIndex(stack_index);
        vm.update_top_caller_window(parent_window);
        Self {
            vm: vm as *mut Vm,
            parent_window,
        }
    }

    pub(super) fn parent_window(&self) -> RegisterWindowRef {
        self.parent_window
    }
}

impl Drop for VmNestedCallGuard {
    fn drop(&mut self) {
        // SAFETY: guard lifetime ensures VM outlives the guard.
        unsafe {
            let vm = &mut *self.vm;
            let callee_regs = mem::take(&mut vm.regs);
            if !callee_regs.is_empty() {
                vm.reg_pool.push(callee_regs);
            }
            let parent_regs = vm.reg_stack.pop().unwrap_or_default();
            vm.regs = parent_regs;
        }
    }
}

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
