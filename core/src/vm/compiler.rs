//! Minimal compiler for the new `Function` IR.
//!
//! This is the first migration point from AST to the new VM path. It is
//! deliberately small and independent from the previous `FunctionBuilder`.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::rc::Rc;
mod assign;
mod builder;
mod call;
mod const_maps;
mod container_lower;
mod control_flow;
mod decls;
mod entry;
mod expr_lower;
mod facts;
#[cfg(test)]
mod facts_tests;
mod for_value_usage;
mod free_vars;
mod inline;
mod loop_consts;
mod lower_into;
mod match_expr;
mod pattern_bind;
mod pattern_control;
mod range_loop;
mod stmt_lower;
mod support;
#[cfg(test)]
mod tests;

use crate::compat::collections::{HashMap, HashSet};
use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::{
    expr::{Expr, Pattern, TemplateStringPart},
    operator::{BinOp, UnaryOp},
    stmt::{ForPattern, Program, Stmt},
    util::fast_map::FastHashMap,
    val::{FunctionNamedParamType, LiteralVal, RuntimeMapKey, ShortStr, Type},
};

use super::{ConstHeapValue, ConstRuntimeValue, Function, GlobalSlot, Instr, Module, NativeEntry, Opcode};
use crate::vm::analysis::{
    PerfCallTargetKind, PerfContainerBuildFact, PerfGlobalFact, PerfKeyFact, PerfRegisterFact, PerfStringIntKeyFact,
    PerfValueKind,
};
use facts::*;
use for_value_usage::{stmt_shadows_name_deep, stmt_uses_for_binding_value};
use free_vars::{collect_expr_closure_captures, collect_expr_free_vars, collect_stmt_closure_captures};
use loop_consts::ScalarLoopConstKey;
use support::*;

#[derive(Debug, Default)]
pub struct Compiler {
    function: Function,
    next_reg: u16,
    peak_reg: u16, // highest next_reg ever reached — used for register_count
    locals: HashMap<String, u16>,
    /// Which scope each live binding was declared in.
    ///
    /// `locals` alone cannot answer "is this name bound *here*, or outside?",
    /// and `let` needs that: a `let` shadowing an outer binding must take a
    /// fresh register, because reusing the outer one overwrites the value the
    /// enclosing scope goes back to reading. `if c { let x = 2; }` left `x` at
    /// 2 outside the block — in every construct, silently.
    local_scopes: HashMap<String, u32>,
    /// How many scopes deep the lowering currently is. Bumped wherever
    /// `locals` is saved and restored.
    scope_depth: u32,
    // The ten tables below describe the *program*, not the function being
    // compiled: names, signatures, inlinable bodies, widths. Every function gets
    // its own `Compiler`, and each one used to receive a deep **clone** of all
    // ten — so compiling the n-th function copied everything the n-1 before it
    // had declared, and `function_bodies` copies an AST per entry. Quadratic in
    // the size of the file, and measurably so: 1000 functions type-checked and
    // compiled in 0.55s, 2000 in 2.30s, 4000 in 10.9s.
    //
    // `Rc` because none of them is ever written after `collect_*` builds it —
    // sharing is the whole truth about them, and a clone is now a refcount bump.
    // Read sites are unchanged: `Rc` derefs.
    function_names: Rc<HashMap<String, u32>>,
    function_signatures: Rc<HashMap<String, FunctionSignature>>,
    function_bodies: Rc<HashMap<String, FunctionInlineBody>>,
    native_names: Rc<HashMap<String, u32>>,
    global_names: Rc<HashMap<String, u32>>,
    /// Top-level `let` names visible to callables: user-data globals, not
    /// module objects — method calls on them dispatch as methods.
    user_let_globals: Rc<HashSet<String>>,
    /// Every top-level name bound to user data (`let` / `const` / `:=`), plus
    /// whatever the host declares as data (the REPL's own bindings). Used only
    /// to tell a value apart from an imported module object at a method call;
    /// unlike [`Self::user_let_globals`] it does not affect register caching.
    top_level_data_globals: Rc<HashSet<String>>,
    capture_names: HashMap<String, u16>,
    capture_cells: HashSet<String>,
    cell_locals: HashSet<String>,
    /// Registers proven to hold a machine integer, and of which width.
    ///
    /// The compiler has no access to the type checker, so it learns this the
    /// only two ways a machine int can enter a register: an annotated `let`,
    /// and an `as` cast. That is enough for the code this exists to serve —
    /// driver-ish code annotates its widths — and anything it cannot prove
    /// simply does not get the wrap, which the type checker has already
    /// rejected by then.
    pub(super) machine_regs: HashMap<u16, crate::val::IntKind>,
    /// Top-level functions that declare a machine-int return, by name. Collected
    /// once so a `let` bound to a call can learn its width — see
    /// [`Compiler::initializer_machine_width`].
    function_machine_returns: Rc<HashMap<String, crate::val::IntKind>>,
    /// Machine-int widths of the names this closure captured, learned from the
    /// enclosing scope at the moment the closure was built. A capture is read
    /// through `LoadCapture` into a fresh register, which carries nothing.
    capture_machine_widths: HashMap<String, crate::val::IntKind>,
    /// Machine-int widths of top-level bindings, by name — see
    /// [`support::collect_top_level_machine_widths`].
    global_machine_widths: Rc<HashMap<String, crate::val::IntKind>>,
    /// Machine-int field widths, by struct name then field name — see
    /// [`support::collect_struct_field_machine_widths`].
    struct_field_machine_widths: Rc<HashMap<String, HashMap<String, crate::val::IntKind>>>,
    /// Method names any `impl` in this program declares — see
    /// [`support::collect_impl_method_names`]. A call to one of these is never
    /// lowered to a builtin opcode.
    impl_method_names: Rc<HashSet<String>>,
    /// Which struct a local is known to hold, learned from a struct-literal
    /// initializer or a declared type. The compiler tracks no other types; this
    /// exists only to give `r.field` a width to wrap to.
    local_struct_types: HashMap<String, String>,
    /// Loop-pattern variables of the enclosing `for` loops: the fused loop
    /// opcodes own the raw register, so a capture takes a fresh snapshot cell
    /// per capture site instead of re-binding the register (per-iteration
    /// binding semantics). `slot` is filled when the pattern binds; a
    /// same-named local whose binding differs (a fresh `let` in the body) is
    /// an ordinary local, not the loop variable.
    loop_snapshot_vars: Vec<LoopSnapshotVar>,
    dynamic_function_base: u32,
    pending_functions: Vec<Function>,
    /// `trait`/`impl` declarations lowered so far, kept structured instead of
    /// only being serialized into registration-call string literals. Filled by
    /// `lower_trait_decl` / `lower_impl_decl`; the module compiler moves the
    /// entry compiler's copy into `Module::type_info`.
    type_info: crate::vm::TypeInfo,
    inline_stack: Vec<String>,
    loops: Vec<LoopPatch>,
    loop_const_scopes: Vec<HashMap<ScalarLoopConstKey, u16>>,
    single_char_string_locals: HashMap<String, u16>,
    const_map_locals: HashMap<String, FastHashMap<RuntimeMapKey, ConstRuntimeValue>>,
    local_rebind_suppression: u16,
    /// The `let` binding whose initializer is being lowered, if any.
    ///
    /// Only a diagnostic: a binding is not in scope inside its own initializer,
    /// so `let fact = |n| … fact(n - 1) …;` cannot resolve `fact` — and the
    /// report was "Compiler undefined callable `fact`", a sentence about an
    /// operand for a rule about scope. Knowing which binding is being
    /// initialized is what lets the message state the rule.
    initializing_binding: Option<String>,
    top_level: bool,
    /// The one register every top-level `fn` declaration publishes through.
    ///
    /// A declaration is `LoadFunction r; SetGlobal r, slot` — the register is
    /// dead the instant the store lands, so 236 declarations paying 236
    /// registers is 235 more than the work needs. That is not a rounding error:
    /// registers are `u8` in the encoding, so the top level has 256, and
    /// `bare-metal-x86/program.lk` with its drivers bundled in declares 236
    /// functions. It ran out on the *constants* that came afterwards, which is
    /// nowhere near the cause.
    ///
    /// One shared register rather than a recycled one, and the difference
    /// matters. Recycling — handing the register back so anything may use it
    /// next — is wrong here for a reason outside this compiler: the AOT
    /// lowering tracks what a register *means* keyed by `(block, register)`
    /// with no notion of time, so a register that once held a function value
    /// keeps that meaning. A later `SetGlobal` from it is then read as
    /// declaration bookkeeping and elided — a global write silently dropped.
    /// A register that only ever holds a function value being published cannot
    /// have that happen to it, because its meaning never changes.
    // TODO: make the AOT lowering's `builtin_regs` time-aware, and this can go
    // back to being an ordinary watermark like every other statement's.
    fn_publish_reg: Option<u16>,
    emitted_return: bool,
}

