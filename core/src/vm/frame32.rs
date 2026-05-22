//! Safe frame/register window model for the VM rewrite.

use crate::val::RuntimeVal;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisterIndex(u16);

impl RegisterIndex {
    #[inline]
    pub const fn new(index: u16) -> Self {
        Self(index)
    }

    #[inline]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallWindow32 {
    pub callee: RegisterIndex,
    pub arg_count: u16,
    pub ret_count: u16,
}

impl CallWindow32 {
    #[inline]
    pub const fn new(callee: RegisterIndex, arg_count: u16, ret_count: u16) -> Self {
        Self {
            callee,
            arg_count,
            ret_count,
        }
    }

    #[inline]
    pub const fn arg_base(self) -> RegisterIndex {
        RegisterIndex(self.callee.0 + 1)
    }

    #[inline]
    pub const fn ret_base(self) -> RegisterIndex {
        self.callee
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame32 {
    regs: Vec<RuntimeVal>,
}

impl Frame32 {
    #[inline]
    pub fn new(register_count: u16) -> Self {
        Self {
            regs: vec![RuntimeVal::Nil; register_count as usize],
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.regs.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.regs.is_empty()
    }

    #[inline]
    pub fn read(&self, register: RegisterIndex) -> Option<&RuntimeVal> {
        self.regs.get(register.as_usize())
    }

    #[inline]
    pub fn write(&mut self, register: RegisterIndex, value: RuntimeVal) {
        let index = register.as_usize();
        assert!(index < self.regs.len(), "Frame32 register write out of bounds");
        self.regs[index] = value;
    }

    #[inline]
    pub fn take(&mut self, register: RegisterIndex) -> RuntimeVal {
        let index = register.as_usize();
        assert!(index < self.regs.len(), "Frame32 register take out of bounds");
        std::mem::take(&mut self.regs[index])
    }

    #[inline]
    pub fn clear_range(&mut self, start: RegisterIndex, len: u16) {
        let start = start.as_usize();
        let end = start + len as usize;
        assert!(end <= self.regs.len(), "Frame32 register clear out of bounds");
        self.regs[start..end].fill(RuntimeVal::Nil);
    }

    #[inline]
    pub fn call_args(&self, window: CallWindow32) -> &[RuntimeVal] {
        let start = window.arg_base().as_usize();
        let end = start + window.arg_count as usize;
        assert!(end <= self.regs.len(), "Frame32 call args out of bounds");
        &self.regs[start..end]
    }

    #[inline]
    pub fn copy_call_args_to_frame(&self, window: CallWindow32, callee: &mut Frame32) {
        let args = self.call_args(window);
        assert!(
            args.len() <= callee.regs.len(),
            "Frame32 callee frame too small for call args"
        );
        for (slot, value) in callee.regs.iter_mut().zip(args.iter().cloned()) {
            *slot = value;
        }
    }

    #[inline]
    pub fn write_returns(&mut self, window: CallWindow32, values: impl IntoIterator<Item = RuntimeVal>) {
        let start = window.ret_base().as_usize();
        let count = window.ret_count as usize;
        let end = start + count;
        assert!(end <= self.regs.len(), "Frame32 returns out of bounds");
        self.regs[start..end].fill(RuntimeVal::Nil);
        for (slot, value) in self.regs[start..end].iter_mut().zip(values) {
            *slot = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame32_initializes_registers_to_nil() {
        let frame = Frame32::new(3);

        assert_eq!(frame.len(), 3);
        assert_eq!(frame.read(RegisterIndex::new(0)), Some(&RuntimeVal::Nil));
        assert_eq!(frame.read(RegisterIndex::new(2)), Some(&RuntimeVal::Nil));
    }

    #[test]
    fn frame32_moves_values_with_take() {
        let mut frame = Frame32::new(2);
        frame.write(RegisterIndex::new(1), RuntimeVal::Int(42));

        let value = frame.take(RegisterIndex::new(1));

        assert_eq!(value, RuntimeVal::Int(42));
        assert_eq!(frame.read(RegisterIndex::new(1)), Some(&RuntimeVal::Nil));
    }

    #[test]
    fn call_window_uses_callee_followed_by_args_and_return_base() {
        let mut frame = Frame32::new(5);
        let window = CallWindow32::new(RegisterIndex::new(1), 2, 1);
        frame.write(RegisterIndex::new(2), RuntimeVal::Int(7));
        frame.write(RegisterIndex::new(3), RuntimeVal::Int(9));

        assert_eq!(frame.call_args(window), &[RuntimeVal::Int(7), RuntimeVal::Int(9)]);

        frame.write_returns(window, [RuntimeVal::Bool(true)]);
        assert_eq!(frame.read(RegisterIndex::new(1)), Some(&RuntimeVal::Bool(true)));
    }

    #[test]
    fn frame32_copies_call_args_into_callee_param_slots() {
        let mut caller = Frame32::new(4);
        let mut callee = Frame32::new(3);
        let window = CallWindow32::new(RegisterIndex::new(1), 2, 1);
        caller.write(RegisterIndex::new(2), RuntimeVal::Int(7));
        caller.write(RegisterIndex::new(3), RuntimeVal::Bool(true));

        caller.copy_call_args_to_frame(window, &mut callee);

        assert_eq!(callee.read(RegisterIndex::new(0)), Some(&RuntimeVal::Int(7)));
        assert_eq!(callee.read(RegisterIndex::new(1)), Some(&RuntimeVal::Bool(true)));
        assert_eq!(callee.read(RegisterIndex::new(2)), Some(&RuntimeVal::Nil));
    }
}
