use anyhow::Result;

use super::Val;
use crate::vm::VmContext;

#[derive(Clone, Copy)]
pub struct NativeArgs<'a> {
    args: &'a [Val],
}

impl<'a> NativeArgs<'a> {
    #[inline]
    pub fn new(args: &'a [Val]) -> Self {
        Self { args }
    }

    #[inline]
    pub fn as_slice(self) -> &'a [Val] {
        self.args
    }

    #[inline]
    pub fn len(self) -> usize {
        self.args.len()
    }

    #[inline]
    pub fn get(self, index: usize) -> Option<&'a Val> {
        self.args.get(index)
    }
}

/// Native fastcall function type. It receives a register-backed argument window.
pub type RustFastFunction = for<'a> fn(args: NativeArgs<'a>, ctx: &mut VmContext) -> Result<Val>;

/// Native fastcall function type for call sites that carry named arguments.
pub type RustFastFunctionNamed =
    for<'a, 'b> fn(args: NativeArgs<'a>, named: &'b [(String, Val)], ctx: &mut VmContext) -> Result<Val>;