/// The most arguments one call can pass, and therefore the most parameters a
/// callable can usefully declare.
///
/// It is the `Call` opcode's positional-count operand — 7 bits — and that is a
/// fact about the encoding, not about the program. Which is exactly why it
/// belongs in *one* named place with the reason written down: it leaked into
/// two different messages as a bare `max 127`, and a third one said `max 255`
/// about the same kind of limit somewhere else.
pub(crate) const MAX_CALL_ARGUMENTS: usize = i8::MAX as usize;

impl Compiler {
    pub(super) fn lower_expr(&mut self, expr: &Expr) -> Result<u16> {
        match expr {
            Expr::Paren(inner) => self.lower_expr(inner),
            Expr::Cast(inner, ty) => self.lower_cast(inner, ty),
            // No runtime cost: the marker exists for the type checker.
            Expr::Unsafe(inner) => self.lower_expr(inner),
            Expr::Literal(value) => self.lower_val(value),
            Expr::Var(name) => self.lower_var(name),
            Expr::List(elements) => self.lower_list(elements),
            Expr::Map(entries) => self.lower_map(entries),
            Expr::StructLiteral { name, fields } => self.lower_struct_literal(name, fields),
            Expr::Access(target, key) => self.lower_access(target, key),
            Expr::Call(name, args) => self.lower_named_call(name, args),
            Expr::CallExpr(callee, args) => self.lower_call_expr(callee, args),
            Expr::CallNamed(callee, positional, named) => self.lower_named_arg_call(callee, positional, named),
            Expr::Closure { params, body, .. } => self.lower_closure(params, body),
            Expr::Unary(op, inner) => self.lower_unary(op, inner),
            Expr::And(lhs, rhs) => self.lower_short_circuit(lhs, rhs, ShortCircuitKind::And),
            Expr::Or(lhs, rhs) => self.lower_short_circuit(lhs, rhs, ShortCircuitKind::Or),
            Expr::NullishCoalescing(lhs, rhs) => self.lower_short_circuit(lhs, rhs, ShortCircuitKind::Nullish),
            Expr::OptionalAccess(target, key) => self.lower_optional_access(target, key),
            Expr::TemplateString(parts) => self.lower_template_string(parts),
            Expr::Block(statements) => self.lower_block_expr(statements),
            Expr::Try {
                body,
                catch_var,
                handler,
            } => self.lower_try_expr(body, catch_var, handler),
            Expr::Range {
                start,
                end,
                inclusive,
                step,
            } => self.lower_range_expr(start.as_deref(), end.as_deref(), *inclusive, step.as_deref()),
            Expr::Match { value, arms } => self.lower_match_expr(value, arms),
            Expr::Bin(lhs, op, rhs) => self.lower_bin(lhs, op, rhs),
            Expr::Conditional(condition, then_expr, else_expr) => {
                self.lower_conditional(condition, then_expr, else_expr)
            } // Every `Expr` variant lowers — parse-time-desugared sugar
              // (try/catch → pcall, select → select$block) never reaches here
              // as a dedicated node.
        }
    }

    /// Remember that `reg` holds a machine integer of `ty`'s width, if it does.
    ///
    /// Anything else clears the note: a register reused for a different value
    /// must not keep an old width, or arithmetic would wrap to a type the
    /// value no longer has.
    /// The machine width a `let`'s initializer produces, when it has one and
    /// nobody wrote it down.
    ///
    /// Machine-int arithmetic wraps to its width, and the wrap is emitted where
    /// the width is *proven*. Proof used to come from exactly two places: an
    /// annotation, and an `as` cast. Everything else was left unproven, on the
    /// grounds that not wrapping is the safe answer — but not wrapping is a
    /// different answer, and the type checker had already decided which one is
    /// right:
    ///
    /// ```lk
    /// fn read() -> u32 { return 4000000000 as u32; }
    /// let a = read();  let b = read();  println(a + b);   // 8000000000
    /// let c: u32 = 4000000000;  let d: u32 = 4000000000;
    /// println(c + d);                                     // 3705032704
    /// ```
    ///
    /// Same types, same values, and the answer turned on whether a width had
    /// been typed out. This closes the three ways it can be known without one:
    /// a call to a function that declares a machine return, a builtin whose
    /// name *is* the width (`volatile_read_u32`, `port_in_u8`), and a read of a
    /// local already known to hold one.
    ///
    /// Deliberately not a general inference pass. Everything it does not
    /// recognize stays unproven and unwrapped, exactly as before — this widens
    /// what can be proven, it does not change what proof means.
    pub(super) fn initializer_machine_width(&self, expr: &Expr) -> Option<crate::val::IntKind> {
        match expr {
            Expr::Paren(inner) => self.initializer_machine_width(inner),
            // A shift or a bitwise operation keeps the width it is given.
            //
            // `let mask = flags << 3;` is a `u32` if `flags` is one, and until
            // this was here it was nothing: the parser desugars `<<` into a call
            // and a call's width came only from a declared return type. That
            // mattered beyond tidiness — `(1u64 << 63) >> 63` could not be told
            // to shift logically, because by the time the `>>` was lowered its
            // operand had no proven width left to consult.
            //
            // The *type checker* deliberately does not do this, and that
            // asymmetry is load-bearing. Teaching it the same rule makes
            // `let top = one << 63; top < one;` type-check — and it is then
            // compiled wrong: comparison and division on the `i64` carrier are
            // signed, so a `u64` with bit 63 set compares as negative and
            // divides as negative. Today the checker calls that expression a
            // width mismatch and refuses it, which is not helpful but is not
            // *wrong*. Closing this properly means unsigned compare, divide and
            // modulo — opcodes, in both backends — and the checker's half is the
            // last piece of that, not the first.
            Expr::Call(name, args)
                if matches!(
                    name.as_str(),
                    "__lk_shl" | "__lk_shr" | "__lk_shr_u" | "__lk_bit_and" | "__lk_bit_or" | "__lk_bit_xor"
                ) && args.len() == 2 =>
            {
                self.expr_machine_width(&args[0])
            }
            Expr::Call(name, args) if name.as_str() == "__lk_bit_not" && args.len() == 1 => {
                self.expr_machine_width(&args[0])
            }
            // Arithmetic keeps the width too, and leaving it out was not merely
            // untidy. `println(top + 5)` printed a negative number where
            // `let big = top + 5; println(big);` printed the right one, because
            // only the second had a *register* to carry the fact. The same hole
            // made `(a + b) >> 1` on a `u64` shift arithmetically — a wrong
            // value, not just a wrong rendering — since the shift asks this
            // question about its left operand and got `None`.
            //
            // The literal is admitted on either side for the reason
            // `lower_unsigned_bin` admits it: the type checker has already
            // measured it against this width, so it is that width.
            Expr::Bin(lhs, op, rhs) if op.is_arith() => {
                match (self.expr_machine_width(lhs), self.expr_machine_width(rhs)) {
                    (Some(left), Some(right)) => (left == right).then_some(left),
                    (Some(left), None) => support::is_int_literal(rhs).then_some(left),
                    (None, Some(right)) => support::is_int_literal(lhs).then_some(right),
                    (None, None) => None,
                }
            }
            // Both shapes, because name resolution rewrites a plain call:
            // `read()` is `Call("read", …)` in the parser's output and
            // `CallExpr(Var("read"), …)` by the time the compiler sees it.
            // Matching only the first is why the first version of this looked
            // correct and changed nothing.
            Expr::Call(name, _) => self.call_machine_width(name),
            Expr::CallExpr(callee, _) => match callee.as_ref() {
                Expr::Var(name) => self.call_machine_width(name),
                _ => None,
            },
            _ => None,
        }
    }

    /// The width a call to `name` produces: a user function that declares one,
    /// or a builtin whose name *is* one.
    fn call_machine_width(&self, name: &str) -> Option<crate::val::IntKind> {
        self.function_machine_returns
            .get(name)
            .copied()
            .or_else(|| crate::typ::builtin_machine_result(name))
    }

    pub(super) fn note_machine_reg(&mut self, reg: u16, ty: Option<&crate::val::Type>) {
        match ty {
            Some(crate::val::Type::MachineInt(kind)) => {
                self.machine_regs.insert(reg, *kind);
            }
            _ => {
                self.machine_regs.remove(&reg);
            }
        }
    }

