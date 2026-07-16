//! `MirModule` → Cranelift IR lowering (the typed-builder backend).
//!
//! The string-IR renderer ([`crate::render_module`]) emits LLVM text; this path
//! instead builds Cranelift IR through the typed `FunctionBuilder`, so a
//! type-mismatched instruction fails to *compile* rather than producing invalid
//! IR caught only downstream. Being SSA-with-block-params, the MIR maps almost
//! 1:1 onto Cranelift blocks/params/branches.
//!
//! Phase 0 scope (the strangler slice): the scalar subset (int/float const and
//! arithmetic, comparisons, widen/narrow, select, boolean ops) plus block-param
//! control flow (`Br`/`CondBr`/`Ret`/`Abort`) of a non-entry function. Guarded
//! divides, ABI calls, strings, and the entry/`main` shape follow in later
//! phases; anything outside the slice returns [`ClifError::Unsupported`].

use std::collections::HashMap;

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{AbiParam, BlockArg, Function, InstBuilder, Signature, Value, types};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use lk_aot_mir::{CmpOp, Const, FloatBinOp, Inst, IntBinOp, MirFunction, Term, Ty};

/// Why a MIR shape is not (yet) lowerable through the Cranelift path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClifError {
    /// An instruction/type outside the current phase's slice.
    Unsupported(&'static str),
}

/// The Cranelift value type carrying a MIR [`Ty`] at the ABI. Scalars are
/// native; every handle/pointer shape is a pointer-sized integer, and the
/// `{i64,i64}`-carried `Maybe`/`Dyn` shapes are out of the Phase 0 slice.
pub fn ty_to_clif(ty: Ty) -> Result<types::Type, ClifError> {
    Ok(match ty {
        Ty::I64 => types::I64,
        Ty::F64 => types::F64,
        // Cranelift comparisons yield an `I8` boolean.
        Ty::Bool => types::I8,
        // Opaque handles / C-string pointers are pointer-sized.
        Ty::Str
        | Ty::ListI64
        | Ty::ListF64
        | Ty::ListStr
        | Ty::MapStrI64
        | Ty::MapI64I64
        | Ty::MapStrF64
        | Ty::MapI64F64
        | Ty::MapStrBool
        | Ty::Cell
        | Ty::Set
        | Ty::ListDyn
        | Ty::MapStrDyn => types::I64,
        Ty::Nil => return Err(ClifError::Unsupported("nil value type")),
        Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool | Ty::Dyn => {
            return Err(ClifError::Unsupported("Maybe/Dyn carrier"));
        }
    })
}

/// The Cranelift call signature of a MIR function under `call_conv`.
pub fn signature_of(func: &MirFunction, call_conv: CallConv) -> Result<Signature, ClifError> {
    let mut sig = Signature::new(call_conv);
    for (_, ty) in &func.params {
        sig.params.push(AbiParam::new(ty_to_clif(*ty)?));
    }
    if !matches!(func.ret, Ty::Nil) {
        sig.returns.push(AbiParam::new(ty_to_clif(func.ret)?));
    }
    Ok(sig)
}

/// Lower a non-entry MIR function body into `clif_func` (whose signature must
/// already match [`signature_of`]).
pub fn build_function(
    func: &MirFunction,
    clif_func: &mut Function,
    fb_ctx: &mut FunctionBuilderContext,
) -> Result<(), ClifError> {
    let mut builder = FunctionBuilder::new(clif_func, fb_ctx);
    let mut lower = Lower {
        values: HashMap::new(),
        blocks: HashMap::new(),
    };

    // Materialize every MIR block up front so branches can target them.
    for block in &func.blocks {
        lower.blocks.insert(block.id, builder.create_block());
    }
    let entry = lower.blocks[&func.entry];
    builder.append_block_params_for_function_params(entry);
    // Bind the function-signature params to the entry block's params.
    let entry_params: Vec<Value> = builder.block_params(entry).to_vec();
    for ((vid, _), value) in func.params.iter().zip(entry_params) {
        lower.values.insert(*vid, value);
    }
    // Non-entry blocks carry the SSA phi params as block params.
    for block in &func.blocks {
        if block.id == func.entry {
            continue;
        }
        let cb = lower.blocks[&block.id];
        for (vid, ty) in &block.params {
            let value = builder.append_block_param(cb, ty_to_clif(*ty)?);
            lower.values.insert(*vid, value);
        }
    }

    for block in &func.blocks {
        let cb = lower.blocks[&block.id];
        builder.switch_to_block(cb);
        for inst in &block.insts {
            lower.inst(&mut builder, inst)?;
        }
        lower.term(&mut builder, &block.term)?;
    }

    builder.seal_all_blocks();
    builder.finalize();
    Ok(())
}

