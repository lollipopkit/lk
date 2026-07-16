use super::*;

/// Why a bytecode artifact cannot (yet) be lowered to MIR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsupported {
    NoEntry,
    EntryHasParams(u16),
    EntryHasCaptures(u16),
    BadInstr {
        pc: usize,
    },
    Opcode {
        pc: usize,
        op: Opcode,
    },
    BadConst {
        pc: usize,
    },
    /// A register (or virtual cell slot) was read with no reaching definition
    /// on any predecessor path.
    UndefinedOperand {
        pc: usize,
        reg: usize,
    },
    /// An empty `[]` literal's guessed element type was contradicted by a
    /// later consumer: retriable — the fixpoint re-lowers with the literal
    /// materialized as a Dyn list (`pc` identifies the `LoadHeapConst`).
    EmptyListGuessWrong {
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
            Unsupported::BadInstr { pc } => format!("undecodable instruction at pc {pc}"),
            Unsupported::Opcode { pc, op } => {
                format!("opcode {op:?} (at pc {pc}) is not natively lowerable yet")
            }
            Unsupported::BadConst { pc } => format!("unsupported constant operand at pc {pc}"),
            Unsupported::UndefinedOperand { pc, reg } => {
                format!("register r{reg} is read at pc {pc} before any definition")
            }
            Unsupported::TypeMismatch { pc } => {
                format!("an operand at pc {pc} has a type outside the natively lowerable subset")
            }
            Unsupported::EmptyListGuessWrong { pcs } => {
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
