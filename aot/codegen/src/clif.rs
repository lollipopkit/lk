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
use cranelift_codegen::ir::{AbiParam, BlockArg, Function, InstBuilder, MemFlagsData, Signature, Value, types};
use cranelift_codegen::isa::{CallConv, TargetIsa};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataDescription, DataId, FuncId as ClifFuncId, Linkage, Module, ModuleError};
use cranelift_object::{ObjectBuilder, ObjectModule};
use lk_aot_mir::{CmpOp, Const, FloatBinOp, FuncId, Inst, IntBinOp, MirFunction, MirModule, Term, Ty};

/// Why a MIR shape is not (yet) lowerable through the Cranelift path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClifError {
    /// An instruction/type outside the current phase's slice.
    Unsupported(&'static str),
    /// A `cranelift-module` failure (declaration/definition/emit).
    Module(String),
}

impl From<ModuleError> for ClifError {
    fn from(err: ModuleError) -> Self {
        ClifError::Module(err.to_string())
    }
}

/// The guarded integer/float divide/mod helpers (`lkrt`), never raw `sdiv`/`srem`
/// — matching the VM's divide-by-zero abort. Declared as imports and resolved by
/// linking the `lkrt` staticlib.
struct Helpers {
    i64_div: ClifFuncId,
    i64_mod: ClifFuncId,
    f64_div: ClifFuncId,
    f64_mod: ClifFuncId,
}

impl Helpers {
    fn declare(module: &mut dyn Module) -> Result<Self, ClifError> {
        let cc = module.isa().default_call_conv();
        let ii = |m: &mut dyn Module, name: &str, ty: types::Type| -> Result<ClifFuncId, ClifError> {
            let mut sig = Signature::new(cc);
            sig.params.push(AbiParam::new(ty));
            sig.params.push(AbiParam::new(ty));
            sig.returns.push(AbiParam::new(ty));
            Ok(m.declare_function(name, Linkage::Import, &sig)?)
        };
        Ok(Helpers {
            i64_div: ii(module, "lkrt_i64_div_checked", types::I64)?,
            i64_mod: ii(module, "lkrt_i64_mod_checked", types::I64)?,
            f64_div: ii(module, "lkrt_f64_div_checked", types::F64)?,
            f64_mod: ii(module, "lkrt_f64_mod_checked", types::F64)?,
        })
    }
}

/// Per-module lowering context threaded into the builder: the module (for
/// importing callees) plus the intra-module function, helper, and on-demand ABI
/// symbol tables.
struct ModuleCtx<'a> {
    module: &'a mut dyn Module,
    fn_ids: &'a HashMap<FuncId, ClifFuncId>,
    helpers: &'a Helpers,
    /// Declared ABI runtime symbols, cached by C symbol name (declared lazily so
    /// a program that never calls one does not force its — possibly `DynVal` —
    /// signature to lower).
    abi_ids: &'a mut HashMap<&'static str, ClifFuncId>,
    /// Interned string-constant data symbols (`lk_str_{i}`), by [`GlobalId`] index.
    str_data: &'a HashMap<u32, DataId>,
    /// Mutable module-global data symbols (`lk_gvar_{i}`) + their MIR type.
    gvar_data: &'a HashMap<u32, (DataId, Ty)>,
}

impl ModuleCtx<'_> {
    /// Declare (once) an ABI runtime function by its schema entry and return its
    /// Cranelift id. Rejects `DynVal`-carrying signatures (the `{i64,i64}`
    /// carrier is Phase 2b).
    fn abi_func(&mut self, abi: &lk_aot_abi::AbiFn) -> Result<ClifFuncId, ClifError> {
        if let Some(id) = self.abi_ids.get(abi.symbol) {
            return Ok(*id);
        }
        let cc = self.module.isa().default_call_conv();
        let mut sig = Signature::new(cc);
        for p in abi.params {
            sig.params.push(AbiParam::new(abi_ty_to_clif(*p)?));
        }
        if !matches!(abi.result, lk_aot_abi::AbiType::Nil) {
            sig.returns.push(AbiParam::new(abi_ty_to_clif(abi.result)?));
        }
        let id = self.module.declare_function(abi.symbol, Linkage::Import, &sig)?;
        self.abi_ids.insert(abi.symbol, id);
        Ok(id)
    }
}

