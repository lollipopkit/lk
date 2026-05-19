use super::*;

pub(super) fn try_exec_math_op(
    op: &Op,
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    f: &Function,
    next_pc_default: usize,
) -> Result<Option<usize>> {
    let pc;
    match *op {
        Op::Add(dst, a, b) => {
            let a_val = rk_read(regs, &f.consts, a);
            let b_val = rk_read(regs, &f.consts, b);
            if let Some(a_str) = a_val.as_str()
                && let Some(out) = Val::concat_str_add_rhs(a_str, b_val)
            {
                assign_reg(frame_raw, regs, dst as usize, out);
            } else if let Some(b_str) = b_val.as_str()
                && let Some(out) = Val::concat_add_lhs_str(a_val, b_str)
            {
                assign_reg(frame_raw, regs, dst as usize, out);
            } else if !Vm::arith2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, "add", |x, y| x + y, |x, y| x + y)
            {
                let out = BinOp::Add.eval_vals(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                assign_reg(frame_raw, regs, dst as usize, out);
            }
            pc = next_pc_default;
        }
        Op::StrConcatKnownCap(dst, a, b) => {
            let a_val = rk_read(regs, &f.consts, a);
            let b_val = rk_read(regs, &f.consts, b);
            let out = match (a_val.as_str(), b_val.as_str()) {
                (Some(a_str), Some(b_str)) => Val::concat_strings(a_str, b_str),
                _ => BinOp::Add.eval_vals(a_val, b_val)?,
            };
            assign_reg(frame_raw, regs, dst as usize, out);
            pc = next_pc_default;
        }
        Op::StrConcatToStr(dst, lhs, src) => {
            let lhs_val = rk_read(regs, &f.consts, lhs);
            let out = if let Some(lhs_str) = lhs_val.as_str()
                && let Some(value) = Val::concat_str_tostr_rhs(lhs_str, &regs[src as usize])
            {
                value
            } else {
                let rhs = Val::to_str_value(&regs[src as usize]);
                BinOp::Add.eval_vals(lhs_val, &rhs)?
            };
            assign_reg(frame_raw, regs, dst as usize, out);
            pc = next_pc_default;
        }
        Op::Sub(dst, a, b) => {
            if !Vm::arith2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, "sub", |x, y| x - y, |x, y| x - y) {
                let out = BinOp::Sub.eval_vals(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                assign_reg(frame_raw, regs, dst as usize, out);
            }
            pc = next_pc_default;
        }
        Op::Mul(dst, a, b) => {
            if !Vm::arith2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, "mul", |x, y| x * y, |x, y| x * y) {
                let out = BinOp::Mul.eval_vals(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                assign_reg(frame_raw, regs, dst as usize, out);
            }
            pc = next_pc_default;
        }
        Op::Div(dst, a, b) => {
            let ar = rk_read(regs, &f.consts, a);
            let br = rk_read(regs, &f.consts, b);
            let dst_idx = dst as usize;
            match (ar, br) {
                (Val::Int(x), Val::Int(y)) => {
                    let res = *x as f64 / *y as f64;
                    if res.fract() == 0.0 {
                        assign_reg(frame_raw, regs, dst_idx, Val::Int(res as i64));
                    } else {
                        assign_reg(frame_raw, regs, dst_idx, Val::Float(res));
                    }
                }
                (Val::Float(x), Val::Float(y)) => {
                    assign_reg(frame_raw, regs, dst_idx, Val::Float(x / y));
                }
                (Val::Int(x), Val::Float(y)) => {
                    assign_reg(frame_raw, regs, dst_idx, Val::Float(*x as f64 / y));
                }
                (Val::Float(x), Val::Int(y)) => {
                    assign_reg(frame_raw, regs, dst_idx, Val::Float(x / *y as f64));
                }
                _ => {
                    let out = BinOp::Div.eval_vals(ar, br)?;
                    assign_reg(frame_raw, regs, dst_idx, out);
                }
            }
            pc = next_pc_default;
        }
        Op::Mod(dst, a, b) => {
            match (rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b)) {
                (Val::Int(x), Val::Int(y)) => assign_reg(frame_raw, regs, dst as usize, Val::Int(x % y)),
                _ => {
                    let out = BinOp::Mod.eval_vals(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                    assign_reg(frame_raw, regs, dst as usize, out);
                }
            }
            pc = next_pc_default;
        }
        Op::AddInt(dst, a, b) => {
            let a_val = &regs[a as usize];
            let b_val = &regs[b as usize];
            if let (Some(a_str), Some(b_str)) = (a_val.as_str(), b_val.as_str()) {
                assign_reg(frame_raw, regs, dst as usize, Val::concat_strings(a_str, b_str));
            } else {
                int_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x + y, BinOp::Add)?;
            }
            pc = next_pc_default;
        }
        Op::AddFloat(dst, a, b) => {
            float_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x + y, BinOp::Add)?;
            pc = next_pc_default;
        }
        Op::AddIntImm(dst, a, imm) => {
            let src_idx = a as usize;
            let dst_idx = dst as usize;
            if let Val::Int(x) = regs[src_idx] {
                assign_reg(frame_raw, regs, dst_idx, Val::Int(x + imm as i64));
            } else {
                int_binop_imm(frame_raw, regs, &f.consts, dst, a, imm, |x, y| x + y, BinOp::Add)?;
            }
            pc = next_pc_default;
        }
        Op::SubInt(dst, a, b) => {
            int_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x - y, BinOp::Sub)?;
            pc = next_pc_default;
        }
        Op::SubFloat(dst, a, b) => {
            float_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x - y, BinOp::Sub)?;
            pc = next_pc_default;
        }
        Op::CmpEqImm(dst, a, imm) => {
            cmp_eq_imm(frame_raw, regs, &f.consts, dst, a, imm, BinOp::Eq)?;
            pc = next_pc_default;
        }
        Op::CmpNeImm(dst, a, imm) => {
            cmp_ne_imm(frame_raw, regs, &f.consts, dst, a, imm, BinOp::Ne)?;
            pc = next_pc_default;
        }
        Op::CmpLtImm(dst, a, imm) => {
            cmp_ord_imm(
                frame_raw,
                regs,
                &f.consts,
                dst,
                a,
                imm,
                |x, y| x < y,
                |x, y| x < y,
                BinOp::Lt,
            )?;
            pc = next_pc_default;
        }
        Op::CmpLeImm(dst, a, imm) => {
            cmp_ord_imm(
                frame_raw,
                regs,
                &f.consts,
                dst,
                a,
                imm,
                |x, y| x <= y,
                |x, y| x <= y,
                BinOp::Le,
            )?;
            pc = next_pc_default;
        }
        Op::CmpGtImm(dst, a, imm) => {
            cmp_ord_imm(
                frame_raw,
                regs,
                &f.consts,
                dst,
                a,
                imm,
                |x, y| x > y,
                |x, y| x > y,
                BinOp::Gt,
            )?;
            pc = next_pc_default;
        }
        Op::CmpGeImm(dst, a, imm) => {
            cmp_ord_imm(
                frame_raw,
                regs,
                &f.consts,
                dst,
                a,
                imm,
                |x, y| x >= y,
                |x, y| x >= y,
                BinOp::Ge,
            )?;
            pc = next_pc_default;
        }
        Op::MulInt(dst, a, b) => {
            int_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x * y, BinOp::Mul)?;
            pc = next_pc_default;
        }
        Op::MulFloat(dst, a, b) => {
            float_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x * y, BinOp::Mul)?;
            pc = next_pc_default;
        }
        Op::DivFloat(dst, a, b) => {
            float_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x / y, BinOp::Div)?;
            pc = next_pc_default;
        }
        Op::ModInt(dst, a, b) => {
            int_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x % y, BinOp::Mod)?;
            pc = next_pc_default;
        }
        Op::ModFloat(dst, a, b) => {
            float_binop(frame_raw, regs, &f.consts, dst, a, b, |x, y| x % y, BinOp::Mod)?;
            pc = next_pc_default;
        }
        Op::CmpEq(dst, a, b) => {
            assign_reg(
                frame_raw,
                regs,
                dst as usize,
                Val::Bool(rk_read(regs, &f.consts, a) == rk_read(regs, &f.consts, b)),
            );
            pc = next_pc_default;
        }
        Op::CmpNe(dst, a, b) => {
            assign_reg(
                frame_raw,
                regs,
                dst as usize,
                Val::Bool(rk_read(regs, &f.consts, a) != rk_read(regs, &f.consts, b)),
            );
            pc = next_pc_default;
        }
        Op::CmpLt(dst, a, b) => {
            if !Vm::cmp2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, |x, y| x < y, |x, y| x < y) {
                let res = BinOp::Lt.cmp(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                assign_reg(frame_raw, regs, dst as usize, Val::Bool(res));
            }
            pc = next_pc_default;
        }
        Op::CmpLe(dst, a, b) => {
            if !Vm::cmp2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, |x, y| x <= y, |x, y| x <= y) {
                let res = BinOp::Le.cmp(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                assign_reg(frame_raw, regs, dst as usize, Val::Bool(res));
            }
            pc = next_pc_default;
        }
        Op::CmpGt(dst, a, b) => {
            if !Vm::cmp2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, |x, y| x > y, |x, y| x > y) {
                let res = BinOp::Gt.cmp(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                assign_reg(frame_raw, regs, dst as usize, Val::Bool(res));
            }
            pc = next_pc_default;
        }
        Op::CmpGe(dst, a, b) => {
            if !Vm::cmp2_try_numeric(frame_raw, regs, &f.consts, dst, a, b, |x, y| x >= y, |x, y| x >= y) {
                let res = BinOp::Ge.cmp(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                assign_reg(frame_raw, regs, dst as usize, Val::Bool(res));
            }
            pc = next_pc_default;
        }
        Op::CmpI { dst, a, b, kind } => {
            let (Val::Int(lhs), Val::Int(rhs)) = (&regs[a as usize], &regs[b as usize]) else {
                anyhow::bail!("CmpI expects integer registers");
            };
            assign_reg(frame_raw, regs, dst as usize, Val::Bool(kind.eval(*lhs, *rhs)));
            pc = next_pc_default;
        }
        Op::Len { dst, src } => {
            let v = &regs[src as usize];
            let out = match v {
                Val::List(l) => Val::Int(l.len() as i64),
                Val::ShortStr(s) => Val::Int(s.as_str().len() as i64),
                Val::Str(s) => Val::Int(s.len() as i64),
                Val::Map(m) => Val::Int(m.len() as i64),
                _ => Val::Int(0),
            };
            assign_reg(frame_raw, regs, dst as usize, out);
            pc = next_pc_default;
        }
        Op::ListLen { dst, src } => {
            let out = match &regs[src as usize] {
                Val::List(list) => Val::Int(list.len() as i64),
                _ => Val::Int(0),
            };
            assign_reg(frame_raw, regs, dst as usize, out);
            pc = next_pc_default;
        }
        Op::MapLen { dst, src } => {
            let out = match &regs[src as usize] {
                Val::Map(map) => Val::Int(map.len() as i64),
                _ => Val::Int(0),
            };
            assign_reg(frame_raw, regs, dst as usize, out);
            pc = next_pc_default;
        }
        Op::StrLen { dst, src } => {
            let out = match &regs[src as usize] {
                Val::ShortStr(value) => Val::Int(value.as_str().len() as i64),
                Val::Str(value) => Val::Int(value.len() as i64),
                _ => Val::Int(0),
            };
            assign_reg(frame_raw, regs, dst as usize, out);
            pc = next_pc_default;
        }
        Op::Floor { dst, src } => {
            let out = match &regs[src as usize] {
                Val::Float(f) => Val::Int(f.floor() as i64),
                Val::Int(i) => Val::Int(*i),
                _ => Val::Int(0),
            };
            assign_reg(frame_raw, regs, dst as usize, out);
            pc = next_pc_default;
        }
        Op::StartsWithK(dst, src, kidx) => {
            let prefix = f.consts[kidx as usize].as_str().unwrap_or("");
            let out = match &regs[src as usize] {
                Val::ShortStr(s) => Val::Bool(s.as_str().starts_with(prefix)),
                Val::Str(s) => Val::Bool(s.as_str().starts_with(prefix)),
                _ => Val::Bool(false),
            };
            assign_reg(frame_raw, regs, dst as usize, out);
            pc = next_pc_default;
        }
        Op::ContainsK(dst, src, kidx) => {
            let needle = f.consts[kidx as usize].as_str().unwrap_or("");
            let out = match &regs[src as usize] {
                Val::ShortStr(s) => Val::Bool(s.as_str().contains(needle)),
                Val::Str(s) => Val::Bool(s.as_str().contains(needle)),
                _ => Val::Bool(false),
            };
            assign_reg(frame_raw, regs, dst as usize, out);
            pc = next_pc_default;
        }
        Op::ListFoldAdd { acc, list } => {
            let folded = if let Val::List(items) = &regs[list as usize] {
                Some(if let Val::Int(mut total) = regs[acc as usize] {
                    let mut all_int = true;
                    for item in items.iter() {
                        if let Val::Int(value) = item {
                            total = total.wrapping_add(*value);
                        } else {
                            all_int = false;
                            break;
                        }
                    }
                    if all_int {
                        Val::Int(total)
                    } else {
                        let mut out = regs[acc as usize].clone();
                        for item in items.iter() {
                            out = BinOp::Add.eval_vals(&out, item)?;
                        }
                        out
                    }
                } else {
                    let mut out = regs[acc as usize].clone();
                    for item in items.iter() {
                        out = BinOp::Add.eval_vals(&out, item)?;
                    }
                    out
                })
            } else {
                None
            };
            if let Some(out) = folded {
                assign_reg(frame_raw, regs, acc as usize, out);
            }
            pc = next_pc_default;
        }
        Op::MapValuesFoldAdd { acc, map } => {
            let folded = if let Val::Map(values) = &regs[map as usize] {
                Some(if let Val::Int(mut total) = regs[acc as usize] {
                    let mut all_int = true;
                    for item in values.values() {
                        if let Val::Int(value) = item {
                            total = total.wrapping_add(*value);
                        } else {
                            all_int = false;
                            break;
                        }
                    }
                    if all_int {
                        Val::Int(total)
                    } else {
                        let mut out = regs[acc as usize].clone();
                        for item in values.values() {
                            out = BinOp::Add.eval_vals(&out, item)?;
                        }
                        out
                    }
                } else {
                    let mut out = regs[acc as usize].clone();
                    for item in values.values() {
                        out = BinOp::Add.eval_vals(&out, item)?;
                    }
                    out
                })
            } else {
                None
            };
            if let Some(out) = folded {
                assign_reg(frame_raw, regs, acc as usize, out);
            }
            pc = next_pc_default;
        }
        _ => return Ok(None),
    }
    Ok(Some(pc))
}
