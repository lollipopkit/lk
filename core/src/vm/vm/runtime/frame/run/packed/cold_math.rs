use super::*;

pub(super) fn try_exec_math_op(
    op: &Op,
    regs: &mut [Val],
    f: &Function,
    next_pc_default: usize,
    collect_metrics: bool,
) -> Result<Option<usize>> {
    let pc;
    match *op {
        Op::Add(dst, a, b) => {
            let a_val = rk_read(regs, &f.consts, a);
            let b_val = rk_read(regs, &f.consts, b);
            if let Some(a_str) = a_val.as_str()
                && let Some(out) = Val::concat_str_add_rhs(a_str, b_val)
            {
                assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            } else if let Some(b_str) = b_val.as_str()
                && let Some(out) = Val::concat_add_lhs_str(a_val, b_str)
            {
                assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            } else if !Vm::arith2_try_numeric(
                regs,
                &f.consts,
                dst,
                a,
                b,
                "add",
                |x, y| x + y,
                |x, y| x + y,
                collect_metrics,
            ) {
                let out = BinOp::Add.eval_vals_with_metrics(
                    rk_read(regs, &f.consts, a),
                    rk_read(regs, &f.consts, b),
                    collect_metrics,
                )?;
                assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            }
            pc = next_pc_default;
        }
        Op::StrConcatKnownCap(dst, a, b) => {
            let a_val = rk_read(regs, &f.consts, a);
            let b_val = rk_read(regs, &f.consts, b);
            let out = match (a_val.as_str(), b_val.as_str()) {
                (Some(a_str), Some(b_str)) => Val::concat_strings(a_str, b_str),
                _ => BinOp::Add.eval_vals_with_metrics(a_val, b_val, collect_metrics)?,
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
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
                BinOp::Add.eval_vals_with_metrics(lhs_val, &rhs, collect_metrics)?
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::Sub(dst, a, b) => {
            if !Vm::arith2_try_numeric(
                regs,
                &f.consts,
                dst,
                a,
                b,
                "sub",
                |x, y| x - y,
                |x, y| x - y,
                collect_metrics,
            ) {
                let out = BinOp::Sub.eval_vals_with_metrics(
                    rk_read(regs, &f.consts, a),
                    rk_read(regs, &f.consts, b),
                    collect_metrics,
                )?;
                assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            }
            pc = next_pc_default;
        }
        Op::Mul(dst, a, b) => {
            if !Vm::arith2_try_numeric(
                regs,
                &f.consts,
                dst,
                a,
                b,
                "mul",
                |x, y| x * y,
                |x, y| x * y,
                collect_metrics,
            ) {
                let out = BinOp::Mul.eval_vals_with_metrics(
                    rk_read(regs, &f.consts, a),
                    rk_read(regs, &f.consts, b),
                    collect_metrics,
                )?;
                assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
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
                        assign_reg_with_metrics(regs, dst_idx, Val::Int(res as i64), collect_metrics);
                    } else {
                        assign_reg_with_metrics(regs, dst_idx, Val::Float(res), collect_metrics);
                    }
                }
                (Val::Float(x), Val::Float(y)) => {
                    assign_reg_with_metrics(regs, dst_idx, Val::Float(x / y), collect_metrics);
                }
                (Val::Int(x), Val::Float(y)) => {
                    assign_reg_with_metrics(regs, dst_idx, Val::Float(*x as f64 / y), collect_metrics);
                }
                (Val::Float(x), Val::Int(y)) => {
                    assign_reg_with_metrics(regs, dst_idx, Val::Float(x / *y as f64), collect_metrics);
                }
                _ => {
                    let out = BinOp::Div.eval_vals_with_metrics(ar, br, collect_metrics)?;
                    assign_reg_with_metrics(regs, dst_idx, out, collect_metrics);
                }
            }
            pc = next_pc_default;
        }
        Op::Mod(dst, a, b) => {
            match (rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b)) {
                (Val::Int(x), Val::Int(y)) => {
                    assign_reg_with_metrics(regs, dst as usize, Val::Int(x % y), collect_metrics)
                }
                _ => {
                    let out = BinOp::Mod.eval_vals_with_metrics(
                        rk_read(regs, &f.consts, a),
                        rk_read(regs, &f.consts, b),
                        collect_metrics,
                    )?;
                    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
                }
            }
            pc = next_pc_default;
        }
        Op::AddInt(dst, a, b) => {
            let a_val = &regs[a as usize];
            let b_val = &regs[b as usize];
            if let (Some(a_str), Some(b_str)) = (a_val.as_str(), b_val.as_str()) {
                assign_reg_with_metrics(regs, dst as usize, Val::concat_strings(a_str, b_str), collect_metrics);
            } else {
                int_binop(regs, &f.consts, dst, a, b, |x, y| x + y, BinOp::Add, true)?;
            }
            pc = next_pc_default;
        }
        Op::AddFloat(dst, a, b) => {
            float_binop(regs, &f.consts, dst, a, b, |x, y| x + y, BinOp::Add, true)?;
            pc = next_pc_default;
        }
        Op::AddIntImm(dst, a, imm) => {
            let src_idx = a as usize;
            let dst_idx = dst as usize;
            if let Val::Int(x) = regs[src_idx] {
                assign_reg_with_metrics(regs, dst_idx, Val::Int(x + imm as i64), collect_metrics);
            } else {
                int_binop_imm(regs, &f.consts, dst, a, imm, |x, y| x + y, BinOp::Add, true)?;
            }
            pc = next_pc_default;
        }
        Op::SubInt(dst, a, b) => {
            int_binop(regs, &f.consts, dst, a, b, |x, y| x - y, BinOp::Sub, true)?;
            pc = next_pc_default;
        }
        Op::SubFloat(dst, a, b) => {
            float_binop(regs, &f.consts, dst, a, b, |x, y| x - y, BinOp::Sub, true)?;
            pc = next_pc_default;
        }
        Op::CmpEqImm(dst, a, imm) => {
            cmp_eq_imm(regs, &f.consts, dst, a, imm, BinOp::Eq, true)?;
            pc = next_pc_default;
        }
        Op::CmpNeImm(dst, a, imm) => {
            cmp_ne_imm(regs, &f.consts, dst, a, imm, BinOp::Ne, true)?;
            pc = next_pc_default;
        }
        Op::CmpLtImm(dst, a, imm) => {
            cmp_ord_imm(
                regs,
                &f.consts,
                dst,
                a,
                imm,
                |x, y| x < y,
                |x, y| x < y,
                BinOp::Lt,
                true,
            )?;
            pc = next_pc_default;
        }
        Op::CmpLeImm(dst, a, imm) => {
            cmp_ord_imm(
                regs,
                &f.consts,
                dst,
                a,
                imm,
                |x, y| x <= y,
                |x, y| x <= y,
                BinOp::Le,
                true,
            )?;
            pc = next_pc_default;
        }
        Op::CmpGtImm(dst, a, imm) => {
            cmp_ord_imm(
                regs,
                &f.consts,
                dst,
                a,
                imm,
                |x, y| x > y,
                |x, y| x > y,
                BinOp::Gt,
                true,
            )?;
            pc = next_pc_default;
        }
        Op::CmpGeImm(dst, a, imm) => {
            cmp_ord_imm(
                regs,
                &f.consts,
                dst,
                a,
                imm,
                |x, y| x >= y,
                |x, y| x >= y,
                BinOp::Ge,
                true,
            )?;
            pc = next_pc_default;
        }
        Op::MulInt(dst, a, b) => {
            int_binop(regs, &f.consts, dst, a, b, |x, y| x * y, BinOp::Mul, true)?;
            pc = next_pc_default;
        }
        Op::MulFloat(dst, a, b) => {
            float_binop(regs, &f.consts, dst, a, b, |x, y| x * y, BinOp::Mul, true)?;
            pc = next_pc_default;
        }
        Op::DivFloat(dst, a, b) => {
            float_binop(regs, &f.consts, dst, a, b, |x, y| x / y, BinOp::Div, true)?;
            pc = next_pc_default;
        }
        Op::ModInt(dst, a, b) => {
            int_binop(regs, &f.consts, dst, a, b, |x, y| x % y, BinOp::Mod, true)?;
            pc = next_pc_default;
        }
        Op::ModFloat(dst, a, b) => {
            float_binop(regs, &f.consts, dst, a, b, |x, y| x % y, BinOp::Mod, true)?;
            pc = next_pc_default;
        }
        Op::CmpEq(dst, a, b) => {
            assign_reg_with_metrics(
                regs,
                dst as usize,
                Val::Bool(rk_read(regs, &f.consts, a) == rk_read(regs, &f.consts, b)),
                collect_metrics,
            );
            pc = next_pc_default;
        }
        Op::CmpNe(dst, a, b) => {
            assign_reg_with_metrics(
                regs,
                dst as usize,
                Val::Bool(rk_read(regs, &f.consts, a) != rk_read(regs, &f.consts, b)),
                collect_metrics,
            );
            pc = next_pc_default;
        }
        Op::CmpLt(dst, a, b) => {
            if !Vm::cmp2_try_numeric(regs, &f.consts, dst, a, b, |x, y| x < y, |x, y| x < y, collect_metrics) {
                let res = BinOp::Lt.cmp(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                assign_reg_with_metrics(regs, dst as usize, Val::Bool(res), collect_metrics);
            }
            pc = next_pc_default;
        }
        Op::CmpLe(dst, a, b) => {
            if !Vm::cmp2_try_numeric(
                regs,
                &f.consts,
                dst,
                a,
                b,
                |x, y| x <= y,
                |x, y| x <= y,
                collect_metrics,
            ) {
                let res = BinOp::Le.cmp(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                assign_reg_with_metrics(regs, dst as usize, Val::Bool(res), collect_metrics);
            }
            pc = next_pc_default;
        }
        Op::CmpGt(dst, a, b) => {
            if !Vm::cmp2_try_numeric(regs, &f.consts, dst, a, b, |x, y| x > y, |x, y| x > y, collect_metrics) {
                let res = BinOp::Gt.cmp(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                assign_reg_with_metrics(regs, dst as usize, Val::Bool(res), collect_metrics);
            }
            pc = next_pc_default;
        }
        Op::CmpGe(dst, a, b) => {
            if !Vm::cmp2_try_numeric(
                regs,
                &f.consts,
                dst,
                a,
                b,
                |x, y| x >= y,
                |x, y| x >= y,
                collect_metrics,
            ) {
                let res = BinOp::Ge.cmp(rk_read(regs, &f.consts, a), rk_read(regs, &f.consts, b))?;
                assign_reg_with_metrics(regs, dst as usize, Val::Bool(res), collect_metrics);
            }
            pc = next_pc_default;
        }
        Op::CmpI { dst, a, b, kind } => {
            let (Val::Int(lhs), Val::Int(rhs)) = (&regs[a as usize], &regs[b as usize]) else {
                anyhow::bail!("CmpI expects integer registers");
            };
            assign_reg_with_metrics(regs, dst as usize, Val::Bool(kind.eval(*lhs, *rhs)), collect_metrics);
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
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::ListLen { dst, src } => {
            let out = match &regs[src as usize] {
                Val::List(list) => Val::Int(list.len() as i64),
                _ => Val::Int(0),
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::MapLen { dst, src } => {
            let out = match &regs[src as usize] {
                Val::Map(map) => Val::Int(map.len() as i64),
                _ => Val::Int(0),
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::StrLen { dst, src } => {
            let out = match &regs[src as usize] {
                Val::ShortStr(value) => Val::Int(value.as_str().len() as i64),
                Val::Str(value) => Val::Int(value.len() as i64),
                _ => Val::Int(0),
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::Floor { dst, src } => {
            let out = match &regs[src as usize] {
                Val::Float(f) => Val::Int(f.floor() as i64),
                Val::Int(i) => Val::Int(*i),
                _ => Val::Int(0),
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::FloorDivImm { dst, src, imm } => {
            let divisor = imm as i64;
            let out = match &regs[src as usize] {
                Val::Int(value) => Val::Int(floor_div_i64(*value, divisor)),
                Val::Float(value) => Val::Int((value / divisor as f64).floor() as i64),
                _ => Val::Int(0),
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::StartsWithK(dst, src, kidx) => {
            let prefix = f.consts[kidx as usize].as_str().unwrap_or("");
            let out = match &regs[src as usize] {
                Val::ShortStr(s) => Val::Bool(s.as_str().starts_with(prefix)),
                Val::Str(s) => Val::Bool(s.as_str().starts_with(prefix)),
                _ => Val::Bool(false),
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::ContainsK(dst, src, kidx) => {
            let needle = f.consts[kidx as usize].as_str().unwrap_or("");
            let out = match &regs[src as usize] {
                Val::ShortStr(s) => Val::Bool(s.as_str().contains(needle)),
                Val::Str(s) => Val::Bool(s.as_str().contains(needle)),
                _ => Val::Bool(false),
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::ListFoldAdd { acc, list } => {
            let folded = if let Val::List(items) = &regs[list as usize] {
                Some(fold_add_values_with_metrics(
                    &regs[acc as usize],
                    items.iter(),
                    collect_metrics,
                )?)
            } else {
                None
            };
            if let Some(out) = folded {
                assign_reg_with_metrics(regs, acc as usize, out, collect_metrics);
            }
            pc = next_pc_default;
        }
        Op::MapValuesFoldAdd { acc, map } => {
            let folded = if let Val::Map(values) = &regs[map as usize] {
                Some(fold_add_values_with_metrics(
                    &regs[acc as usize],
                    values.values(),
                    collect_metrics,
                )?)
            } else {
                None
            };
            if let Some(out) = folded {
                assign_reg_with_metrics(regs, acc as usize, out, collect_metrics);
            }
            pc = next_pc_default;
        }
        _ => return Ok(None),
    }
    Ok(Some(pc))
}
