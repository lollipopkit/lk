use super::*;

/// Why a bytecode artifact cannot (yet) be lowered to MIR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsupported {
    NoEntry,
    EntryHasParams(u16),
    EntryHasCaptures(u16),
    /// Which function the blocker below is in.
    ///
    /// Every other variant names a `pc`, which is an index into one function's
    /// code — and a program that is a kernel has hundreds. A pc alone sends the
    /// reader looking through every one of them; the name turns the same
    /// diagnostic into a line to open. Wrapped rather than a field on each
    /// variant because the name is known at one place — the failure list, which
    /// already carries the function index — and not at the dozens of places
    /// that construct a blocker.
    In {
        function: String,
        inner: Box<Unsupported>,
    },
    /// A container written to a global slot the lowering had to widen to `Dyn`.
    ///
    /// Boxing a container re-represents it — a `List<i64>` and a `List<Dyn>` are
    /// different memory — so the slot ends up holding a *second* container and
    /// the writer's own register goes on referring to the first. Nothing after
    /// that is wrong in a way anything can see: the program runs and prints a
    /// number. Refusing is what turns it into a fallback.
    ContainerGlobalBoxed {
        pc: usize,
        name: String,
    },
    BadInstr {
        pc: usize,
    },
    Opcode {
        pc: usize,
        op: Opcode,
    },
    /// A **call shape** the lowering does not cover, carrying *why*.
    ///
    /// These sites used to say `Unsupported::Opcode { pc, op: Opcode::Call }`
    /// with the opcode written in by hand, because the helper that refuses does
    /// not have the instruction. The message then named an opcode the program
    /// need not contain: eight of the twelve blockers in the x86 bare-metal
    /// kernel reported `opcode Call (at pc N)` where pc N held a
    /// `CallMethodK`. A reader who went to look found something else there.
    ///
    /// Same shape and same reason as [`Unsupported::TryRegion`]: the answer is
    /// always a specific property of one call, never "calls are unsupported".
    CallShape {
        pc: usize,
        reason: &'static str,
    },
    /// A `try` region whose shape would change meaning if the body were called
    /// instead of run in place. Carries *why*, because "opcode TryBegin is not
    /// natively lowerable" is what this replaces: it named a feature where the
    /// answer is always a specific property of one region.
    TryRegion {
        pc: usize,
        reason: &'static str,
    },
    /// Two bundled modules define the same top-level name. Reported rather than
    /// resolved: the bundle flattens them into one namespace, so one would
    /// silently shadow the other for every nested read.
    BundledNameCollision {
        name: String,
    },
    /// A global read that resolved to nothing the lowering knows: not a
    /// builtin, not a module, not an import binding, not a proven-initialized
    /// scalar. Carries the *name*, because "opcode GetGlobal is not natively
    /// lowerable" sends the reader looking for a missing feature when the
    /// answer is almost always a specific name that did not resolve — a
    /// mistyped import, a function defined in a module that was not bundled, or
    /// a global written on a path the lowering cannot see.
    UnresolvedGlobal {
        pc: usize,
        name: String,
    },
    BadConst {
        pc: usize,
    },
    /// A register (or virtual cell slot) was read with no reaching definition
    /// on any predecessor path.
    ///
    /// `body` names the try body whose poison caused it, when one did — the
    /// register held a value that body wrote in its own frame and did not carry
    /// back. That is the body that needs a cell for it, and *only* that body:
    /// attributing the read to every region in the function gave a cell to
    /// regions whose bodies merely reused the register as a scratch, and the
    /// parent then had to seed a cell from a register it had never defined.
    /// `None` is an ordinary undefined read, which no cell can fix.
    UndefinedOperand {
        pc: usize,
        reg: usize,
        body: Option<u32>,
    },
    /// A register the lowering tracks as a *compile-time reference* (a lambda,
    /// a module object, an argument pack) was read where a runtime value is
    /// required.
    ///
    /// This is not an undefined read, and reporting it as one — "register r1 is
    /// read at pc 2 before any definition" — described the lowering's
    /// bookkeeping instead of the program. `let fs = [|x| x + 1];` says nothing
    /// about registers; what it does is put a closure in a container, which has
    /// no native representation yet.
    ReferenceAsValue {
        pc: usize,
        reg: usize,
        what: &'static str,
    },
    /// An empty `[]` literal's guessed element type was contradicted by a
    /// later consumer: retriable — the fixpoint re-lowers with the literal
    /// materialized as a Dyn list (`pc` identifies the `LoadHeapConst`).
    LiteralElemTypeContradicted {
        pcs: Vec<usize>,
    },
    /// A loop-header phi merged heterogeneous boxable types: retriable —
    /// the fixpoint re-lowers the function with this phi pre-typed `Dyn`
    /// (its body then consumes it through the Dyn arms from the start).
    DynLoopPhi {
        block: usize,
        slot: usize,
    },
    /// An operand had the wrong type for the operation.
    TypeMismatch {
        pc: usize,
    },
    /// The same, from a site that knows both types.
    ///
    /// Worth a second variant rather than fields on the first: `TypeMismatch`
    /// is constructed in ninety-odd places, most of them a `_ =>` arm that has
    /// nothing to say beyond "not this". The handful that *do* know — anything
    /// reading an operand it requires a specific type for — can say it, and
    /// that is the difference between "an operand at pc 88 has a type outside
    /// the natively lowerable subset" and "wanted I64, found Dyn".
    ///
    /// The first of those cost a round of patching every construction site with
    /// a print to find out which one had fired.
    OperandType {
        pc: usize,
        want: &'static str,
        got: &'static str,
    },
    NoReturn,
    /// A branch condition register was not a `Bool` (int-truthiness not yet lowered).
    NonBoolCondition {
        pc: usize,
    },
    /// Two returns disagree on the value type.
    ReturnTypeConflict,
    /// A branch/jump target fell outside the code.
    BadTarget {
        pc: usize,
    },
    /// The lowered module failed `lk_aot_mir::validate` — an edge-case shape
    /// combination (e.g. a Tier 1 hybrid rerun re-lowering a caller against a
    /// now-bridged callee) produced structurally-invalid MIR. Rather than emit
    /// it (codegen would reject it as an internal error), the module is treated
    /// as not-natively-lowerable so the caller falls back to the VM.
    InvalidMir,
}

