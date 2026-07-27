//! Mutable-state opcodes: module globals and capture cells.

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
    let sig = &mut *ctx.sig;
    let module_globals = ctx.module_globals;
    let capture_params = ctx.capture_params;
    match instr.opcode() {
        Opcode::LoadCapture => {
            // `a` = dst, `bx` = capture index. Captures are cells: the loaded
            // register carries a cell ref whose `LoadCellVal` reads the hidden
            // trailing parameter (the cell's value at the call site). A direct
            // (non-cell) use of the register finds no SSA value and rejects.
            let k = instr.bx() as usize;
            if k >= capture_params.len() {
                return Err(Unsupported::BadConst { pc });
            }
            ssa.builtin_regs.insert((block, instr.a()), GlobalRef::CellParam(k));
        }
        Opcode::LoadCellVal => {
            // `a` = dst, `b` = cell register: reads the cell's current content.
            // The cell ref backtracks across blocks like any global ref; the
            // content read goes through the virtual slot (phis on demand).
            match ssa.builtin_ref_at(instr.b(), block) {
                Some(GlobalRef::CellParam(k)) => {
                    let &(v, ty) = capture_params.get(k).ok_or(Unsupported::BadConst { pc })?;
                    // A runtime cell (a `try$call` boundary capture) reads
                    // through the shared slot; a spawned goroutine reads its
                    // thread-private copy; by-value captures stay as-is.
                    if ty == Ty::Cell {
                        let dst = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(dst),
                            callee: AbiRef::new("rt", "cell_get"),
                            args: vec![v],
                        });
                        ssa.write(instr.a(), block, (dst, Ty::Dyn));
                    } else if ssa.spawned_isolate {
                        let slot = ssa.cellparam_slot(k);
                        let (sv, sty) = ssa.read_slot(slot, block, pc)?;
                        ssa.write(instr.a(), block, (sv, sty));
                    } else {
                        ssa.write(instr.a(), block, (v, ty));
                    }
                }
                Some(GlobalRef::Cell(cid)) => {
                    let slot = ssa.cell_slot(cid);
                    let (v, ty) = ssa.read_slot(slot, block, pc)?;
                    ssa.write(instr.a(), block, (v, ty));
                }
                _ => return Err(Unsupported::Opcode { pc, op: instr.opcode() }),
            }
        }
        Opcode::StoreCellVal => {
            // `a` = cell register, `b` = value register: updates the tracked
            // cell content. A `CellParam` backed by a *runtime* cell (the
            // `try$call` boundary) writes through the shared slot; a
            // by-value capture parameter still rejects (no write-back path).
            match ssa.builtin_ref_at(instr.a(), block) {
                Some(GlobalRef::Cell(cid)) => {
                    let (v, ty) = ssa.read(instr.b(), block, pc)?;
                    let slot = ssa.cell_slot(cid);
                    ssa.write_slot(slot, block, (v, ty));
                }
                Some(GlobalRef::CellParam(k)) => {
                    let &(cell, cty) = capture_params.get(k).ok_or(Unsupported::BadConst { pc })?;
                    if cty == Ty::Cell {
                        let (v, ty) = ssa.read(instr.b(), block, pc)?;
                        let boxed = to_dyn_any(ssa, insts, v, ty, pc)?;
                        insts.push(Inst::Call {
                            dst: None,
                            callee: AbiRef::new("rt", "cell_set"),
                            args: vec![cell, boxed],
                        });
                    } else if ssa.spawned_isolate {
                        // Isolate: the write lands in the goroutine's private
                        // slot, never visible to the spawner (VM snapshot).
                        let (v, ty) = ssa.read(instr.b(), block, pc)?;
                        let slot = ssa.cellparam_slot(k);
                        ssa.write_slot(slot, block, (v, ty));
                    } else {
                        // A by-value capture parameter has no write-back path.
                        return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                    }
                }
                _ => return Err(Unsupported::Opcode { pc, op: instr.opcode() }),
            }
        }
        Opcode::SetGlobal => {
            // Storing a function value into the global table is the compiler's
            // top-level `fn` bookkeeping — a no-op natively.
            if let Some(GlobalRef::UserFn) = ssa.builtin_regs.get(&(block, instr.a())) {
                return Ok(());
            }
            // A top-level `let f = |x| …` stores a lambda ref: a no-op when the
            // prescan proved the slot single-assigned with this exact closure
            // (readers resolve it statically); anything else would let readers
            // observe a stale ref, so it rejects.
            if let Some(GlobalRef::Lambda(fidx)) = ssa.builtin_regs.get(&(block, instr.a())) {
                if sig.lambda_globals.get(instr.bx() as usize).copied().flatten() == Some(*fidx) {
                    return Ok(());
                }
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            }
            // Writing a global whose *name* this lowering recognizes would let
            // later `GetGlobal` reads resolve to the stale builtin/module
            // meaning and miscompile (`println = f; println(x)`), so those
            // writes reject the program.
            let slot = instr.bx();
            let name = module_globals.get(slot as usize).map(String::as_str);
            if let Some(name) = name
                && (builtin_for_name(name).is_some() || module_global(name))
            {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            }
            // Mutable module global (a top-level `let` shared with functions).
            // Scalar slots stay typed when every write agrees; disagreeing or
            // non-scalar (but boxable) writes join the slot to `Dyn` — each
            // write boxes, reads flow through the Dyn arms (plan M4.2).
            let (v, ty) = ssa.read(instr.a(), block, pc)?;
            let obs = match ty {
                Ty::I64 | Ty::F64 | Ty::Bool | Ty::Str => ty,
                t if dyn_boxable_ty(t) => Ty::Dyn,
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            // A reader discovered this slot can be observed before its first
            // (non-prefix) write: only the Dyn carrier's zeroinit is nil.
            let obs = if sig.force_dyn_globals.contains(&slot) {
                Ty::Dyn
            } else {
                obs
            };
            let slot_ty = match sig.global_tys.get_mut(slot as usize) {
                Some(state @ None) => {
                    *state = Some(obs);
                    obs
                }
                Some(Some(prev)) if *prev != obs => {
                    *prev = Ty::Dyn;
                    Ty::Dyn
                }
                Some(Some(prev)) => *prev,
                None => return Err(Unsupported::Opcode { pc, op: instr.opcode() }),
            };
            let v = if slot_ty == Ty::Dyn && ty != Ty::Dyn {
                to_dyn_any(ssa, insts, v, ty, pc)?
            } else {
                v
            };
            insts.push(Inst::GlobalSet {
                gvar: sig.gvar(slot),
                src: v,
            });
        }
        Opcode::GetGlobal => {
            // Reads of known runtime builtins / stdlib module objects load a
            // global ref (no SSA value — any unrecognized use finds it
            // undefined and rejects). Reads of mutable scalar globals load the
            // typed value, but only for slots provably initialized in the
            // entry prefix (the VM's pre-write value is nil, native storage is
            // zero — a read that could observe it must reject).
            let slot = instr.bx();
            let name = module_globals.get(slot as usize).map(String::as_str);
            let global_ref = match name {
                Some(name) if let Some(builtin) = builtin_for_name(name) => Some(GlobalRef::Builtin(builtin)),
                // Two-level stdlib exports arrive as `module::member` global
                // names (`chan.close(c)` → `GetGlobal "chan::close"`).
                Some(name) if name.contains("::") => {
                    let (module, member) = name.split_once("::").expect("checked");
                    Some(GlobalRef::ModuleFn(module.to_string(), member.to_string()))
                }
                Some(name) if module_global(name) => Some(GlobalRef::Module(name.to_string())),
                _ => None,
            };
            if let Some(global_ref) = global_ref {
                ssa.builtin_regs.insert((block, instr.a()), global_ref);
                return Ok(());
            }
            // Import-derived bindings (aliases, `use {..} from`, bundled file
            // modules): only when the slot is never written (a user global of
            // the same name shadows the import, like the VM's environment).
            if sig.global_tys.get(slot as usize).copied().flatten().is_none()
                && let Some(name) = name
            {
                if let Some(module) = sig.imports.module_aliases.get(name) {
                    let global_ref = GlobalRef::Module(module.clone());
                    ssa.builtin_regs.insert((block, instr.a()), global_ref);
                    return Ok(());
                }
                if let Some((module, member)) = sig.imports.module_items.get(name) {
                    // `use { json } from encoding` binds a *submodule* object,
                    // not a function: member reads route through Module.
                    let global_ref = if is_submodule(module, member) {
                        GlobalRef::Module(member.clone())
                    } else {
                        GlobalRef::ModuleFn(module.clone(), member.clone())
                    };
                    ssa.builtin_regs.insert((block, instr.a()), global_ref);
                    return Ok(());
                }
                if let Some(&fidx) = sig.imports.file_items.get(name) {
                    ssa.builtin_regs.insert((block, instr.a()), GlobalRef::Lambda(fidx));
                    return Ok(());
                }
                if let Some(&bundle) = sig.imports.file_namespaces.get(name) {
                    ssa.builtin_regs
                        .insert((block, instr.a()), GlobalRef::UserModule(bundle));
                    return Ok(());
                }
            }
            // A single-assignment top-level lambda slot resolves statically to
            // its function reference (initialization-order safe: the prescan
            // only accepts entry-prefix writes, which precede any user call).
            if let Some(fidx) = sig.lambda_globals.get(slot as usize).copied().flatten() {
                ssa.builtin_regs.insert((block, instr.a()), GlobalRef::Lambda(fidx));
                return Ok(());
            }
            let initialized = sig.initialized_globals.get(slot as usize).copied().unwrap_or(false);
            let ty = sig.global_tys.get(slot as usize).copied().flatten();
            let Some(ty) = ty else {
                return Err(Unsupported::UnresolvedGlobal {
                    pc,
                    name: name.unwrap_or("<unnamed slot>").to_string(),
                });
            };
            // A typed slot read before its entry-prefix initialization could
            // observe native zero where the VM has nil. A `Dyn` slot is
            // exempt — its zeroinit `{0, 0}` *is* the nil tag, VM-exact — so
            // force the slot Dyn and rerun (retriable discovery: writes box,
            // an early read observes boxed nil).
            if !initialized && ty != Ty::Dyn {
                sig.force_dyn_globals.insert(slot);
                return Err(Unsupported::TypeMismatch { pc });
            }
            let dst = ssa.new_val();
            insts.push(Inst::GlobalGet {
                dst,
                gvar: sig.gvar(slot),
            });
            ssa.write(instr.a(), block, (dst, ty));
        }
        op => return Err(Unsupported::Opcode { pc, op }),
    }
    Ok(())
}

