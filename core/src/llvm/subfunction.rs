//! Standalone LLVM function compilation for user functions that can't be inlined.
//!
//! When the block compiler encounters a CallDirect to a self-recursive function,
//! this module generates a separate LLVM function definition with proper
//! control flow (Test branching, Jmp jumps).

mod list;

use anyhow::Result;

use crate::vm::{ConstHeapValueData, FunctionData, Instr, ModuleArtifact, Opcode};

use super::{
    const_display::llvm_string_constant,
    dynamic_containers::{
        emit_dynamic_int_list_allocas, emit_dynamic_ptr_list_concat, emit_dynamic_ptr_list_copy,
        emit_dynamic_ptr_list_get, emit_dynamic_ptr_list_push, emit_dynamic_ptr_list_slice, emit_dynamic_ptr_list_take,
    },
    ir_text::{native_label, native_relative_target, next_tmp},
    output::emit_native_print_text_parts,
    scalar::block_helpers::{
        concat_text_values, emit_static_formatted_print, local_heap_kind_before, local_register_kind_before,
        scalar_arg_value, text_value_from_reg,
    },
    scalar::emit::{
        emit_f64_binary_block, emit_i64_binary_block, emit_i64_immediate_block, emit_numeric_compare_block,
    },
    scalar::facts::{NativeScalarFacts, NativeScalarKind, native_scalar_block_facts_with_initial},
    straightline_value::{
        NativeBuiltin, NativeListElementKind, NativeStraightlineValue, native_static_global,
        native_straightline_heap_const_value,
    },
};

const PTR_LIST_PARAM_BASE: usize = 900_000;
const PTR_LIST_REG_BASE: usize = 800_000;