struct Lower {
    values: HashMap<lk_aot_mir::ValueId, Value>,
    blocks: HashMap<lk_aot_mir::BlockId, cranelift_codegen::ir::Block>,
}

impl Lower {
    fn v(&self, id: lk_aot_mir::ValueId) -> Result<Value, ClifError> {
        self.values
            .get(&id)
            .copied()
            .ok_or(ClifError::Unsupported("value used before def"))
    }

    /// Block-call arguments (branch-passed values become the target block's params).
    fn block_args(&self, ids: &[lk_aot_mir::ValueId]) -> Result<Vec<BlockArg>, ClifError> {
        ids.iter().map(|id| Ok(BlockArg::Value(self.v(*id)?))).collect()
    }

    fn inst(&mut self, b: &mut FunctionBuilder, inst: &Inst) -> Result<(), ClifError> {
        match inst {
            Inst::Const { dst, value } => {
                let v = match value {
                    Const::I64(x) => b.ins().iconst(types::I64, *x),
                    Const::F64(x) => b.ins().f64const(*x),
                    Const::Bool(x) => b.ins().iconst(types::I8, i64::from(*x)),
                    Const::Str(_) | Const::FnAddr(_) | Const::Nil => {
                        return Err(ClifError::Unsupported("str/fnaddr/nil const"));
                    }
                };
                self.values.insert(*dst, v);
            }
            Inst::IntBin { dst, op, lhs, rhs } => {
                let (l, r) = (self.v(*lhs)?, self.v(*rhs)?);
                let v = match op {
                    IntBinOp::Add => b.ins().iadd(l, r),
                    IntBinOp::Sub => b.ins().isub(l, r),
                    IntBinOp::Mul => b.ins().imul(l, r),
                    IntBinOp::And => b.ins().band(l, r),
                    IntBinOp::Or => b.ins().bor(l, r),
                    IntBinOp::Xor => b.ins().bxor(l, r),
                    IntBinOp::Min => {
                        let c = b.ins().icmp(IntCC::SignedLessThan, l, r);
                        b.ins().select(c, l, r)
                    }
                    IntBinOp::Max => {
                        let c = b.ins().icmp(IntCC::SignedGreaterThan, l, r);
                        b.ins().select(c, l, r)
                    }
                    // Guarded divide/mod call the lkrt helpers (Phase 2, needs symbols).
                    IntBinOp::Div | IntBinOp::Mod => return Err(ClifError::Unsupported("guarded int div/mod")),
                };
                self.values.insert(*dst, v);
            }
            Inst::FloatBin { dst, op, lhs, rhs } => {
                let (l, r) = (self.v(*lhs)?, self.v(*rhs)?);
                let v = match op {
                    FloatBinOp::Add => b.ins().fadd(l, r),
                    FloatBinOp::Sub => b.ins().fsub(l, r),
                    FloatBinOp::Mul => b.ins().fmul(l, r),
                    FloatBinOp::Div | FloatBinOp::Mod => return Err(ClifError::Unsupported("guarded float div/mod")),
                };
                self.values.insert(*dst, v);
            }
            Inst::Cmp {
                dst,
                op,
                float,
                lhs,
                rhs,
            } => {
                let (l, r) = (self.v(*lhs)?, self.v(*rhs)?);
                let v = if *float {
                    b.ins().fcmp(float_cc(*op), l, r)
                } else {
                    b.ins().icmp(int_cc(*op), l, r)
                };
                self.values.insert(*dst, v);
            }
            Inst::IntToFloat { dst, src } => {
                let s = self.v(*src)?;
                let v = b.ins().fcvt_from_sint(types::F64, s);
                self.values.insert(*dst, v);
            }
            Inst::ZextBool { dst, src } => {
                let s = self.v(*src)?;
                let v = b.ins().uextend(types::I64, s);
                self.values.insert(*dst, v);
            }
            Inst::Not { dst, src } => {
                let s = self.v(*src)?;
                let v = b.ins().bxor_imm(s, 1);
                self.values.insert(*dst, v);
            }
            Inst::BoolAnd { dst, lhs, rhs } => {
                let (l, r) = (self.v(*lhs)?, self.v(*rhs)?);
                let v = b.ins().band(l, r);
                self.values.insert(*dst, v);
            }
            Inst::Select {
                dst,
                cond,
                then_v,
                else_v,
                ..
            } => {
                let (c, t, e) = (self.v(*cond)?, self.v(*then_v)?, self.v(*else_v)?);
                let v = b.ins().select(c, t, e);
                self.values.insert(*dst, v);
            }
            _ => return Err(ClifError::Unsupported("instruction outside Phase 0 slice")),
        }
        Ok(())
    }

