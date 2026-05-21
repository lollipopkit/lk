mod tests {
    use super::super::*;

    use crate::vm::bc32::Bc32Function;

    fn function_with(code: Vec<Op>) -> Function {
        Function {
            consts: vec![Val::Int(1)],
            code,
            n_regs: 8,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }

    #[test]
    fn packed_hot_slot_decodes_cmove_int() {
        let function = function_with(vec![Op::CMoveInt {
            dst: 0,
            src: 1,
            a: 1,
            b: 0,
            kind: crate::vm::IntCmpKind::Lt,
        }]);
        let bc = Bc32Function::try_from_function(&function).expect("CMoveInt must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("CMoveInt hot slot");

        assert!(matches!(slot.kind, PackedHotKind::CMoveInt { .. }));
        assert_eq!(slot.next_pc, 4);
    }

    #[test]
    fn packed_hot_slot_fuses_dynamic_compare_followed_by_jmp_false() {
        let function = function_with(vec![
            Op::CmpLt(2, 0, 1),
            Op::JmpFalse(2, 2),
            Op::LoadK(3, 0),
            Op::Ret { base: 3, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("cmp+jmp must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("cmp hot slot");

        match slot.kind {
            PackedHotKind::CmpJmp {
                op: PackedCmpOp::Lt,
                a: 0,
                b: 1,
                ofs,
            } => assert_eq!(ofs, 3),
            _ => panic!("expected CmpJmp hot slot"),
        }
        assert_eq!(slot.next_pc, 2, "fused slot must skip the following JmpFalse word");
    }

    #[test]
    fn packed_hot_slot_uses_nop_elided_source_pc() {
        let function = function_with(vec![Op::Nop, Op::Ret { base: 0, retc: 1 }]);
        let bc = Bc32Function::try_from_function(&function).expect("nop must be BC32 encodable");
        let decoded = bc.decoded.as_deref().expect("decoded table");

        assert_eq!(bc.code32.len(), 1);
        assert!(matches!(decoded.instrs[0].op, Op::Ret { base: 0, retc: 1 }));
        assert_eq!(decoded.instrs[0].source_pc, 1);
    }

    #[test]
    fn packed_hot_slot_fuses_cmp_int_jmp_followed_by_move() {
        let function = function_with(vec![
            Op::CmpIntJmp {
                kind: crate::vm::IntCmpKind::Lt,
                a: 0,
                b: 1,
                ofs: 2,
            },
            Op::Move(2, 0),
            Op::Ret { base: 2, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("cmp-int-jmp+move must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("cmp hot slot");
        let next_pc = slot.next_pc;

        match slot.kind {
            PackedHotKind::CmpIntMove {
                op: PackedCmpOp::Lt,
                a: 0,
                b: 1,
                dst: 2,
                src: 0,
                ofs,
            } => assert_eq!(ofs as usize, next_pc),
            _ => panic!("expected CmpIntMove hot slot"),
        }
        assert_eq!(next_pc, 4, "fused slot must skip compare words and following Move word");
    }

    #[test]
    fn packed_hot_slot_fuses_int_arith_cmp_int_jmp_followed_by_move() {
        let function = function_with(vec![
            Op::SubInt(2, 0, 1),
            Op::CmpIntJmp {
                kind: crate::vm::IntCmpKind::Gt,
                a: 2,
                b: 3,
                ofs: 2,
            },
            Op::Move(4, 2),
            Op::Ret { base: 4, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("sub-int+cmp-int-jmp+move must encode");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("arith cmp move hot slot");

        match slot.kind {
            PackedHotKind::IntArithCmpIntMove {
                arith_op: PackedArithOp::Sub,
                arith_dst: 2,
                arith_a: 0,
                arith_b: 1,
                cmp_op: PackedCmpOp::Gt,
                cmp_a: 2,
                cmp_b: 3,
                move_dst: 4,
                move_src: 2,
            } => {}
            PackedHotKind::IntArithCmpIntJmp { .. } => {
                panic!("expected IntArithCmpIntMove hot slot, got IntArithCmpIntJmp")
            }
            PackedHotKind::CmpIntMove { .. } => panic!("expected IntArithCmpIntMove hot slot, got CmpIntMove"),
            other => panic!("expected IntArithCmpIntMove hot slot, got {other:?}"),
        }
        assert_eq!(
            slot.next_pc,
            bc.decoded.as_ref().unwrap().instrs[2].next_pc,
            "fused slot must skip arith, compare and following Move"
        );
    }

    #[test]
    fn packed_hot_slot_fuses_cmp_int_jmp_followed_by_add_int_imm() {
        let function = function_with(vec![
            Op::CmpIntJmp {
                kind: crate::vm::IntCmpKind::Lt,
                a: 0,
                b: 1,
                ofs: 3,
            },
            Op::AddIntImm(2, 3, 1),
            Op::Jmp(2),
            Op::AddIntImm(4, 5, -1),
            Op::Ret { base: 2, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("cmp-int-jmp+add-imm must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("cmp hot slot");

        match slot.kind {
            PackedHotKind::CmpIntAddIntImm {
                op: PackedCmpOp::Lt,
                a: 0,
                b: 1,
                dst: 2,
                src: 3,
                imm: 1,
                ofs: 5,
            } => {}
            PackedHotKind::CmpIntJmp { .. } => panic!("expected CmpIntAddIntImm hot slot, got CmpIntJmp"),
            PackedHotKind::CmpIntMove { .. } => panic!("expected CmpIntAddIntImm hot slot, got CmpIntMove"),
            PackedHotKind::AddIntImm { .. } => panic!("expected CmpIntAddIntImm hot slot, got AddIntImm"),
            PackedHotKind::AddIntImmJmp { .. } => panic!("expected CmpIntAddIntImm hot slot, got AddIntImmJmp"),
            PackedHotKind::Jmp { .. } => panic!("expected CmpIntAddIntImm hot slot, got Jmp"),
            _ => panic!("expected CmpIntAddIntImm hot slot"),
        }
        assert_eq!(
            slot.next_pc,
            bc.decoded.as_ref().unwrap().instrs[1].next_pc,
            "fused slot must skip compare words and following AddIntImm word"
        );
    }

    #[test]
    fn packed_hot_slot_fuses_cmp_int_jmp_followed_by_sub_access_sub() {
        let function = function_with(vec![
            Op::CmpIntJmp {
                kind: crate::vm::IntCmpKind::Ge,
                a: 0,
                b: 1,
                ofs: 4,
            },
            Op::SubInt(4, 0, 1),
            Op::Access(5, 2, 4),
            Op::SubInt(3, 3, 5),
            Op::Ret { base: 3, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("cmp-int-jmp+sub-access-sub must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("cmp hot slot");
        let next_pc = slot.next_pc;

        match slot.kind {
            PackedHotKind::CmpIntSubAccessSub {
                op: PackedCmpOp::Ge,
                a: 0,
                b: 1,
                first_dst: 4,
                first_a: 0,
                first_b: 1,
                access_dst: 5,
                access_base: 2,
                access_field: 4,
                final_dst: 3,
                final_a: 3,
                final_b: 5,
                ofs,
                ..
            } => assert_eq!(ofs as usize, next_pc),
            PackedHotKind::CmpIntJmp { .. } => panic!("expected CmpIntSubAccessSub hot slot, got CmpIntJmp"),
            _ => panic!("expected CmpIntSubAccessSub hot slot"),
        }
        assert_eq!(
            next_pc,
            bc.decoded.as_ref().unwrap().instrs[3].next_pc,
            "fused slot must skip compare words and following SubInt/Access/SubInt words"
        );
    }

    #[test]
    fn packed_hot_slot_fuses_mul_int_feeding_cmp_int_jmp() {
        let const_rhs = crate::vm::bytecode::rk_make_const(0);
        let function = function_with(vec![
            Op::Mul(2, 0, const_rhs),
            Op::CmpIntJmp {
                kind: crate::vm::IntCmpKind::Eq,
                a: 2,
                b: 3,
                ofs: 1,
            },
            Op::Ret { base: 2, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("mul-int+cmp-int-jmp must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("mul hot slot");

        match slot.kind {
            PackedHotKind::IntArithCmpIntJmp {
                arith_op: PackedArithOp::Mul,
                arith_dst: 2,
                arith_a: 0,
                arith_b,
                cmp_op: PackedCmpOp::Eq,
                cmp_a: 2,
                cmp_b: 3,
                jump_pc,
            } => {
                assert_eq!(arith_b, const_rhs);
                assert_eq!(jump_pc, 4);
            }
            PackedHotKind::IntArith { .. } => panic!("expected IntArithCmpIntJmp hot slot, got IntArith"),
            PackedHotKind::Arith { .. } => panic!("expected IntArithCmpIntJmp hot slot, got Arith"),
            PackedHotKind::CmpIntJmp { .. } => panic!("expected IntArithCmpIntJmp hot slot, got CmpIntJmp"),
            other => panic!("expected IntArithCmpIntJmp hot slot, got {other:?}"),
        }
        assert_eq!(
            slot.next_pc, 4,
            "fused slot must skip MulInt and following CmpIntJmp words"
        );
    }

    #[test]
    fn packed_hot_slot_fuses_sub_int_feeding_cmp_int_jmp() {
        let function = function_with(vec![
            Op::SubInt(2, 0, 1),
            Op::CmpIntJmp {
                kind: crate::vm::IntCmpKind::Gt,
                a: 2,
                b: 3,
                ofs: 1,
            },
            Op::Ret { base: 2, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("sub-int+cmp-int-jmp must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("sub hot slot");

        match slot.kind {
            PackedHotKind::IntArithCmpIntJmp {
                arith_op: PackedArithOp::Sub,
                arith_dst: 2,
                arith_a: 0,
                arith_b: 1,
                cmp_op: PackedCmpOp::Gt,
                cmp_a: 2,
                cmp_b: 3,
                jump_pc: 5,
            } => {}
            PackedHotKind::IntArith { .. } => panic!("expected IntArithCmpIntJmp hot slot, got IntArith"),
            PackedHotKind::FloatArith { .. } => panic!("expected IntArithCmpIntJmp hot slot, got FloatArith"),
            PackedHotKind::CmpIntJmp { .. } => panic!("expected IntArithCmpIntJmp hot slot, got CmpIntJmp"),
            other => panic!("expected IntArithCmpIntJmp hot slot, got {other:?}"),
        }
        assert_eq!(
            slot.next_pc, 5,
            "fused slot must skip SubInt and following CmpIntJmp words"
        );
    }

    #[test]
    fn packed_hot_slot_fuses_cmp_imm_jmp_followed_by_mul_int_add_int() {
        let const_rhs = crate::vm::bytecode::rk_make_const(0);
        let function = function_with(vec![
            Op::CmpGeImmJmp { r: 0, imm: 70, ofs: 4 },
            Op::MulInt(3, 0, const_rhs),
            Op::AddInt(2, 2, 3),
            Op::Jmp(3),
            Op::AddIntImm(4, 0, 1),
            Op::AddInt(2, 2, 4),
            Op::Ret { base: 2, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("cmp-imm mul-add chain must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("cmp-imm hot slot");

        match slot.kind {
            PackedHotKind::CmpImmMulIntAddInt {
                op: PackedCmpImmOp::Ge,
                src: 0,
                imm: 70,
                mul_dst: 3,
                mul_a: 0,
                mul_b,
                add_dst: 2,
                add_a: 2,
                add_b: 3,
                ofs: 6,
            } => assert_eq!(mul_b, const_rhs),
            PackedHotKind::CmpImmJmp { .. } => panic!("expected CmpImmMulIntAddInt hot slot, got CmpImmJmp"),
            other => panic!("expected CmpImmMulIntAddInt hot slot, got {other:?}"),
        }
        assert_eq!(
            slot.next_pc,
            instr_pc(bc.decoded.as_ref().unwrap(), 3).unwrap(),
            "fused slot must skip compare, MulInt, and AddInt"
        );
    }

    #[test]
    fn packed_hot_slot_fuses_moves_into_closure_exact_call_window() {
        let function = function_with(vec![
            Op::Move(2, 0),
            Op::Move(3, 1),
            Op::CallClosureExact {
                f: 4,
                base: 2,
                argc: 2,
                retc: 1,
            },
            Op::Ret { base: 2, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("move+call must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("move+call hot slot");

        match slot.kind {
            PackedHotKind::MoveCall {
                moves,
                f: 4,
                base: 2,
                argc: 2,
                retc: 1,
                call_kind,
            } => {
                assert_eq!(moves, vec![(2, 0), (3, 1)]);
                assert!(matches!(call_kind, PackedHotCallKind::ClosureExact));
            }
            _ => panic!("expected MoveCall hot slot"),
        }
        assert_eq!(slot.next_pc, 4, "fused slot must skip argument moves and call words");
    }

    #[test]
    fn packed_hot_slot_fuses_moves_into_native_fast_call_window() {
        let function = function_with(vec![
            Op::Move(2, 0),
            Op::Move(3, 1),
            Op::CallNativeFast {
                f: 4,
                base: 2,
                argc: 2,
                retc: 1,
            },
            Op::Ret { base: 2, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("move+native call must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("move+native call hot slot");

        match slot.kind {
            PackedHotKind::MoveCall {
                moves,
                f: 4,
                base: 2,
                argc: 2,
                retc: 1,
                call_kind,
            } => {
                assert_eq!(moves, vec![(2, 0), (3, 1)]);
                assert!(matches!(call_kind, PackedHotCallKind::NativeFast));
            }
            _ => panic!("expected native MoveCall hot slot"),
        }
        assert_eq!(
            slot.next_pc, 4,
            "fused slot must skip argument moves and native call words"
        );
    }

    #[test]
    fn packed_hot_slot_decodes_map_has_k() {
        let function = Function {
            consts: vec![Val::Nil, Val::from_str("needle")],
            code: vec![Op::LoadK(0, 0), Op::MapHasK(1, 0, 1), Op::Ret { base: 1, retc: 1 }],
            n_regs: 2,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("MapHasK must be BC32 encodable");
        let pc = 1;
        let word = bc.code32[pc];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            pc,
            word,
            bc32::tag_of(word),
        )
        .expect("MapHasK hot slot");

        assert!(matches!(slot.kind, PackedHotKind::MapHasK { dst: 1, map: 0, key: 1 }));
    }

    #[test]
    fn packed_hot_slot_fuses_map_has_branch_increment() {
        let function = Function {
            consts: vec![Val::Nil, Val::from_str("needle")],
            code: vec![
                Op::LoadK(2, 1),
                Op::MapHas(3, 0, 2),
                Op::BoolBranch(3, 2),
                Op::AddIntImmJmp { r: 4, imm: 1, ofs: -2 },
                Op::Ret { base: 4, retc: 1 },
            ],
            n_regs: 5,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("map-has branch inc pattern must be BC32 encodable");
        let pc = instr_pc(bc.decoded.as_ref().unwrap(), 1).unwrap();
        let word = bc.code32[pc];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            pc,
            word,
            bc32::tag_of(word),
        )
        .expect("map-has inc hot slot");

        assert!(matches!(
            slot.kind,
            PackedHotKind::MapHasIncJmp {
                dst: 3,
                map: 0,
                key: 2,
                inc_r: 4,
                inc_imm: 1,
                ..
            }
        ));
        if let PackedHotKind::MapHasIncJmp { true_pc, false_pc, .. } = slot.kind {
            assert_eq!(true_pc, pc);
            assert_eq!(false_pc, instr_pc(bc.decoded.as_ref().unwrap(), 4).unwrap());
        }
        assert_eq!(slot.next_pc, instr_pc(bc.decoded.as_ref().unwrap(), 4).unwrap());
    }

    #[test]
    fn packed_hot_slot_fuses_map_get_compare_branch() {
        let nil_rk = crate::vm::bytecode::rk_make_const(0);
        let function = Function {
            consts: vec![Val::Nil, Val::from_str("needle")],
            code: vec![
                Op::LoadK(2, 1),
                Op::MapGetDynamic(1, 0, 2),
                Op::CmpNe(3, 1, nil_rk),
                Op::BoolBranch(3, 2),
                Op::LoadK(4, 0),
                Op::MapGetInterned(5, 0, 1),
                Op::CmpEq(6, 5, nil_rk),
                Op::BoolBranch(6, 2),
                Op::LoadK(7, 0),
                Op::Ret { base: 1, retc: 1 },
            ],
            n_regs: 8,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("map-get cmp branch must be BC32 encodable");

        let dynamic_pc = 1;
        let dynamic_word = bc.code32[dynamic_pc];
        let dynamic_slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            dynamic_pc,
            dynamic_word,
            bc32::tag_of(dynamic_word),
        )
        .expect("dynamic map-get cmp hot slot");
        assert!(matches!(
            dynamic_slot.kind,
            PackedHotKind::MapGetDynamicCmpJmp {
                dst: 1,
                map: 0,
                key: 2,
                op: PackedCmpOp::Ne,
                ..
            }
        ));

        let interned_pc = dynamic_slot.next_pc + 1;
        let interned_word = bc.code32[interned_pc];
        let interned_slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            interned_pc,
            interned_word,
            bc32::tag_of(interned_word),
        )
        .expect("interned map-get cmp hot slot");
        assert!(matches!(
            interned_slot.kind,
            PackedHotKind::MapGetInternedCmpJmp {
                dst: 5,
                map: 0,
                key: 1,
                op: PackedCmpOp::Eq,
                ..
            }
        ));
    }

    #[test]
    fn packed_hot_slot_fuses_map_get_nil_upsert_add() {
        let nil_rk = crate::vm::bytecode::rk_make_const(0);
        let function = Function {
            consts: vec![Val::Nil, Val::from_str("needle"), Val::Int(1)],
            code: vec![
                Op::LoadK(2, 1),
                Op::MapGetDynamic(1, 0, 2),
                Op::CmpEq(3, 1, nil_rk),
                Op::BoolBranch(3, 4),
                Op::LoadK(4, 2),
                Op::MapSet { map: 0, key: 2, val: 4 },
                Op::Jmp(3),
                Op::AddIntImm(5, 1, 1),
                Op::MapSet { map: 0, key: 2, val: 5 },
                Op::Ret { base: 0, retc: 1 },
            ],
            n_regs: 6,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("map upsert-add pattern must be BC32 encodable");
        let pc = 1;
        let word = bc.code32[pc];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            pc,
            word,
            bc32::tag_of(word),
        )
        .expect("map upsert-add hot slot");

        assert!(matches!(
            slot.kind,
            PackedHotKind::MapGetDynamicUpsertAdd {
                get_dst: 1,
                cmp_dst: 3,
                map: 0,
                key: 2,
                default: PackedValueOperand::Const(2),
                default_load: Some((4, 2)),
                add_dst: 5,
                add_rhs: PackedAddOperand::Imm(1),
                write_temps: false,
            }
        ));
        assert_eq!(slot.next_pc, instr_pc(bc.decoded.as_ref().unwrap(), 9).unwrap());
    }

    #[test]
    fn packed_hot_slot_fuses_add_int_feeding_floor_div_imm() {
        let function = Function {
            consts: Vec::new(),
            code: vec![
                Op::AddInt(2, 0, 1),
                Op::FloorDivImm { dst: 3, src: 2, imm: 2 },
                Op::Ret { base: 3, retc: 1 },
            ],
            n_regs: 4,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("add floor-div chain must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("add floor-div hot slot");

        assert!(matches!(
            slot.kind,
            PackedHotKind::AddIntFloorDivImm {
                add_dst: 2,
                a: 0,
                b: 1,
                div_dst: 3,
                imm: 2,
            }
        ));
        assert_eq!(slot.next_pc, bc.decoded.as_ref().unwrap().instrs[1].next_pc);
    }

    #[test]
    fn packed_hot_slot_fuses_mul_int_feeding_floor_div_imm() {
        let function = Function {
            consts: Vec::new(),
            code: vec![
                Op::MulInt(2, 0, 1),
                Op::FloorDivImm {
                    dst: 3,
                    src: 2,
                    imm: 100,
                },
                Op::Ret { base: 3, retc: 1 },
            ],
            n_regs: 4,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("mul floor-div chain must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("mul floor-div hot slot");

        assert!(matches!(
            slot.kind,
            PackedHotKind::MulIntFloorDivImm {
                mul_dst: 2,
                a: 0,
                b: 1,
                div_dst: 3,
                imm: 100,
            }
        ));
        assert_eq!(slot.next_pc, bc.decoded.as_ref().unwrap().instrs[1].next_pc);
    }

    #[test]
    fn packed_hot_slot_fuses_access_feeding_int_arith() {
        let function = Function {
            consts: Vec::new(),
            code: vec![Op::Access(2, 0, 1), Op::AddInt(3, 3, 2), Op::Ret { base: 3, retc: 1 }],
            n_regs: 4,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("access add chain must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("access add hot slot");

        assert!(matches!(
            slot.kind,
            PackedHotKind::AccessIntArith {
                access_dst: 2,
                base: 0,
                field: 1,
                write_access_dst: false,
                arith_op: PackedArithOp::Add,
                arith_dst: 3,
                arith_a: 3,
                arith_b: 2,
            }
        ));
        assert_eq!(slot.next_pc, bc.decoded.as_ref().unwrap().instrs[1].next_pc);
    }

    #[test]
    fn packed_access_int_arith_keeps_access_temp_when_live_after_fusion() {
        let function = Function {
            consts: Vec::new(),
            code: vec![
                Op::ListIndex(2, 0, 1),
                Op::AddInt(3, 3, 2),
                Op::Move(4, 2),
                Op::Ret { base: 3, retc: 1 },
            ],
            n_regs: 5,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("list index add chain must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("list index add hot slot");

        assert!(matches!(
            slot.kind,
            PackedHotKind::AccessIntArith {
                access_dst: 2,
                write_access_dst: true,
                ..
            }
        ));
    }

    #[test]
    fn packed_hot_slot_fuses_string_predicate_branch() {
        let function = Function {
            consts: vec![Val::from_str("api/"), Val::from_str("admin")],
            code: vec![
                Op::StartsWithK(2, 0, 0),
                Op::BoolBranch(2, 3),
                Op::ContainsK(3, 1, 1),
                Op::BoolBranch(3, 1),
                Op::Ret { base: 0, retc: 1 },
            ],
            n_regs: 4,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("string predicate branches must be BC32 encodable");

        let starts_slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            bc.code32[0],
            bc32::tag_of(bc.code32[0]),
        )
        .expect("starts-with branch hot slot");
        assert!(
            matches!(
                starts_slot.kind,
                PackedHotKind::StartsWithKJmp { src: 0, key: 0, ofs: 6 }
            ),
            "got {:?}",
            starts_slot.kind
        );
        assert_eq!(starts_slot.next_pc, bc.decoded.as_ref().unwrap().instrs[1].next_pc);

        let contains_pc = starts_slot.next_pc;
        let contains_slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            contains_pc,
            bc.code32[contains_pc],
            bc32::tag_of(bc.code32[contains_pc]),
        )
        .expect("contains branch hot slot");
        assert!(
            matches!(
                contains_slot.kind,
                PackedHotKind::ContainsKJmp { src: 1, key: 1, ofs: 3 }
            ),
            "got {:?}",
            contains_slot.kind
        );
        assert_eq!(contains_slot.next_pc, bc.decoded.as_ref().unwrap().instrs[3].next_pc);
    }

    #[test]
    fn packed_hot_slot_fuses_two_mul_ints_feeding_add_int() {
        let function = Function {
            consts: Vec::new(),
            code: vec![
                Op::MulInt(2, 0, 1),
                Op::MulInt(3, 4, 5),
                Op::AddInt(6, 2, 3),
                Op::Ret { base: 6, retc: 1 },
            ],
            n_regs: 7,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("mul mul add chain must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("mul mul add hot slot");

        assert!(matches!(
            slot.kind,
            PackedHotKind::MulIntMulIntAddInt {
                first_dst: 2,
                first_a: 0,
                first_b: 1,
                second_dst: 3,
                second_a: 4,
                second_b: 5,
                add_dst: 6,
                add_a: 2,
                add_b: 3,
            }
        ));
        assert_eq!(slot.next_pc, bc.decoded.as_ref().unwrap().instrs[2].next_pc);
    }

    #[test]
    fn packed_hot_slot_fuses_mul_int_feeding_add_int() {
        let function = Function {
            consts: Vec::new(),
            code: vec![Op::MulInt(2, 0, 1), Op::AddInt(3, 4, 2), Op::Ret { base: 3, retc: 1 }],
            n_regs: 5,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("mul add chain must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("mul add hot slot");

        assert!(matches!(
            slot.kind,
            PackedHotKind::MulIntAddInt {
                mul_dst: 2,
                mul_a: 0,
                mul_b: 1,
                add_dst: 3,
                add_a: 4,
                add_b: 2,
            }
        ));
        assert_eq!(slot.next_pc, bc.decoded.as_ref().unwrap().instrs[1].next_pc);
    }

    #[test]
    fn packed_hot_slot_fuses_mul_add_mod_int_when_temps_are_dead() {
        let function = Function {
            consts: Vec::new(),
            code: vec![
                Op::MulInt(2, 0, 1),
                Op::AddInt(3, 2, 4),
                Op::ModInt(5, 3, 6),
                Op::Ret { base: 5, retc: 1 },
            ],
            n_regs: 7,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("mul add mod chain must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("mul add mod hot slot");

        assert!(matches!(
            slot.kind,
            PackedHotKind::MulIntAddIntModInt {
                mul_dst: 2,
                mul_a: 0,
                mul_b: 1,
                add_dst: 3,
                add_a: 2,
                add_b: 4,
                mod_dst: 5,
                mod_rhs: 6,
            }
        ));
        assert_eq!(slot.next_pc, bc.decoded.as_ref().unwrap().instrs[2].next_pc);
    }

    #[test]
    fn packed_hot_slot_fuses_int_arith_feeding_add_int_imm() {
        let function = Function {
            consts: Vec::new(),
            code: vec![
                Op::ModInt(2, 0, 1),
                Op::AddIntImm(3, 2, 5),
                Op::Ret { base: 3, retc: 1 },
            ],
            n_regs: 4,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("mod add-imm chain must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("mod add-imm hot slot");

        assert!(matches!(
            slot.kind,
            PackedHotKind::IntArithAddIntImm {
                arith_op: PackedArithOp::Mod,
                arith_dst: 2,
                arith_a: 0,
                arith_b: 1,
                add_dst: 3,
                add_imm: 5,
            }
        ));
        assert_eq!(slot.next_pc, bc.decoded.as_ref().unwrap().instrs[1].next_pc);
    }

    #[test]
    fn packed_hot_slot_fuses_rk_arith_feeding_add_int_imm() {
        let const_rhs = crate::vm::bytecode::rk_make_const(0);
        let function = function_with(vec![
            Op::Mod(2, 0, const_rhs),
            Op::AddIntImm(3, 2, 5),
            Op::Ret { base: 3, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("rk mod add-imm chain must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("rk mod add-imm hot slot");

        assert!(matches!(
            slot.kind,
            PackedHotKind::ArithAddIntImm {
                op: PackedArithOp::Mod,
                arith_dst: 2,
                a: 0,
                b,
                add_dst: 3,
                add_imm: 5,
            } if b == const_rhs
        ));
        assert_eq!(slot.next_pc, bc.decoded.as_ref().unwrap().instrs[1].next_pc);
    }

    #[test]
    fn packed_hot_slot_decodes_contains_k() {
        let function = Function {
            consts: vec![Val::from_str("needle")],
            code: vec![Op::ContainsK(1, 0, 0), Op::Ret { base: 1, retc: 1 }],
            n_regs: 2,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&function).expect("ContainsK must be BC32 encodable");
        let word = bc.code32[0];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            word,
            bc32::tag_of(word),
        )
        .expect("ContainsK hot slot");

        assert!(matches!(slot.kind, PackedHotKind::ContainsK { dst: 1, src: 0, key: 0 }));
    }

    #[test]
    fn packed_hot_slot_decodes_capture_bool_and_set_branches() {
        let function = function_with(vec![
            Op::LoadCapture { dst: 1, idx: 0 },
            Op::ToBool(2, 1),
            Op::JmpTrueSet { r: 2, dst: 3, ofs: 1 },
            Op::JmpFalseSet { r: 2, dst: 3, ofs: 1 },
            Op::Ret { base: 3, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("control hot slots must be BC32 encodable");

        let load_word = bc.code32[0];
        let load_slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            0,
            load_word,
            bc32::tag_of(load_word),
        )
        .expect("LoadCapture hot slot");
        assert!(matches!(load_slot.kind, PackedHotKind::LoadCapture { dst: 1, idx: 0 }));

        let bool_word = bc.code32[1];
        let bool_slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            1,
            bool_word,
            bc32::tag_of(bool_word),
        )
        .expect("ToBool hot slot");
        assert!(matches!(bool_slot.kind, PackedHotKind::ToBool { dst: 2, src: 1 }));

        let true_word = bc.code32[2];
        let true_slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            2,
            true_word,
            bc32::tag_of(true_word),
        )
        .expect("JmpTrueSet hot slot");
        assert!(matches!(
            true_slot.kind,
            PackedHotKind::JmpTrueSet { r: 2, dst: 3, ofs: 1 }
        ));

        let false_word = bc.code32[3];
        let false_slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            3,
            false_word,
            bc32::tag_of(false_word),
        )
        .expect("JmpFalseSet hot slot");
        assert!(matches!(
            false_slot.kind,
            PackedHotKind::JmpFalseSet { r: 2, dst: 3, ofs: 1 }
        ));
    }

    #[test]
    fn packed_for_range_step_carries_generic_tail_guard() {
        let function = function_with(vec![
            Op::ForRangePrep {
                idx: 0,
                limit: 1,
                step: 2,
                inclusive: false,
                explicit: false,
            },
            Op::ForRangeLoop {
                idx: 0,
                limit: 1,
                step: 2,
                inclusive: false,
                write_idx: true,
                ofs: 3,
            },
            Op::AddIntImm(3, 3, 1),
            Op::ForRangeStep {
                idx: 0,
                step: 2,
                back_ofs: -2,
            },
            Op::Ret { base: 3, retc: 1 },
        ]);
        let bc = Bc32Function::try_from_function(&function).expect("range loop must be BC32 encodable");
        let step_pc = 5;
        let word = bc.code32[step_pc];
        let slot = build_hot_slot(
            &bc.code32,
            bc.decoded.as_deref(),
            &bc.consts,
            step_pc,
            word,
            bc32::tag_of(word),
        )
        .expect("range step hot slot");

        match slot.kind {
            PackedHotKind::ForRangeStep { tail: Some(tail), .. } => {
                assert_eq!(tail.guard_pc, 2);
                assert_eq!(tail.body_pc, 4);
                assert_eq!(tail.exit_pc, 7);
                assert_eq!(tail.idx, 0);
                assert!(tail.write_idx);
            }
            _ => panic!("expected generic range tail guard metadata"),
        }
    }
}