/// Compile a single user function into a standalone LLVM function definition.
///
/// The function is defined as `define private i64 @lk_fn_{index}(i64 %arg0, ...)`.
/// Returns the full LLVM IR string for the function definition, or None if the
/// function can't be compiled as a standalone function.
pub(super) fn compile_native_scalar_subfunction(
    artifact: &ModuleArtifact,
    function_index: usize,
    recursive_indices: &[u16],
) -> Result<Option<String>> {
    let Some(function) = artifact.module.functions.get(function_index) else {
        return Ok(None);
    };

    let Ok(code) = function
        .code
        .iter()
        .copied()
        .map(Instr::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
    else {
        return Ok(None);
    };

    let register_count = function.register_count as usize;
    let param_count = function.param_count as usize;
    let code_len = code.len();

    if code.iter().any(|instr| instr.opcode() == Opcode::ListPush) {
        return Ok(None);
    }

    let Some(callee_facts) = compute_callee_facts(artifact, function, &code)? else {
        return Ok(None);
    };

    // Determine the return kind from callee facts
    let Some(return_kind) = determine_return_kind(&code, &callee_facts) else {
        return Ok(None);
    };
    // Standalone subfunctions return scalar register payloads. Bool/Nil share
    // the i64 representation used by the main block slots.
    if !matches!(
        return_kind,
        NativeScalarKind::I64 | NativeScalarKind::Bool | NativeScalarKind::Nil | NativeScalarKind::StrPtr
    ) {
        return Ok(None);
    }

    let mut ir = String::new();
    let mut extra_globals = String::new();
    let mut static_regs: Vec<Option<NativeStraightlineValue>> = vec![None; register_count];

    // Function header
    let fn_name = format!("@lk_fn_{function_index}");
    ir.push_str(&format!("define private {} {fn_name}(", return_kind.llvm_type()));
    // Determine parameter types from callee_facts
    let mut param_kinds = Vec::with_capacity(param_count);
    for i in 0..param_count {
        let kind = callee_facts
            .register_kind_before(0, i as u8)
            .unwrap_or(NativeScalarKind::I64);
        param_kinds.push(kind);
    }
    for i in 0..param_count {
        if i > 0 {
            ir.push_str(", ");
        }
        ir.push_str(&format!("{} %arg{i}", param_kinds[i].llvm_type()));
    }
    ir.push_str(") {\n");
    ir.push_str("entry:\n");

    // Allocate alloca slots for registers
    for reg in 0..register_count {
        ir.push_str(&format!("  %r{reg}.slot = alloca i64\n"));
        ir.push_str(&format!("  %r{reg}.present.slot = alloca i64\n"));
        ir.push_str(&format!("  store i64 1, ptr %r{reg}.present.slot\n"));
    }

    // Store parameters into register slots (using their native type)
    for i in 0..param_count {
        let param_ty = param_kinds[i].llvm_type();
        ir.push_str(&format!("  store {} %arg{i}, ptr %r{i}.slot\n", param_ty));
    }

    // Pre-scan for basic block targets (from Test/Jmp branches)
    let block_targets = find_block_targets(&code, code_len);

    let mut tmp_index = 0usize;
    let mut after_return = false;
    let mut emitted_terminator = true; // Start true (entry block is already labeled) // Skip dead code after ret until next branch target

    for (pc, instr) in code.iter().copied().enumerate() {
        // Emit basic block label if this PC is a branch target
        if pc > 0 && block_targets.contains(&pc) {
            // If previous block didn't end with a terminator, add a branch
            if !emitted_terminator {
                ir.push_str(&format!("  br label {}\n", native_label(pc, code_len)));
            }
            ir.push_str(&format!("{}:\n", native_label(pc, code_len).trim_start_matches('%')));
            static_regs.fill(None);
            after_return = false;
            emitted_terminator = false;
        }

        // Skip dead code after return
        if after_return {
            continue;
        }

        emitted_terminator = false;
        match instr.opcode() {
            Opcode::Nop => {}

            Opcode::Jmp => {
                let Some(target) = native_relative_target(pc, instr.sj_arg(), code_len) else {
                    return Ok(None);
                };
                ir.push_str(&format!("  br label {}\n", native_label(target, code_len)));
                emitted_terminator = true;
            }

            Opcode::Test | Opcode::BrFalse | Opcode::BrTrue => {
                if (instr.a() as usize) >= register_count {
                    return Ok(None);
                }
                let Some(kind) = callee_facts.register_kind_before(pc, instr.a()) else {
                    return Ok(None);
                };
                let Some((truthy_target, falsy_target)) = branch_truthy_falsy_targets(pc, instr, code_len) else {
                    return Ok(None);
                };
                match kind {
                    NativeScalarKind::Bool => {
                        let value = next_tmp(&mut tmp_index);
                        let cond = next_tmp(&mut tmp_index);
                        ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.a()));
                        ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
                        ir.push_str(&format!(
                            "  br i1 {cond}, label {}, label {}\n",
                            native_label(truthy_target, code_len),
                            native_label(falsy_target, code_len)
                        ));
                    }
                    NativeScalarKind::Nil => {
                        ir.push_str(&format!("  br label {}\n", native_label(falsy_target, code_len)));
                    }
                    NativeScalarKind::I64
                    | NativeScalarKind::F64
                    | NativeScalarKind::StrPtr
                    | NativeScalarKind::MaybeI64
                    | NativeScalarKind::MaybeStrPtr => {
                        ir.push_str(&format!("  br label {}\n", native_label(truthy_target, code_len)));
                    }
                }
                emitted_terminator = true;
            }
            opcode if opcode.is_compare_test() => {
                if (instr.a() as usize) >= register_count
                    || (!opcode.is_int_immediate_compare_test() && (instr.b() as usize) >= register_count)
                {
                    return Ok(None);
                }
                let Some((taken, fallthrough)) = compare_test_targets(&code, pc, code_len) else {
                    return Ok(None);
                };
                let Some(pred) = compare_test_i64_pred(instr.opcode()) else {
                    return Ok(None);
                };
                let lhs = next_tmp(&mut tmp_index);
                let cond = next_tmp(&mut tmp_index);
                let branch_cond = next_tmp(&mut tmp_index);
                ir.push_str(&format!("  {lhs} = load i64, ptr %r{}.slot\n", instr.a()));
                let rhs = if opcode.is_int_immediate_compare_test() {
                    i64::from(instr.sc()).to_string()
                } else {
                    let rhs = next_tmp(&mut tmp_index);
                    ir.push_str(&format!("  {rhs} = load i64, ptr %r{}.slot\n", instr.b()));
                    rhs
                };
                ir.push_str(&format!("  {cond} = icmp {pred} i64 {lhs}, {rhs}\n"));
                let jump_when = if opcode.is_int_immediate_compare_test() {
                    instr.b() != 0
                } else {
                    instr.c() != 0
                };
                if jump_when {
                    ir.push_str(&format!("  {branch_cond} = xor i1 {cond}, false\n"));
                } else {
                    ir.push_str(&format!("  {branch_cond} = xor i1 {cond}, true\n"));
                }
                ir.push_str(&format!(
                    "  br i1 {branch_cond}, label {}, label {}\n",
                    native_label(taken, code_len),
                    native_label(fallthrough, code_len)
                ));
                emitted_terminator = true;
            }

            Opcode::LoadNil => {
                if (instr.a() as usize) < register_count {
                    ir.push_str(&format!("  store i64 0, ptr %r{}.slot\n", instr.a()));
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Nil);
                }
            }
            Opcode::LoadBool => {
                if (instr.a() as usize) < register_count {
                    let val = if instr.b() != 0 { 1i64 } else { 0 };
                    ir.push_str(&format!("  store i64 {val}, ptr %r{}.slot\n", instr.a()));
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::Bool(val.to_string()));
                }
            }
            Opcode::LoadInt => {
                let Some(value) = function.consts.ints.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if (instr.a() as usize) < register_count {
                    ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(value.to_string()));
                }
            }
            Opcode::LoadFloat => {
                if (instr.a() as usize) >= register_count {
                    return Ok(None);
                }
                let Some(value) = function.consts.floats.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                ir.push_str(&format!("  store double {value}, ptr %r{}.slot\n", instr.a()));
                static_regs[instr.a() as usize] = Some(NativeStraightlineValue::F64(value.to_string()));
                emitted_terminator = false;
            }
            Opcode::Not => {
                if (instr.a() as usize) >= register_count || (instr.b() as usize) >= register_count {
                    return Ok(None);
                }
                let Some(kind) = callee_facts.register_kind_before(pc, instr.b()) else {
                    return Ok(None);
                };
                match kind {
                    NativeScalarKind::Bool => {
                        let value = next_tmp(&mut tmp_index);
                        let cond = next_tmp(&mut tmp_index);
                        let out = next_tmp(&mut tmp_index);
                        ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.b()));
                        ir.push_str(&format!("  {cond} = icmp eq i64 {value}, 0\n"));
                        ir.push_str(&format!("  {out} = zext i1 {cond} to i64\n"));
                        ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
                    }
                    NativeScalarKind::Nil => {
                        ir.push_str(&format!("  store i64 1, ptr %r{}.slot\n", instr.a()));
                    }
                    NativeScalarKind::I64
                    | NativeScalarKind::F64
                    | NativeScalarKind::StrPtr
                    | NativeScalarKind::MaybeI64
                    | NativeScalarKind::MaybeStrPtr => {
                        let value = next_tmp(&mut tmp_index);
                        let cond = next_tmp(&mut tmp_index);
                        let out = next_tmp(&mut tmp_index);
                        ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.b()));
                        ir.push_str(&format!("  {cond} = icmp eq i64 {value}, 0\n"));
                        ir.push_str(&format!("  {out} = zext i1 {cond} to i64\n"));
                        ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
                    }
                }
                emitted_terminator = false;
            }
            Opcode::Move => {
                if (instr.a() as usize) >= register_count || (instr.b() as usize) >= register_count {
                    return Ok(None);
                }
                let tmp = next_tmp(&mut tmp_index);
                ir.push_str(&format!("  {tmp} = load i64, ptr %r{}.slot\n", instr.b()));
                ir.push_str(&format!("  store i64 {tmp}, ptr %r{}.slot\n", instr.a()));
                static_regs[instr.a() as usize] = static_regs.get(instr.b() as usize).and_then(Clone::clone);
            }
            Opcode::AddInt | Opcode::SubInt | Opcode::MulInt | Opcode::DivInt | Opcode::ModInt => {
                if (instr.a() as usize) >= register_count
                    || (instr.b() as usize) >= register_count
                    || instr.c() as usize >= register_count
                {
                    return Ok(None);
                }
                emit_i64_binary_block(&mut ir, instr, &mut tmp_index);
                static_regs[instr.a() as usize] = None;
            }
            Opcode::AddIntI | Opcode::MulIntI | Opcode::ModIntI => {
                if (instr.a() as usize) >= register_count || (instr.b() as usize) >= register_count {
                    return Ok(None);
                }
                emit_i64_immediate_block(&mut ir, instr, &mut tmp_index);
                static_regs[instr.a() as usize] = None;
            }
            Opcode::CmpInt
            | Opcode::CmpNeInt
            | Opcode::CmpLtInt
            | Opcode::CmpLeInt
            | Opcode::CmpGtInt
            | Opcode::CmpGeInt => {
                if (instr.a() as usize) >= register_count
                    || (instr.b() as usize) >= register_count
                    || instr.c() as usize >= register_count
                {
                    return Ok(None);
                }
                let Some(kind) = callee_facts.register_kind_before(pc, instr.b()) else {
                    return Ok(None);
                };
                let Some(rhs_kind) = callee_facts.register_kind_before(pc, instr.c()) else {
                    return Ok(None);
                };
                emit_numeric_compare_block(&mut ir, instr, kind, rhs_kind, &mut tmp_index);
                static_regs[instr.a() as usize] = None;
                emitted_terminator = false;
            }
            Opcode::AddFloat | Opcode::SubFloat | Opcode::MulFloat | Opcode::DivFloat | Opcode::ModFloat => {
                if (instr.a() as usize) >= register_count
                    || (instr.b() as usize) >= register_count
                    || instr.c() as usize >= register_count
                {
                    return Ok(None);
                }
                let Some(lhs) = callee_facts.register_kind_before(pc, instr.b()) else {
                    return Ok(None);
                };
                let Some(rhs) = callee_facts.register_kind_before(pc, instr.c()) else {
                    return Ok(None);
                };
                if !matches!(lhs, NativeScalarKind::I64 | NativeScalarKind::F64)
                    || !matches!(rhs, NativeScalarKind::I64 | NativeScalarKind::F64)
                {
                    return Ok(None);
                }
                emit_f64_binary_block(&mut ir, instr, lhs, rhs, "", &mut tmp_index);
                static_regs[instr.a() as usize] = None;
                emitted_terminator = false;
            }
            Opcode::CallDirect => {
                let callee_idx = instr.b();
                if (instr.a() as usize) >= register_count {
                    return Ok(None);
                }
                if recursive_indices.contains(&u16::from(callee_idx)) || callee_idx as usize == function_index {
                    // Load arguments and build call argument list
                    let mut call_args = String::new();
                    for i in 0..instr.c() as usize {
                        let arg_reg = instr.a() as usize + 1 + i;
                        if (arg_reg as usize) >= register_count {
                            return Ok(None);
                        }
                        let arg_kind = callee_facts
                            .register_kind_before(pc, arg_reg as u8)
                            .unwrap_or(NativeScalarKind::I64);
                        let arg_ty = arg_kind.llvm_type();
                        let arg_tmp = next_tmp(&mut tmp_index);
                        ir.push_str(&format!("  {arg_tmp} = load {arg_ty}, ptr %r{arg_reg}.slot\n"));
                        if i > 0 {
                            call_args.push_str(", ");
                        }
                        call_args.push_str(&format!("{arg_ty} {arg_tmp}"));
                    }
                    let result = next_tmp(&mut tmp_index);
                    ir.push_str(&format!(
                        "  {result} = call {} @lk_fn_{callee_idx}({call_args})\n",
                        return_kind.llvm_type()
                    ));
                    ir.push_str(&format!(
                        "  store {} {result}, ptr %r{}.slot\n",
                        return_kind.llvm_type(),
                        instr.a()
                    ));
                    static_regs[instr.a() as usize] = None;
                } else {
                    return Ok(None);
                }
            }
            opcode if opcode.is_return() => {
                if instr.return_count() == 0 {
                    ir.push_str(&format!(
                        "  ret {} {}\n",
                        return_kind.llvm_type(),
                        native_return_zero(return_kind)
                    ));
                } else if instr.return_count() == 1 && (instr.a() as usize) < register_count {
                    let Some(kind) = callee_facts.register_kind_before(pc, instr.a()) else {
                        return Ok(None);
                    };
                    let result = next_tmp(&mut tmp_index);
                    ir.push_str(&format!(
                        "  {result} = load {}, ptr %r{}.slot\n",
                        kind.llvm_type(),
                        instr.a()
                    ));
                    ir.push_str(&format!("  ret {} {result}\n", kind.llvm_type()));
                } else {
                    return Ok(None);
                }
                after_return = true;
                emitted_terminator = true;
            }
            Opcode::IsNil => {
                if (instr.a() as usize) >= register_count || (instr.b() as usize) >= register_count {
                    return Ok(None);
                }
                let value = next_tmp(&mut tmp_index);
                let cond = next_tmp(&mut tmp_index);
                let out = next_tmp(&mut tmp_index);
                ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.b()));
                ir.push_str(&format!("  {cond} = icmp eq i64 {value}, 0\n"));
                ir.push_str(&format!("  {out} = zext i1 {cond} to i64\n"));
                ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
                static_regs[instr.a() as usize] = None;
                emitted_terminator = false;
            }
            Opcode::GetGlobal => {
                if (instr.a() as usize) >= register_count || instr.bx() as usize >= artifact.module.globals.len() {
                    return Ok(None);
                }
                let name = artifact
                    .module
                    .globals
                    .get(instr.bx() as usize)
                    .ok_or_else(|| anyhow::anyhow!("unexpected global index"))?;
                // Only support known static globals (println, panic, __lk_call_method)
                if super::straightline_value::native_static_global(name).is_some() {
                    ir.push_str(&format!("  store i64 0, ptr %r{}.slot\n", instr.a()));
                    static_regs[instr.a() as usize] = super::straightline_value::native_static_global(name);
                } else {
                    return Ok(None);
                }
                emitted_terminator = false;
            }
            Opcode::Call => {
                if instr.a() != instr.b() || (instr.a() as usize) >= register_count {
                    return Ok(None);
                }
                // In subfunctions, Call targets Builtin (println, panic, or dynamic).
                // For panic: abort.
                // For print/println: emit printf based on arg kind from callee facts.
                // The callee_facts tell us the per-instruction register kind.
                if instr.c() == 0 {
                    ir.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_empty_text)\n");
                } else if instr.c() == 1 {
                    let arg_reg = instr.a() as usize + 1;
                    if arg_reg < register_count {
                        if let Some(NativeStraightlineValue::Text(parts)) =
                            static_regs.get(arg_reg).and_then(Clone::clone)
                        {
                            emit_native_print_text_parts(&mut ir, &parts, true)
                                .ok_or_else(|| anyhow::anyhow!("unsupported text print"))?;
                            emitted_terminator = false;
                            continue;
                        }
                        // Check register kind from callee facts for this PC
                        let arg_kind = callee_facts
                            .register_kind_before(pc, arg_reg as u8)
                            .unwrap_or(NativeScalarKind::I64);
                        match arg_kind {
                            NativeScalarKind::I64 | NativeScalarKind::MaybeI64 => {
                                let val = next_tmp(&mut tmp_index);
                                ir.push_str(&format!("  {val} = load i64, ptr %r{arg_reg}.slot\n"));
                                ir.push_str(&format!("  call i32 (ptr, ...) @printf(ptr @lk_i64_fmt, i64 {val})\n"));
                            }
                            NativeScalarKind::F64 => {
                                let val = next_tmp(&mut tmp_index);
                                ir.push_str(&format!("  {val} = load double, ptr %r{arg_reg}.slot\n"));
                                ir.push_str(&format!(
                                    "  call i32 (ptr, ...) @printf(ptr @lk_f64_fmt, double {val})\n"
                                ));
                            }
                            NativeScalarKind::Bool => {
                                let value = next_tmp(&mut tmp_index);
                                let cond = next_tmp(&mut tmp_index);
                                let text = next_tmp(&mut tmp_index);
                                ir.push_str(&format!("  {value} = load i64, ptr %r{arg_reg}.slot\n"));
                                ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
                                ir.push_str(&format!(
                                    "  {text} = select i1 {cond}, ptr @lk_bool_true, ptr @lk_bool_false\n"
                                ));
                                ir.push_str(&format!("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {text})\n"));
                            }
                            NativeScalarKind::Nil => {
                                ir.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_nil_text)\n");
                            }
                            NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr => {
                                let val = next_tmp(&mut tmp_index);
                                ir.push_str(&format!("  {val} = load ptr, ptr %r{arg_reg}.slot\n"));
                                ir.push_str(&format!("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {val})\n"));
                            }
                        }
                    }
                } else {
                    let Some(NativeStraightlineValue::Builtin(builtin)) =
                        static_regs.get(instr.b() as usize).and_then(Clone::clone)
                    else {
                        return Ok(None);
                    };
                    let start = instr.a() as usize + 1;
                    let end = start
                        .checked_add(instr.c() as usize)
                        .ok_or_else(|| anyhow::anyhow!("arg overflow"))?;
                    if end > register_count {
                        return Ok(None);
                    }
                    let args = (start..end)
                        .map(|reg| scalar_arg_value(&mut ir, "", &callee_facts, pc, &static_regs, reg, &mut tmp_index))
                        .collect::<Option<Vec<_>>>();
                    let Some(args) = args else {
                        return Ok(None);
                    };
                    let Some(value) =
                        emit_static_formatted_print(&mut ir, &mut extra_globals, builtin, &args, &mut tmp_index)
                    else {
                        return Ok(None);
                    };
                    static_regs[instr.a() as usize] = Some(value);
                }
                emitted_terminator = false;
            }
            Opcode::LoadString => {
                let Some(value) = function.consts.strings.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if (instr.a() as usize) < register_count {
                    let symbol = format!("@lk_fn{function_index}_str_{pc}");
                    extra_globals.push_str(&llvm_string_constant(&symbol, value));
                    ir.push_str(&format!("  store ptr {symbol}, ptr %r{}.slot\n", instr.a()));
                    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::String {
                        symbol,
                        value: value.clone(),
                        len: value.chars().count(),
                        key_kind: super::straightline_value::NativeStringKeyKind::Short,
                    });
                }
                emitted_terminator = false;
            }
            Opcode::LoadHeapConst => {
                let Some(value) = function.consts.heap_values.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                if (instr.a() as usize) < register_count {
                    if let ConstHeapValueData::LongString(text) = value {
                        let symbol = format!("@lk_fn{function_index}_heap_str_{}", instr.bx());
                        extra_globals.push_str(&llvm_string_constant(&symbol, text));
                        ir.push_str(&format!("  store ptr {symbol}, ptr %r{}.slot\n", instr.a()));
                        static_regs[instr.a() as usize] =
                            native_straightline_heap_const_value(function_index, instr.bx(), value);
                    } else {
                        ir.push_str(&format!("  store i64 0, ptr %r{}.slot\n", instr.a()));
                        static_regs[instr.a() as usize] = None;
                    }
                }
                emitted_terminator = false;
            }
            Opcode::ToString => {
                if (instr.a() as usize) >= register_count || (instr.b() as usize) >= register_count {
                    return Ok(None);
                }
                let Some(value) = text_value_from_reg(
                    &mut ir,
                    instr.b(),
                    callee_facts
                        .register_kind_before(pc, instr.b())
                        .or_else(|| local_register_kind_before(&code, pc, instr.b())),
                    &static_regs,
                    &mut tmp_index,
                ) else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = Some(value);
                emitted_terminator = false;
            }
            Opcode::ConcatString => {
                if (instr.a() as usize) >= register_count
                    || (instr.b() as usize) >= register_count
                    || (instr.c() as usize) >= register_count
                {
                    return Ok(None);
                }
                let Some(lhs) = text_value_from_reg(
                    &mut ir,
                    instr.b(),
                    callee_facts
                        .register_kind_before(pc, instr.b())
                        .or_else(|| local_register_kind_before(&code, pc, instr.b())),
                    &static_regs,
                    &mut tmp_index,
                ) else {
                    return Ok(None);
                };
                let Some(rhs) = text_value_from_reg(
                    &mut ir,
                    instr.c(),
                    callee_facts
                        .register_kind_before(pc, instr.c())
                        .or_else(|| local_register_kind_before(&code, pc, instr.c())),
                    &static_regs,
                    &mut tmp_index,
                ) else {
                    return Ok(None);
                };
                let Some(value) = concat_text_values(lhs, rhs) else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = Some(value);
                emitted_terminator = false;
            }
            Opcode::ConcatN => {
                // N-ary concat: fallback for now; static folding is complex.
                return Ok(None);
            }
            _ => {
                return Ok(None);
            }
        }
    }

    // Add exit block (may be needed by Jmp targets)
    ir.push_str("exit:\n");
    ir.push_str(&format!(
        "  ret {} {}\n",
        return_kind.llvm_type(),
        native_return_zero(return_kind)
    ));
    // Add divisor-zero block (needed by DivInt/ModInt)
    ir.push_str("lk_divisor_zero:\n");
    ir.push_str(&format!(
        "  ret {} {}\n",
        return_kind.llvm_type(),
        native_return_zero(return_kind)
    ));
    ir.push_str("}\n");
    ir.push_str(&extra_globals);
    Ok(Some(ir))
}

