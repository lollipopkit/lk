//! `lk-aot-codegen` — the *total* `MirModule -> LLVM text` renderer.
//!
//! This is the only crate in the AOT family that knows LLVM syntax. It consumes a
//! validated [`lk_aot_mir::MirModule`] and emits textual LLVM IR (the existing
//! pipeline then compiles the `.ll` with `clang`). Because the MIR is already
//! typed and SSA-formed, rendering is a straightforward, panic-free walk — there
//! is no "can we lower this?" decision left to make here.
//!
//! Scope today: the scalar subset (I64/F64/Bool arithmetic, comparisons, ABI
//! calls, block-argument control flow, scalar returns). Container/string shapes
//! arrive as [`lk_aot_mir::Inst::Call`]s, so no new codegen arms are needed for
//! them beyond declaring their ABI symbols.

use std::fmt::Write as _;

use lk_aot_abi::{ABI_FUNCTIONS, AbiType};
use lk_aot_mir::{
    AbiRef, Block, BlockId, CmpOp, Const, FloatBinOp, FuncId, Inst, IntBinOp, MirFunction, MirModule, Term, Ty, ValueId,
};

/// Renders a validated module to LLVM IR text.
pub fn render_module(module: &MirModule) -> String {
    let mut out = String::new();
    out.push_str("; ModuleID = 'lk_aot'\n\n");
    render_prelude(&mut out);
    render_hybrid_prelude(&mut out, module);
    for func in &module.functions {
        let is_entry = func.id == module.entry;
        render_function(&mut out, module, func, is_entry);
    }
    render_globals(&mut out, module);
    out
}

/// Tier 1 hybrid support (`docs/llvm/tier1-hybrid.md`), emitted only when the
/// module has VM-executed functions: the tagged-argument struct (layout matches
/// lk-api's `#[repr(C)] LkHybridArg`), the one-way bridge declaration, and a
/// single `internal global` marshaling buffer sized to the widest call site.
/// A global (not per-call `alloca`) keeps bridge calls inside loops from
/// growing the stack; it is safe because native binaries are single-threaded
/// and the bridge never re-enters native code (no nested buffer use).
fn render_hybrid_prelude(out: &mut String, module: &MirModule) {
    if module.vm_functions.is_empty() {
        return;
    }
    out.push_str("%LkHybridArg = type { i8, i64 }\n");
    out.push_str("declare void @lk_hybrid_call_v(i32, ptr, i64)\n");
    // C stdio flush: generated prints go through C `printf` (block-buffered on
    // pipes) while the bridge VM prints through Rust's line-buffered stdout —
    // flushing *C* stdio before each bridge call keeps output ordered. (The
    // lkrt io flush helper flushes lkrt's Rust-side stdout: wrong buffer.)
    out.push_str("declare i32 @fflush(ptr)\n");
    let max_args = hybrid_argbuf_len(module);
    if max_args > 0 {
        let _ = writeln!(
            out,
            "@lk_hybrid_argbuf = internal global [{max_args} x %LkHybridArg] zeroinitializer"
        );
    }
    out.push('\n');
}