/// The single table of global *names* this lowering gives a builtin meaning.
///
/// `GetGlobal` resolves a read through it and `SetGlobal` rejects a write to
/// any name in it — two lists that must not drift, because a write the guard
/// lets through makes a later read resolve to the *builtin* meaning and ignore
/// the rebinding. They had drifted: the write guard spelled out eight names
/// while the read arm recognized twenty-one (`error`, `chan`, `send`, `recv`,
/// `spawn`, `try$call`, the `__lk_*` internals).
///
/// Nothing reaches that gap today — the type checker rejects rebinding
/// `chan`/`send`/`recv`/`spawn`/`println`/`Set`, and the `error`/`panic`/
/// `typeof` shapes that survive it reject later at the read — so this closes a
/// latent divergence rather than a reproducible miscompile. Keeping one table
/// is what makes the next `Builtin` addition safe by default.
pub(crate) fn builtin_for_name(name: &str) -> Option<Builtin> {
    // `cpu_*` is a rule, not a list: the LK name is `cpu_` followed by the
    // entry's name under the ABI table's `cpu` module, and the table already
    // knows which those are. Spelled out one arm per intrinsic, this was
    // fourteen copies of that rule, and the fifteenth `cpu` entry would compile
    // and link with no native meaning at all — the arm nobody remembered to
    // add. The `&'static str` comes back out of the table rather than from
    // `name`, which is also what gives the payload its lifetime.
    if let Some(entry) = name.strip_prefix("cpu_")
        && let Some(abi) = lk_aot_abi::find("cpu", entry)
    {
        return Some(Builtin::Cpu(abi.name));
    }
    Some(match name {
        "symbol_address" => Builtin::SymbolAddress,
        "call_address_2" => Builtin::CallAddress2,
        "volatile_read_u8" => Builtin::VolatileRead(8),
        "volatile_read_u16" => Builtin::VolatileRead(16),
        "volatile_read_u32" => Builtin::VolatileRead(32),
        "volatile_read_u64" => Builtin::VolatileRead(64),
        "volatile_write_u8" => Builtin::VolatileWrite(8),
        "volatile_write_u16" => Builtin::VolatileWrite(16),
        "volatile_write_u32" => Builtin::VolatileWrite(32),
        "volatile_write_u64" => Builtin::VolatileWrite(64),
        "port_in_u8" => Builtin::PortIn(8),
        "port_in_u16" => Builtin::PortIn(16),
        "port_in_u32" => Builtin::PortIn(32),
        "port_out_u8" => Builtin::PortOut(8),
        "port_out_u16" => Builtin::PortOut(16),
        "port_out_u32" => Builtin::PortOut(32),
        "println" => Builtin::Println,
        "print" => Builtin::Print,
        "assert" => Builtin::Assert,
        "assert_eq" => Builtin::AssertEq,
        "assert_ne" => Builtin::AssertNe,
        "panic" => Builtin::Panic,
        "typeof" => Builtin::Typeof,
        "__lk_call_method" => Builtin::CallMethod,
        "Set" => Builtin::SetCtor,
        "try$call" => Builtin::TryCall,
        "error" => Builtin::ErrorRaise,
        "__lk_merge_fields" => Builtin::MergeFields,
        "__lk_make_struct" => Builtin::MakeStruct,
        "__lk_bit_and" => Builtin::BitAnd,
        "__lk_bit_or" => Builtin::BitOr,
        "__lk_bit_not" => Builtin::BitNot,
        "__lk_shl" => Builtin::Shl,
        "__lk_shr" => Builtin::Shr,
        "chan" => Builtin::ChanNew,
        "send" => Builtin::ChanSend,
        "recv" => Builtin::ChanRecv,
        "spawn" => Builtin::Spawn,
        "select$block" => Builtin::SelectBlock,
        _ => return None,
    })
}