pub(super) fn compile_native_ptr_list_subfunction(
    artifact: &ModuleArtifact,
    function_index: usize,
) -> Result<Option<String>> {
    let Some(function) = artifact.module.functions.get(function_index) else {
        return Ok(None);
    };
    if function.param_count != 1 || function.capture_count != 0 {
        return Ok(None);
    }
    let Ok(code) = function
        .code
        .iter()
        .copied()
        .map(Instr::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
    else {
        return Ok(None);
    };
    if !code.iter().any(|instr| instr.opcode() == Opcode::ListPush) {
        return Ok(None);
    }

    let register_count = function.register_count as usize;
    let code_len = code.len();
    let mut ir = String::new();
    let mut extra_globals = String::new();
    let mut static_regs: Vec<Option<NativeStraightlineValue>> = vec![None; register_count];
    let param_list_id = ptr_list_param_id(function_index, 0);
    let fn_name = format!("@lk_fn_{function_index}_list");
    ir.push_str(&format!(
        "define private void {fn_name}(ptr %arg0.values, ptr %arg0.len.slot, ptr %out.values, ptr %out.len.slot) {{\n"
    ));
    ir.push_str("entry:\n");
    for reg in 0..register_count {
        ir.push_str(&format!("  %r{reg}.slot = alloca i64\n"));
        ir.push_str(&format!("  %r{reg}.present.slot = alloca i64\n"));
        ir.push_str(&format!("  store i64 1, ptr %r{reg}.present.slot\n"));
        emit_dynamic_int_list_allocas(&mut ir, &format!("list{}", ptr_list_reg_id(function_index, reg)));
    }
    emit_dynamic_int_list_allocas(&mut ir, &format!("list{param_list_id}"));
    for (pc, instr) in code.iter().copied().enumerate() {
        if ptr_list_alloca_needed(function, instr) || matches!(instr.opcode(), Opcode::Call) {
            emit_dynamic_int_list_allocas(&mut ir, &format!("list{pc}"));
        }
    }
    let arg_len = next_tmp(&mut 0usize);
    ir.push_str(&format!("  {arg_len} = load i64, ptr %arg0.len.slot\n"));
    ir.push_str(&format!(
        "  call void @lk_slice_ptr_list(ptr %arg0.values, i64 {arg_len}, i64 0, ptr %list{param_list_id}.ptr.slots, ptr %list{param_list_id}.len.slot)\n"
    ));
    ir.push_str("  br label %bb0\n\n");
    static_regs[0] = Some(NativeStraightlineValue::DynamicList {
        id: param_list_id,
        element: NativeListElementKind::StrPtr,
    });

    let block_targets = find_block_targets(&code, code_len);
    let mut tmp_index = 1usize;
    let mut emitted_terminator = true;
    let mut after_return = false;
    for (pc, instr) in code.iter().copied().enumerate() {
        if pc == 0 {
            ir.push_str("bb0:\n");
            emitted_terminator = false;
        } else if block_targets.contains(&pc) {
            if !emitted_terminator {
                ir.push_str(&format!("  br label {}\n", native_label(pc, code_len)));
            }
            ir.push_str(&format!("{}:\n", native_label(pc, code_len).trim_start_matches('%')));
            after_return = false;
            emitted_terminator = false;
        }
        if after_return {
            continue;
        }
        emitted_terminator = false;
        match instr.opcode() {
            Opcode::Nop => {}
            Opcode::Jmp => {
                let Some(target) = native_relative_target(pc, instr.sj_arg(), code_len) else {
                    return Ok(None);
                };
                ir.push_str(&format!("  br label {}\n", native_label(target, code_len)));
                emitted_terminator = true;
            }
            Opcode::Test | Opcode::BrFalse | Opcode::BrTrue => {
                let value = next_tmp(&mut tmp_index);
                let cond = next_tmp(&mut tmp_index);
                ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.a()));
                ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
                let Some((truthy_target, falsy_target)) = branch_truthy_falsy_targets(pc, instr, code_len) else {
                    return Ok(None);
                };
                ir.push_str(&format!(
                    "  br i1 {cond}, label {}, label {}\n",
                    native_label(truthy_target, code_len),
                    native_label(falsy_target, code_len)
                ));
                emitted_terminator = true;
            }
            opcode if opcode.is_compare_test() => {
                let Some((taken, fallthrough)) = compare_test_targets(&code, pc, code_len) else {
                    return Ok(None);
                };
                let Some(pred) = compare_test_i64_pred(instr.opcode()) else {
                    return Ok(None);
                };
                let lhs = next_tmp(&mut tmp_index);
                let cond = next_tmp(&mut tmp_index);
                let branch_cond = next_tmp(&mut tmp_index);
                ir.push_str(&format!("  {lhs} = load i64, ptr %r{}.slot\n", instr.a()));
                let rhs = if opcode.is_int_immediate_compare_test() {
                    i64::from(instr.sc()).to_string()
                } else {
                    let rhs = next_tmp(&mut tmp_index);
                    ir.push_str(&format!("  {rhs} = load i64, ptr %r{}.slot\n", instr.b()));
                    rhs
                };
                ir.push_str(&format!("  {cond} = icmp {pred} i64 {lhs}, {rhs}\n"));
                let jump_when = if opcode.is_int_immediate_compare_test() {
                    instr.b() != 0
                } else {
                    instr.c() != 0
                };
                if jump_when {
                    ir.push_str(&format!("  {branch_cond} = xor i1 {cond}, false\n"));
                } else {
                    ir.push_str(&format!("  {branch_cond} = xor i1 {cond}, true\n"));
                }
                ir.push_str(&format!(
                    "  br i1 {branch_cond}, label {}, label {}\n",
                    native_label(taken, code_len),
                    native_label(fallthrough, code_len)
                ));
                emitted_terminator = true;
            }
            Opcode::LoadInt => {
                let Some(value) = function.consts.ints.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
                static_regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(value.to_string()));
            }
            Opcode::LoadHeapConst => {
                let Some(ConstHeapValueData::List(values)) = function.consts.heap_values.get(instr.bx() as usize)
                else {
                    return Ok(None);
                };
                if !values.is_empty() {
                    return Ok(None);
                }
                let list_id = ptr_list_reg_id(function_index, instr.a() as usize);
                ir.push_str(&format!("  store i64 0, ptr %list{list_id}.len.slot\n"));
                static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
                    id: list_id,
                    element: NativeListElementKind::StrPtr,
                });
            }
            Opcode::LoadString => {
                let Some(value) = function.consts.strings.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                let symbol = format!("@lk_fn{function_index}_list_str_{pc}");
                extra_globals.push_str(&llvm_string_constant(&symbol, value));
                ir.push_str(&format!("  store ptr {symbol}, ptr %r{}.slot\n", instr.a()));
                static_regs[instr.a() as usize] = Some(NativeStraightlineValue::String {
                    symbol,
                    value: value.clone(),
                    len: value.chars().count(),
                    key_kind: super::straightline_value::NativeStringKeyKind::Short,
                });
            }
            Opcode::GetGlobal => {
                let Some(name) = artifact.module.globals.get(instr.bx() as usize) else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = native_static_global(name);
                if static_regs[instr.a() as usize].is_none() {
                    return Ok(None);
                }
            }
            Opcode::Move => {
                if let Some(NativeStraightlineValue::DynamicList { id: src_id, element }) =
                    static_regs.get(instr.b() as usize).and_then(Clone::clone)
                {
                    let dst_id = ptr_list_reg_id(function_index, instr.a() as usize);
                    if emit_dynamic_ptr_list_copy(&mut ir, src_id, dst_id, &mut tmp_index).is_none() {
                        return Ok(None);
                    }
                    static_regs[instr.a() as usize] =
                        Some(NativeStraightlineValue::DynamicList { id: dst_id, element });
                } else {
                    let value = next_tmp(&mut tmp_index);
                    ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.b()));
                    ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
                    static_regs[instr.a() as usize] = static_regs.get(instr.b() as usize).and_then(Clone::clone);
                }
            }
            Opcode::Len => {
                let Some(NativeStraightlineValue::DynamicList { id, .. }) =
                    static_regs.get(instr.b() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                let len = next_tmp(&mut tmp_index);
                ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
                ir.push_str(&format!("  store i64 {len}, ptr %r{}.slot\n", instr.a()));
                static_regs[instr.a() as usize] = None;
            }
            Opcode::AddInt => {
                emit_i64_binary_block(&mut ir, instr, &mut tmp_index);
                static_regs[instr.a() as usize] = None;
            }
            Opcode::CmpLtInt | Opcode::CmpGtInt | Opcode::CmpInt => {
                if emit_ptr_list_compare(&mut ir, instr, &static_regs, &mut tmp_index).is_none() {
                    return Ok(None);
                }
                static_regs[instr.a() as usize] = None;
            }
            Opcode::GetIndex | Opcode::GetList => {
                let Some(NativeStraightlineValue::DynamicList { id, .. }) =
                    static_regs.get(instr.b() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                let Some(value) = emit_dynamic_ptr_list_get(&mut ir, id, instr.a(), instr.c(), &mut tmp_index) else {
                    return Ok(None);
                };
                static_regs[instr.a() as usize] = Some(NativeStraightlineValue::StringPtr(value));
            }
            Opcode::ListPush => {
                let Some(NativeStraightlineValue::DynamicList { id, .. }) =
                    static_regs.get(instr.a() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                if emit_dynamic_ptr_list_push(&mut ir, id, instr.b(), &mut tmp_index).is_none() {
                    return Ok(None);
                }
            }
            Opcode::NewList => {
                let start = instr.b() as usize;
                let end = start
                    .checked_add(instr.c() as usize)
                    .ok_or_else(|| anyhow::anyhow!("arg overflow"))?;
                static_regs[instr.a() as usize] = static_regs
                    .get(start..end)
                    .and_then(|values| values.iter().cloned().collect())
                    .map(|elements| NativeStraightlineValue::ArgList { elements });
            }
            Opcode::Call => {
                let Some(NativeStraightlineValue::Builtin(NativeBuiltin::CoreCallMethod)) =
                    static_regs.get(instr.b() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                if emit_ptr_list_core_method(&mut ir, &mut static_regs, &code, instr, pc, &mut tmp_index).is_none() {
                    return Ok(None);
                }
            }
            opcode if opcode.is_return() && instr.return_count() == 1 => {
                let Some(NativeStraightlineValue::DynamicList { id, .. }) =
                    static_regs.get(instr.a() as usize).and_then(Clone::clone)
                else {
                    return Ok(None);
                };
                let len = next_tmp(&mut tmp_index);
                let base = next_tmp(&mut tmp_index);
                ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
                ir.push_str(&format!(
                    "  {base} = getelementptr [4096 x ptr], ptr %list{id}.ptr.slots, i64 0, i64 0\n"
                ));
                ir.push_str(&format!(
                    "  call void @lk_slice_ptr_list(ptr {base}, i64 {len}, i64 0, ptr %out.values, ptr %out.len.slot)\n"
                ));
                ir.push_str("  ret void\n");
                after_return = true;
                emitted_terminator = true;
            }
            _ => return Ok(None),
        }
    }
    ir.push_str("exit:\n  ret void\n");
    ir.push_str("}\n");
    ir.push_str(&extra_globals);
    Ok(Some(ir))
}

pub(super) use list::compile_native_i64_list_subfunction;

fn native_return_zero(kind: NativeScalarKind) -> &'static str {
    match kind {
        NativeScalarKind::StrPtr => "null",
        _ => "0",
    }
}