/// The Cranelift type of an ABI parameter/return. Pointers are pointer-sized;
/// the `{i64,i64}` `DynVal` carrier is Phase 2b.
fn abi_ty_to_clif(ty: lk_aot_abi::AbiType) -> Result<types::Type, ClifError> {
    use lk_aot_abi::AbiType;
    Ok(match ty {
        AbiType::I64 => types::I64,
        AbiType::F64 => types::F64,
        AbiType::Ptr | AbiType::StrPtr => types::I64,
        AbiType::Nil => return Err(ClifError::Unsupported("nil ABI operand")),
        AbiType::DynVal => return Err(ClifError::Unsupported("DynVal {i64,i64} ABI carrier")),
    })
}

/// Compile a whole [`MirModule`]'s non-entry functions to a native object for
/// `isa`. Returns the object bytes (to be linked against `lkrt`). The
/// entry/`main` shape and the ABI-runtime/string/container instructions arrive
/// in later phases; a function using anything outside the slice returns
/// [`ClifError::Unsupported`].
pub fn compile_module(mir: &MirModule, isa: std::sync::Arc<dyn TargetIsa>) -> Result<Vec<u8>, ClifError> {
    let name = "lk_aot";
    let builder = ObjectBuilder::new(isa, name, cranelift_module::default_libcall_names())
        .map_err(|e| ClifError::Module(e.to_string()))?;
    let mut module = ObjectModule::new(builder);
    // Pointer-sized ABI shapes (handles/strings) and the `LkDyn {i64,i64}` carrier
    // are all 64-bit in the shared `lkrt` ABI, so pointers lower to `I64`. Reject
    // a non-64-bit target rather than silently emit wrong-width values.
    if module.isa().pointer_type() != types::I64 {
        return Err(ClifError::Unsupported("non-64-bit target (the lkrt ABI is 64-bit)"));
    }
    let cc = module.isa().default_call_conv();

    // Declare every function up front so calls can resolve intra-module targets.
    // The entry becomes the exported C `main() -> i32`; the rest are local
    // `lk_fn_N` with their MIR signatures.
    let mut fn_ids = HashMap::new();
    for func in &mir.functions {
        let (sym, linkage, sig) = if func.id == mir.entry {
            ("main".to_string(), Linkage::Export, main_signature(cc))
        } else {
            (format!("lk_fn_{}", func.id.0), Linkage::Local, signature_of(func, cc)?)
        };
        let id = module.declare_function(&sym, linkage, &sig)?;
        fn_ids.insert(func.id, id);
    }
    let helpers = Helpers::declare(&mut module)?;

    // Interned string constants → read-only, NUL-terminated data symbols.
    let mut str_data: HashMap<u32, DataId> = HashMap::new();
    for (i, s) in mir.globals.iter().enumerate() {
        let id = module.declare_data(&format!("lk_str_{i}"), Linkage::Local, false, false)?;
        let mut bytes = s.clone().into_bytes();
        bytes.push(0);
        let mut desc = DataDescription::new();
        desc.define(bytes.into_boxed_slice());
        module.define_data(id, &desc)?;
        str_data.insert(i as u32, id);
    }
    // Mutable module globals → writable, zero-initialized data symbols.
    let mut gvar_data: HashMap<u32, (DataId, Ty)> = HashMap::new();
    for (i, (_, ty)) in mir.mutable_globals.iter().enumerate() {
        let size = ty_to_clif(*ty)?.bytes() as usize;
        let id = module.declare_data(&format!("lk_gvar_{i}"), Linkage::Local, true, false)?;
        let mut desc = DataDescription::new();
        desc.define_zeroinit(size);
        module.define_data(id, &desc)?;
        gvar_data.insert(i as u32, (id, *ty));
    }

    // ABI runtime symbols are declared lazily but must dedup across functions.
    let mut abi_ids: HashMap<&'static str, ClifFuncId> = HashMap::new();

    for func in &mir.functions {
        let is_entry = func.id == mir.entry;
        let mut ctx = module.make_context();
        ctx.func.signature = if is_entry {
            main_signature(cc)
        } else {
            signature_of(func, cc)?
        };
        let mut fb_ctx = FunctionBuilderContext::new();
        {
            let mut mctx = ModuleCtx {
                module: &mut module,
                fn_ids: &fn_ids,
                helpers: &helpers,
                abi_ids: &mut abi_ids,
                str_data: &str_data,
                gvar_data: &gvar_data,
            };
            build_function(func, &mut ctx.func, &mut fb_ctx, &mut mctx, is_entry, mir.abi_version)?;
        }
        module.define_function(fn_ids[&func.id], &mut ctx)?;
        module.clear_context(&mut ctx);
    }

    let product = module.finish();
    product.emit().map_err(|e| ClifError::Module(e.to_string()))
}