fn hybrid_argbuf_len(module: &MirModule) -> usize {
    module
        .functions
        .iter()
        .flat_map(|f| f.blocks.iter())
        .flat_map(|b| b.insts.iter())
        .filter_map(|inst| match inst {
            Inst::CallVm { args, .. } => Some(args.len()),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// One bridge call: marshal each scalar into its tagged slot, flush C stdio
/// (the VM prints through Rust's line-buffered stdout — unflushed C buffers
/// would reorder pipe output; precedent: `lkrt_abort` flushes), then call the
/// bridge. No result: the lowering left the destination register unbound.
fn render_call_vm(out: &mut String, module: &MirModule, func: FuncId, args: &[ValueId]) {
    let params = &module
        .vm_function(func)
        .expect("validated: CallVm target is recorded in vm_functions")
        .params;
    let uid = out.len();
    let buf_len = hybrid_argbuf_len(module);
    for (i, (value, ty)) in args.iter().zip(params.iter()).enumerate() {
        let tag = match ty {
            Ty::I64 => 0,
            Ty::F64 => 1,
            Ty::Bool => 2,
            Ty::Str => 3,
            other => unreachable!("non-scalar hybrid marshaling type {other:?}"),
        };
        let _ = writeln!(
            out,
            "  %hytag{uid}_{i} = getelementptr inbounds [{buf_len} x %LkHybridArg], ptr @lk_hybrid_argbuf, i64 0, i64 {i}, i32 0"
        );
        let _ = writeln!(out, "  store i8 {tag}, ptr %hytag{uid}_{i}");
        let _ = writeln!(
            out,
            "  %hyval{uid}_{i} = getelementptr inbounds [{buf_len} x %LkHybridArg], ptr @lk_hybrid_argbuf, i64 0, i64 {i}, i32 1"
        );
        match ty {
            Ty::I64 => {
                let _ = writeln!(out, "  store i64 {}, ptr %hyval{uid}_{i}", val(*value));
            }
            Ty::F64 => {
                let _ = writeln!(out, "  store double {}, ptr %hyval{uid}_{i}", val(*value));
            }
            Ty::Bool => {
                let _ = writeln!(out, "  %hyzext{uid}_{i} = zext i1 {} to i64", val(*value));
                let _ = writeln!(out, "  store i64 %hyzext{uid}_{i}, ptr %hyval{uid}_{i}");
            }
            Ty::Str => {
                let _ = writeln!(out, "  store ptr {}, ptr %hyval{uid}_{i}", val(*value));
            }
            other => unreachable!("non-scalar hybrid marshaling type {other:?}"),
        }
    }
    let _ = writeln!(out, "  %hyflush{uid} = call i32 @fflush(ptr null)");
    let buffer = if args.is_empty() {
        "null".to_string()
    } else {
        "@lk_hybrid_argbuf".to_string()
    };
    let _ = writeln!(
        out,
        "  call void @lk_hybrid_call_v(i32 {}, ptr {buffer}, i64 {})",
        func.0,
        args.len()
    );
}

fn render_prelude(out: &mut String) {
    out.push_str("@lk_i64_fmt = private unnamed_addr constant [5 x i8] c\"%ld\\0A\\00\", align 1\n");
    out.push_str("@lk_f64_fmt = private unnamed_addr constant [7 x i8] c\"%.16g\\0A\\00\", align 1\n");
    out.push_str("@lk_str_fmt = private unnamed_addr constant [4 x i8] c\"%s\\0A\\00\", align 1\n");
    out.push_str("@lk_str_raw_fmt = private unnamed_addr constant [3 x i8] c\"%s\\00\", align 1\n");
    out.push_str("@lk_bool_true = private unnamed_addr constant [5 x i8] c\"true\\00\", align 1\n");
    out.push_str("@lk_bool_false = private unnamed_addr constant [6 x i8] c\"false\\00\", align 1\n\n");
    out.push_str("declare i32 @printf(ptr, ...)\n");
    out.push_str("declare void @abort()\n");
    // Dynamic `List<i64>` indexing returns a by-value `Maybe<i64>` (`{i64, i64}`),
    // which is outside the scalar ABI schema, so declare it directly here.
    out.push_str("declare { i64, i64 } @lkrt_lklist_i64_get_pair(ptr, i64)\n");
    out.push_str("declare { double, i64 } @lkrt_lklist_f64_get_pair(ptr, i64)\n");
    out.push_str("declare { i64, i64 } @lkrt_lkmap_str_i64_get_pair(ptr, ptr)\n");
    out.push_str("declare { i64, i64 } @lkrt_lkmap_i64_i64_get_pair(ptr, i64)\n");
    out.push_str("declare { double, i64 } @lkrt_lkmap_str_f64_get_pair(ptr, ptr)\n");
    out.push_str("declare { double, i64 } @lkrt_lkmap_i64_f64_get_pair(ptr, i64)\n");
    out.push_str("declare { ptr, i64 } @lkrt_lklist_str_get_pair(ptr, i64)\n");
    out.push_str("declare i64 @lkrt_maybe_i64_unwrap(i64, i64)\n");
    out.push_str("declare double @lkrt_maybe_f64_unwrap(double, i64)\n");
    out.push_str("declare ptr @lkrt_maybe_str_unwrap(ptr, i64)\n\n");
    out.push_str(&abi_declarations());
    out.push('\n');
}

/// Renders `declare` lines for every `lkrt_`-exported ABI function. This is the
/// LLVM-specific rendering of the shared schema (lives here, not in `lk-aot-abi`).
pub fn abi_declarations() -> String {
    let mut out = String::new();
    for f in ABI_FUNCTIONS {
        if !f.symbol.starts_with("lkrt_") {
            continue;
        }
        let _ = write!(out, "declare {} @{}(", llvm_ty(f.result), f.symbol);
        for (i, p) in f.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(llvm_ty(*p));
        }
        out.push_str(")\n");
    }
    out
}

fn llvm_ty(t: AbiType) -> &'static str {
    match t {
        AbiType::I64 => "i64",
        AbiType::F64 => "double",
        AbiType::Ptr | AbiType::StrPtr => "ptr",
        AbiType::Nil => "void",
        AbiType::DynVal => "{ i64, i64 }",
    }
}

fn ty_str(t: Ty) -> &'static str {
    match t {
        Ty::I64 => "i64",
        Ty::F64 => "double",
        Ty::Bool => "i1",
        Ty::Str
        | Ty::ListI64
        | Ty::ListF64
        | Ty::ListStr
        | Ty::MapStrI64
        | Ty::MapI64I64
        | Ty::MapStrF64
        | Ty::MapI64F64
        | Ty::MapStrBool => "ptr",
        Ty::Nil => "void",
        Ty::MaybeI64 | Ty::MaybeBool => "{ i64, i64 }",
        Ty::MaybeF64 => "{ double, i64 }",
        Ty::MaybeStr => "{ ptr, i64 }",
        Ty::Dyn => "{ i64, i64 }",
        Ty::ListDyn => "ptr",
    }
}