    /// The machine width an expression is known to produce, without lowering it.
    ///
    /// A local whose slot was recorded, or a call whose declared return type
    /// says so. Deliberately narrow: anything it cannot prove stays unproven,
    /// which everywhere else in this path means "do the ordinary thing".
    pub(in crate::vm::compiler) fn expr_machine_width(&self, expr: &Expr) -> Option<crate::val::IntKind> {
        match expr {
            Expr::Paren(inner) => self.expr_machine_width(inner),
            Expr::Var(name) => self
                .locals
                .get(name)
                .copied()
                .and_then(|reg| self.machine_regs.get(&reg).copied()),
            // `r.value` where `value` is declared `u32`.
            //
            // The register a field lands in has no width of its own — it came
            // out of a container — so without this `r.value + 1` on a `u32`
            // field added at 64 bits and answered 4294967296. Narrow on
            // purpose: only a local whose struct is known, which is the shape a
            // register block is read through.
            Expr::Access(target, key) => self.access_machine_width_of(target, key),
            other => self.initializer_machine_width(other),
        }
    }

    /// Records that `name` holds a struct, from a literal or a declared type.
    pub(in crate::vm::compiler) fn note_local_struct_type(
        &mut self,
        name: &str,
        type_annotation: Option<&crate::val::Type>,
        value: &Expr,
    ) {
        let declared = match type_annotation {
            Some(crate::val::Type::Named(struct_name)) => Some(struct_name.clone()),
            _ => None,
        };
        let from_literal = match value {
            Expr::StructLiteral { name, .. } => Some(name.clone()),
            _ => None,
        };
        match declared.or(from_literal) {
            Some(struct_name) => {
                self.local_struct_types.insert(String::from(name), struct_name);
            }
            // Rebinding the name to something else ends the fact, the same way
            // the width fact ends when a register changes hands.
            None => {
                self.local_struct_types.remove(name);
            }
        }
    }

    /// The declared width of `target.key`, when the compiler knows both.
    pub(in crate::vm::compiler) fn access_machine_width_of(
        &self,
        target: &Expr,
        key: &Expr,
    ) -> Option<crate::val::IntKind> {
        let target = match target {
            Expr::Paren(inner) => inner.as_ref(),
            other => other,
        };
        let Expr::Var(name) = target else {
            return None;
        };
        let field = match key {
            Expr::Var(field) => field.as_str(),
            Expr::Literal(value) => value.as_str()?,
            _ => return None,
        };
        let struct_name = self.local_struct_types.get(name.as_str())?;
        self.struct_field_machine_widths
            .get(struct_name.as_str())?
            .get(field)
            .copied()
    }

    /// A string concatenation's operands, with a carrier-filling one rendered.
    ///
    /// The third display site, and the one the first two made easy to miss.
    /// `println(top)` and `"${top}"` were fixed by choosing the rendering where
    /// the width still exists; `"addr " + top` renders in the *`+`*, which sees
    /// two runtime values and an `i64` carrier, so it printed the negative
    /// number the other two had stopped printing.
    ///
    /// Only when the other side is statically a string: `a + b` on two numbers
    /// is arithmetic, and the result of that is displayed by whoever displays
    /// it — this is about the operator that *is* the rendering.
    pub(in crate::vm::compiler) fn rendered_concat_operands(
        &self,
        lhs: &Expr,
        op: &BinOp,
        rhs: &Expr,
    ) -> Option<(Expr, Expr)> {
        if !matches!(op, BinOp::Add) {
            return None;
        }
        let is_string = |expr: &Expr| expr_static_value_kind(expr) == PerfValueKind::String;
        if is_string(lhs)
            && let Some(rendered) = self.unsigned_rendering_if_carrier_filling(rhs)
        {
            return Some((lhs.clone(), rendered));
        }
        if is_string(rhs)
            && let Some(rendered) = self.unsigned_rendering_if_carrier_filling(lhs)
        {
            return Some((rendered, rhs.clone()));
        }
        None
    }

    /// `__lk_u64_str(expr)` when `expr` is a `u64`/`usize`, otherwise `None`.
    ///
    /// The narrow question the two display sites — a rendering call's arguments
    /// and a template string's parts — both have to ask. Only the widths that
    /// *fill* the carrier: below 64 bits the high bits are zero, so the signed
    /// reading and the unsigned one are the same digits.
    pub(in crate::vm::compiler) fn unsigned_rendering_if_carrier_filling(&self, expr: &Expr) -> Option<Expr> {
        let kind = self.expr_machine_width(expr)?;
        matches!(kind, crate::val::IntKind::U64 | crate::val::IntKind::Usize).then(|| call::unsigned_rendering_of(expr))
    }

    /// The machine width both operands share, if they have one.
    ///
    /// Returns `None` when either side is unproven or the widths differ — the
    /// type checker rejects mixed widths, so a disagreement here means the
    /// compiler simply could not prove it, and the safe answer is not to wrap.
    pub(super) fn shared_machine_width(&self, lhs: u16, rhs: u16) -> Option<crate::val::IntKind> {
        let left = self.machine_regs.get(&lhs).copied()?;
        let right = self.machine_regs.get(&rhs).copied()?;
        (left == right).then_some(left)
    }

    /// Gives an integer literal the machine width of the operand beside it.
    ///
    /// Only a literal, and only when the other side is *proven*: a variable of
    /// another numeric type is a width mistake the type checker rejects, and a
    /// register whose width the compiler could not prove stays unwrapped, which
    /// is the safe answer everywhere else in this path.
    ///
    /// The literal's range was already checked — the checker measured it against
    /// this very width — so the normalisation here cannot lose anything the
    /// program was entitled to.
    pub(in crate::vm::compiler) fn adopt_machine_width_for_literal(
        &mut self,
        lhs: u16,
        rhs: u16,
        lhs_is_literal: bool,
        rhs_is_literal: bool,
    ) -> Result<()> {
        let left = self.machine_regs.get(&lhs).copied();
        let right = self.machine_regs.get(&rhs).copied();
        match (left, right) {
            (Some(kind), None) if rhs_is_literal => {
                self.emit_machine_wrap(rhs, kind)?;
                self.machine_regs.insert(rhs, kind);
            }
            (None, Some(kind)) if lhs_is_literal => {
                self.emit_machine_wrap(lhs, kind)?;
                self.machine_regs.insert(lhs, kind);
            }
            _ => {}
        }
        Ok(())
    }

    /// Normalise `reg` to `kind`'s width in place, reusing the `as` path so the
    /// VM and Cranelift agree by construction rather than by two parallel
    /// implementations of the same masking.
    pub(super) fn emit_machine_wrap(&mut self, reg: u16, kind: crate::val::IntKind) -> Result<()> {
        let Some(target) = super::ir::CastTarget::from_type(&crate::val::Type::MachineInt(kind)) else {
            return Ok(());
        };
        let encoded = checked_u8("wrap reg", reg)?;
        self.emit(Instr::abc(super::ir::Opcode::CastTo, encoded, encoded, target as u8));
        self.machine_regs.insert(reg, kind);
        Ok(())
    }

    pub(super) fn lower_template_string(&mut self, parts: &[TemplateStringPart]) -> Result<u16> {
        let parts = parts
            .iter()
            .filter(|part| !matches!(part, TemplateStringPart::Literal(value) if value.is_empty()))
            .collect::<Vec<_>>();
        if parts.is_empty() {
            return self.lower_val(&LiteralVal::from_str(""));
        }
        let force_single_expr_string = parts.len() == 1;

        // Use ConcatN when we have 3+ parts and they fit in C operand (max 255).
        // 2-part templates still use ConcatString which is well-supported by LLVM lowering.
        // ConcatN A B C: concatenate values r[B]..r[B+C-1] into r[A]
        if parts.len() >= 3 && parts.len() <= 255 {
            let start_reg = self.alloc_reg();
            // Allocate contiguous registers for the remaining parts
            for _ in 1..parts.len() {
                self.alloc_reg();
            }

            // Lower each part into its register
            for (i, part) in parts.iter().enumerate() {
                let target_reg = start_reg + i as u16;
                self.lower_template_string_part_to_register(target_reg, part, force_single_expr_string)?;
            }

            let dst = self.alloc_reg();
            self.emit(Instr::abc(
                Opcode::ConcatN,
                checked_u8("template concatn dst", dst)?,
                checked_u8("template concatn start", start_reg)?,
                checked_u8("template concatn count", parts.len() as u16)?,
            ));
            self.set_register_kind(dst, PerfValueKind::String);
            return Ok(dst);
        }

        // Fallback: chain ConcatString for 1 part or 255+ parts
        let mut acc = None;
        for part in parts {
            let part_reg = self.lower_template_string_part(part, force_single_expr_string)?;
            let Some(lhs) = acc else {
                acc = Some(part_reg);
                continue;
            };
            let dst = self.alloc_reg();
            self.emit(Instr::abc(
                Opcode::ConcatString,
                checked_u8("template concat dst", dst)?,
                checked_u8("template concat lhs", lhs)?,
                checked_u8("template concat rhs", part_reg)?,
            ));
            self.set_register_kind(dst, PerfValueKind::String);
            acc = Some(dst);
        }
        acc.map_or_else(|| self.lower_val(&LiteralVal::from_str("")), Ok)
    }