fn ptr_list_param_id(function_index: usize, param: usize) -> usize {
    PTR_LIST_PARAM_BASE + function_index.saturating_mul(16) + param
}

fn ptr_list_reg_id(function_index: usize, reg: usize) -> usize {
    PTR_LIST_REG_BASE + function_index.saturating_mul(256) + reg
}

fn ptr_list_alloca_needed(function: &FunctionData, instr: Instr) -> bool {
    matches!(instr.opcode(), Opcode::Call | Opcode::SliceFrom)
        || matches!(instr.opcode(), Opcode::LoadHeapConst)
            && matches!(
                function.consts.heap_values.get(instr.bx() as usize),
                Some(ConstHeapValueData::List(values)) if values.is_empty()
            )
}

fn emit_ptr_list_compare(
    ir: &mut String,
    instr: Instr,
    static_regs: &[Option<NativeStraightlineValue>],
    tmp_index: &mut usize,
) -> Option<()> {
    let lhs_is_str = matches!(
        static_regs.get(instr.b() as usize).and_then(|value| value.as_ref()),
        Some(NativeStraightlineValue::StringPtr(_) | NativeStraightlineValue::String { .. })
    );
    let rhs_is_str = matches!(
        static_regs.get(instr.c() as usize).and_then(|value| value.as_ref()),
        Some(NativeStraightlineValue::StringPtr(_) | NativeStraightlineValue::String { .. })
    );
    if lhs_is_str || rhs_is_str {
        let lhs = next_tmp(tmp_index);
        let rhs = next_tmp(tmp_index);
        let cmp = next_tmp(tmp_index);
        let cond = next_tmp(tmp_index);
        let out = next_tmp(tmp_index);
        ir.push_str(&format!("  {lhs} = load ptr, ptr %r{}.slot\n", instr.b()));
        ir.push_str(&format!("  {rhs} = load ptr, ptr %r{}.slot\n", instr.c()));
        ir.push_str(&format!("  {cmp} = call i32 @strcmp(ptr {lhs}, ptr {rhs})\n"));
        let op = match instr.opcode() {
            Opcode::CmpLtInt => "slt",
            Opcode::CmpGtInt => "sgt",
            Opcode::CmpInt => "eq",
            _ => return None,
        };
        ir.push_str(&format!("  {cond} = icmp {op} i32 {cmp}, 0\n"));
        ir.push_str(&format!("  {out} = zext i1 {cond} to i64\n"));
        ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
        return Some(());
    }
    emit_numeric_compare_block(ir, instr, NativeScalarKind::I64, NativeScalarKind::I64, tmp_index);
    Some(())
}

