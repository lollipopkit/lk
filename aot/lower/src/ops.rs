use super::*;

pub(crate) fn int_bin_op(op: Opcode) -> IntBinOp {
    match op {
        Opcode::AddInt => IntBinOp::Add,
        Opcode::SubInt => IntBinOp::Sub,
        Opcode::MulInt => IntBinOp::Mul,
        Opcode::DivInt => IntBinOp::Div,
        Opcode::ModInt => IntBinOp::Mod,
        _ => unreachable!("integer arithmetic opcode"),
    }
}

pub(crate) fn imm_int_bin_op(op: Opcode) -> IntBinOp {
    match op {
        Opcode::AddIntI => IntBinOp::Add,
        Opcode::MulIntI => IntBinOp::Mul,
        Opcode::ModIntI => IntBinOp::Mod,
        _ => unreachable!("immediate integer opcode"),
    }
}

/// The float form of an `AddInt`/…/`ModInt` opcode (used when runtime dispatch
/// selects float arithmetic for a float/mixed operand pair).
pub(crate) fn int_to_float_bin_op(op: Opcode) -> FloatBinOp {
    match op {
        Opcode::AddInt => FloatBinOp::Add,
        Opcode::SubInt => FloatBinOp::Sub,
        Opcode::MulInt => FloatBinOp::Mul,
        Opcode::DivInt => FloatBinOp::Div,
        Opcode::ModInt => FloatBinOp::Mod,
        _ => unreachable!("integer arithmetic opcode"),
    }
}

pub(crate) fn float_bin_op(op: Opcode) -> FloatBinOp {
    match op {
        Opcode::AddFloat => FloatBinOp::Add,
        Opcode::SubFloat => FloatBinOp::Sub,
        Opcode::MulFloat => FloatBinOp::Mul,
        Opcode::DivFloat => FloatBinOp::Div,
        Opcode::ModFloat => FloatBinOp::Mod,
        _ => unreachable!("float arithmetic opcode"),
    }
}

pub(crate) fn cmp_op(op: Opcode) -> CmpOp {
    match op {
        Opcode::CmpInt => CmpOp::Eq,
        Opcode::CmpNeInt => CmpOp::Ne,
        Opcode::CmpLtInt => CmpOp::Lt,
        Opcode::CmpLeInt => CmpOp::Le,
        Opcode::CmpGtInt => CmpOp::Gt,
        Opcode::CmpGeInt => CmpOp::Ge,
        _ => unreachable!("integer compare opcode"),
    }
}

pub(crate) fn test_cmp_op(op: Opcode) -> Option<CmpOp> {
    Some(match op {
        Opcode::TestEqInt | Opcode::TestEqIntI => CmpOp::Eq,
        Opcode::TestNeInt | Opcode::TestNeIntI => CmpOp::Ne,
        Opcode::TestLtInt | Opcode::TestLtIntI => CmpOp::Lt,
        Opcode::TestLeInt | Opcode::TestLeIntI => CmpOp::Le,
        Opcode::TestGtInt | Opcode::TestGtIntI => CmpOp::Gt,
        Opcode::TestGeInt | Opcode::TestGeIntI => CmpOp::Ge,
        _ => return None,
    })
}