    pub(super) fn lower_template_string_to_register(&mut self, dst: u16, parts: &[TemplateStringPart]) -> Result<()> {
        let parts = parts
            .iter()
            .filter(|part| !matches!(part, TemplateStringPart::Literal(value) if value.is_empty()))
            .collect::<Vec<_>>();
        if parts.is_empty() {
            self.emit_literal_to_register(dst, &LiteralVal::from_str(""))?;
            return Ok(());
        }
        let force_single_expr_string = parts.len() == 1;

        if parts.len() == 1 {
            self.lower_template_string_part_to_register(dst, parts[0], force_single_expr_string)?;
            self.set_register_kind(dst, PerfValueKind::String);
            return Ok(());
        }

        if parts.len() >= 3 && parts.len() <= 255 {
            let start_reg = self.alloc_reg();
            for _ in 1..parts.len() {
                self.alloc_reg();
            }
            for (index, part) in parts.iter().enumerate() {
                self.lower_template_string_part_to_register(start_reg + index as u16, part, force_single_expr_string)?;
            }
            self.emit(Instr::abc(
                Opcode::ConcatN,
                checked_u8("template concatn dst", dst)?,
                checked_u8("template concatn start", start_reg)?,
                checked_u8("template concatn count", parts.len() as u16)?,
            ));
            self.set_register_kind(dst, PerfValueKind::String);
            return Ok(());
        }

        let mut acc = self.lower_template_string_part(parts[0], force_single_expr_string)?;
        for (index, part) in parts.iter().enumerate().skip(1) {
            let part_reg = self.lower_template_string_part(part, force_single_expr_string)?;
            let concat_dst = if index == parts.len() - 1 {
                dst
            } else {
                self.alloc_reg()
            };
            self.emit(Instr::abc(
                Opcode::ConcatString,
                checked_u8("template concat dst", concat_dst)?,
                checked_u8("template concat lhs", acc)?,
                checked_u8("template concat rhs", part_reg)?,
            ));
            self.set_register_kind(concat_dst, PerfValueKind::String);
            acc = concat_dst;
        }
        Ok(())
    }

    pub(super) fn lower_template_string_part(
        &mut self,
        part: &TemplateStringPart,
        force_expr_string: bool,
    ) -> Result<u16> {
        match part {
            TemplateStringPart::Literal(value) => self.lower_val(&LiteralVal::from_str(value)),
            TemplateStringPart::Expr(expr) => {
                // The other half of the rendering fix in `lower_named_call`: a
                // template part is a display site too, and `"${top}"` was
                // showing the same negative number `println(top)` did.
                let rendered = self.unsigned_rendering_if_carrier_filling(expr);
                let expr = rendered.as_ref().unwrap_or(expr.as_ref());
                let value = self.lower_readonly_operand(expr)?;
                if !force_expr_string || self.function.performance.value_kind(value) == PerfValueKind::String {
                    return Ok(value);
                }
                let dst = self.alloc_reg();
                self.emit(Instr::abc(
                    Opcode::ToString,
                    checked_u8("template string dst", dst)?,
                    checked_u8("template string src", value)?,
                    0,
                ));
                self.set_register_kind(dst, PerfValueKind::String);
                Ok(dst)
            }
        }
    }

    pub(super) fn lower_template_string_part_to_register(
        &mut self,
        dst: u16,
        part: &TemplateStringPart,
        force_expr_string: bool,
    ) -> Result<()> {
        match part {
            TemplateStringPart::Literal(value) => self.emit_literal_to_register(dst, &LiteralVal::from_str(value)),
            TemplateStringPart::Expr(expr) => {
                // Before the `force_expr_string` split, not after: with several
                // parts the flag is *off* because `Concat` stringifies at
                // runtime — which is exactly where the width is already gone.
                let rendered = self.unsigned_rendering_if_carrier_filling(expr);
                let expr = rendered.as_ref().unwrap_or(expr.as_ref());
                if !force_expr_string {
                    return self.lower_expr_to_register(dst, expr, "template part");
                }
                let value = self.lower_readonly_operand(expr)?;
                if self.function.performance.value_kind(value) == PerfValueKind::String {
                    let move_source = !self.is_current_local_slot(value);
                    return self.emit_move_with_policy(dst, value, "template string part", move_source);
                }
                self.emit(Instr::abc(
                    Opcode::ToString,
                    checked_u8("template string dst", dst)?,
                    checked_u8("template string src", value)?,
                    0,
                ));
                self.set_register_kind(dst, PerfValueKind::String);
                Ok(())
            }
        }
    }

    pub(super) fn lower_block_expr(&mut self, statements: &[Box<Stmt>]) -> Result<u16> {
        // A block expression is a scope, like the statement form. Without the
        // restore a `let` inside one rebound the name for good — `match x { 1
        // => { let u = 5; } }` left `u` at 5 afterwards, and a match arm body
        // *is* a block expression.
        //
        // The registers are deliberately not rolled back: the block's value
        // lives in one of them, and the caller has not read it yet.
        let saved_locals = self.locals.clone();
        let saved_cell_locals = self.cell_locals.clone();
        let saved_const_maps = self.const_map_locals.clone();
        let saved_scopes = self.enter_scope();
        let result = self.lower_block_expr_inner(statements);
        self.cell_locals = self.scope_restored_cell_locals(&saved_locals, saved_cell_locals);
        self.locals = saved_locals;
        self.const_map_locals = saved_const_maps;
        self.exit_scope(saved_scopes);
        result
    }

    fn lower_block_expr_inner(&mut self, statements: &[Box<Stmt>]) -> Result<u16> {
        let mut last = None;
        for stmt in statements {
            match stmt.as_ref() {
                Stmt::Expr { value: expr, .. } => {
                    last = Some(self.lower_expr(expr)?);
                }
                Stmt::Return { .. } => {
                    self.lower_stmt(stmt)?;
                    let nil = self.alloc_reg();
                    self.emit(Instr::abc(
                        Opcode::LoadNil,
                        checked_u8("block after return", nil)?,
                        0,
                        0,
                    ));
                    return Ok(nil);
                }
                stmt => {
                    self.lower_stmt(stmt)?;
                    if self.emitted_return {
                        let nil = self.alloc_reg();
                        self.emit(Instr::abc(Opcode::LoadNil, checked_u8("block returned", nil)?, 0, 0));
                        return Ok(nil);
                    }
                }
            }
        }
        if let Some(last) = last {
            Ok(last)
        } else {
            let nil = self.alloc_reg();
            self.emit(Instr::abc(Opcode::LoadNil, checked_u8("empty block", nil)?, 0, 0));
            Ok(nil)
        }
    }