fn emit_ptr_list_core_method(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    instr: Instr,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let start = instr.a() as usize + 1;
    let end = start.checked_add(instr.c() as usize)?;
    if end > static_regs.len() || instr.c() != 3 {
        return None;
    }
    let NativeStraightlineValue::DynamicList { id, .. } = static_regs.get(start)?.clone()? else {
        return None;
    };
    let NativeStraightlineValue::String { value: method, .. } = static_regs.get(start + 1)?.clone()? else {
        return None;
    };
    match method.as_str() {
        "take" => {
            let arg_list_reg = instr.a().checked_add(3)?;
            let count_reg = single_arg_list_source_reg_before(code, pc, arg_list_reg)?;
            emit_dynamic_ptr_list_take(ir, id, pc, count_reg, tmp_index)?;
        }
        "skip" => {
            let arg_list_reg = instr.a().checked_add(3)?;
            let start_reg = single_arg_list_source_reg_before(code, pc, arg_list_reg)?;
            emit_dynamic_ptr_list_slice(ir, id, pc, start_reg, tmp_index)?;
        }
        "concat" | "chain" => {
            let arg_list_reg = instr.a().checked_add(3)?;
            let rhs_reg = single_arg_list_source_reg_before(code, pc, arg_list_reg)?;
            let NativeStraightlineValue::DynamicList { id: rhs_id, .. } =
                static_regs.get(rhs_reg as usize).cloned().flatten()?
            else {
                return None;
            };
            emit_dynamic_ptr_list_concat(ir, id, rhs_id, pc, tmp_index)?;
        }
        _ => return None,
    }
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
        id: pc,
        element: NativeListElementKind::StrPtr,
    });
    Some(())
}

