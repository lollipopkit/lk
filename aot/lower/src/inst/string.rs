//! String opcodes: literals, display conversion, concatenation, split/join.

use super::LowerCtx;
use crate::*;

pub(super) fn lower(
    ctx: &mut LowerCtx<'_>,
    block: usize,
    insts: &mut Vec<Inst>,
    instr: &Instr,
    pc: usize,
) -> Result<(), Unsupported> {
    let ssa = &mut *ctx.ssa;
    let globals = &mut *ctx.globals;
    let sig = &mut *ctx.sig;
    let func = ctx.func;
    let funcs = ctx.funcs;
    let entry = ctx.entry;
    match instr.opcode() {
        // A string constant materializes an interned module global (a C-string) with
        // type `Str`. String *operations* (concat/compare/…) aren't modelled, so a
        // `Str` flowing into one rejects (falls back); but a returned literal prints,
        // and a constant map key is consumed directly by the map ABI.
        Opcode::LoadString => {
            let s = func
                .consts
                .strings
                .get(instr.bx() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            let gid = intern_global(globals, s);
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::Str(GlobalId(gid)),
            });
            ssa.const_strs.insert(dst, s.clone());
            ssa.write(instr.a(), block, (dst, Ty::Str));
        }
        Opcode::ToString => {
            // `a` = dst, `b` = source. Display-convert to a `Str` (Str/Int/Bool
            // supported; float/other fall back).
            let (v, ty) = ssa.read(instr.b(), block, pc)?;
            // Auto-Display (plan J1): a single-interpolation template string
            // (`"${point}"`) compiles to a bare `ToString`.
            let (v, ty) = apply_display_show(ssa, insts, funcs, entry, sig, v, ty, pc)?;
            // The result is register-visible, so it stays arena-owned (never
            // freed eagerly, reclaimed by `lkrt_cleanup` at exit).
            let (s, _fresh) = to_display_str(ssa, insts, globals, v, ty, false, pc)?;
            ssa.write(instr.a(), block, (s, Ty::Str));
        }
        Opcode::ConcatString => {
            // `a` = dst, `b` = lhs, `c` = rhs. Concatenate `display(lhs) ++
            // display(rhs)` (each operand display-converted, as the VM does);
            // an int rhs fuses into a single `concat_i64` call.
            let (lv, lty) = ssa.read(instr.b(), block, pc)?;
            let (rv, rty) = ssa.read(instr.c(), block, pc)?;
            // Auto-Display (plan J1): a struct-instance operand with a
            // registered `show` interpolates its result, like the VM.
            let (lv, lty) = apply_display_show(ssa, insts, funcs, entry, sig, lv, lty, pc)?;
            let (rv, rty) = apply_display_show(ssa, insts, funcs, entry, sig, rv, rty, pc)?;
            let (l, l_fresh) = to_display_str(ssa, insts, globals, lv, lty, false, pc)?;
            let dst = concat_display(ssa, insts, globals, l, rv, rty, false, pc)?;
            if l_fresh {
                free_owned_str(insts, l);
            }
            ssa.write(instr.a(), block, (dst, Ty::Str));
        }
        Opcode::ConcatN => {
            // `a` = dst, `b` = first element register, `c` = element count. The VM
            // display-converts each element then concatenates; each element is
            // display-converted (`Str`/`Int`/`Bool`) and folded via repeated
            // `str_concat` — int elements fuse into `concat_i64` (no suffix
            // temporary). A float/other element rejects (falls back).
            let start = instr.b();
            let count = instr.c() as usize;
            let result = if count == 0 {
                // Empty concat → the empty string.
                let gid = intern_global(globals, "");
                let dst = ssa.new_val();
                insts.push(Inst::Const {
                    dst,
                    value: Const::Str(GlobalId(gid)),
                });
                dst
            } else {
                let (v0, ty0) = ssa.read(start, block, pc)?;
                let (v0, ty0) = apply_display_show(ssa, insts, funcs, entry, sig, v0, ty0, pc)?;
                let (mut acc, mut acc_fresh) = to_display_str(ssa, insts, globals, v0, ty0, false, pc)?;
                for i in 1..count {
                    let (v, ty) = ssa.read(start.wrapping_add(i as u8), block, pc)?;
                    let (v, ty) = apply_display_show(ssa, insts, funcs, entry, sig, v, ty, pc)?;
                    let dst = concat_display(ssa, insts, globals, acc, v, ty, false, pc)?;
                    // The consumed accumulator is dead; free it if this
                    // lowering allocated it.
                    if acc_fresh {
                        free_owned_str(insts, acc);
                    }
                    acc = dst;
                    acc_fresh = true;
                }
                acc
            };
            ssa.write(instr.a(), block, (result, Ty::Str));
        }
        Opcode::ListJoin => {
            // `a` = dst, `b` = list, `c` = separator. The VM joins a *string* list; we
            // support `List<str>` with a `Str` separator → a fresh `Str`.
            let (handle, list_ty) = ssa.read(instr.b(), block, pc)?;
            if list_ty != Ty::ListStr {
                return Err(Unsupported::TypeMismatch { pc });
            }
            let sep = ssa.read_typed(instr.c(), block, Ty::Str, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "str_join"),
                args: vec![handle, sep],
            });
            ssa.write(instr.a(), block, (dst, Ty::Str));
        }
        Opcode::StringSplit => {
            // `a` = dst (List<str>), `b` = target string, `c` = separator string.
            // The runtime uses Rust `str::split`, so the result matches the VM's
            // `string_split` exactly.
            let target = ssa.read_typed(instr.b(), block, Ty::Str, pc)?;
            let sep = ssa.read_typed(instr.c(), block, Ty::Str, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "split"),
                args: vec![target, sep],
            });
            ssa.write(instr.a(), block, (dst, Ty::ListStr));
        }
        op => return Err(Unsupported::Opcode { pc, op }),
    }
    Ok(())
}