    pub(super) fn lower_closure(&mut self, params: &[String], body: &Expr) -> Result<u16> {
        let captures = self.collect_closure_captures(params, body);
        let function_index = self
            .dynamic_function_base
            .checked_add(self.pending_functions.len() as u32)
            .ok_or_else(|| anyhow!("Compiler dynamic function index overflow"))?;
        let capture_base = self.alloc_regs(captures.len())?;
        let mut capture_names = HashMap::new();
        let mut capture_cells = HashSet::new();
        let mut capture_widths = HashMap::new();
        for (index, name) in captures.iter().enumerate() {
            // Asked *before* the capture is lowered, while the name still
            // resolves to the enclosing scope's register.
            if let Some(kind) = self.expr_machine_width(&Expr::Var(name.clone())) {
                capture_widths.insert(name.clone(), kind);
            }
            let (value, is_cell) = self.lower_capture_value(name)?;
            self.emit_move(capture_base + index as u16, value, "closure capture")?;
            capture_names.insert(name.clone(), index as u16);
            if is_cell {
                capture_cells.insert(name.clone());
            }
        }

        let mut compiled = self.compile_closure_function(
            params,
            body,
            capture_names,
            capture_cells,
            capture_widths,
            function_index + 1,
        )?;
        let dst = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::MakeClosure,
            checked_u8("closure dst", dst)?,
            checked_u8("closure function", function_index as u16)?,
            checked_u8("closure capture base", capture_base)?,
        ));
        self.pending_functions.push(compiled.function);
        self.pending_functions.append(&mut compiled.pending_functions);
        Ok(dst)
    }

    pub(in crate::vm::compiler) fn compile_closure_function(
        &self,
        params: &[String],
        body: &Expr,
        capture_names: HashMap<String, u16>,
        capture_cells: HashSet<String>,
        capture_widths: HashMap<String, crate::val::IntKind>,
        dynamic_function_base: u32,
    ) -> Result<CompiledFunction> {
        // Said at the declaration, because that is where the mistake is: a call
        // can pass at most `MAX_CALL_ARGUMENTS`, so a closure with more
        // parameters than that could never be called at all. Reported as a
        // register overflow before this — advice ("split the body") that cannot
        // be followed, about a body that is not the problem.
        if params.len() > MAX_CALL_ARGUMENTS {
            bail!(
                "this closure declares {} parameters, and {MAX_CALL_ARGUMENTS} is the most a call can pass, \
                 so it could never be called. Take a list or a map instead",
                params.len()
            );
        }
        let mut compiler = Self::with_names(
            self.function_names.clone(),
            self.function_signatures.clone(),
            self.function_bodies.clone(),
            self.native_names.clone(),
            self.global_names.clone(),
            false,
        );
        compiler.user_let_globals = self.user_let_globals.clone();
        compiler.top_level_data_globals = self.top_level_data_globals.clone();
        compiler.capture_names = capture_names;
        compiler.capture_cells = capture_cells;
        compiler.capture_machine_widths = capture_widths;
        // The width facts a closure body needs are the enclosing compiler's, and
        // none of them were being inherited: a closure reading a top-level
        // `const MASK: u32` computed at 64 bits for the same reason a function
        // body did.
        compiler.function_machine_returns = self.function_machine_returns.clone();
        compiler.struct_field_machine_widths = self.struct_field_machine_widths.clone();
        compiler.global_machine_widths = self.global_machine_widths.clone();
        compiler.dynamic_function_base = dynamic_function_base;
        // Inherited so a self-call inside the body can be recognised: the body
        // is a compiler of its own, and the binding being initialized is a fact
        // about the enclosing `let`.
        compiler.initializing_binding = self.initializing_binding.clone();
        compiler.function.param_count = params.len() as u16;
        compiler.function.positional_param_count = params.len() as u16;
        compiler.function.param_names = Vec::with_capacity(params.len());
        for name in params {
            compiler.function.param_names.push(Arc::<str>::from(name.as_str()));
        }
        compiler.function.capture_count = compiler.capture_names.len() as u16;
        compiler.next_reg = params.len() as u16;
        compiler.peak_reg = params.len() as u16;
        for (index, param) in params.iter().enumerate() {
            compiler.insert_local(param.clone(), index as u16);
        }
        match body {
            Expr::Block(statements) => {
                for stmt in statements {
                    compiler.lower_stmt(stmt)?;
                    if compiler.emitted_return {
                        break;
                    }
                }
                if !compiler.emitted_return {
                    let nil = compiler.alloc_reg();
                    compiler.emit(Instr::abc(Opcode::LoadNil, checked_u8("dst", nil)?, 0, 0));
                    compiler.emit_return(nil)?;
                }
            }
            body => {
                let value = compiler.lower_expr(body)?;
                compiler.emit_return(value)?;
            }
        }
        Ok(CompiledFunction {
            function: compiler.finish()?,
            pending_functions: compiler.pending_functions,
        })
    }

    pub(super) fn lower_conditional(&mut self, condition: &Expr, then_expr: &Expr, else_expr: &Expr) -> Result<u16> {
        let dst = self.alloc_reg();
        let false_jumps = self.emit_condition_false_jumps(condition)?;

        self.lower_expr_to_register(dst, then_expr, "conditional then")?;
        let jmp_end = self.emit_jmp_placeholder();

        let else_start = self.function.code.len();
        self.patch_condition_false_jumps(false_jumps, else_start)?;
        self.lower_expr_to_register(dst, else_expr, "conditional else")?;

        let end = self.function.code.len();
        self.patch_jmp(jmp_end, end)?;
        Ok(dst)
    }

    pub(super) fn materialize_list(&mut self, values: Vec<u16>) -> Result<u16> {
        let len = values.len();
        if len > u8::MAX as usize {
            // Not a list literal, whatever the old message said: this packs an
            // argument list for the `__lk_call_method` helper, and the values
            // are already in registers — so unlike `lower_list` there is no
            // build-empty-and-push route available here, because 256 live
            // argument registers have already overflowed the same operand.
            bail!(
                "this method call packs {} arguments, and {} is the most it can: the helper receives them in \
                 one register window, and a window is addressed in 8 bits. Pass a list instead",
                len,
                u8::MAX
            );
        }

        let base = self.alloc_regs(len)?;
        for (offset, value) in values.into_iter().enumerate() {
            let move_source = !self.is_current_local_slot(value);
            self.emit_move_with_policy(base + offset as u16, value, "list element", move_source)?;
        }

        let dst = self.alloc_reg();
        let pc = self.function.code.len();
        self.emit(Instr::abc(
            Opcode::NewList,
            checked_u8("list dst", dst)?,
            checked_u8("list base", base)?,
            checked_u8("list len", len as u16)?,
        ));
        self.function.performance.set_container_build_fact(
            pc,
            PerfContainerBuildFact {
                move_keys: false,
                move_values: true,
            },
        );
        Ok(dst)
    }

    pub(super) fn lower_var(&mut self, name: &str) -> Result<u16> {
        if let Some(src) = self.locals.get(name).copied() {
            if self.cell_locals.contains(name) {
                return self.emit_load_cell_value(src);
            }
            let dst = self.alloc_reg();
            self.emit_move(dst, src, "var")?;
            return Ok(dst);
        }
        if let Some(capture) = self.capture_names.get(name).copied() {
            let cell_or_value = self.emit_load_capture(capture)?;
            let width = self.capture_machine_widths.get(name).copied();
            let dst = if self.capture_cells.contains(name) {
                self.emit_load_cell_value(cell_or_value)?
            } else {
                cell_or_value
            };
            if let Some(kind) = width {
                self.machine_regs.insert(dst, kind);
            }
            return Ok(dst);
        }
        if let Some(slot) = self.global_names.get(name).copied() {
            return self.emit_get_global_named(slot, Some(name));
        }
        Err(anyhow!("undefined name `{name}`{}", self.suggest_known_name(name)))
    }

    /// What the writer probably meant, as a trailing ` — did you mean …` or the
    /// empty string.
    ///
    /// The most common mistake in any language used to report `Compiler
    /// undefined local/global `nope`` — a sentence naming this compiler and two
    /// of its storage classes, for a typo. The reader's question is "what *is*
    /// spelled here", and the names in scope are right here to answer it.
    ///
    /// The measurement is [`crate::typ::edit_distance`], the same one the
    /// unknown-*type* hint uses, with the same budget: one edit for a short
    /// name, two for a longer one, so `nmae` finds `name` and `x` finds
    /// nothing.
    pub(super) fn suggest_known_name(&self, name: &str) -> String {
        let mut candidates: Vec<&str> = self.locals.keys().map(String::as_str).collect();
        candidates.extend(self.global_names.keys().map(String::as_str));
        candidates.extend(self.function_names.keys().map(String::as_str));

        // A case difference first: likeliest mistake, surest answer.
        if let Some(exact) = candidates.iter().find(|candidate| candidate.eq_ignore_ascii_case(name)) {
            return alloc::format!(" — did you mean `{exact}`?");
        }
        let budget = if name.len() <= 4 { 1 } else { 2 };
        let mut best: Option<(usize, &str)> = None;
        for candidate in candidates {
            let distance = crate::typ::edit_distance(name, candidate);
            if distance <= budget && best.is_none_or(|(previous, _)| distance < previous) {
                best = Some((distance, candidate));
            }
        }
        match best {
            Some((_, candidate)) => alloc::format!(" — did you mean `{candidate}`?"),
            None => String::new(),
        }
    }

    pub(super) fn lower_bin(&mut self, lhs: &Expr, op: &BinOp, rhs: &Expr) -> Result<u16> {
        // `"addr " + top` renders the `u64` unsigned — see
        // `rendered_concat_operands`.
        if let Some((lhs, rhs)) = self.rendered_concat_operands(lhs, op, rhs) {
            return self.lower_bin(&lhs, op, &rhs);
        }
        // `u64` compares and divides unsigned.
        //
        // A value with bit 63 set *is* a negative `i64` carrier, so the ordinary
        // opcodes put `1u64 << 63` below 1 and divide it to a negative. One
        // comparison primitive covers all four orderings — `a > b` is `b < a`,
        // and the inclusive forms are those negated — so this rewrite is three
        // builtins rather than six.
        //
        // Rewritten *here*, before anything is lowered, because this is the
        // first place with both the operator and a proven width, and because a
        // call cannot easily be emitted from inside the opcode path.
        if let Some(result) = self.lower_unsigned_bin(lhs, op, rhs)? {
            return Ok(result);
        }
        let static_flavor = numeric_flavor(lhs, op, rhs);
        // Whether each side is written as an integer literal, before the names
        // are shadowed by the registers they lower into.
        let lhs_is_literal = support::is_int_literal(lhs);
        let rhs_is_literal = support::is_int_literal(rhs);
        // `1 + expr`: the immediate form wants the constant on the right, so the
        // commuted attempt must lower `expr` to ask whether its value is a proven
        // `Int`. When the answer is no, **the register it just produced is the
        // operand** — falling through to lower `rhs` a second time left the first
        // lowering's instructions in the stream and ran the expression twice. So
        // `1 + f(x)` called `f` twice, and `return 1 + f(n - 1)` cost 2^n calls:
        // `f(5)` made 63 of them, `f(50)` never finished. The answer stayed right
        // for a pure function, which is how it survived.
        //
        // Lowering `rhs` before `lhs` reorders nothing observable: only an integer
        // literal reaches `commuted_int_immediate_operand`.
        // Where the operands' scratch registers are handed back.
        //
        // A register VM needs one temporary for a chain of `+`, not one per
        // term: the result may be written over the left operand, which is
        // exactly what `x += 1` has always compiled to (`AddIntI r0 r0 …`). It
        // did not, and a 300-term chain — or 27 list elements each holding a
        // comparison — hit the 256-register ceiling and the program was
        // refused. Locals live below `live_register_floor()`, so nothing that
        // outlives the expression can be reused here.
        let watermark = self.next_reg;
        let mut commuted_rhs = None;
        if static_flavor == NumericFlavor::Int
            && let Some(immediate) = support::commuted_int_immediate_operand(op, lhs)
        {
            let rhs = self.lower_readonly_operand(rhs)?;
            // Not for a machine integer: the immediate form skips the width
            // normalisation below, so `1 + reg` would run at 64 bits while the
            // type says otherwise.
            if self.function.performance.value_kind(rhs) == PerfValueKind::Int && !self.machine_regs.contains_key(&rhs)
            {
                self.next_reg = self.live_register_floor().max(watermark);
                let dst = self.alloc_reg();
                return self.emit_int_immediate_to_register(dst, op, rhs, immediate);
            }
            commuted_rhs = Some(rhs);
        }
        let lhs = self.lower_readonly_operand(lhs)?;
        if commuted_rhs.is_none()
            && let Some(immediate) = int_immediate_operand(op, rhs)
            && !self.machine_regs.contains_key(&lhs)
        {
            let flavor = if self.function.performance.value_kind(lhs) == PerfValueKind::Int {
                Some(NumericFlavor::Int)
            } else {
                None
            };
            // The destination is named only once this path is taken: it used to
            // be allocated first, so an attempt that fell through left a
            // register nobody would ever write.
            if flavor == Some(static_flavor) {
                self.next_reg = self.live_register_floor().max(watermark);
                let dst = self.alloc_reg();
                return self.emit_int_immediate_to_register(dst, op, lhs, immediate);
            }
        }
        let rhs = match commuted_rhs {
            Some(reg) => reg,
            None => self.lower_readonly_operand(rhs)?,
        };
        // A literal beside a machine integer takes its width, so the wrap below
        // has two proven operands to agree about.
        self.adopt_machine_width_for_literal(lhs, rhs, lhs_is_literal, rhs_is_literal)?;
        // Both facts about the operands are read **before** the destination is
        // named, because naming it may take one of their registers back — and
        // `alloc_reg` ends a register's facts, which is what makes the reuse
        // safe in the first place. Read after, `a + 1` in `fn f(a: u8)` lost the
        // width it had just proven and answered 256.
        let flavor =
            numeric_flavor_from_register_facts(&self.function.performance, op, lhs, rhs).unwrap_or(static_flavor);
        let machine_width = binary_machine_width(op)
            .then(|| self.shared_machine_width(lhs, rhs))
            .flatten();
        self.next_reg = self.live_register_floor().max(watermark);
        let dst = self.alloc_reg();
        self.emit_bin_op_with_width(dst, op, lhs, rhs, flavor, machine_width)
    }

    /// The unsigned form of an operator, when both operands fill the carrier.
    ///
    /// `u64` and `usize` only: for every narrower width the high bits are zero,
    /// so the signed opcode has no sign to misread and is both correct and
    /// faster. Equality is not here either — bit equality is the same question
    /// in both signednesses.
    fn lower_unsigned_bin(&mut self, lhs: &Expr, op: &BinOp, rhs: &Expr) -> Result<Option<u16>> {
        let fills_carrier = |kind| matches!(kind, crate::val::IntKind::U64 | crate::val::IntKind::Usize);
        // One side proven, and the other proven *or a literal*.
        //
        // The literal is the case that matters and the one that was missed: the
        // type checker gives an integer literal the width of the operand beside
        // it, so `top / 2` type-checks as a `u64` division — and then divided
        // *signed*, because this asked for two proven operands and a literal is
        // never proven. Two correct features composing into a wrong answer.
        //
        // A literal is safe here because the checker has already measured it
        // against this width: a value that fits `u64` has the same bits read
        // either way, so the unsigned operation is the right one.
        //
        // One thing in this class is still signed: `println(top)` on a `u64`
        // with bit 63 set shows a negative number.
        //
        // Not done, and the reason is that it is a different kind of change from
        // the five before it. Shift, compare, divide, modulo and `as Float` are
        // all *operators* — the compiler chooses an unsigned form where it has
        // the width. `println` is a variadic stdlib function that receives
        // runtime values, so making it right means rewriting its *arguments* at
        // the call site, and the same would go for every other function a `u64`
        // is handed to. What is wrong there is the display, not the value; the
        // arithmetic above is exact, and printing the halves or the hex is a
        // workaround that computes the right thing.
        let proven_or_literal = |this: &Self, expr: &Expr| {
            this.expr_machine_width(expr).is_some_and(fills_carrier) || support::is_int_literal(expr)
        };
        let left_proven = self.expr_machine_width(lhs).is_some_and(fills_carrier);
        let right_proven = self.expr_machine_width(rhs).is_some_and(fills_carrier);
        if !(left_proven || right_proven) || !proven_or_literal(self, lhs) || !proven_or_literal(self, rhs) {
            return Ok(None);
        }
        let call = |name: &str, a: &Expr, b: &Expr| {
            Expr::Call(
                alloc::string::String::from(name),
                alloc::vec![Box::new(a.clone()), Box::new(b.clone())],
            )
        };
        let rewritten = match op {
            BinOp::Lt => call("__lk_lt_u", lhs, rhs),
            BinOp::Gt => call("__lk_lt_u", rhs, lhs),
            BinOp::Div => call("__lk_div_u", lhs, rhs),
            BinOp::Mod => call("__lk_mod_u", lhs, rhs),
            // `a <= b` is `!(b < a)`, `a >= b` is `!(a < b)`.
            BinOp::Le => Expr::Unary(crate::operator::UnaryOp::Not, Box::new(call("__lk_lt_u", rhs, lhs))),
            BinOp::Ge => Expr::Unary(crate::operator::UnaryOp::Not, Box::new(call("__lk_lt_u", lhs, rhs))),
            _ => return Ok(None),
        };
        self.lower_expr(&rewritten).map(Some)
    }

    /// The same rewrite, for the lower-into-a-given-register path.
    pub(in crate::vm::compiler) fn lower_unsigned_bin_into(
        &mut self,
        dst: u16,
        lhs: &Expr,
        op: &BinOp,
        rhs: &Expr,
    ) -> Result<Option<()>> {
        let Some(src) = self.lower_unsigned_bin(lhs, op, rhs)? else {
            return Ok(None);
        };
        self.emit_move(dst, src, "unsigned bin")?;
        Ok(Some(()))
    }

    pub(super) fn emit_bin_op_to_register(&mut self, dst: u16, op: &BinOp, lhs: u16, rhs: u16) -> Result<u16> {
        let flavor =
            numeric_flavor_from_register_facts(&self.function.performance, op, lhs, rhs).unwrap_or(NumericFlavor::Int);
        self.emit_bin_op_to_register_with_flavor(dst, op, lhs, rhs, flavor)
    }

    /// Every binary-arithmetic lowering path converges here — `lower_bin`, the
    /// lower-into-register fast path, compound assignment — which is why the
    /// machine-int wrap lives at this point rather than at any one caller.
    pub(in crate::vm::compiler) fn emit_bin_op_to_register_with_flavor(
        &mut self,
        dst: u16,
        op: &BinOp,
        lhs: u16,
        rhs: u16,
        flavor: NumericFlavor,
    ) -> Result<u16> {
        let machine_width = binary_machine_width(op)
            .then(|| self.shared_machine_width(lhs, rhs))
            .flatten();
        self.emit_bin_op_with_width(dst, op, lhs, rhs, flavor, machine_width)
    }

    /// [`emit_bin_op_to_register_with_flavor`] with the operands' shared width
    /// already read — for the caller that reuses an operand's register as the
    /// destination and so must ask before it does.
    pub(in crate::vm::compiler) fn emit_bin_op_with_width(
        &mut self,
        dst: u16,
        op: &BinOp,
        lhs: u16,
        rhs: u16,
        flavor: NumericFlavor,
        machine_width: Option<crate::val::IntKind>,
    ) -> Result<u16> {
        let dst = self.emit_bin_op_unwrapped(dst, op, lhs, rhs, flavor)?;
        // Machine-int arithmetic wraps to its width. The operation itself runs
        // at 64 bits and is normalised afterwards, reusing the `as` path: two
        // hand-written maskings (one per backend) would be two places to
        // disagree, and that kind of divergence is invisible without the
        // differential tests.
        if let Some(kind) = machine_width {
            self.emit_machine_wrap(dst, kind)?;
        } else {
            // The result is not a machine int; a register reused here must not
            // keep a stale width.
            self.machine_regs.remove(&dst);
        }
        Ok(dst)
    }

    fn emit_bin_op_unwrapped(
        &mut self,
        dst: u16,
        op: &BinOp,
        lhs: u16,
        rhs: u16,
        flavor: NumericFlavor,
    ) -> Result<u16> {
        let opcode = match op {
            BinOp::Add => match flavor {
                NumericFlavor::Int => Opcode::AddInt,
                NumericFlavor::Float => Opcode::AddFloat,
            },
            BinOp::Sub => match flavor {
                NumericFlavor::Int => Opcode::SubInt,
                NumericFlavor::Float => Opcode::SubFloat,
            },
            BinOp::Mul => match flavor {
                NumericFlavor::Int => Opcode::MulInt,
                NumericFlavor::Float => Opcode::MulFloat,
            },
            BinOp::Div => match flavor {
                NumericFlavor::Int => Opcode::DivInt,
                NumericFlavor::Float => Opcode::DivFloat,
            },
            BinOp::Mod => match flavor {
                NumericFlavor::Int => Opcode::ModInt,
                NumericFlavor::Float => Opcode::ModFloat,
            },
            BinOp::Eq => Opcode::CmpInt,
            BinOp::Ne => Opcode::CmpNeInt,
            BinOp::Lt => Opcode::CmpLtInt,
            BinOp::Le => Opcode::CmpLeInt,
            BinOp::Gt => Opcode::CmpGtInt,
            BinOp::Ge => Opcode::CmpGeInt,
            BinOp::In => Opcode::Contains,
        };
        self.emit(Instr::abc(
            opcode,
            checked_u8("dst", dst)?,
            checked_u8("lhs", lhs)?,
            checked_u8("rhs", rhs)?,
        ));
        self.set_register_kind(dst, bin_op_result_kind(op, flavor));
        Ok(dst)
    }

    pub(super) fn emit_int_immediate_to_register(
        &mut self,
        dst: u16,
        op: &BinOp,
        lhs: u16,
        immediate: i8,
    ) -> Result<u16> {
        let opcode = match op {
            BinOp::Add | BinOp::Sub => Opcode::AddIntI,
            BinOp::Mul => Opcode::MulIntI,
            BinOp::Mod => Opcode::ModIntI,
            _ => unreachable!("int immediate operand only supports arithmetic ops"),
        };
        self.emit(Instr::abc(
            opcode,
            checked_u8("dst", dst)?,
            checked_u8("lhs", lhs)?,
            immediate as u8,
        ));
        self.set_register_kind(dst, PerfValueKind::Int);
        Ok(dst)
    }
}