fn single_arg_list_source_reg_before(code: &[Instr], pc: usize, reg: u8) -> Option<u8> {
    let start = pc.saturating_sub(16);
    for prev_pc in (start..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode::Move if prev.b() != reg => single_arg_list_source_reg_before(code, prev_pc, prev.b()),
            Opcode::NewList if prev.c() == 1 => Some(prev.b()),
            _ => None,
        };
    }
    None
}

/// Find all PCs that are targets of Jmp or Test branch instructions.
fn find_block_targets(code: &[Instr], code_len: usize) -> Vec<usize> {
    let mut targets = vec![0]; // entry
    for (pc, instr) in code.iter().copied().enumerate() {
        match instr.opcode() {
            Opcode::Jmp => {
                if let Some(target) = native_relative_target(pc, instr.sj_arg(), code_len) {
                    targets.push(target);
                }
            }
            Opcode::Test | Opcode::BrFalse | Opcode::BrTrue => {
                if let Some((truthy, falsy)) = branch_truthy_falsy_targets(pc, instr, code_len) {
                    targets.push(truthy);
                    targets.push(falsy);
                }
            }
            opcode if opcode.is_compare_test() => {
                if let Some(jmp) = code.get(pc + 1).copied()
                    && jmp.opcode() == Opcode::Jmp
                {
                    if let Some(target) = native_relative_target(pc + 1, jmp.sj_arg(), code_len) {
                        targets.push(target);
                    }
                    targets.push(pc + 2);
                }
            }
            _ => {}
        }
    }
    targets.sort();
    targets.dedup();
    targets
}