/// Compile a module to a native relocatable object for the **host** target (the
/// common case: `lk compile` on the current machine). Link the result against
/// `lkrt` (see `lk-llvm`'s `compile_native_executable_from_object`).
pub fn compile_host_object(mir: &MirModule) -> Result<Vec<u8>, ClifError> {
    use cranelift_codegen::settings::{self, Configurable};
    let mut flags = settings::builder();
    let _ = flags.set("opt_level", "speed");
    // Native executables link the runtime dynamically, so calls to `lkrt_*`
    // must go through position-independent relocations (GOT/PLT). Without this
    // macOS' linker rejects the object with "illegal text-relocations".
    let _ = flags.set("is_pic", "true");
    let isa = cranelift_native::builder()
        .map_err(|e| ClifError::Module(e.to_string()))?
        .finish(settings::Flags::new(flags))
        .map_err(|e| ClifError::Module(e.to_string()))?;
    compile_module(mir, isa)
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

/// The C `main() -> i32` signature the entry function is compiled to.
fn main_signature(call_conv: CallConv) -> Signature {
    let mut sig = Signature::new(call_conv);
    sig.returns.push(AbiParam::new(types::I32));
    sig
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

/// Lower a MIR function body into `clif_func`. When `is_entry`, `clif_func` must
/// be the program `main` (`() -> i32`): its entry block gets an `abi_check`
/// prologue and its returns print the top-level result before `ret 0`, matching
/// the string-IR backend.
fn build_function(
    func: &MirFunction,
    clif_func: &mut Function,
    fb_ctx: &mut FunctionBuilderContext,
    mctx: &mut ModuleCtx,
    is_entry: bool,
    abi_version: i64,
) -> Result<(), ClifError> {
    let mut builder = FunctionBuilder::new(clif_func, fb_ctx);
    let mut lower = Lower {
        values: HashMap::new(),
        blocks: HashMap::new(),
        is_entry,
        ret_ty: func.ret,
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
        // The entry/`main` guards against ABI drift before any user code runs.
        if is_entry && block.id == func.entry {
            let check = mctx.abi_func(resolve_abi("lkrt", "abi_check")?)?;
            let version = builder.ins().iconst(types::I64, abi_version);
            lower.call(&mut builder, mctx, check, None, &[version])?;
        }
        for inst in &block.insts {
            lower.inst(&mut builder, mctx, inst)?;
        }
        lower.term(&mut builder, mctx, &block.term)?;
    }

    builder.seal_all_blocks();
    builder.finalize();
    Ok(())
}

fn resolve_abi(module: &str, name: &str) -> Result<&'static lk_aot_abi::AbiFn, ClifError> {
    lk_aot_abi::find(module, name).ok_or(ClifError::Unsupported("missing ABI function"))
}

struct Lower {
    values: HashMap<lk_aot_mir::ValueId, Value>,
    blocks: HashMap<lk_aot_mir::BlockId, cranelift_codegen::ir::Block>,
    /// This function is the program `main` (`() -> i32`): returns print the
    /// top-level result and `ret 0`.
    is_entry: bool,
    /// The MIR return type — selects the entry's result-printing conversion.
    ret_ty: Ty,
}

impl Lower {
    fn v(&self, id: lk_aot_mir::ValueId) -> Result<Value, ClifError> {
        self.values
            .get(&id)
            .copied()
            .ok_or(ClifError::Unsupported("value used before def"))
    }

    /// Resolve a list of value operands (call arguments).
    fn args_v(&self, ids: &[lk_aot_mir::ValueId]) -> Result<Vec<Value>, ClifError> {
        ids.iter().map(|id| self.v(*id)).collect()
    }

    /// Block-call arguments (branch-passed values become the target block's params).
    fn block_args(&self, ids: &[lk_aot_mir::ValueId]) -> Result<Vec<BlockArg>, ClifError> {
        ids.iter().map(|id| Ok(BlockArg::Value(self.v(*id)?))).collect()
    }

    /// Import `callee` into the current function and emit a call, binding the
    /// single return value (if any) to `dst`.
    fn call(
        &mut self,
        b: &mut FunctionBuilder,
        mctx: &mut ModuleCtx,
        callee: ClifFuncId,
        dst: Option<lk_aot_mir::ValueId>,
        args: &[Value],
    ) -> Result<(), ClifError> {
        let func_ref = mctx.module.declare_func_in_func(callee, b.func);
        let call = b.ins().call(func_ref, args);
        if let Some(dst) = dst {
            let results = b.inst_results(call);
            let v = *results
                .first()
                .ok_or(ClifError::Unsupported("call has no result for dst"))?;
            self.values.insert(dst, v);
        }
        Ok(())
    }

    fn inst(&mut self, b: &mut FunctionBuilder, mctx: &mut ModuleCtx, inst: &Inst) -> Result<(), ClifError> {
        match inst {
            Inst::Const { dst, value } => {
                let v = match value {
                    Const::I64(x) => b.ins().iconst(types::I64, *x),
                    Const::F64(x) => b.ins().f64const(*x),
                    Const::Bool(x) => b.ins().iconst(types::I8, i64::from(*x)),
                    Const::Str(g) => {
                        let data_id = *mctx
                            .str_data
                            .get(&g.0)
                            .ok_or(ClifError::Unsupported("undeclared string const"))?;
                        let gv = mctx.module.declare_data_in_func(data_id, b.func);
                        b.ins().global_value(types::I64, gv)
                    }
                    Const::FnAddr(_) | Const::Nil => {
                        return Err(ClifError::Unsupported("fnaddr/nil const"));
                    }
                };
                self.values.insert(*dst, v);
            }
            Inst::GlobalGet { dst, gvar } => {
                let (data_id, ty) = *mctx
                    .gvar_data
                    .get(gvar)
                    .ok_or(ClifError::Unsupported("undeclared global"))?;
                let gv = mctx.module.declare_data_in_func(data_id, b.func);
                let addr = b.ins().global_value(types::I64, gv);
                let v = b.ins().load(ty_to_clif(ty)?, MemFlagsData::trusted(), addr, 0);
                self.values.insert(*dst, v);
            }
            Inst::GlobalSet { gvar, src } => {
                let s = self.v(*src)?;
                let (data_id, _) = *mctx
                    .gvar_data
                    .get(gvar)
                    .ok_or(ClifError::Unsupported("undeclared global"))?;
                let gv = mctx.module.declare_data_in_func(data_id, b.func);
                let addr = b.ins().global_value(types::I64, gv);
                b.ins().store(MemFlagsData::trusted(), s, addr, 0);
            }
            Inst::PrintStr { value, newline } => {
                // Print via the existing `io.std.write(fd, str, newline)` ABI fn
                // (Rust `stdout`), not variadic `printf` — Cranelift cannot model
                // the C vararg calling convention. Single mechanism, byte-exact.
                let abi = lk_aot_mir::AbiRef::new("io.std", "write")
                    .resolve()
                    .ok_or(ClifError::Unsupported("io.std.write ABI fn missing"))?;
                let clif_id = mctx.abi_func(abi)?;
                let fd = b.ins().iconst(types::I64, 1);
                let s = self.v(*value)?;
                let nl = b.ins().iconst(types::I64, i64::from(*newline));
                return self.call(b, mctx, clif_id, None, &[fd, s, nl]);
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
                    // Guarded divide/mod call the lkrt helpers (never raw sdiv/srem).
                    IntBinOp::Div => return self.call(b, mctx, mctx.helpers.i64_div, Some(*dst), &[l, r]),
                    IntBinOp::Mod => return self.call(b, mctx, mctx.helpers.i64_mod, Some(*dst), &[l, r]),
                };
                self.values.insert(*dst, v);
            }
            Inst::FloatBin { dst, op, lhs, rhs } => {
                let (l, r) = (self.v(*lhs)?, self.v(*rhs)?);
                let v = match op {
                    FloatBinOp::Add => b.ins().fadd(l, r),
                    FloatBinOp::Sub => b.ins().fsub(l, r),
                    FloatBinOp::Mul => b.ins().fmul(l, r),
                    FloatBinOp::Div => return self.call(b, mctx, mctx.helpers.f64_div, Some(*dst), &[l, r]),
                    FloatBinOp::Mod => return self.call(b, mctx, mctx.helpers.f64_mod, Some(*dst), &[l, r]),
                };
                self.values.insert(*dst, v);
            }
            Inst::CallFn { dst, func, args } => {
                let a = self.args_v(args)?;
                let callee = *mctx
                    .fn_ids
                    .get(func)
                    .ok_or(ClifError::Unsupported("call to undeclared function"))?;
                return self.call(b, mctx, callee, *dst, &a);
            }
            Inst::Call { dst, callee, args } => {
                let abi = callee.resolve().ok_or(ClifError::Unsupported("unknown ABI function"))?;
                let a = self.args_v(args)?;
                let clif_id = mctx.abi_func(abi)?;
                return self.call(b, mctx, clif_id, *dst, &a);
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

    fn term(&mut self, b: &mut FunctionBuilder, mctx: &mut ModuleCtx, term: &Term) -> Result<(), ClifError> {
        match term {
            Term::Ret(value) if self.is_entry => return self.entry_return(b, mctx, *value),
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

    /// The entry/`main` return: print the top-level result (matching the
    /// string-IR auto-print, always via `io.std.write` with a newline) then
    /// `ret 0`. A `nil` return prints nothing.
    fn entry_return(
        &mut self,
        b: &mut FunctionBuilder,
        mctx: &mut ModuleCtx,
        value: Option<lk_aot_mir::ValueId>,
    ) -> Result<(), ClifError> {
        if let Some(v) = value {
            let sv = self.v(v)?;
            let str_ptr = match self.ret_ty {
                Ty::Str => sv,
                Ty::I64 => self.abi_call1(b, mctx, "str", "from_i64", sv)?,
                Ty::F64 => self.abi_call1(b, mctx, "str", "from_f64", sv)?,
                Ty::Bool => {
                    let widened = b.ins().uextend(types::I64, sv);
                    self.abi_call1(b, mctx, "str", "from_bool", widened)?
                }
                _ => return Err(ClifError::Unsupported("entry return type outside slice")),
            };
            let write = mctx.abi_func(resolve_abi("io.std", "write")?)?;
            let fd = b.ins().iconst(types::I64, 1);
            let nl = b.ins().iconst(types::I64, 1);
            self.call(b, mctx, write, None, &[fd, str_ptr, nl])?;
        }
        let zero = b.ins().iconst(types::I32, 0);
        b.ins().return_(&[zero]);
        Ok(())
    }

    /// Call an ABI fn with a single argument, returning its single result value.
    fn abi_call1(
        &mut self,
        b: &mut FunctionBuilder,
        mctx: &mut ModuleCtx,
        module: &str,
        name: &str,
        arg: Value,
    ) -> Result<Value, ClifError> {
        let id = mctx.abi_func(resolve_abi(module, name)?)?;
        let func_ref = mctx.module.declare_func_in_func(id, b.func);
        let call = b.ins().call(func_ref, &[arg]);
        b.inst_results(call)
            .first()
            .copied()
            .ok_or(ClifError::Unsupported("ABI call produced no result"))
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
    use cranelift_codegen::settings::{self, Configurable};
    use lk_aot_mir::{Block as MirBlock, BlockId, ValueId};

    fn host_isa() -> std::sync::Arc<dyn TargetIsa> {
        let mut flags = settings::builder();
        flags.set("opt_level", "speed").unwrap();
        cranelift_native::builder()
            .unwrap()
            .finish(settings::Flags::new(flags))
            .unwrap()
    }

    /// Lower `funcs` as a module and drive the full Cranelift pipeline
    /// (`compile_module` verifies every function and emits a native object).
    /// Success proves the lowering produced valid, codegen-able CLIF — the
    /// typed-builder correctness win the string-IR path lacked.
    fn compile_ok(funcs: Vec<MirFunction>) -> Result<Vec<u8>, ClifError> {
        let mir = MirModule {
            abi_version: 0,
            globals: vec![],
            mutable_globals: vec![],
            vm_functions: vec![],
            // Sentinel: these helper functions are compiled as ordinary
            // `lk_fn_N` (with params/returns); the entry/`main` shape has its own
            // test.
            entry: FuncId(u32::MAX),
            functions: funcs,
        };
        compile_module(&mir, host_isa())
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
        compile_ok(vec![func]).expect("scalar arithmetic must compile");
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
        compile_ok(vec![func]).expect("block-param control flow must compile");
    }

    // Guarded div/mod (lkrt helpers) and a cross-function call — Phase 1.
    #[test]
    fn lowers_guarded_div_and_direct_call() {
        // fn callee(a, b) -> i64 { return a / b }  (guarded lkrt div)
        let callee = MirFunction {
            id: FuncId(1),
            params: vec![(vid(0), Ty::I64), (vid(1), Ty::I64)],
            blocks: vec![MirBlock {
                id: BlockId(0),
                params: vec![],
                insts: vec![Inst::IntBin {
                    dst: vid(2),
                    op: IntBinOp::Div,
                    lhs: vid(0),
                    rhs: vid(1),
                }],
                term: Term::Ret(Some(vid(2))),
            }],
            entry: BlockId(0),
            ret: Ty::I64,
        };
        // fn caller(x) -> i64 { return callee(x, 3) }
        let caller = MirFunction {
            id: FuncId(0),
            params: vec![(vid(0), Ty::I64)],
            blocks: vec![MirBlock {
                id: BlockId(0),
                params: vec![],
                insts: vec![
                    Inst::Const {
                        dst: vid(1),
                        value: Const::I64(3),
                    },
                    Inst::CallFn {
                        dst: Some(vid(2)),
                        func: FuncId(1),
                        args: vec![vid(0), vid(1)],
                    },
                ],
                term: Term::Ret(Some(vid(2))),
            }],
            entry: BlockId(0),
            ret: Ty::I64,
        };
        compile_ok(vec![caller, callee]).expect("div + direct call must compile");
    }

    // A scalar ABI runtime call (`lkrt.abi_version() -> i64`) — Phase 2. The
    // symbol is declared as an import and resolved by linking `lkrt`.
    #[test]
    fn lowers_scalar_abi_call() {
        use lk_aot_mir::AbiRef;
        let func = MirFunction {
            id: FuncId(0),
            params: vec![],
            blocks: vec![MirBlock {
                id: BlockId(0),
                params: vec![],
                insts: vec![Inst::Call {
                    dst: Some(vid(0)),
                    callee: AbiRef::new("lkrt", "abi_version"),
                    args: vec![],
                }],
                term: Term::Ret(Some(vid(0))),
            }],
            entry: BlockId(0),
            ret: Ty::I64,
        };
        compile_ok(vec![func]).expect("scalar ABI call must compile");
    }

    // Const::Str (data symbol) + mutable-global load/store — Phase 2b.
    #[test]
    fn lowers_string_const_and_globals() {
        use lk_aot_mir::GlobalId;
        // fn() -> i64 { gvar0 = 42; return gvar0 }
        let set_get = MirFunction {
            id: FuncId(0),
            params: vec![],
            blocks: vec![MirBlock {
                id: BlockId(0),
                params: vec![],
                insts: vec![
                    Inst::Const {
                        dst: vid(0),
                        value: Const::I64(42),
                    },
                    Inst::GlobalSet { gvar: 0, src: vid(0) },
                    Inst::GlobalGet { dst: vid(1), gvar: 0 },
                ],
                term: Term::Ret(Some(vid(1))),
            }],
            entry: BlockId(0),
            ret: Ty::I64,
        };
        // fn() -> str { return "hi" }
        let get_str = MirFunction {
            id: FuncId(1),
            params: vec![],
            blocks: vec![MirBlock {
                id: BlockId(0),
                params: vec![],
                insts: vec![Inst::Const {
                    dst: vid(0),
                    value: Const::Str(GlobalId(0)),
                }],
                term: Term::Ret(Some(vid(0))),
            }],
            entry: BlockId(0),
            ret: Ty::Str,
        };
        let mir = MirModule {
            abi_version: 0,
            globals: vec!["hi".to_string()],
            mutable_globals: vec![("g".to_string(), Ty::I64)],
            vm_functions: vec![],
            entry: FuncId(0),
            functions: vec![set_get, get_str],
        };
        compile_module(&mir, host_isa()).expect("string const + globals must compile");
    }

    // PrintStr via the `io.std.write` ABI fn — Phase 2b.
    #[test]
    fn lowers_print_str() {
        use lk_aot_mir::GlobalId;
        let func = MirFunction {
            id: FuncId(0),
            params: vec![],
            blocks: vec![MirBlock {
                id: BlockId(0),
                params: vec![],
                insts: vec![
                    Inst::Const {
                        dst: vid(0),
                        value: Const::Str(GlobalId(0)),
                    },
                    Inst::PrintStr {
                        value: vid(0),
                        newline: true,
                    },
                ],
                term: Term::Ret(None),
            }],
            entry: BlockId(0),
            ret: Ty::Nil,
        };
        let mir = MirModule {
            abi_version: 0,
            globals: vec!["hello".to_string()],
            mutable_globals: vec![],
            vm_functions: vec![],
            entry: FuncId(0),
            functions: vec![func],
        };
        compile_module(&mir, host_isa()).expect("print str must compile");
    }

    // The entry function compiles to C `main`: `abi_check` prologue + top-level
    // result print + `ret 0`.
    #[test]
    fn lowers_entry_main() {
        use lk_aot_mir::GlobalId;
        // `println("hi"); return;` — entry with a print + nil return.
        let prog = MirFunction {
            id: FuncId(0),
            params: vec![],
            blocks: vec![MirBlock {
                id: BlockId(0),
                params: vec![],
                insts: vec![
                    Inst::Const {
                        dst: vid(0),
                        value: Const::Str(GlobalId(0)),
                    },
                    Inst::PrintStr {
                        value: vid(0),
                        newline: true,
                    },
                ],
                term: Term::Ret(None),
            }],
            entry: BlockId(0),
            ret: Ty::Nil,
        };
        let mir = MirModule {
            abi_version: 1,
            globals: vec!["hi".to_string()],
            mutable_globals: vec![],
            vm_functions: vec![],
            entry: FuncId(0),
            functions: vec![prog],
        };
        compile_module(&mir, host_isa()).expect("entry main must compile");
    }

    // An entry returning an int prints it (top-level auto-print) then `ret 0`.
    #[test]
    fn lowers_entry_scalar_return() {
        let ret42 = MirFunction {
            id: FuncId(0),
            params: vec![],
            blocks: vec![MirBlock {
                id: BlockId(0),
                params: vec![],
                insts: vec![Inst::Const {
                    dst: vid(0),
                    value: Const::I64(42),
                }],
                term: Term::Ret(Some(vid(0))),
            }],
            entry: BlockId(0),
            ret: Ty::I64,
        };
        let mir = MirModule {
            abi_version: 1,
            globals: vec![],
            mutable_globals: vec![],
            vm_functions: vec![],
            entry: FuncId(0),
            functions: vec![ret42],
        };
        compile_module(&mir, host_isa()).expect("entry scalar return must compile");
    }

    // A shape still outside the slice (a `Dyn` param) rejects at the capability
    // boundary — specifically `Unsupported`, not an unrelated module/emit error.
    #[test]
    fn rejects_unsupported_shape() {
        let func = MirFunction {
            id: FuncId(0),
            params: vec![(vid(0), Ty::Dyn)],
            blocks: vec![MirBlock {
                id: BlockId(0),
                params: vec![],
                insts: vec![],
                term: Term::Ret(Some(vid(0))),
            }],
            entry: BlockId(0),
            ret: Ty::Dyn,
        };
        assert!(matches!(compile_ok(vec![func]), Err(ClifError::Unsupported(_))));
    }
}
