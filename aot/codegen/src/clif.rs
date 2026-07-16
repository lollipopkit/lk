//! `MirModule` → Cranelift IR lowering (the typed-builder backend).
//!
//! The string-IR renderer ([`crate::render_module`]) emits LLVM text; this path
//! instead builds Cranelift IR through the typed `FunctionBuilder`, so a
//! type-mismatched instruction fails to *compile* rather than producing invalid
//! IR caught only downstream. Being SSA-with-block-params, the MIR maps almost
//! 1:1 onto Cranelift blocks/params/branches.
//!
//! Current slice (the strangler front): the scalar subset (int/float const and
//! arithmetic, comparisons, widen/narrow, select, boolean ops), block-param
//! control flow (`Br`/`CondBr`/`Ret`/`Abort`), guarded div/mod, ABI runtime
//! calls, string constants and mutable globals, direct calls, the entry/`main`
//! shape (with top-level auto-print + arena cleanup), and the `{i64,i64}`
//! carriers — `Dyn` and `Maybe*` flow as register pairs ([`Slot`]), so maps,
//! dynamic container reads (`*_get_pair`), and unwraps lower. Still outside the
//! slice — anything returning [`ClifError::Unsupported`]: the Tier 1 VM bridge
//! (`CallVm`), protected calls (`TryCall`/setjmp), trait dispatch, and the
//! `{double,i64}` float-carrier `get_pair`s (whose by-value return diverges from
//! the two-scalar ABI on AArch64).

use std::collections::HashMap;

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    AbiParam, BlockArg, Function, InstBuilder, MemFlagsData, Signature, StackSlotData, StackSlotKind, Value, types,
};
use cranelift_codegen::isa::{CallConv, TargetIsa};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataDescription, DataId, FuncId as ClifFuncId, Linkage, Module, ModuleError};
use cranelift_object::{ObjectBuilder, ObjectModule};
use lk_aot_mir::{CmpOp, Const, FloatBinOp, FuncId, Inst, IntBinOp, MirFunction, MirModule, Term, Ty, ValueId};

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
    /// Intra-module function return types, for binding a [`Inst::CallFn`] result
    /// as one value or a `{i64,i64}` pair.
    fn_rets: &'a HashMap<FuncId, Ty>,
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
    /// Cranelift id. A `DynVal` param/return expands to a register pair (see
    /// [`abi_ty_clif_parts`]).
    fn abi_func(&mut self, abi: &lk_aot_abi::AbiFn) -> Result<ClifFuncId, ClifError> {
        if let Some(id) = self.abi_ids.get(abi.symbol) {
            return Ok(*id);
        }
        let cc = self.module.isa().default_call_conv();
        let mut sig = Signature::new(cc);
        for p in abi.params {
            for t in abi_ty_clif_parts(*p)? {
                sig.params.push(AbiParam::new(t));
            }
        }
        for t in abi_ty_clif_parts(abi.result)? {
            sig.returns.push(AbiParam::new(t));
        }
        let id = self.module.declare_function(abi.symbol, Linkage::Import, &sig)?;
        self.abi_ids.insert(abi.symbol, id);
        Ok(id)
    }

    /// Declare (once) an imported `lkrt_*` symbol by explicit signature. Used for
    /// the carrier `get_pair`/`unwrap` helpers, which live outside the ABI schema
    /// (their by-value `{value, present}` shapes are not scalar-ABI). Deduped in
    /// the same `abi_ids` table by symbol name.
    fn raw_func(
        &mut self,
        symbol: &'static str,
        params: &[types::Type],
        returns: &[types::Type],
    ) -> Result<ClifFuncId, ClifError> {
        if let Some(id) = self.abi_ids.get(symbol) {
            return Ok(*id);
        }
        let cc = self.module.isa().default_call_conv();
        let mut sig = Signature::new(cc);
        for t in params {
            sig.params.push(AbiParam::new(*t));
        }
        for t in returns {
            sig.returns.push(AbiParam::new(*t));
        }
        let id = self.module.declare_function(symbol, Linkage::Import, &sig)?;
        self.abi_ids.insert(symbol, id);
        Ok(id)
    }
}