/// Whether an operator can *produce* a machine integer, and so needs its result
/// wrapped to the width.
///
/// A comparison of two `u8`s is a `Bool`, and running it through the width path
/// both emitted a pointless `CastTo` on a 0/1 and recorded the destination
/// register as holding a `u8` — a stale width fact that a later, unrelated value
/// in the same register would inherit.
fn binary_machine_width(op: &BinOp) -> bool {
    matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod)
}

fn impl_method_type(
    target_type: &Type,
    params: &[String],
    param_types: &[Option<Type>],
    named_params: &[crate::stmt::NamedParamDecl],
    return_type: &Option<Type>,
) -> Type {
    let params = params
        .iter()
        .enumerate()
        .map(|(index, name)| {
            param_types
                .get(index)
                .and_then(Clone::clone)
                .unwrap_or_else(|| if name == "self" { target_type.clone() } else { Type::Any })
        })
        .collect();
    let named_params = named_params
        .iter()
        .map(|param| FunctionNamedParamType {
            name: param.name.clone(),
            ty: param.type_annotation.clone().unwrap_or(Type::Any),
            has_default: param.default.is_some(),
        })
        .collect();
    Type::Function {
        params,
        named_params,
        return_type: Box::new(return_type.clone().unwrap_or(Type::Any)),
    }
}