fn val(v: ValueId) -> String {
    format!("%v{}", v.0)
}

fn blk(module_fn: FuncId, b: BlockId) -> String {
    format!("f{}bb{}", module_fn.0, b.0)
}

fn render_function(out: &mut String, module: &MirModule, func: &MirFunction, is_entry: bool) {
    if is_entry {
        out.push_str("define i32 @main() {\n");
        out.push_str("entry:\n");
        // ABI drift guard (see aot-redesign §3.3); does not alter block CFG.
        let _ = writeln!(out, "  call void @lkrt_abi_check(i64 {})", module.abi_version);
        let _ = writeln!(out, "  br label %{}", blk(func.id, func.entry));
    } else {
        let params = func
            .params
            .iter()
            .map(|(v, t)| format!("{} {}", ty_str(*t), val(*v)))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "define {} @lk_fn_{}({}) {{", ty_str(func.ret), func.id.0, params);
    }
    for block in &func.blocks {
        render_block(out, module, func, block, is_entry);
    }
    out.push_str("}\n\n");
}

fn render_block(out: &mut String, module: &MirModule, func: &MirFunction, block: &Block, is_entry: bool) {
    let _ = writeln!(out, "{}:", blk(func.id, block.id));
    // SSA block parameters become phi nodes gathering the matching branch argument
    // from every predecessor.
    for (idx, (param, ty)) in block.params.iter().enumerate() {
        let mut incoming = Vec::new();
        for pred in &func.blocks {
            collect_phi_incoming(func.id, pred, block.id, idx, &mut incoming);
        }
        let joined = incoming.join(", ");
        let _ = writeln!(out, "  {} = phi {} {}", val(*param), ty_str(*ty), joined);
    }
    for inst in &block.insts {
        render_inst(out, module, inst);
    }
    render_term(out, func, &block.term, is_entry);
}