/// The Cranelift component types an ABI parameter/return occupies. Scalars and
/// pointers are one register; the `DynVal` carrier (`LkDyn {i64 tag, i64
/// payload}`) is two, passed/returned as a register pair — the two-scalar
/// lowering of a by-value all-integer 16-byte struct, which matches the C ABI on
/// both x86-64 (rax:rdx) and AArch64 (x0:x1). `Nil` occupies none.
fn abi_ty_clif_parts(ty: lk_aot_abi::AbiType) -> Result<Vec<types::Type>, ClifError> {
    use lk_aot_abi::AbiType;
    Ok(match ty {
        AbiType::I64 => vec![types::I64],
        AbiType::F64 => vec![types::F64],
        AbiType::Ptr | AbiType::StrPtr => vec![types::I64],
        AbiType::Nil => vec![],
        AbiType::DynVal => vec![types::I64, types::I64],
    })
}

/// A lowered MIR value. Scalars and handles occupy one Cranelift SSA value; the
/// `{i64,i64}` carriers (`Dyn`, `Maybe*`) occupy a pair — component 0 is the
/// value/tag, component 1 is the present/payload — matching the two-register
/// carrier ABI [`abi_ty_clif_parts`] describes.
#[derive(Clone, Copy)]
enum Slot {
    One(Value),
    Two(Value, Value),
}

/// The Cranelift component types a MIR value of type `ty` occupies (see [`Slot`]).
/// `MaybeF64`'s value component is an `F64`; every other carrier component is an
/// `I64`. `Nil` occupies none.
fn ty_clif_parts(ty: Ty) -> Result<Vec<types::Type>, ClifError> {
    Ok(match ty {
        Ty::Nil => vec![],
        Ty::Dyn | Ty::MaybeI64 | Ty::MaybeBool | Ty::MaybeStr => vec![types::I64, types::I64],
        Ty::MaybeF64 => vec![types::F64, types::I64],
        other => vec![ty_to_clif(other)?],
    })
}