    fn term(&mut self, b: &mut FunctionBuilder, term: &Term) -> Result<(), ClifError> {
        match term {
            Term::Ret(None) => {
                b.ins().return_(&[]);
            }
            Term::Ret(Some(v)) => {
                let value = self.v(*v)?;
                b.ins().return_(&[value]);
            }
            Term::Br { target, args } => {
                let a = self.block_args(args)?;
                let blk = self.blocks[target];
                b.ins().jump(blk, &a);
            }
            Term::CondBr {
                cond,
                then_blk,
                then_args,
                else_blk,
                else_args,
            } => {
                let c = self.v(*cond)?;
                let ta = self.block_args(then_args)?;
                let ea = self.block_args(else_args)?;
                let (tb, eb) = (self.blocks[then_blk], self.blocks[else_blk]);
                b.ins().brif(c, tb, &ta, eb, &ea);
            }
            Term::Abort => {
                b.ins()
                    .trap(cranelift_codegen::ir::TrapCode::user(1).expect("nonzero trap code"));
            }
        }
        Ok(())
    }
}

fn int_cc(op: CmpOp) -> IntCC {
    match op {
        CmpOp::Eq => IntCC::Equal,
        CmpOp::Ne => IntCC::NotEqual,
        CmpOp::Lt => IntCC::SignedLessThan,
        CmpOp::Le => IntCC::SignedLessThanOrEqual,
        CmpOp::Gt => IntCC::SignedGreaterThan,
        CmpOp::Ge => IntCC::SignedGreaterThanOrEqual,
    }
}