fn expr_is_nil_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) => expr_is_nil_literal(inner),
        Expr::Literal(LiteralVal::Nil) => true,
        _ => false,
    }
}

fn get_field_key(
    index_fact: Option<crate::vm::analysis::PerfIndexFact>,
    key_fact: Option<crate::vm::analysis::PerfKeyFact>,
) -> Option<u16> {
    let index_fact = index_fact?;
    if !matches!(
        index_fact.target_kind,
        crate::vm::analysis::PerfIndexTargetKind::Map | crate::vm::analysis::PerfIndexTargetKind::Object
    ) {
        return None;
    }
    let key = key_fact?.const_key?;
    (key <= u8::MAX as u16).then_some(key)
}

fn string_int_template_key(expr: &Expr) -> Option<(&str, &Expr)> {
    let Expr::TemplateString(parts) = strip_expr_parens(expr) else {
        return None;
    };
    let parts = parts
        .iter()
        .filter(|part| !matches!(part, TemplateStringPart::Literal(value) if value.is_empty()))
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [TemplateStringPart::Expr(expr)] => Some(("", strip_expr_parens(expr))),
        [TemplateStringPart::Literal(prefix), TemplateStringPart::Expr(expr)] => {
            Some((prefix.as_str(), strip_expr_parens(expr)))
        }
        _ => None,
    }
}

/// Whether a `"…${suffix}"` map key's suffix is a proven `Int`.
///
/// It gates a speculative lowering: `try_lower_string_int_key_for_map` lowers the
/// suffix and *then* checks a register fact, so an answer of `None` after that
/// point leaves the emitted instructions in the stream and the caller lowers the
/// key again — the operand would run twice. What keeps that unreachable is the
/// shape list below: a literal, a name, and arithmetic over them are all free to
/// lower twice (`Compiler::is_free_to_lower_twice` says the same thing for the
/// fused branch forms, which had exactly this bug).
///
/// So widening this — accepting, say, a call annotated `-> Int` — would
/// reintroduce it. Whatever is added here has to be free to lower twice as well,
/// or the lowering has to stop deciding after it emits.
fn string_int_key_suffix_is_int_like(
    expr: &Expr,
    locals: &crate::compat::collections::HashMap<String, u16>,
    facts: &crate::vm::analysis::PerformanceFacts,
) -> bool {
    match strip_expr_parens(expr) {
        Expr::Literal(LiteralVal::Int(_)) => true,
        Expr::Var(name) => locals
            .get(name)
            .is_some_and(|reg| facts.value_kind(*reg) == PerfValueKind::Int),
        Expr::Bin(lhs, op, rhs) if op.is_arith() => {
            string_int_key_suffix_is_int_like(lhs, locals, facts)
                && string_int_key_suffix_is_int_like(rhs, locals, facts)
        }
        _ => false,
    }
}

fn strip_expr_parens(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(inner) => strip_expr_parens(inner),
        other => other,
    }
}

fn list_int_key(
    index_fact: Option<crate::vm::analysis::PerfIndexFact>,
    facts: &crate::vm::analysis::PerformanceFacts,
    key: u16,
) -> bool {
    ENABLE_GET_LIST_LOWERING
        && index_fact.is_some_and(|fact| fact.target_kind == crate::vm::analysis::PerfIndexTargetKind::List)
        && facts.value_kind(key) == crate::vm::analysis::PerfValueKind::Int
}

fn default_assign_candidate(stmt: &Stmt) -> Option<(&str, &Expr, bool)> {
    match stmt {
        Stmt::Let {
            pattern: Pattern::Variable(name),
            value,
            is_const,
            ..
        } if !*is_const => Some((name.as_str(), value, true)),
        Stmt::Assign { name, value, .. } => Some((name.as_str(), value, false)),
        _ => None,
    }
}

fn pure_default_expr(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Literal(_) | Expr::Var(_))
}

fn expr_mentions_name(expr: &Expr, name: &str) -> bool {
    let mut free = Vec::new();
    collect_expr_free_vars(expr, &mut HashSet::new(), &mut free);
    free.iter().any(|candidate| candidate == name)
}

