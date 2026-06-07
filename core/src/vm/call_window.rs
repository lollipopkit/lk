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
pub struct CallWindow {
    pub callee: RegisterIndex,
    pub arg_count: u16,
    pub ret_count: u16,
}

impl CallWindow {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_window_uses_callee_followed_by_args_and_return_base() {
        let window = CallWindow::new(RegisterIndex::new(3), 2, 1);

        assert_eq!(window.callee.as_usize(), 3);
        assert_eq!(window.arg_base().as_usize(), 4);
        assert_eq!(window.ret_base().as_usize(), 3);
    }
}