fn collect_phi_incoming(fid: FuncId, pred: &Block, target: BlockId, param_idx: usize, out: &mut Vec<String>) {
    let arg = match &pred.term {
        Term::Br { target: t, args } if *t == target => args.get(param_idx).copied(),
        Term::CondBr {
            then_blk,
            then_args,
            else_blk,
            else_args,
            ..
        } => {
            if *then_blk == target {
                then_args.get(param_idx).copied()
            } else if *else_blk == target {
                else_args.get(param_idx).copied()
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(a) = arg {
        out.push(format!("[ {}, %{} ]", val(a), blk(fid, pred.id)));
    }
}

fn render_inst(out: &mut String, module: &MirModule, inst: &Inst) {
    match inst {
        Inst::Const { dst, value } => render_const(out, *dst, value),
        Inst::IntBin { dst, op, lhs, rhs } => render_int_bin(out, *dst, *op, *lhs, *rhs),
        Inst::FloatBin { dst, op, lhs, rhs } => render_float_bin(out, *dst, *op, *lhs, *rhs),
        Inst::Cmp {
            dst,
            op,
            float,
            lhs,
            rhs,
        } => render_cmp(out, *dst, *op, *float, *lhs, *rhs),
        Inst::IntToFloat { dst, src } => {
            let _ = writeln!(out, "  {} = sitofp i64 {} to double", val(*dst), val(*src));
        }
        Inst::ZextBool { dst, src } => {
            let _ = writeln!(out, "  {} = zext i1 {} to i64", val(*dst), val(*src));
        }
        Inst::Not { dst, src } => {
            let _ = writeln!(out, "  {} = xor i1 {}, true", val(*dst), val(*src));
        }
        Inst::BoolAnd { dst, lhs, rhs } => {
            let _ = writeln!(out, "  {} = and i1 {}, {}", val(*dst), val(*lhs), val(*rhs));
        }
        Inst::MaybePresent { dst, src, maybe_ty } => {
            let n = dst.0;
            let carrier = ty_str(*maybe_ty);
            let _ = writeln!(out, "  %mp{n} = extractvalue {carrier} {}, 1", val(*src));
            let _ = writeln!(out, "  {} = icmp ne i64 %mp{n}, 0", val(*dst));
        }
        Inst::Call { dst, callee, args } => render_call(out, *dst, *callee, args),
        Inst::CallFn { dst, func, args } => render_call_fn(out, module, *dst, *func, args),
        Inst::CallVm { func, args } => render_call_vm(out, module, *func, args),
        Inst::PrintStr { value, newline } => {
            let fmt = if *newline { "@lk_str_fmt" } else { "@lk_str_raw_fmt" };
            let _ = writeln!(out, "  call i32 (ptr, ...) @printf(ptr {fmt}, ptr {})", val(*value));
        }
        Inst::Select {
            dst,
            cond,
            then_v,
            else_v,
            ty,
        } => {
            let t = ty_str(*ty);
            let _ = writeln!(
                out,
                "  {} = select i1 {}, {t} {}, {t} {}",
                val(*dst),
                val(*cond),
                val(*then_v),
                val(*else_v)
            );
        }
        Inst::MaybeValue { dst, src, maybe_ty } => {
            let carrier = ty_str(*maybe_ty);
            let _ = writeln!(out, "  {} = extractvalue {carrier} {}, 0", val(*dst), val(*src));
        }
        Inst::MaybeWrap { dst, src, maybe_ty } => {
            let n = dst.0;
            let carrier = ty_str(*maybe_ty);
            let value_ty = match maybe_ty {
                Ty::MaybeF64 => "double",
                Ty::MaybeStr => "ptr",
                Ty::MaybeBool => "i64",
                _ => "i64",
            };
            let _ = writeln!(
                out,
                "  %mw{n} = insertvalue {carrier} poison, {value_ty} {}, 0",
                val(*src)
            );
            let _ = writeln!(out, "  {} = insertvalue {carrier} %mw{n}, i64 1, 1", val(*dst));
        }
        Inst::GlobalGet { dst, gvar } => {
            let ty = gvar_ty(module, *gvar);
            let _ = writeln!(out, "  {} = load {ty}, ptr @lk_gvar_{gvar}", val(*dst));
        }
        Inst::GlobalSet { gvar, src } => {
            let ty = gvar_ty(module, *gvar);
            let _ = writeln!(out, "  store {ty} {}, ptr @lk_gvar_{gvar}", val(*src));
        }
        Inst::ListGetMaybe { dst, handle, index } => {
            let _ = writeln!(
                out,
                "  {} = call {{ i64, i64 }} @lkrt_lklist_i64_get_pair(ptr {}, i64 {})",
                val(*dst),
                val(*handle),
                val(*index)
            );
        }
        Inst::ListGetMaybeF64 { dst, handle, index } => {
            let _ = writeln!(
                out,
                "  {} = call {{ double, i64 }} @lkrt_lklist_f64_get_pair(ptr {}, i64 {})",
                val(*dst),
                val(*handle),
                val(*index)
            );
        }
        Inst::UnwrapMaybeF64 { dst, src } => {
            let n = dst.0;
            let _ = writeln!(out, "  %u{n}v = extractvalue {{ double, i64 }} {}, 0", val(*src));
            let _ = writeln!(out, "  %u{n}p = extractvalue {{ double, i64 }} {}, 1", val(*src));
            let _ = writeln!(
                out,
                "  {} = call double @lkrt_maybe_f64_unwrap(double %u{n}v, i64 %u{n}p)",
                val(*dst)
            );
        }
        Inst::MapGetMaybe { dst, handle, key } => {
            let _ = writeln!(
                out,
                "  {} = call {{ i64, i64 }} @lkrt_lkmap_str_i64_get_pair(ptr {}, ptr {})",
                val(*dst),
                val(*handle),
                val(*key)
            );
        }
        Inst::MapGetMaybeI64Key { dst, handle, key } => {
            let _ = writeln!(
                out,
                "  {} = call {{ i64, i64 }} @lkrt_lkmap_i64_i64_get_pair(ptr {}, i64 {})",
                val(*dst),
                val(*handle),
                val(*key)
            );
        }
        Inst::MapGetMaybeStrF64 { dst, handle, key } => {
            let _ = writeln!(
                out,
                "  {} = call {{ double, i64 }} @lkrt_lkmap_str_f64_get_pair(ptr {}, ptr {})",
                val(*dst),
                val(*handle),
                val(*key)
            );
        }
        Inst::MapGetMaybeI64F64 { dst, handle, key } => {
            let _ = writeln!(
                out,
                "  {} = call {{ double, i64 }} @lkrt_lkmap_i64_f64_get_pair(ptr {}, i64 {})",
                val(*dst),
                val(*handle),
                val(*key)
            );
        }
        Inst::UnwrapMaybeI64 { dst, src } => {
            let n = dst.0;
            let _ = writeln!(out, "  %u{n}v = extractvalue {{ i64, i64 }} {}, 0", val(*src));
            let _ = writeln!(out, "  %u{n}p = extractvalue {{ i64, i64 }} {}, 1", val(*src));
            let _ = writeln!(
                out,
                "  {} = call i64 @lkrt_maybe_i64_unwrap(i64 %u{n}v, i64 %u{n}p)",
                val(*dst)
            );
        }
        Inst::ListGetMaybeStr { dst, handle, index } => {
            let _ = writeln!(
                out,
                "  {} = call {{ ptr, i64 }} @lkrt_lklist_str_get_pair(ptr {}, i64 {})",
                val(*dst),
                val(*handle),
                val(*index)
            );
        }
        Inst::UnwrapMaybeStr { dst, src } => {
            let n = dst.0;
            let _ = writeln!(out, "  %u{n}v = extractvalue {{ ptr, i64 }} {}, 0", val(*src));
            let _ = writeln!(out, "  %u{n}p = extractvalue {{ ptr, i64 }} {}, 1", val(*src));
            let _ = writeln!(
                out,
                "  {} = call ptr @lkrt_maybe_str_unwrap(ptr %u{n}v, i64 %u{n}p)",
                val(*dst)
            );
        }
    }
}

/// Direct call to another function in the module (`@lk_fn_N`). Both the parameter
/// types and the result type are the callee's inferred (monomorphic) types, looked
/// up from its `MirFunction` — so an `f64`/`bool` parameter or return is rendered
/// correctly.
fn render_call_fn(out: &mut String, module: &MirModule, dst: Option<ValueId>, func: FuncId, args: &[ValueId]) {
    let callee = module.function(func);
    let ret = callee.map(|f| ty_str(f.ret)).unwrap_or("i64");
    let arg_list = args
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let ty = callee
                .and_then(|f| f.params.get(i))
                .map(|(_, t)| ty_str(*t))
                .unwrap_or("i64");
            format!("{ty} {}", val(*v))
        })
        .collect::<Vec<_>>()
        .join(", ");
    match dst {
        Some(d) => {
            let _ = writeln!(out, "  {} = call {ret} @lk_fn_{}({})", val(d), func.0, arg_list);
        }
        None => {
            let _ = writeln!(out, "  call {ret} @lk_fn_{}({})", func.0, arg_list);
        }
    }
}

fn render_const(out: &mut String, dst: ValueId, value: &Const) {
    // LK stores scalars in memory-less SSA; a constant is materialized via a no-op
    // `add`/`fadd` with an identity so the value has a definition site.
    match value {
        Const::I64(n) => {
            let _ = writeln!(out, "  {} = add i64 0, {}", val(dst), n);
        }
        Const::F64(x) => {
            let _ = writeln!(out, "  {} = fadd double 0.0, {}", val(dst), render_f64(*x));
        }
        Const::Bool(b) => {
            let _ = writeln!(out, "  {} = add i1 0, {}", val(dst), i32::from(*b));
        }
        Const::Str(g) => {
            let _ = writeln!(out, "  {} = getelementptr i8, ptr @lk_str_{}, i64 0", val(dst), g.0);
        }
        Const::FnAddr(f) => {
            let _ = writeln!(out, "  {} = getelementptr i8, ptr @lk_fn_{}, i64 0", val(dst), f.0);
        }
        Const::Nil => {
            let _ = writeln!(out, "  {} = add i64 0, 0", val(dst));
        }
    }
}

fn render_f64(x: f64) -> String {
    if !x.is_finite() {
        // LLVM's textual parser has no `NaN`/`inf` literal — non-finite
        // constants (reachable via `math.nan`/`math.inf`) spell the exact
        // IEEE-754 bit pattern in hex-float form.
        format!("0x{:016X}", x.to_bits())
    } else if x == x.trunc() {
        format!("{x:.1}")
    } else {
        format!("{x}")
    }
}

fn render_int_bin(out: &mut String, dst: ValueId, op: IntBinOp, lhs: ValueId, rhs: ValueId) {
    let (l, r) = (val(lhs), val(rhs));
    match op {
        IntBinOp::Add => {
            let _ = writeln!(out, "  {} = add i64 {}, {}", val(dst), l, r);
        }
        IntBinOp::Sub => {
            let _ = writeln!(out, "  {} = sub i64 {}, {}", val(dst), l, r);
        }
        IntBinOp::Mul => {
            let _ = writeln!(out, "  {} = mul i64 {}, {}", val(dst), l, r);
        }
        // Guarded helpers, never raw sdiv/srem (matches VM divide-by-zero abort).
        IntBinOp::Div => {
            let _ = writeln!(
                out,
                "  {} = call i64 @lkrt_i64_div_checked(i64 {}, i64 {})",
                val(dst),
                l,
                r
            );
        }
        IntBinOp::Mod => {
            let _ = writeln!(
                out,
                "  {} = call i64 @lkrt_i64_mod_checked(i64 {}, i64 {})",
                val(dst),
                l,
                r
            );
        }
        IntBinOp::Min | IntBinOp::Max => {
            let pred = if matches!(op, IntBinOp::Min) { "slt" } else { "sgt" };
            let cond = format!("{}c", val(dst));
            let _ = writeln!(out, "  {} = icmp {} i64 {}, {}", cond, pred, l, r);
            let _ = writeln!(out, "  {} = select i1 {}, i64 {}, i64 {}", val(dst), cond, l, r);
        }
    }
}

fn render_float_bin(out: &mut String, dst: ValueId, op: FloatBinOp, lhs: ValueId, rhs: ValueId) {
    let (l, r) = (val(lhs), val(rhs));
    let line = match op {
        FloatBinOp::Add => format!("  {} = fadd double {}, {}", val(dst), l, r),
        FloatBinOp::Sub => format!("  {} = fsub double {}, {}", val(dst), l, r),
        FloatBinOp::Mul => format!("  {} = fmul double {}, {}", val(dst), l, r),
        FloatBinOp::Div => format!(
            "  {} = call double @lkrt_f64_div_checked(double {}, double {})",
            val(dst),
            l,
            r
        ),
        FloatBinOp::Mod => format!(
            "  {} = call double @lkrt_f64_mod_checked(double {}, double {})",
            val(dst),
            l,
            r
        ),
    };
    let _ = writeln!(out, "{line}");
}

fn render_cmp(out: &mut String, dst: ValueId, op: CmpOp, float: bool, lhs: ValueId, rhs: ValueId) {
    let (l, r) = (val(lhs), val(rhs));
    if float {
        let pred = match op {
            CmpOp::Eq => "oeq",
            // `une`, not `one`: Rust/VM `!=` is true when either operand is
            // NaN (`math.nan` is reachable), and the ordered predicate would
            // return false for that case.
            CmpOp::Ne => "une",
            CmpOp::Lt => "olt",
            CmpOp::Le => "ole",
            CmpOp::Gt => "ogt",
            CmpOp::Ge => "oge",
        };
        let _ = writeln!(out, "  {} = fcmp {} double {}, {}", val(dst), pred, l, r);
    } else {
        let pred = match op {
            CmpOp::Eq => "eq",
            CmpOp::Ne => "ne",
            CmpOp::Lt => "slt",
            CmpOp::Le => "sle",
            CmpOp::Gt => "sgt",
            CmpOp::Ge => "sge",
        };
        let _ = writeln!(out, "  {} = icmp {} i64 {}, {}", val(dst), pred, l, r);
    }
}

fn render_call(out: &mut String, dst: Option<ValueId>, callee: AbiRef, args: &[ValueId]) {
    let sig = callee.resolve().expect("validated: ABI call resolves");
    let arg_list = args
        .iter()
        .zip(sig.params.iter())
        .map(|(v, p)| format!("{} {}", llvm_ty(*p), val(*v)))
        .collect::<Vec<_>>()
        .join(", ");
    match dst {
        Some(d) => {
            let _ = writeln!(
                out,
                "  {} = call {} @{}({})",
                val(d),
                llvm_ty(sig.result),
                sig.symbol,
                arg_list
            );
        }
        None => {
            let _ = writeln!(out, "  call {} @{}({})", llvm_ty(sig.result), sig.symbol, arg_list);
        }
    }
}

fn render_term(out: &mut String, func: &MirFunction, term: &Term, is_entry: bool) {
    match term {
        Term::Ret(value) => render_ret(out, func.ret, *value, is_entry),
        Term::Br { target, .. } => {
            let _ = writeln!(out, "  br label %{}", blk(func.id, *target));
        }
        Term::CondBr {
            cond,
            then_blk,
            else_blk,
            ..
        } => {
            let _ = writeln!(
                out,
                "  br i1 {}, label %{}, label %{}",
                val(*cond),
                blk(func.id, *then_blk),
                blk(func.id, *else_blk)
            );
        }
        Term::Abort => {
            // `lkrt_abort` flushes C stdio before aborting so already-printed
            // output is not discarded (the VM keeps it on a fatal error).
            out.push_str("  call void @lkrt_abort()\n  unreachable\n");
        }
    }
}

fn render_ret(out: &mut String, ret_ty: Ty, value: Option<ValueId>, is_entry: bool) {
    if is_entry {
        // The entry prints its result (like the existing backend) and returns 0.
        if let Some(v) = value {
            match ret_ty {
                Ty::I64 => {
                    let _ = writeln!(out, "  call i32 (ptr, ...) @printf(ptr @lk_i64_fmt, i64 {})", val(v));
                }
                Ty::F64 => {
                    // Format via the runtime helper (Rust `to_string`, exactly the
                    // VM's float display) rather than `printf`'s `%g`, whose fixed
                    // precision diverges from the VM's shortest round-trip output.
                    let n = v.0;
                    let _ = writeln!(out, "  %f{n}s = call ptr @lkrt_f64_to_str(double {})", val(v));
                    let _ = writeln!(out, "  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr %f{n}s)");
                }
                Ty::Bool => {
                    let _ = writeln!(
                        out,
                        "  {}p = select i1 {}, ptr @lk_bool_true, ptr @lk_bool_false",
                        val(v),
                        val(v)
                    );
                    let _ = writeln!(out, "  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {}p)", val(v));
                }
                Ty::Str => {
                    let _ = writeln!(out, "  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {})", val(v));
                }
                // A dynamic index result: print the element when present. When
                // absent, the VM's top-level auto-print of a `nil` return emits
                // *nothing* (unlike `print(nil)`), so the absent branch prints
                // nothing and just returns — matching `return xs[oob]` exactly.
                Ty::MaybeI64 => {
                    let n = v.0;
                    let _ = writeln!(out, "  %m{n}p = extractvalue {{ i64, i64 }} {}, 1", val(v));
                    let _ = writeln!(out, "  %m{n}v = extractvalue {{ i64, i64 }} {}, 0", val(v));
                    let _ = writeln!(out, "  %m{n}c = icmp ne i64 %m{n}p, 0");
                    let _ = writeln!(out, "  br i1 %m{n}c, label %m{n}some, label %m{n}none");
                    let _ = writeln!(out, "m{n}some:");
                    let _ = writeln!(out, "  call i32 (ptr, ...) @printf(ptr @lk_i64_fmt, i64 %m{n}v)");
                    let _ = writeln!(out, "  call void @lkrt_cleanup()");
                    let _ = writeln!(out, "  ret i32 0");
                    let _ = writeln!(out, "m{n}none:");
                    let _ = writeln!(out, "  call void @lkrt_cleanup()");
                    let _ = writeln!(out, "  ret i32 0");
                    return;
                }
                Ty::MaybeF64 => {
                    let n = v.0;
                    let _ = writeln!(out, "  %m{n}p = extractvalue {{ double, i64 }} {}, 1", val(v));
                    let _ = writeln!(out, "  %m{n}v = extractvalue {{ double, i64 }} {}, 0", val(v));
                    let _ = writeln!(out, "  %m{n}c = icmp ne i64 %m{n}p, 0");
                    let _ = writeln!(out, "  br i1 %m{n}c, label %m{n}some, label %m{n}none");
                    let _ = writeln!(out, "m{n}some:");
                    let _ = writeln!(out, "  %m{n}str = call ptr @lkrt_f64_to_str(double %m{n}v)");
                    let _ = writeln!(out, "  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr %m{n}str)");
                    let _ = writeln!(out, "  call void @lkrt_cleanup()");
                    let _ = writeln!(out, "  ret i32 0");
                    let _ = writeln!(out, "m{n}none:");
                    let _ = writeln!(out, "  call void @lkrt_cleanup()");
                    let _ = writeln!(out, "  ret i32 0");
                    return;
                }
                Ty::MaybeStr => {
                    let n = v.0;
                    let _ = writeln!(out, "  %m{n}p = extractvalue {{ ptr, i64 }} {}, 1", val(v));
                    let _ = writeln!(out, "  %m{n}v = extractvalue {{ ptr, i64 }} {}, 0", val(v));
                    let _ = writeln!(out, "  %m{n}c = icmp ne i64 %m{n}p, 0");
                    let _ = writeln!(out, "  br i1 %m{n}c, label %m{n}some, label %m{n}none");
                    let _ = writeln!(out, "m{n}some:");
                    let _ = writeln!(out, "  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr %m{n}v)");
                    let _ = writeln!(out, "  call void @lkrt_cleanup()");
                    let _ = writeln!(out, "  ret i32 0");
                    let _ = writeln!(out, "m{n}none:");
                    let _ = writeln!(out, "  call void @lkrt_cleanup()");
                    let _ = writeln!(out, "  ret i32 0");
                    return;
                }
                Ty::MaybeBool => {
                    let n = v.0;
                    let _ = writeln!(out, "  %m{n}p = extractvalue {{ i64, i64 }} {}, 1", val(v));
                    let _ = writeln!(out, "  %m{n}v = extractvalue {{ i64, i64 }} {}, 0", val(v));
                    let _ = writeln!(out, "  %m{n}c = icmp ne i64 %m{n}p, 0");
                    let _ = writeln!(out, "  br i1 %m{n}c, label %m{n}some, label %m{n}none");
                    let _ = writeln!(out, "m{n}some:");
                    let _ = writeln!(out, "  %m{n}b = icmp ne i64 %m{n}v, 0");
                    let _ = writeln!(
                        out,
                        "  %m{n}s = select i1 %m{n}b, ptr @lk_bool_true, ptr @lk_bool_false"
                    );
                    let _ = writeln!(out, "  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr %m{n}s)");
                    let _ = writeln!(out, "  call void @lkrt_cleanup()");
                    let _ = writeln!(out, "  ret i32 0");
                    let _ = writeln!(out, "m{n}none:");
                    let _ = writeln!(out, "  call void @lkrt_cleanup()");
                    let _ = writeln!(out, "  ret i32 0");
                    return;
                }
                // Printing a returned container/nil is not modelled; the lowering
                // rejects an entry that returns these, so this is unreachable.
                Ty::Nil
                | Ty::ListI64
                | Ty::ListF64
                | Ty::ListStr
                | Ty::MapStrI64
                | Ty::MapI64I64
                | Ty::MapStrF64
                | Ty::MapI64F64
                | Ty::MapStrBool
                // D1: lowering never returns Dyn from the entry yet (Dyn is
                // function-body-local); silent like the container arm.
                | Ty::Dyn
                | Ty::ListDyn => {}
            }
        }
        // Default-arena ownership (RFC §3.4): reclaim every runtime allocation
        // (strings + container handles) on the clean exit path. Must run after
        // the result print above, which may reference an arena-owned string.
        out.push_str("  call void @lkrt_cleanup()\n");
        out.push_str("  ret i32 0\n");
    } else {
        match value {
            Some(v) => {
                let _ = writeln!(out, "  ret {} {}", ty_str(ret_ty), val(v));
            }
            None => out.push_str("  ret void\n"),
        }
    }
}

fn gvar_ty(module: &MirModule, gvar: u32) -> &'static str {
    module
        .mutable_globals
        .get(gvar as usize)
        .map(|(_, ty)| ty_str(*ty))
        .unwrap_or("i64")
}