impl Unsupported {
    /// A user-facing explanation of why the program is not natively lowerable
    /// (yet). Every enum variant maps to one sentence here, so the capability
    /// boundary is testable and documentable (RFC aot-redesign §3.5).
    pub fn reason(&self) -> String {
        match self {
            Unsupported::NoEntry => "the module has no entry function".to_string(),
            Unsupported::EntryHasParams(n) => format!("the entry function takes {n} parameter(s)"),
            Unsupported::EntryHasCaptures(n) => format!("the entry function captures {n} value(s)"),
            Unsupported::In { function, inner } => format!("in `{function}`: {inner}"),
            Unsupported::ContainerGlobalBoxed { pc, name } => format!(
                "the container written to global `{name}` (at pc {pc}) would have to be boxed, \
                 which copies it — the slot and the writer would stop being the same container"
            ),
            Unsupported::BadInstr { pc } => format!("undecodable instruction at pc {pc}"),
            Unsupported::Opcode { pc, op } => {
                format!("opcode {op:?} (at pc {pc}) is not natively lowerable yet")
            }
            Unsupported::CallShape { pc, reason } => {
                format!("the call at pc {pc} is not natively lowerable: {reason}")
            }
            Unsupported::TryRegion { pc, reason } => {
                format!("the try region at pc {pc} cannot be outlined: {reason}")
            }
            Unsupported::BundledNameCollision { name } => format!(
                "two bundled modules both define `{name}`. Bundling flattens them into one namespace, \
                 so one would silently shadow the other — rename one of them"
            ),
            Unsupported::UnresolvedGlobal { pc, name } => {
                format!("global `{name}` (read at pc {pc}) does not resolve to anything natively lowerable")
            }
            Unsupported::BadConst { pc } => format!("unsupported constant operand at pc {pc}"),
            Unsupported::UndefinedOperand { pc, reg, .. } => {
                format!("register r{reg} is read at pc {pc} before any definition")
            }
            Unsupported::ReferenceAsValue { pc, reg, what } => format!(
                "the {what} in r{reg} at pc {pc} is a compile-time reference, not a runtime value \
                 — storing one in a container, or otherwise using it where a value is required, \
                 has no native form yet"
            ),
            Unsupported::OperandType { pc, want, got } => {
                format!("an operand at pc {pc} is a {got} where a {want} is required")
            }
            Unsupported::TypeMismatch { pc } => {
                format!("an operand at pc {pc} has a type outside the natively lowerable subset")
            }
            Unsupported::LiteralElemTypeContradicted { pcs } => {
                format!("empty list literal(s) at pc {pcs:?} were mis-guessed (retried as Dyn)")
            }
            Unsupported::DynLoopPhi { block, slot } => {
                format!("a loop-header phi (block {block}, slot {slot}) merges heterogeneous types")
            }
            Unsupported::NoReturn => "the entry function never returns".to_string(),
            Unsupported::NonBoolCondition { pc } => {
                format!("the branch condition at pc {pc} is not a bool")
            }
            Unsupported::ReturnTypeConflict => "returns disagree on the value type".to_string(),
            Unsupported::BadTarget { pc } => format!("a branch at pc {pc} targets an out-of-range pc"),
            Unsupported::InvalidMir => "the lowered module did not pass MIR validation".to_string(),
        }
    }
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason())
    }
}