fn if_chain_assigns_only_target(stmt: &Stmt, name: &str) -> bool {
    let Stmt::If {
        then_stmt, else_stmt, ..
    } = stmt
    else {
        return false;
    };
    let Some((assigned, value)) = single_assign_stmt(then_stmt) else {
        return false;
    };
    if assigned != name || expr_mentions_name(value, name) {
        return false;
    }
    match else_stmt.as_deref() {
        None => true,
        Some(nested @ Stmt::If { .. }) => if_chain_assigns_only_target(nested, name),
        Some(_) => false,
    }
}

fn if_chain_condition_mentions_name(stmt: &Stmt, name: &str) -> bool {
    let Stmt::If {
        condition, else_stmt, ..
    } = stmt
    else {
        return false;
    };
    expr_mentions_name(condition, name)
        || else_stmt
            .as_deref()
            .is_some_and(|nested| if_chain_condition_mentions_name(nested, name))
}

fn single_assign_stmt(stmt: &Stmt) -> Option<(&str, &Expr)> {
    match stmt {
        Stmt::Assign { name, value, .. } => Some((name.as_str(), value)),
        Stmt::Block { statements } if statements.len() == 1 => single_assign_stmt(&statements[0]),
        _ => None,
    }
}

fn min_max_update_opcode(condition: &Expr, assigned_name: &str, value: &Expr) -> Option<Opcode> {
    let Expr::Bin(lhs, op, rhs) = strip_parens(condition) else {
        return None;
    };
    let value_name = local_expr_name(value)?;
    match op {
        BinOp::Lt if local_expr_name(lhs)? == value_name && local_expr_name(rhs)? == assigned_name => {
            Some(Opcode::MinInt)
        }
        BinOp::Gt if local_expr_name(lhs)? == value_name && local_expr_name(rhs)? == assigned_name => {
            Some(Opcode::MaxInt)
        }
        BinOp::Gt if local_expr_name(rhs)? == value_name && local_expr_name(lhs)? == assigned_name => {
            Some(Opcode::MinInt)
        }
        BinOp::Lt if local_expr_name(rhs)? == value_name && local_expr_name(lhs)? == assigned_name => {
            Some(Opcode::MaxInt)
        }
        _ => None,
    }
}

fn local_expr_name(expr: &Expr) -> Option<&str> {
    match strip_parens(expr) {
        Expr::Var(name) => Some(name.as_str()),
        _ => None,
    }
}

fn strip_parens(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(inner) => strip_parens(inner),
        _ => expr,
    }
}

const ENABLE_GET_LIST_LOWERING: bool = true;
const ENABLE_COMPARE_TEST_LOWERING: bool = true;
const ENABLE_COMPARE_TEST_IMMEDIATE_LOWERING: bool = true;
const ENABLE_COMPARE_TEST_PAIR_IMMEDIATE_LOWERING: bool = true;

fn compare_test_opcode(op: &BinOp) -> Option<Opcode> {
    match op {
        BinOp::Eq => Some(Opcode::TestEqInt),
        BinOp::Ne => Some(Opcode::TestNeInt),
        BinOp::Lt => Some(Opcode::TestLtInt),
        BinOp::Le => Some(Opcode::TestLeInt),
        BinOp::Gt => Some(Opcode::TestGtInt),
        BinOp::Ge => Some(Opcode::TestGeInt),
        _ => None,
    }
}

fn compare_test_immediate_opcode(op: &BinOp) -> Option<Opcode> {
    match op {
        BinOp::Eq => Some(Opcode::TestEqIntI),
        BinOp::Ne => Some(Opcode::TestNeIntI),
        BinOp::Lt => Some(Opcode::TestLtIntI),
        BinOp::Le => Some(Opcode::TestLeIntI),
        BinOp::Gt => Some(Opcode::TestGtIntI),
        BinOp::Ge => Some(Opcode::TestGeIntI),
        _ => None,
    }
}

fn reverse_compare_test_immediate_opcode(op: &BinOp) -> Option<Opcode> {
    match op {
        BinOp::Eq => Some(Opcode::TestEqIntI),
        BinOp::Ne => Some(Opcode::TestNeIntI),
        BinOp::Lt => Some(Opcode::TestGtIntI),
        BinOp::Le => Some(Opcode::TestGeIntI),
        BinOp::Gt => Some(Opcode::TestLtIntI),
        BinOp::Ge => Some(Opcode::TestLeIntI),
        _ => None,
    }
}

fn compare_test_immediate_operand(expr: &Expr) -> Option<i8> {
    match expr {
        Expr::Paren(inner) => compare_test_immediate_operand(inner),
        Expr::Literal(LiteralVal::Int(value)) => i8::try_from(*value).ok(),
        _ => None,
    }
}

fn zero_int_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) => zero_int_literal(inner),
        Expr::Literal(LiteralVal::Int(0)) => true,
        _ => false,
    }
}

fn equality_u4_local_immediate(expr: &Expr) -> Option<(&str, u8)> {
    let Expr::Bin(lhs, BinOp::Eq, rhs) = expr else {
        return None;
    };
    if let Some(name) = simple_local_expr_name(lhs)
        && let Some(value) = u4_literal(rhs)
    {
        return Some((name, value));
    }
    if let Some(name) = simple_local_expr_name(rhs)
        && let Some(value) = u4_literal(lhs)
    {
        return Some((name, value));
    }
    None
}

fn mod_i4_zero_operands<'a>(lhs: &'a Expr, rhs: &'a Expr) -> Option<(&'a Expr, u8)> {
    if zero_int_literal(rhs)
        && let Some(candidate) = mod_i4_operand(lhs)
    {
        return Some(candidate);
    }
    if zero_int_literal(lhs)
        && let Some(candidate) = mod_i4_operand(rhs)
    {
        return Some(candidate);
    }
    None
}

fn mod_i4_operand(expr: &Expr) -> Option<(&Expr, u8)> {
    let Expr::Bin(lhs, BinOp::Mod, rhs) = strip_parens(expr) else {
        return None;
    };
    let divisor = u4_literal(rhs).filter(|value| *value != 0)?;
    Some((lhs.as_ref(), divisor))
}

fn u4_literal(expr: &Expr) -> Option<u8> {
    match expr {
        Expr::Paren(inner) => u4_literal(inner),
        Expr::Literal(LiteralVal::Int(value)) => u8::try_from(*value).ok().filter(|value| *value < 16),
        _ => None,
    }
}

fn compare_test_operands_are_int(facts: &crate::vm::analysis::PerformanceFacts, lhs: u16, rhs: u16) -> bool {
    facts.value_kind(lhs) == PerfValueKind::Int && facts.value_kind(rhs) == PerfValueKind::Int
}

#[derive(Debug)]
pub(in crate::vm::compiler) struct CompiledFunction {
    function: Function,
    pending_functions: Vec<Function>,
}

pub fn compile_expr(expr: &Expr) -> Result<Function> {
    Compiler::compile_expr(expr)
}

pub fn compile_program(program: &Program) -> Result<Function> {
    Compiler::compile_program(program)
}

pub fn compile_module(program: &Program) -> Result<Module> {
    Compiler::compile_module(program)
}

pub fn compile_module_with_natives(program: &Program, natives: Vec<NativeEntry>) -> Result<Module> {
    Compiler::compile_module_with_natives(program, natives)
}

pub fn compile_source(source: &str) -> Result<Function> {
    Compiler::compile_source(source)
}

/// One active `for`-pattern variable: the binding slot is recorded when the
/// pattern binds (`None` while the loop head — range/iterable expressions —
/// still lowers against the enclosing scope).
#[derive(Debug)]
struct LoopSnapshotVar {
    name: String,
    slot: Option<u16>,
}

/// One name bound by a `for` pattern: the shadowed previous slot (if any) and
/// whether that previous binding carried a capture-cell mark to re-instate.
struct ForPatternBinding {
    name: String,
    slot: Option<u16>,
    was_cell: bool,
}

fn collect_for_pattern_names(pattern: &ForPattern, out: &mut Vec<LoopSnapshotVar>) {
    match pattern {
        ForPattern::Variable(name) => out.push(LoopSnapshotVar {
            name: name.clone(),
            slot: None,
        }),
        ForPattern::Ignore => {}
        ForPattern::Tuple(patterns) => {
            for pattern in patterns {
                collect_for_pattern_names(pattern, out);
            }
        }
        ForPattern::Array { patterns, rest } => {
            for pattern in patterns {
                collect_for_pattern_names(pattern, out);
            }
            if let Some(rest) = rest {
                out.push(LoopSnapshotVar {
                    name: rest.clone(),
                    slot: None,
                });
            }
        }
        ForPattern::Object(entries) => {
            for (_, pattern) in entries {
                collect_for_pattern_names(pattern, out);
            }
        }
    }
}

pub fn compile_source_module(source: &str) -> Result<Module> {
    Compiler::compile_source_module(source)
}

pub fn compile_source_module_with_natives(source: &str, natives: Vec<NativeEntry>) -> Result<Module> {
    Compiler::compile_source_module_with_natives(source, natives)
}