fn render_globals(out: &mut String, module: &MirModule) {
    for (i, g) in module.globals.iter().enumerate() {
        let bytes = g.len() + 1;
        let escaped: String = g.bytes().map(|b| format!("\\{b:02x}")).collect();
        let _ = writeln!(
            out,
            "@lk_str_{i} = private unnamed_addr constant [{bytes} x i8] c\"{escaped}\\00\", align 1"
        );
    }
    for (i, (name, ty)) in module.mutable_globals.iter().enumerate() {
        let zero = match ty {
            Ty::F64 => "0.0",
            Ty::Bool => "false",
            Ty::Str => "null",
            _ => "0",
        };
        let _ = writeln!(out, "@lk_gvar_{i} = internal global {} {zero} ; {name}", ty_str(*ty));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lk_aot_mir::*;

    fn div_module() -> MirModule {
        let (a, b, out) = (ValueId(0), ValueId(1), ValueId(2));
        MirModule {
            abi_version: lk_aot_abi::ABI_VERSION,
            globals: vec![],
            mutable_globals: Vec::new(),
            vm_functions: Vec::new(),
            entry: FuncId(0),
            functions: vec![MirFunction {
                id: FuncId(0),
                params: vec![],
                entry: BlockId(0),
                ret: Ty::I64,
                blocks: vec![Block {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        Inst::Const {
                            dst: a,
                            value: Const::I64(20),
                        },
                        Inst::Const {
                            dst: b,
                            value: Const::I64(4),
                        },
                        Inst::IntBin {
                            dst: out,
                            op: IntBinOp::Div,
                            lhs: a,
                            rhs: b,
                        },
                    ],
                    term: Term::Ret(Some(out)),
                }],
            }],
        }
    }

    #[test]
    fn renders_valid_module_and_uses_guarded_div() {
        let m = div_module();
        assert_eq!(validate(&m), Ok(()));
        let ir = render_module(&m);
        assert!(ir.contains("define i32 @main()"));
        assert!(ir.contains("call void @lkrt_abi_check(i64 1)"));
        // Division must route through the guarded helper, not raw sdiv.
        assert!(ir.contains("call i64 @lkrt_i64_div_checked(i64 %v0, i64 %v1)"));
        assert!(!ir.contains("sdiv"));
        assert!(ir.contains("@printf(ptr @lk_i64_fmt, i64 %v2)"));
        assert!(ir.contains("ret i32 0"));
        // The ABI helper it calls must be declared.
        assert!(ir.contains("declare i64 @lkrt_i64_div_checked(i64, i64)"));
    }

    #[test]
    fn renders_block_params_as_phis() {
        // fn main() { if 1<2 { ret 10 } else { ret 20 } } via a join block param.
        let (one, two, cond, ten, twenty, r) = (ValueId(0), ValueId(1), ValueId(2), ValueId(3), ValueId(4), ValueId(5));
        let m = MirModule {
            abi_version: lk_aot_abi::ABI_VERSION,
            globals: vec![],
            mutable_globals: Vec::new(),
            vm_functions: Vec::new(),
            entry: FuncId(0),
            functions: vec![MirFunction {
                id: FuncId(0),
                params: vec![],
                entry: BlockId(0),
                ret: Ty::I64,
                blocks: vec![
                    Block {
                        id: BlockId(0),
                        params: vec![],
                        insts: vec![
                            Inst::Const {
                                dst: one,
                                value: Const::I64(1),
                            },
                            Inst::Const {
                                dst: two,
                                value: Const::I64(2),
                            },
                            Inst::Cmp {
                                dst: cond,
                                op: CmpOp::Lt,
                                float: false,
                                lhs: one,
                                rhs: two,
                            },
                        ],
                        term: Term::CondBr {
                            cond,
                            then_blk: BlockId(1),
                            then_args: vec![],
                            else_blk: BlockId(2),
                            else_args: vec![],
                        },
                    },
                    Block {
                        id: BlockId(1),
                        params: vec![],
                        insts: vec![Inst::Const {
                            dst: ten,
                            value: Const::I64(10),
                        }],
                        term: Term::Br {
                            target: BlockId(3),
                            args: vec![ten],
                        },
                    },
                    Block {
                        id: BlockId(2),
                        params: vec![],
                        insts: vec![Inst::Const {
                            dst: twenty,
                            value: Const::I64(20),
                        }],
                        term: Term::Br {
                            target: BlockId(3),
                            args: vec![twenty],
                        },
                    },
                    Block {
                        id: BlockId(3),
                        params: vec![(r, Ty::I64)],
                        insts: vec![],
                        term: Term::Ret(Some(r)),
                    },
                ],
            }],
        };
        assert_eq!(validate(&m), Ok(()));
        let ir = render_module(&m);
        assert!(ir.contains("%v5 = phi i64 [ %v3, %f0bb1 ], [ %v4, %f0bb2 ]"), "{ir}");
        assert!(ir.contains("br i1 %v2, label %f0bb1, label %f0bb2"));
    }
}
