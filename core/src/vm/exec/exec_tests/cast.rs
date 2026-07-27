//! `expr as T` — the conversion semantics machine integers rest on.
//!
//! These pin the *truncating* reading of a cast. A cast is a request for those
//! bits at that width, not a range check, so `300 as u8` is 44 rather than an
//! error. The range check lives on the annotation path (`let x: u8 = 300`),
//! where a mistake is more likely than an intent.

use super::*;
use crate::vm::ir::CastTarget;

/// Runs `LoadInt src; CastTo dst <- src as target; Return dst`.
fn cast_int(value: i64, target: CastTarget) -> RuntimeVal {
    let function = Function {
        consts: ConstPool {
            ints: vec![value],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abc(Opcode::CastTo, 1, 0, target as u8),
            Instr::abc(Opcode::Return, 1, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![function],
        ..Module::default()
    };
    let result = crate::vm::execute_module(&module).expect("cast executes");
    result.returns.first().cloned().expect("a return value")
}

#[test]
fn narrowing_truncates_rather_than_erroring() {
    // 300 & 0xFF == 44.
    assert_eq!(cast_int(300, CastTarget::U8), RuntimeVal::Int(44));
    // 44 is below i8's sign bit, so the signed reading agrees here.
    assert_eq!(cast_int(300, CastTarget::I8), RuntimeVal::Int(44));
}

#[test]
fn unsigned_targets_reinterpret_negatives_as_their_bit_pattern() {
    assert_eq!(cast_int(-1, CastTarget::U8), RuntimeVal::Int(255));
    assert_eq!(cast_int(-1, CastTarget::U16), RuntimeVal::Int(65_535));
    assert_eq!(cast_int(-1, CastTarget::U32), RuntimeVal::Int(4_294_967_295));
}

/// The `i64` carrier is why sign extension has to happen at all: two values
/// equal as `i8` must stay equal as `RuntimeVal::Int`.
#[test]
fn signed_targets_sign_extend_back_into_the_carrier() {
    assert_eq!(cast_int(255, CastTarget::I8), RuntimeVal::Int(-1));
    assert_eq!(cast_int(128, CastTarget::I8), RuntimeVal::Int(-128));
    assert_eq!(cast_int(127, CastTarget::I8), RuntimeVal::Int(127));
    assert_eq!(cast_int(65_535, CastTarget::I16), RuntimeVal::Int(-1));
}

#[test]
fn full_width_targets_pass_the_value_through() {
    assert_eq!(cast_int(i64::MIN, CastTarget::I64), RuntimeVal::Int(i64::MIN));
    assert_eq!(cast_int(-1, CastTarget::Int), RuntimeVal::Int(-1));
    // Pointer width is the carrier's width on any host the VM runs on; a
    // 32-bit *deployment* target gets its real width from the AOT path.
    assert_eq!(cast_int(-1, CastTarget::Isize), RuntimeVal::Int(-1));
}

#[test]
fn casts_compose() {
    // `300 as u8 as u32` — the second cast sees 44, not 300.
    let function = Function {
        consts: ConstPool {
            ints: vec![300],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abc(Opcode::CastTo, 1, 0, CastTarget::U8 as u8),
            Instr::abc(Opcode::CastTo, 2, 1, CastTarget::U32 as u8),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![function],
        ..Module::default()
    };
    let result = crate::vm::execute_module(&module).expect("chained casts execute");
    assert_eq!(result.returns.first(), Some(&RuntimeVal::Int(44)));
}

#[test]
fn an_unknown_target_encoding_is_an_error_not_a_wrong_answer() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abc(Opcode::CastTo, 1, 0, 200),
            Instr::abc(Opcode::Return, 1, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![function],
        ..Module::default()
    };
    let err = crate::vm::execute_module(&module).expect_err("unknown encoding must fail");
    assert!(err.to_string().contains("unknown target encoding"), "{err}");
}
