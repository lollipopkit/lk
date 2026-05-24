use crate::vm::{Instr32, Opcode32};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NativeScalarKind {
    I64,
    F64,
    Bool,
    Nil,
}

pub(super) struct NativeScalarFacts {
    registers_before: Vec<Vec<Option<NativeScalarKind>>>,
    globals_before: Vec<Vec<Option<NativeScalarKind>>>,
}

impl NativeScalarFacts {
    pub(super) fn register_kind_before(&self, pc: usize, reg: u8) -> Option<NativeScalarKind> {
        self.registers_before
            .get(pc)
            .and_then(|kinds| kinds.get(reg as usize))
            .copied()
            .flatten()
    }

    pub(super) fn global_kind_before(&self, pc: usize, slot: u16) -> Option<NativeScalarKind> {
        self.globals_before
            .get(pc)
            .and_then(|kinds| kinds.get(slot as usize))
            .copied()
            .flatten()
    }
}

impl NativeScalarKind {
    pub(super) const fn llvm_type(self) -> &'static str {
        match self {
            Self::F64 => "double",
            Self::I64 | Self::Bool | Self::Nil => "i64",
        }
    }

    pub(super) const fn is_numeric(self) -> bool {
        matches!(self, Self::I64 | Self::F64)
    }
}

pub(super) fn native_scalar_block_facts(
    register_count: usize,
    global_count: usize,
    code: &[Instr32],
) -> Option<NativeScalarFacts> {
    let mut kinds = vec![None; register_count];
    let mut global_kinds = vec![None; global_count];
    let mut registers_before = Vec::with_capacity(code.len());
    let mut globals_before = Vec::with_capacity(code.len());
    for instr in code {
        registers_before.push(kinds.clone());
        globals_before.push(global_kinds.clone());
        match instr.opcode() {
            Opcode32::Nop | Opcode32::Jmp => {}
            Opcode32::LoadNil => {
                if !set_native_kind(&mut kinds, instr.a(), NativeScalarKind::Nil) {
                    return None;
                }
            }
            Opcode32::LoadInt => {
                if !set_native_kind(&mut kinds, instr.a(), NativeScalarKind::I64) {
                    return None;
                }
            }
            Opcode32::LoadFloat => {
                if !set_native_kind(&mut kinds, instr.a(), NativeScalarKind::F64) {
                    return None;
                }
            }
            Opcode32::LoadBool => {
                if !set_native_kind(&mut kinds, instr.a(), NativeScalarKind::Bool) {
                    return None;
                }
            }
            Opcode32::Move => {
                let Some(kind) = native_kind(&kinds, instr.b()) else {
                    return None;
                };
                if !set_native_kind(&mut kinds, instr.a(), kind) {
                    return None;
                }
            }
            Opcode32::AddFloat | Opcode32::SubFloat | Opcode32::MulFloat | Opcode32::DivFloat | Opcode32::ModFloat => {
                if native_kind(&kinds, instr.b()) != Some(NativeScalarKind::F64)
                    || native_kind(&kinds, instr.c()) != Some(NativeScalarKind::F64)
                    || !set_native_kind(&mut kinds, instr.a(), NativeScalarKind::F64)
                {
                    return None;
                }
            }
            Opcode32::AddInt | Opcode32::SubInt | Opcode32::MulInt | Opcode32::DivInt | Opcode32::ModInt => {
                if native_kind(&kinds, instr.b()) != Some(NativeScalarKind::I64)
                    || native_kind(&kinds, instr.c()) != Some(NativeScalarKind::I64)
                    || !set_native_kind(&mut kinds, instr.a(), NativeScalarKind::I64)
                {
                    return None;
                }
            }
            Opcode32::CmpInt
            | Opcode32::CmpNeInt
            | Opcode32::CmpLtInt
            | Opcode32::CmpLeInt
            | Opcode32::CmpGtInt
            | Opcode32::CmpGeInt => {
                let Some(lhs) = native_kind(&kinds, instr.b()) else {
                    return None;
                };
                let Some(rhs) = native_kind(&kinds, instr.c()) else {
                    return None;
                };
                if (lhs != rhs || !lhs.is_numeric()) && !matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt)
                {
                    return None;
                }
                if matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt)
                    && lhs != rhs
                    && (lhs.is_numeric() || rhs.is_numeric())
                {
                    return None;
                }
                if !set_native_kind(&mut kinds, instr.a(), NativeScalarKind::Bool) {
                    return None;
                }
            }
            Opcode32::Test => {
                if native_kind(&kinds, instr.a()).is_none() {
                    return None;
                }
            }
            Opcode32::Not => {
                let Some(kind) = native_kind(&kinds, instr.b()) else {
                    return None;
                };
                if !matches!(kind, NativeScalarKind::Bool | NativeScalarKind::Nil)
                    || !set_native_kind(&mut kinds, instr.a(), NativeScalarKind::Bool)
                {
                    return None;
                }
            }
            Opcode32::IsNil => {
                if native_kind(&kinds, instr.b()).is_none()
                    || !set_native_kind(&mut kinds, instr.a(), NativeScalarKind::Bool)
                {
                    return None;
                }
            }
            Opcode32::GetGlobal => {
                let Some(kind) = native_global_kind(&global_kinds, instr.bx()) else {
                    return None;
                };
                if !set_native_kind(&mut kinds, instr.a(), kind) {
                    return None;
                }
            }
            Opcode32::SetGlobal => {
                let Some(kind) = native_kind(&kinds, instr.a()) else {
                    return None;
                };
                if !set_native_global_kind(&mut global_kinds, instr.bx(), kind) {
                    return None;
                }
            }
            Opcode32::Return => {
                if instr.b() > 1 {
                    return None;
                }
                if instr.b() == 1 && native_kind(&kinds, instr.a()).is_none() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(NativeScalarFacts {
        registers_before,
        globals_before,
    })
}

fn native_kind(kinds: &[Option<NativeScalarKind>], reg: u8) -> Option<NativeScalarKind> {
    kinds.get(reg as usize).copied().flatten()
}

fn set_native_kind(kinds: &mut [Option<NativeScalarKind>], reg: u8, kind: NativeScalarKind) -> bool {
    let Some(slot) = kinds.get_mut(reg as usize) else {
        return false;
    };
    *slot = Some(kind);
    true
}

fn native_global_kind(kinds: &[Option<NativeScalarKind>], slot: u16) -> Option<NativeScalarKind> {
    kinds.get(slot as usize).copied().flatten()
}

fn set_native_global_kind(kinds: &mut [Option<NativeScalarKind>], slot: u16, kind: NativeScalarKind) -> bool {
    let Some(slot) = kinds.get_mut(slot as usize) else {
        return false;
    };
    *slot = Some(kind);
    true
}
