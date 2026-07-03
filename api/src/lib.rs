//! `lk-api` — L5 host-embedding API for LK.
//!
//! A minimal, safe surface for embedding the LK VM in a Rust host. Each [`Vm`]
//! is an **isolated instance**: it owns its own `VmContext` (heap, globals,
//! async runtime handle), so multiple VMs are fully independent with no shared
//! global state — this is exactly what the M0 "去全局状态" work enabled. Add a
//! fuel budget to sandbox execution (the instruction-budget knob of M2.6).

use std::sync::Arc;

use anyhow::Result;
use lk_core::module::ModuleRegistry;
use lk_core::stmt::ModuleResolver;
use lk_core::syntax::{ParseOptions, parse_program_source};
use lk_core::typ::TypeChecker;
use lk_core::vm::{VmContext, execute_program_with_ctx_and_budget};

/// An isolated LK virtual machine instance.
pub struct Vm {
    ctx: VmContext,
    fuel: Option<u64>,
}

impl Vm {
    /// Create a VM with the full standard library registered.
    pub fn new() -> Self {
        let mut registry = ModuleRegistry::new();
        lk_stdlib::register_stdlib_globals(&mut registry);
        lk_stdlib::register_stdlib_modules(&mut registry).expect("stdlib registration should not fail");
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let ctx = VmContext::new()
            .with_resolver(resolver)
            .with_type_checker(Some(TypeChecker::new_strict()));
        Self { ctx, fuel: None }
    }

    /// Bound execution to `budget` instructions (fuel). Beyond it the VM aborts
    /// with a step-limit error instead of running unbounded (sandbox, plan M2.6).
    pub fn with_fuel(mut self, budget: u64) -> Self {
        self.fuel = Some(budget);
        self
    }

    /// Parse and execute `source`, returning the display of the program's first
    /// return value (empty string when it is `nil`).
    pub fn eval(&mut self, source: &str) -> Result<String> {
        let program = parse_program_source(source, ParseOptions::default())
            .map_err(|err| anyhow::anyhow!("parse error: {err}"))?;
        let result = match self.fuel {
            Some(budget) => execute_program_with_ctx_and_budget(&program, &mut self.ctx, budget)?,
            None => program.execute_with_ctx(&mut self.ctx)?,
        };
        if result.first_return_is_nil() {
            Ok(String::new())
        } else {
            Ok(result.display_first_return())
        }
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_returns_value() {
        let mut vm = Vm::new();
        assert_eq!(vm.eval("return 6 * 7;").unwrap(), "42");
    }

    #[test]
    fn instances_are_isolated() {
        // Two independent VMs share no global state (M0 去全局状态).
        let mut a = Vm::new();
        let mut b = Vm::new();
        assert_eq!(a.eval("let x = 10; return x;").unwrap(), "10");
        assert_eq!(b.eval("let y = 20; return y;").unwrap(), "20");
    }

    #[test]
    fn fuel_bounds_execution() {
        let mut vm = Vm::new().with_fuel(200);
        let err = vm
            .eval("let s = 0; for i in 1..=1000000 { s += i; } return s;")
            .expect_err("fuel-exhausted run should error");
        assert!(err.to_string().contains("step limit"), "unexpected error: {err}");
    }
}

/// C ABI surface (`ffi` feature). Opaque `Vm` pointer + eval returning an owned
/// C string; pair every `lk_vm_new`/`lk_vm_eval` with the matching free. A
/// header can be generated with cbindgen. Enables embedding from C/C++/Dart FFI
/// (plan M3.3).
#[cfg(feature = "ffi")]
pub mod ffi {
    use core::ffi::{CStr, c_char};

    use alloc::boxed::Box;
    use alloc::ffi::CString;

    extern crate alloc;

    use super::Vm;

    /// Create a new isolated VM. Free with [`lk_vm_free`].
    #[unsafe(no_mangle)]
    pub extern "C" fn lk_vm_new() -> *mut Vm {
        Box::into_raw(Box::new(Vm::new()))
    }

    /// Evaluate `src` (a NUL-terminated UTF-8 string) on `vm`, returning an owned
    /// C string with the first return value's display (free with [`lk_string_free`]),
    /// or NULL on error/invalid input.
    ///
    /// # Safety
    /// `vm` must come from [`lk_vm_new`] and not be freed; `src` must be a valid
    /// NUL-terminated string valid for the call.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn lk_vm_eval(vm: *mut Vm, src: *const c_char) -> *mut c_char {
        if vm.is_null() || src.is_null() {
            return core::ptr::null_mut();
        }
        let vm = unsafe { &mut *vm };
        let Ok(src) = (unsafe { CStr::from_ptr(src) }).to_str() else {
            return core::ptr::null_mut();
        };
        match vm.eval(src) {
            Ok(out) => CString::new(out)
                .map(CString::into_raw)
                .unwrap_or(core::ptr::null_mut()),
            Err(_) => core::ptr::null_mut(),
        }
    }

    /// Free a VM created by [`lk_vm_new`].
    ///
    /// # Safety
    /// `vm` must come from [`lk_vm_new`] and not be used afterwards.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn lk_vm_free(vm: *mut Vm) {
        if !vm.is_null() {
            drop(unsafe { Box::from_raw(vm) });
        }
    }

    /// Free a string returned by [`lk_vm_eval`].
    ///
    /// # Safety
    /// `s` must come from [`lk_vm_eval`] and not be used afterwards.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn lk_string_free(s: *mut c_char) {
        if !s.is_null() {
            drop(unsafe { CString::from_raw(s) });
        }
    }
}