fn branch_truthy_falsy_targets(pc: usize, instr: Instr, code_len: usize) -> Option<(usize, usize)> {
    let fallthrough = pc + 1;
    let relative = match instr.opcode() {
        Opcode::Test => native_relative_target(pc, instr.c() as i8 as i32, code_len)?,
        Opcode::BrFalse | Opcode::BrTrue => native_relative_target(pc, instr.sbx() as i32, code_len)?,
        _ => return None,
    };
    let truthy = if matches!(instr.opcode(), Opcode::Test if instr.b() == 0) || instr.opcode() == Opcode::BrTrue {
        relative
    } else {
        fallthrough
    };
    let falsy = if matches!(instr.opcode(), Opcode::Test if instr.b() != 0) || instr.opcode() == Opcode::BrFalse {
        relative
    } else {
        fallthrough
    };
    Some((truthy, falsy))
}

fn compare_test_targets(code: &[Instr], pc: usize, code_len: usize) -> Option<(usize, usize)> {
    let jmp = code.get(pc + 1).copied()?;
    if jmp.opcode() != Opcode::Jmp {
        return None;
    }
    Some((native_relative_target(pc + 1, jmp.sj_arg(), code_len)?, pc + 2))
}

fn compare_test_i64_pred(opcode: Opcode) -> Option<&'static str> {
    Some(match opcode {
        Opcode::TestEqInt | Opcode::TestEqIntI => "eq",
        Opcode::TestNeInt | Opcode::TestNeIntI => "ne",
        Opcode::TestLtInt | Opcode::TestLtIntI => "slt",
        Opcode::TestLeInt | Opcode::TestLeIntI => "sle",
        Opcode::TestGtInt | Opcode::TestGtIntI => "sgt",
        Opcode::TestGeInt | Opcode::TestGeIntI => "sge",
        _ => return None,
    })
}