/// Whether a MIR type is a two-register `{i64,i64}` carrier (see [`Slot`]).
fn ty_is_pair(ty: Ty) -> bool {
    matches!(ty, Ty::Dyn | Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeBool | Ty::MaybeStr)
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
    let mut fn_rets = HashMap::new();
    for func in &mir.functions {
        let (sym, linkage, sig) = if func.id == mir.entry {
            ("main".to_string(), Linkage::Export, main_signature(cc))
        } else {
            (format!("lk_fn_{}", func.id.0), Linkage::Local, signature_of(func, cc)?)
        };
        let id = module.declare_function(&sym, linkage, &sig)?;
        fn_ids.insert(func.id, id);
        fn_rets.insert(func.id, func.ret);
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
        // A carrier global occupies both components (16 bytes); a scalar one.
        let size: usize = ty_clif_parts(*ty)?.iter().map(|t| t.bytes() as usize).sum();
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
                fn_rets: &fn_rets,
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

/// The Cranelift call signature of a MIR function under `call_conv`. A `Dyn` or
/// `Maybe*` param/return expands to a register pair (see [`ty_clif_parts`]).
pub fn signature_of(func: &MirFunction, call_conv: CallConv) -> Result<Signature, ClifError> {
    let mut sig = Signature::new(call_conv);
    for (_, ty) in &func.params {
        for t in ty_clif_parts(*ty)? {
            sig.params.push(AbiParam::new(t));
        }
    }
    for t in ty_clif_parts(func.ret)? {
        sig.returns.push(AbiParam::new(t));
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
    // Bind the function-signature params to the entry block's params, consuming
    // one or two Cranelift params per MIR value (carriers are a pair).
    let entry_params: Vec<Value> = builder.block_params(entry).to_vec();
    let mut cursor = 0;
    for (vid, ty) in &func.params {
        lower.bind_params(*vid, *ty, &entry_params, &mut cursor)?;
    }
    // Non-entry blocks carry the SSA phi params as block params (each carrier phi
    // is two Cranelift block params).
    for block in &func.blocks {
        if block.id == func.entry {
            continue;
        }
        let cb = lower.blocks[&block.id];
        for (vid, ty) in &block.params {
            let mut parts = Vec::new();
            for t in ty_clif_parts(*ty)? {
                parts.push(builder.append_block_param(cb, t));
            }
            lower.bind_slot(*vid, &parts)?;
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
    values: HashMap<ValueId, Slot>,
    blocks: HashMap<lk_aot_mir::BlockId, cranelift_codegen::ir::Block>,
    /// This function is the program `main` (`() -> i32`): returns print the
    /// top-level result and `ret 0`.
    is_entry: bool,
    /// The MIR return type — selects the entry's result-printing conversion.
    ret_ty: Ty,
}

impl Lower {
    fn set1(&mut self, id: ValueId, v: Value) {
        self.values.insert(id, Slot::One(v));
    }

    fn set2(&mut self, id: ValueId, a: Value, b: Value) {
        self.values.insert(id, Slot::Two(a, b));
    }

    fn slot(&self, id: ValueId) -> Result<Slot, ClifError> {
        self.values
            .get(&id)
            .copied()
            .ok_or(ClifError::Unsupported("value used before def"))
    }

    /// A scalar value — rejects a `{i64,i64}` carrier used where one register was
    /// expected (a lowering-consistency guard, not a user error).
    fn v(&self, id: ValueId) -> Result<Value, ClifError> {
        match self.slot(id)? {
            Slot::One(v) => Ok(v),
            Slot::Two(..) => Err(ClifError::Unsupported("carrier value used as scalar")),
        }
    }

    /// A carrier's `(component0, component1)` — rejects a scalar used as a pair.
    fn two(&self, id: ValueId) -> Result<(Value, Value), ClifError> {
        match self.slot(id)? {
            Slot::Two(a, b) => Ok((a, b)),
            Slot::One(_) => Err(ClifError::Unsupported("scalar value used as carrier")),
        }
    }

    /// Flatten a value into its Cranelift components (one for a scalar, two for a
    /// carrier) — the call-argument / return / block-argument representation.
    fn parts(&self, id: ValueId) -> Result<Vec<Value>, ClifError> {
        Ok(match self.slot(id)? {
            Slot::One(v) => vec![v],
            Slot::Two(a, b) => vec![a, b],
        })
    }

    /// Bind `parts[*cursor..]` to `vid` per `ty`'s width, advancing the cursor.
    fn bind_params(&mut self, vid: ValueId, ty: Ty, parts: &[Value], cursor: &mut usize) -> Result<(), ClifError> {
        let n = ty_clif_parts(ty)?.len();
        let slice = parts
            .get(*cursor..*cursor + n)
            .ok_or(ClifError::Unsupported("function param arity mismatch"))?;
        self.bind_slot(vid, slice)?;
        *cursor += n;
        Ok(())
    }

    /// Bind an already-sliced component list (one or two values) to `vid`.
    fn bind_slot(&mut self, vid: ValueId, parts: &[Value]) -> Result<(), ClifError> {
        match parts {
            [v] => self.set1(vid, *v),
            [a, b] => self.set2(vid, *a, *b),
            _ => return Err(ClifError::Unsupported("value arity outside {1,2}")),
        }
        Ok(())
    }

    /// Flatten every operand into its Cranelift components (call arguments).
    fn args_v(&self, ids: &[ValueId]) -> Result<Vec<Value>, ClifError> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.extend(self.parts(*id)?);
        }
        Ok(out)
    }

    /// Block-call arguments (branch-passed values become the target block's
    /// params) — a carrier phi passes two args.
    fn block_args(&self, ids: &[ValueId]) -> Result<Vec<BlockArg>, ClifError> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            for v in self.parts(*id)? {
                out.push(BlockArg::Value(v));
            }
        }
        Ok(out)
    }

    /// Import `callee` and emit a call with pre-flattened `args`, binding a single
    /// scalar result to `dst`.
    fn call(
        &mut self,
        b: &mut FunctionBuilder,
        mctx: &mut ModuleCtx,
        callee: ClifFuncId,
        dst: Option<ValueId>,
        args: &[Value],
    ) -> Result<(), ClifError> {
        self.call_raw(b, mctx, callee, dst, false, args)
    }

    /// Import `callee` and emit a call with pre-flattened `args`. When `dst` is
    /// set, bind the result as a `{i64,i64}` pair (`dst_pair`) or a single value.
    fn call_raw(
        &mut self,
        b: &mut FunctionBuilder,
        mctx: &mut ModuleCtx,
        callee: ClifFuncId,
        dst: Option<ValueId>,
        dst_pair: bool,
        args: &[Value],
    ) -> Result<(), ClifError> {
        let func_ref = mctx.module.declare_func_in_func(callee, b.func);
        let call = b.ins().call(func_ref, args);
        if let Some(dst) = dst {
            let results = b.inst_results(call);
            if dst_pair {
                let (a, c) = (
                    *results
                        .first()
                        .ok_or(ClifError::Unsupported("carrier call missing lo"))?,
                    *results
                        .get(1)
                        .ok_or(ClifError::Unsupported("carrier call missing hi"))?,
                );
                self.set2(dst, a, c);
            } else {
                let v = *results
                    .first()
                    .ok_or(ClifError::Unsupported("call has no result for dst"))?;
                self.set1(dst, v);
            }
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
                    // The address of a lowered user function (`ptr @lk_fn_N`),
                    // passed to runtime HOF helpers that invoke compiled callbacks.
                    Const::FnAddr(f) => {
                        let callee = *mctx
                            .fn_ids
                            .get(f)
                            .ok_or(ClifError::Unsupported("fnaddr of undeclared function"))?;
                        let func_ref = mctx.module.declare_func_in_func(callee, b.func);
                        b.ins().func_addr(types::I64, func_ref)
                    }
                    // A scalar `nil` is the integer 0 (matching the string-IR
                    // `add i64 0, 0`); nil flowing into a `Dyn` context is wrapped
                    // by an explicit `dyn.from_nil` call in the MIR, not here.
                    Const::Nil => b.ins().iconst(types::I64, 0),
                };
                self.set1(*dst, v);
            }
            Inst::GlobalGet { dst, gvar } => {
                let (data_id, ty) = *mctx
                    .gvar_data
                    .get(gvar)
                    .ok_or(ClifError::Unsupported("undeclared global"))?;
                let gv = mctx.module.declare_data_in_func(data_id, b.func);
                let addr = b.ins().global_value(types::I64, gv);
                // A carrier global holds two components at offsets 0 and 8
                // (component 0 is always 8 bytes: `i64` or `f64`).
                let parts = ty_clif_parts(ty)?;
                match parts.as_slice() {
                    [t] => {
                        let v = b.ins().load(*t, MemFlagsData::trusted(), addr, 0);
                        self.set1(*dst, v);
                    }
                    [t0, t1] => {
                        let v0 = b.ins().load(*t0, MemFlagsData::trusted(), addr, 0);
                        let v1 = b.ins().load(*t1, MemFlagsData::trusted(), addr, 8);
                        self.set2(*dst, v0, v1);
                    }
                    _ => return Err(ClifError::Unsupported("global type arity outside {1,2}")),
                }
            }
            Inst::GlobalSet { gvar, src } => {
                let (data_id, ty) = *mctx
                    .gvar_data
                    .get(gvar)
                    .ok_or(ClifError::Unsupported("undeclared global"))?;
                let gv = mctx.module.declare_data_in_func(data_id, b.func);
                let addr = b.ins().global_value(types::I64, gv);
                if ty_is_pair(ty) {
                    let (a, c) = self.two(*src)?;
                    b.ins().store(MemFlagsData::trusted(), a, addr, 0);
                    b.ins().store(MemFlagsData::trusted(), c, addr, 8);
                } else {
                    let s = self.v(*src)?;
                    b.ins().store(MemFlagsData::trusted(), s, addr, 0);
                }
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
                self.set1(*dst, v);
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
                self.set1(*dst, v);
            }
            Inst::CallFn { dst, func, args } => {
                let a = self.args_v(args)?;
                let callee = *mctx
                    .fn_ids
                    .get(func)
                    .ok_or(ClifError::Unsupported("call to undeclared function"))?;
                let dst_pair = mctx.fn_rets.get(func).is_some_and(|t| ty_is_pair(*t));
                return self.call_raw(b, mctx, callee, *dst, dst_pair, &a);
            }
            Inst::Call { dst, callee, args } => {
                let abi = callee.resolve().ok_or(ClifError::Unsupported("unknown ABI function"))?;
                let a = self.args_v(args)?;
                let dst_pair = matches!(abi.result, lk_aot_abi::AbiType::DynVal);
                let clif_id = mctx.abi_func(abi)?;
                return self.call_raw(b, mctx, clif_id, *dst, dst_pair, &a);
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
                self.set1(*dst, v);
            }
            Inst::IntToFloat { dst, src } => {
                let s = self.v(*src)?;
                let v = b.ins().fcvt_from_sint(types::F64, s);
                self.set1(*dst, v);
            }
            Inst::ZextBool { dst, src } => {
                let s = self.v(*src)?;
                let v = b.ins().uextend(types::I64, s);
                self.set1(*dst, v);
            }
            Inst::Not { dst, src } => {
                let s = self.v(*src)?;
                let v = b.ins().bxor_imm(s, 1);
                self.set1(*dst, v);
            }
            Inst::BoolAnd { dst, lhs, rhs } => {
                let (l, r) = (self.v(*lhs)?, self.v(*rhs)?);
                let v = b.ins().band(l, r);
                self.set1(*dst, v);
            }
            Inst::Select {
                dst,
                cond,
                then_v,
                else_v,
                ty,
            } => {
                let c = self.v(*cond)?;
                // A carrier `select` picks each component independently (Cranelift
                // has no aggregate select); scalars are the common one-value case.
                if ty_is_pair(*ty) {
                    let ((t0, t1), (e0, e1)) = (self.two(*then_v)?, self.two(*else_v)?);
                    let v0 = b.ins().select(c, t0, e0);
                    let v1 = b.ins().select(c, t1, e1);
                    self.set2(*dst, v0, v1);
                } else {
                    let (t, e) = (self.v(*then_v)?, self.v(*else_v)?);
                    let v = b.ins().select(c, t, e);
                    self.set1(*dst, v);
                }
            }
            // A `Maybe`'s presence bit (component 1) as a `Bool` (`present != 0`).
            Inst::MaybePresent { dst, src, .. } => {
                let (_, present) = self.two(*src)?;
                let v = b.ins().icmp_imm(IntCC::NotEqual, present, 0);
                self.set1(*dst, v);
            }
            // A `Maybe`'s value (component 0) without asserting presence.
            Inst::MaybeValue { dst, src, .. } => {
                let (value, _) = self.two(*src)?;
                self.set1(*dst, value);
            }
            // Wrap a plain scalar into a present carrier `{value, 1}`.
            Inst::MaybeWrap { dst, src, .. } => {
                let value = self.v(*src)?;
                let present = b.ins().iconst(types::I64, 1);
                self.set2(*dst, value, present);
            }
            // Dynamic container reads returning a `{value, present}` carrier — the
            // `lkrt_*_get_pair` symbols (declared via the ABI schema's dedicated
            // entries) return a register pair, bound directly as a [`Slot::Two`].
            Inst::ListGetMaybe { dst, handle, index } => {
                return self.pair_call(b, mctx, "lkrt_lklist_i64_get_pair", *dst, &[*handle, *index]);
            }
            Inst::ListGetMaybeStr { dst, handle, index } => {
                return self.pair_call(b, mctx, "lkrt_lklist_str_get_pair", *dst, &[*handle, *index]);
            }
            Inst::MapGetMaybe { dst, handle, key } => {
                return self.pair_call(b, mctx, "lkrt_lkmap_str_i64_get_pair", *dst, &[*handle, *key]);
            }
            Inst::MapGetMaybeI64Key { dst, handle, key } => {
                return self.pair_call(b, mctx, "lkrt_lkmap_i64_i64_get_pair", *dst, &[*handle, *key]);
            }
            // Narrow a carrier to a scalar, aborting if absent (the lkrt unwrap
            // helper takes the two components and halts on `present == 0`).
            Inst::UnwrapMaybeI64 { dst, src } => {
                return self.unwrap_call(b, mctx, "lkrt_maybe_i64_unwrap", *dst, *src);
            }
            Inst::UnwrapMaybeStr { dst, src } => {
                return self.unwrap_call(b, mctx, "lkrt_maybe_str_unwrap", *dst, *src);
            }
            Inst::TraitDispatch { dst, self_arg, arms } => {
                return self.trait_dispatch(b, mctx, *dst, *self_arg, arms);
            }
            Inst::TryCall { dst, func, args } => {
                return self.try_call(b, mctx, *dst, *func, args);
            }
            _ => return Err(ClifError::Unsupported("instruction outside current slice")),
        }
        Ok(())
    }

    /// Native protected call (`try$call`, plan G): drive the lkrt `setjmp`
    /// trampoline (`lkrt_rt_try_call`) — Cranelift cannot emit `setjmp` itself —
    /// and join its outcome into the `[ok, value]` dyn-list the desugared
    /// destructuring consumes. Only integer/pointer try-body params are
    /// supported: each argument is marshaled as one `i64` word into a stack
    /// buffer (float/carrier args, or arity above the trampoline cap, reject).
    fn try_call(
        &mut self,
        b: &mut FunctionBuilder,
        mctx: &mut ModuleCtx,
        dst: ValueId,
        func: FuncId,
        args: &[ValueId],
    ) -> Result<(), ClifError> {
        // Keep in step with the trampoline's arity switch (`try_trampoline.c`).
        const MAX_ARGS: usize = 8;
        if args.len() > MAX_ARGS {
            return Err(ClifError::Unsupported("try-call arity over trampoline cap"));
        }
        // Marshal each argument to an `i64` word in a stack buffer.
        let slot_bytes = (args.len().max(1) * 8) as u32;
        let args_slot = b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, slot_bytes, 3));
        for (i, arg) in args.iter().enumerate() {
            let word = match self.slot(*arg)? {
                Slot::Two(..) => return Err(ClifError::Unsupported("try-call carrier argument")),
                Slot::One(v) => {
                    let t = b.func.dfg.value_type(v);
                    if t == types::I64 {
                        v
                    } else if t == types::I8 {
                        b.ins().uextend(types::I64, v)
                    } else {
                        return Err(ClifError::Unsupported("try-call non-integer argument"));
                    }
                }
            };
            b.ins().stack_store(word, args_slot, (i * 8) as i32);
        }
        let argv = b.ins().stack_addr(types::I64, args_slot, 0);
        let ok_slot = b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
        let out_ok = b.ins().stack_addr(types::I64, ok_slot, 0);
        // The try-body's address (`ptr @lk_fn_N`) and argument count.
        let callee = *mctx
            .fn_ids
            .get(&func)
            .ok_or(ClifError::Unsupported("try-call to undeclared function"))?;
        let body_ref = mctx.module.declare_func_in_func(callee, b.func);
        let body_addr = b.ins().func_addr(types::I64, body_ref);
        let argc = b.ins().iconst(types::I64, args.len() as i64);
        // Call the trampoline: returns the body result / caught error as a `Dyn`
        // pair, and writes the ok flag through `out_ok`.
        let tramp = mctx.raw_func("lkrt_rt_try_call", &[types::I64; 4], &[types::I64, types::I64])?;
        let tramp_ref = mctx.module.declare_func_in_func(tramp, b.func);
        let call = b.ins().call(tramp_ref, &[body_addr, argc, argv, out_ok]);
        let (val_t, val_p) = {
            let r = b.inst_results(call);
            (
                *r.first().ok_or(ClifError::Unsupported("try-call result missing lo"))?,
                *r.get(1).ok_or(ClifError::Unsupported("try-call result missing hi"))?,
            )
        };
        let ok = b.ins().stack_load(types::I64, ok_slot, 0);
        // Join into the `[ok, value]` dyn list the desugaring destructures.
        let list = self.abi_call(b, mctx, "list_h", "dyn_new", &[])?;
        let (ok_t, ok_p) = self.abi_call_pair(b, mctx, "dyn", "from_bool", &[ok])?;
        let push = mctx.abi_func(resolve_abi("list_h", "dyn_push")?)?;
        self.call(b, mctx, push, None, &[list, ok_t, ok_p])?;
        self.call(b, mctx, push, None, &[list, val_t, val_p])?;
        self.set1(dst, list);
        Ok(())
    }

    /// Call an ABI fn with pre-flattened scalar `args`, returning its `{i64,i64}`
    /// carrier result as a `(component0, component1)` pair.
    fn abi_call_pair(
        &mut self,
        b: &mut FunctionBuilder,
        mctx: &mut ModuleCtx,
        module: &str,
        name: &str,
        args: &[Value],
    ) -> Result<(Value, Value), ClifError> {
        let id = mctx.abi_func(resolve_abi(module, name)?)?;
        let func_ref = mctx.module.declare_func_in_func(id, b.func);
        let call = b.ins().call(func_ref, args);
        let r = b.inst_results(call);
        Ok((
            *r.first().ok_or(ClifError::Unsupported("carrier ABI call missing lo"))?,
            *r.get(1).ok_or(ClifError::Unsupported("carrier ABI call missing hi"))?,
        ))
    }

    /// Runtime trait-method dispatch (plan J1): read the boxed receiver's arena
    /// type mark (`dyn.obj_type_id`) and walk an `icmp` chain, calling the impl
    /// whose registered type id matches. Every arm takes the `Dyn` receiver and
    /// returns `Dyn`, so results merge at a join block via a two-param phi. No
    /// matching mark calls `dyn.method_missing` (which raises/aborts).
    fn trait_dispatch(
        &mut self,
        b: &mut FunctionBuilder,
        mctx: &mut ModuleCtx,
        dst: ValueId,
        self_arg: ValueId,
        arms: &[(i64, FuncId)],
    ) -> Result<(), ClifError> {
        let (s0, s1) = self.two(self_arg)?;
        let type_id = self.abi_call(b, mctx, "dyn", "obj_type_id", &[s0, s1])?;
        // The join block carries the dispatched `Dyn` result as two block params.
        let join = b.create_block();
        let j0 = b.append_block_param(join, types::I64);
        let j1 = b.append_block_param(join, types::I64);
        for (tid, func) in arms {
            let arm = b.create_block();
            let next = b.create_block();
            let eq = b.ins().icmp_imm(IntCC::Equal, type_id, *tid);
            b.ins().brif(eq, arm, &[], next, &[]);
            b.switch_to_block(arm);
            let callee = *mctx
                .fn_ids
                .get(func)
                .ok_or(ClifError::Unsupported("trait arm to undeclared function"))?;
            let func_ref = mctx.module.declare_func_in_func(callee, b.func);
            let call = b.ins().call(func_ref, &[s0, s1]);
            let (r0, r1) = {
                let r = b.inst_results(call);
                (
                    *r.first().ok_or(ClifError::Unsupported("trait arm result missing lo"))?,
                    *r.get(1).ok_or(ClifError::Unsupported("trait arm result missing hi"))?,
                )
            };
            b.ins().jump(join, &[BlockArg::Value(r0), BlockArg::Value(r1)]);
            b.switch_to_block(next);
        }
        // Fall-through (no arm matched): raise via `method_missing`, then trap to
        // terminate the block (the raise aborts, so the trap is unreachable).
        let missing = mctx.abi_func(resolve_abi("dyn", "method_missing")?)?;
        self.call(b, mctx, missing, None, &[])?;
        b.ins()
            .trap(cranelift_codegen::ir::TrapCode::user(1).expect("nonzero trap code"));
        // Continue the MIR block from the join, with the result bound.
        b.switch_to_block(join);
        self.set2(dst, j0, j1);
        Ok(())
    }

    /// Emit a call to a `{value, present}`-returning `lkrt_*_get_pair` symbol
    /// (declared in codegen, not the ABI schema — its by-value carrier return is
    /// outside the scalar ABI vocabulary), binding `dst` as a [`Slot::Two`].
    /// `arg_ids` are scalars/handles, one `I64` component each.
    fn pair_call(
        &mut self,
        b: &mut FunctionBuilder,
        mctx: &mut ModuleCtx,
        symbol: &'static str,
        dst: ValueId,
        arg_ids: &[ValueId],
    ) -> Result<(), ClifError> {
        let args = self.args_v(arg_ids)?;
        let params = vec![types::I64; args.len()];
        let clif_id = mctx.raw_func(symbol, &params, &[types::I64, types::I64])?;
        self.call_raw(b, mctx, clif_id, Some(dst), true, &args)
    }

    /// Emit an `lkrt_maybe_*_unwrap(value, present) -> scalar` call: flatten the
    /// carrier `src` into its two components and bind the single result to `dst`.
    fn unwrap_call(
        &mut self,
        b: &mut FunctionBuilder,
        mctx: &mut ModuleCtx,
        symbol: &'static str,
        dst: ValueId,
        src: ValueId,
    ) -> Result<(), ClifError> {
        let (value, present) = self.two(src)?;
        let clif_id = mctx.raw_func(symbol, &[types::I64, types::I64], &[types::I64])?;
        self.call_raw(b, mctx, clif_id, Some(dst), false, &[value, present])
    }

    fn term(&mut self, b: &mut FunctionBuilder, mctx: &mut ModuleCtx, term: &Term) -> Result<(), ClifError> {
        match term {
            Term::Ret(value) if self.is_entry => return self.entry_return(b, mctx, *value),
            Term::Ret(None) => {
                b.ins().return_(&[]);
            }
            Term::Ret(Some(v)) => {
                // A carrier return yields two values (the signature has two
                // returns); a scalar yields one.
                let vals = self.parts(*v)?;
                b.ins().return_(&vals);
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
    /// string-IR auto-print, always via `io.std.write` with a newline), reclaim
    /// the default arena (`lkrt.cleanup`), then `ret 0`. A `nil` return — and the
    /// absent branch of a `Maybe`/nil-tagged `Dyn` — prints nothing (the VM's
    /// top-level auto-print of `nil` is silent, unlike `print(nil)`).
    fn entry_return(
        &mut self,
        b: &mut FunctionBuilder,
        mctx: &mut ModuleCtx,
        value: Option<ValueId>,
    ) -> Result<(), ClifError> {
        // A shared tail: reclaim the arena and return 0. Present/scalar prints
        // fall through to it; absent carrier branches jump straight to it.
        let exit = b.create_block();
        match value {
            None => {
                b.ins().jump(exit, &[]);
            }
            Some(v) => match self.ret_ty {
                Ty::Str => {
                    let s = self.v(v)?;
                    self.entry_write(b, mctx, s)?;
                    b.ins().jump(exit, &[]);
                }
                Ty::I64 => {
                    let s = self.v(v)?;
                    let sp = self.abi_call(b, mctx, "str", "from_i64", &[s])?;
                    self.entry_write(b, mctx, sp)?;
                    b.ins().jump(exit, &[]);
                }
                Ty::F64 => {
                    let s = self.v(v)?;
                    let sp = self.abi_call(b, mctx, "str", "from_f64", &[s])?;
                    self.entry_write(b, mctx, sp)?;
                    b.ins().jump(exit, &[]);
                }
                Ty::Bool => {
                    let s = self.v(v)?;
                    let widened = b.ins().uextend(types::I64, s);
                    let sp = self.abi_call(b, mctx, "str", "from_bool", &[widened])?;
                    self.entry_write(b, mctx, sp)?;
                    b.ins().jump(exit, &[]);
                }
                // A boxed `Dyn`: print its display unless nil-tagged (tag == 0).
                Ty::Dyn => {
                    let (tag, payload) = self.two(v)?;
                    let present = b.ins().icmp_imm(IntCC::NotEqual, tag, 0);
                    let some = b.create_block();
                    b.ins().brif(present, some, &[], exit, &[]);
                    b.switch_to_block(some);
                    let sp = self.abi_call(b, mctx, "dyn", "display", &[tag, payload])?;
                    self.entry_write(b, mctx, sp)?;
                    b.ins().jump(exit, &[]);
                }
                // A `Maybe`: print the value when present, nothing when absent.
                Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => {
                    let (val, present_bit) = self.two(v)?;
                    let present = b.ins().icmp_imm(IntCC::NotEqual, present_bit, 0);
                    let some = b.create_block();
                    b.ins().brif(present, some, &[], exit, &[]);
                    b.switch_to_block(some);
                    let sp = match self.ret_ty {
                        Ty::MaybeStr => val,
                        Ty::MaybeI64 => self.abi_call(b, mctx, "str", "from_i64", &[val])?,
                        Ty::MaybeF64 => self.abi_call(b, mctx, "str", "from_f64", &[val])?,
                        Ty::MaybeBool => self.abi_call(b, mctx, "str", "from_bool", &[val])?,
                        _ => unreachable!("guarded by the outer match"),
                    };
                    self.entry_write(b, mctx, sp)?;
                    b.ins().jump(exit, &[]);
                }
                _ => return Err(ClifError::Unsupported("entry return type outside slice")),
            },
        }
        b.switch_to_block(exit);
        let cleanup = mctx.abi_func(resolve_abi("lkrt", "cleanup")?)?;
        self.call(b, mctx, cleanup, None, &[])?;
        let zero = b.ins().iconst(types::I32, 0);
        b.ins().return_(&[zero]);
        Ok(())
    }

    /// Write a finished `Str` value to stdout with a trailing newline, via the
    /// `io.std.write(fd, str, newline)` ABI fn (the entry auto-print mechanism).
    fn entry_write(&mut self, b: &mut FunctionBuilder, mctx: &mut ModuleCtx, str_ptr: Value) -> Result<(), ClifError> {
        let write = mctx.abi_func(resolve_abi("io.std", "write")?)?;
        let fd = b.ins().iconst(types::I64, 1);
        let nl = b.ins().iconst(types::I64, 1);
        self.call(b, mctx, write, None, &[fd, str_ptr, nl])
    }

    /// Call an ABI fn with pre-flattened scalar `args`, returning its single
    /// result value.
    fn abi_call(
        &mut self,
        b: &mut FunctionBuilder,
        mctx: &mut ModuleCtx,
        module: &str,
        name: &str,
        args: &[Value],
    ) -> Result<Value, ClifError> {
        let id = mctx.abi_func(resolve_abi(module, name)?)?;
        let func_ref = mctx.module.declare_func_in_func(id, b.func);
        let call = b.ins().call(func_ref, args);
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

    // A `Dyn` param/return now lowers (as a register pair), so the capability
    // boundary is checked with an instruction still outside the slice — the Tier
    // 1 hybrid bridge call (`CallVm`). It must reject with `Unsupported`, not an
    // unrelated module/emit error.
    #[test]
    fn rejects_unsupported_shape() {
        let func = MirFunction {
            id: FuncId(0),
            params: vec![],
            blocks: vec![MirBlock {
                id: BlockId(0),
                params: vec![],
                insts: vec![Inst::CallVm {
                    dst: Some(vid(0)),
                    func: FuncId(1),
                    args: vec![],
                }],
                term: Term::Ret(Some(vid(0))),
            }],
            entry: BlockId(0),
            ret: Ty::Dyn,
        };
        assert!(matches!(compile_ok(vec![func]), Err(ClifError::Unsupported(_))));
    }

    // A `Dyn`-typed function (pair param → pair return) lowers cleanly: the
    // carrier ABI expands each `Dyn` to two `I64` registers.
    #[test]
    fn lowers_dyn_pair_identity() {
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
        compile_ok(vec![func]).expect("Dyn pair identity must compile");
    }
}