fn float_cc(op: CmpOp) -> FloatCC {
    match op {
        CmpOp::Eq => FloatCC::Equal,
        CmpOp::Ne => FloatCC::NotEqual,
        CmpOp::Lt => FloatCC::LessThan,
        CmpOp::Le => FloatCC::LessThanOrEqual,
        CmpOp::Gt => FloatCC::GreaterThan,
        CmpOp::Ge => FloatCC::GreaterThanOrEqual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cranelift_codegen::Context;
    use cranelift_codegen::settings::{self, Configurable};
    use lk_aot_mir::{Block as MirBlock, BlockId, FuncId, ValueId};

    /// Lower `func` and drive it through the full Cranelift pipeline
    /// (`Context::compile` runs the verifier and emits machine code). Success
    /// proves the lowering produced valid, codegen-able CLIF — the typed-builder
    /// correctness win the string-IR path lacked.
    fn compile_ok(func: &MirFunction) -> Result<(), String> {
        let mut flags = settings::builder();
        flags.set("opt_level", "speed").unwrap();
        let isa = cranelift_native::builder()
            .unwrap()
            .finish(settings::Flags::new(flags))
            .map_err(|e| e.to_string())?;
        let sig = signature_of(func, isa.default_call_conv()).map_err(|e| format!("{e:?}"))?;
        let mut ctx = Context::new();
        ctx.func.signature = sig;
        let mut fb_ctx = FunctionBuilderContext::new();
        build_function(func, &mut ctx.func, &mut fb_ctx).map_err(|e| format!("{e:?}"))?;
        ctx.compile(&*isa, &mut Default::default())
            .map(|_| ())
            .map_err(|e| format!("{e:?}"))
    }

    fn vid(n: u32) -> ValueId {
        ValueId(n)
    }

    // fn(a: i64, b: i64) -> i64 { return a * b + 1 }
    #[test]
    fn lowers_scalar_arithmetic() {
        let block = MirBlock {
            id: BlockId(0),
            params: vec![],
            insts: vec![
                Inst::IntBin {
                    dst: vid(2),
                    op: IntBinOp::Mul,
                    lhs: vid(0),
                    rhs: vid(1),
                },
                Inst::Const {
                    dst: vid(3),
                    value: Const::I64(1),
                },
                Inst::IntBin {
                    dst: vid(4),
                    op: IntBinOp::Add,
                    lhs: vid(2),
                    rhs: vid(3),
                },
            ],
            term: Term::Ret(Some(vid(4))),
        };
        let func = MirFunction {
            id: FuncId(0),
            params: vec![(vid(0), Ty::I64), (vid(1), Ty::I64)],
            blocks: vec![block],
            entry: BlockId(0),
            ret: Ty::I64,
        };
        compile_ok(&func).expect("scalar arithmetic must compile");
    }

    // fn(x: i64) -> i64 { if x < 10 { return x } else { return 10 } } — block-param CFG.
    #[test]
    fn lowers_block_param_control_flow() {
        let entry = MirBlock {
            id: BlockId(0),
            params: vec![],
            insts: vec![
                Inst::Const {
                    dst: vid(1),
                    value: Const::I64(10),
                },
                Inst::Cmp {
                    dst: vid(2),
                    op: CmpOp::Lt,
                    float: false,
                    lhs: vid(0),
                    rhs: vid(1),
                },
            ],
            term: Term::CondBr {
                cond: vid(2),
                then_blk: BlockId(1),
                then_args: vec![vid(0)],
                else_blk: BlockId(1),
                else_args: vec![vid(1)],
            },
        };
        let join = MirBlock {
            id: BlockId(1),
            params: vec![(vid(3), Ty::I64)],
            insts: vec![],
            term: Term::Ret(Some(vid(3))),
        };
        let func = MirFunction {
            id: FuncId(0),
            params: vec![(vid(0), Ty::I64)],
            blocks: vec![entry, join],
            entry: BlockId(0),
            ret: Ty::I64,
        };
        compile_ok(&func).expect("block-param control flow must compile");
    }

    // A deliberately-out-of-slice shape rejects cleanly instead of miscompiling.
    #[test]
    fn rejects_unsupported_shape() {
        let block = MirBlock {
            id: BlockId(0),
            params: vec![],
            insts: vec![Inst::IntBin {
                dst: vid(2),
                op: IntBinOp::Div,
                lhs: vid(0),
                rhs: vid(1),
            }],
            term: Term::Ret(Some(vid(2))),
        };
        let func = MirFunction {
            id: FuncId(0),
            params: vec![(vid(0), Ty::I64), (vid(1), Ty::I64)],
            blocks: vec![block],
            entry: BlockId(0),
            ret: Ty::I64,
        };
        assert!(compile_ok(&func).is_err());
    }
}