fn compute_callee_facts(
    artifact: &ModuleArtifact,
    function: &FunctionData,
    code: &[Instr],
) -> Result<Option<NativeScalarFacts>> {
    let global_count = artifact.module.globals.len();
    let register_count = function.register_count as usize;
    let param_count = function.param_count as usize;

    let fn_index = artifact
        .module
        .functions
        .iter()
        .position(|f| std::ptr::eq(f, function))
        .map(|i| i as u16);

    let mut param_candidates = fn_index
        .map(|idx| callsite_param_kind_candidates(artifact, idx, param_count))
        .unwrap_or_default();
    for candidate in subfunction_param_kind_candidates(param_count) {
        if !param_candidates.contains(&candidate) {
            param_candidates.push(candidate);
        }
    }

    let candidates = [NativeScalarKind::I64, NativeScalarKind::F64];
    for candidate in candidates {
        for param_kinds in &param_candidates {
            let mut kinds = vec![None; register_count];
            let mut static_values = vec![None; register_count];
            for (arg, kind) in param_kinds.iter().copied().enumerate() {
                kinds[arg] = Some(kind);
                static_values[arg] = Some(match kind {
                    NativeScalarKind::StrPtr => NativeStraightlineValue::StringPtr("@lk_empty_text".to_string()),
                    NativeScalarKind::F64 => NativeStraightlineValue::F64("0.0".to_string()),
                    NativeScalarKind::Bool => NativeStraightlineValue::Bool("0".to_string()),
                    NativeScalarKind::Nil => NativeStraightlineValue::Nil,
                    NativeScalarKind::I64 | NativeScalarKind::MaybeI64 => NativeStraightlineValue::I64("0".to_string()),
                    NativeScalarKind::MaybeStrPtr => NativeStraightlineValue::StringPtr("@lk_empty_text".to_string()),
                });
            }

            let mut recursive_hints: Vec<(u16, Option<NativeScalarKind>)> = Vec::new();
            if let Some(idx) = fn_index {
                recursive_hints.push((idx, Some(candidate)));
            }

            if let Some(facts) = native_scalar_block_facts_with_initial(
                register_count,
                global_count,
                &artifact.module.globals,
                &function.consts.ints,
                &function.consts.strings,
                &function.consts.heap_values,
                code,
                kinds,
                static_values,
                vec![None; global_count],
                vec![None; global_count],
                Some(&artifact.module.functions),
                &[],
                0,
                &recursive_hints,
            ) {
                return Ok(Some(facts));
            }
        }
    }
    Ok(None)
}

fn subfunction_param_kind_candidates(param_count: usize) -> Vec<Vec<NativeScalarKind>> {
    if param_count == 0 {
        return vec![Vec::new()];
    }
    let mut candidates = Vec::new();
    let total = 3usize.saturating_pow(param_count as u32);
    for mut encoded in 0..total {
        let mut kinds = Vec::with_capacity(param_count);
        for _ in 0..param_count {
            kinds.push(match encoded % 3 {
                0 => NativeScalarKind::I64,
                1 => NativeScalarKind::Bool,
                _ => NativeScalarKind::StrPtr,
            });
            encoded /= 3;
        }
        candidates.push(kinds);
    }
    candidates
}

fn callsite_param_kind_candidates(
    artifact: &ModuleArtifact,
    function_index: u16,
    param_count: usize,
) -> Vec<Vec<NativeScalarKind>> {
    let mut out = Vec::new();
    for function in &artifact.module.functions {
        let Ok(code) = function
            .code
            .iter()
            .copied()
            .map(Instr::try_from_raw)
            .collect::<Result<Vec<_>, _>>()
        else {
            continue;
        };
        for (pc, instr) in code.iter().copied().enumerate() {
            if instr.opcode() != Opcode::CallDirect
                || instr.b() as u16 != function_index
                || instr.c() as usize != param_count
            {
                continue;
            }
            let start = instr.a() as usize + 1;
            let Some(kinds) = (start..start + param_count)
                .map(|reg| {
                    let reg = u8::try_from(reg).ok()?;
                    local_heap_kind_before(&code, &function.consts.heap_values, pc, reg)
                        .or_else(|| local_register_kind_before(&code, pc, reg))
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            if !out.contains(&kinds) {
                out.push(kinds);
            }
        }
    }
    out
}

fn determine_return_kind(code: &[Instr], facts: &NativeScalarFacts) -> Option<NativeScalarKind> {
    let mut return_kind: Option<NativeScalarKind> = None;
    for (pc, instr) in code.iter().copied().enumerate() {
        if !instr.opcode().is_return() || instr.return_count() != 1 {
            continue;
        }
        let kind = facts.register_kind_before(pc, instr.a())?;
        match return_kind {
            None => return_kind = Some(kind),
            Some(prev) if prev != kind => return None,
            _ => {}
        }
    }
    return_kind
}
