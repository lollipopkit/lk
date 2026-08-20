//! Canonical 32-bit VM instruction model for the VM rewrite.
//!
//! Compiler and executor work should target this representation directly;
//! alternate instruction models must not be reintroduced.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::util::value_map::ValueMap;
use alloc::sync::Arc;
use core::fmt::Write as _;
use core::mem::size_of;

use anyhow::{Result, bail};

use crate::{
    val::{RuntimeMapKey, ShortStr},
    vm::analysis::PerformanceFacts,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalSlot {
    pub name: Arc<str>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConstPool {
    pub ints: Vec<i64>,
    pub floats: Vec<f64>,
    /// `Arc<str>`, not `String`: a constant string key inserted into a map
    /// becomes an `Arc<str>` there, and every insert used to allocate a fresh
    /// one and free it when the map died. Sharing the pool's makes it a
    /// refcount bump — `Arc<str>::drop_slow` alone was 4.6% of a map-building
    /// workload. Reads still hand out `&str`.
    pub strings: Vec<Arc<str>>,
    pub heap_values: Vec<ConstHeapValue>,
}

impl ConstPool {
    const MAX_ABX_CONSTS: usize = 1 << 16;

    pub fn push_int(&mut self, value: i64) -> Result<u16> {
        push_const(&mut self.ints, value, "int")
    }

    pub fn push_float(&mut self, value: f64) -> Result<u16> {
        push_const_by(&mut self.floats, value, "float", |a, b| a.to_bits() == b.to_bits())
    }

    pub fn push_string(&mut self, value: impl AsRef<str>) -> Result<u16> {
        push_const(&mut self.strings, Arc::<str>::from(value.as_ref()), "string")
    }

    pub fn push_heap_value(&mut self, value: ConstHeapValue) -> Result<u16> {
        push_const_by(&mut self.heap_values, value, "heap value", const_heap_value_is_same)
    }

    #[inline]
    pub fn int(&self, index: u16) -> Option<i64> {
        self.ints.get(index as usize).copied()
    }

    #[inline]
    pub fn float(&self, index: u16) -> Option<f64> {
        self.floats.get(index as usize).copied()
    }

    #[inline]
    pub fn string(&self, index: u16) -> Option<&str> {
        self.strings.get(index as usize).map(Arc::as_ref)
    }

    /// The pooled string itself, for a caller that is about to *store* it —
    /// a map key. See [`Self::strings`].
    #[inline]
    pub fn shared_string(&self, index: u16) -> Option<&Arc<str>> {
        self.strings.get(index as usize)
    }

    #[inline]
    pub fn heap_value(&self, index: u16) -> Option<&ConstHeapValue> {
        self.heap_values.get(index as usize)
    }
}

fn push_const<T: PartialEq>(values: &mut Vec<T>, value: T, name: &str) -> Result<u16> {
    push_const_by(values, value, name, |existing, value| existing == value)
}

/// Deduplication is *identity*, and for a float that is its bits.
///
/// `PartialEq` is the wrong question here: `-0.0 == 0.0` is true and the two
/// are different values, so whichever literal a file wrote first swallowed
/// every later occurrence of the other. `println(-0.0); println(0.0);` printed
/// `-0` twice, and — because the sign of zero reaches division —
/// `println(1.0 / 0.0)` answered `-inf`. The answer depended on the spelling
/// and position of an unrelated line in the same file.
///
/// Bit identity also merges two NaNs of the same payload, which `==` never did.
fn push_const_by<T>(values: &mut Vec<T>, value: T, name: &str, eq: impl Fn(&T, &T) -> bool) -> Result<u16> {
    if let Some(index) = values.iter().position(|existing| eq(existing, &value)) {
        return Ok(index as u16);
    }
    let index = values.len();
    if index >= ConstPool::MAX_ABX_CONSTS {
        bail!("Instr {name} const pool overflow");
    }
    values.push(value);
    Ok(index as u16)
}

/// [`ConstRuntimeValue`] equality for pooling: identical to the derived one
/// except that floats compare by bits. See [`push_const_by`].
fn const_value_is_same(left: &ConstRuntimeValue, right: &ConstRuntimeValue) -> bool {
    match (left, right) {
        (ConstRuntimeValue::Float(a), ConstRuntimeValue::Float(b)) => a.to_bits() == b.to_bits(),
        (ConstRuntimeValue::Heap(a), ConstRuntimeValue::Heap(b)) => const_heap_value_is_same(a, b),
        _ => left == right,
    }
}

/// [`ConstHeapValue`] equality for pooling. A container constant holds
/// [`ConstRuntimeValue`]s, so the float rule has to reach through it: `[0.0]`
/// and `[-0.0]` were the same pool entry too.
fn const_heap_value_is_same(left: &ConstHeapValue, right: &ConstHeapValue) -> bool {
    match (left, right) {
        (ConstHeapValue::List(a), ConstHeapValue::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| const_value_is_same(a, b))
        }
        (ConstHeapValue::Map(a), ConstHeapValue::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| const_value_is_same(value, other)))
        }
        (ConstHeapValue::UpvalCell(a), ConstHeapValue::UpvalCell(b)) => const_value_is_same(a, b),
        _ => left == right,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConstRuntimeValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    ShortStr(ShortStr),
    Heap(Box<ConstHeapValue>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConstHeapValue {
    LongString(Arc<str>),
    List(Vec<ConstRuntimeValue>),
    /// Insertion-ordered: a map literal's entries reach the heap in the order
    /// they were written, because that is the order the value iterates in
    /// (`util::value_map`).
    Map(ValueMap<RuntimeMapKey, ConstRuntimeValue>),
    UpvalCell(Box<ConstRuntimeValue>),
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstrFormat {
    Abc = 0,
    Abx = 1,
    AsBx = 2,
    Ax = 3,
    Sj = 4,
}

impl InstrFormat {
    #[inline]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Abc),
            1 => Some(Self::Abx),
            2 => Some(Self::AsBx),
            3 => Some(Self::Ax),
            4 => Some(Self::Sj),
            _ => None,
        }
    }
}

/// How [`Opcode::CastTo`] encodes its target type in the `C` byte.
///
/// A cast's target is static, so it rides in the instruction rather than
/// costing a constant-pool load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CastTarget {
    Int = 0,
    Float = 1,
    Bool = 2,
    I8 = 3,
    I16 = 4,
    I32 = 5,
    I64 = 6,
    U8 = 7,
    U16 = 8,
    U32 = 9,
    U64 = 10,
    Isize = 11,
    Usize = 12,
}

impl CastTarget {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Int,
            1 => Self::Float,
            2 => Self::Bool,
            3 => Self::I8,
            4 => Self::I16,
            5 => Self::I32,
            6 => Self::I64,
            7 => Self::U8,
            8 => Self::U16,
            9 => Self::U32,
            10 => Self::U64,
            11 => Self::Isize,
            12 => Self::Usize,
            _ => return None,
        })
    }

    /// The machine-int kind this targets, if any.
    pub fn int_kind(self) -> Option<crate::val::IntKind> {
        use crate::val::IntKind;
        Some(match self {
            Self::I8 => IntKind::I8,
            Self::I16 => IntKind::I16,
            Self::I32 => IntKind::I32,
            Self::I64 => IntKind::I64,
            Self::U8 => IntKind::U8,
            Self::U16 => IntKind::U16,
            Self::U32 => IntKind::U32,
            Self::U64 => IntKind::U64,
            Self::Isize => IntKind::Isize,
            Self::Usize => IntKind::Usize,
            Self::Int | Self::Float | Self::Bool => return None,
        })
    }

    pub fn from_type(ty: &crate::val::Type) -> Option<Self> {
        use crate::val::{IntKind, Type};
        Some(match ty {
            Type::Int => Self::Int,
            Type::Float => Self::Float,
            Type::Bool => Self::Bool,
            Type::MachineInt(kind) => match kind {
                IntKind::I8 => Self::I8,
                IntKind::I16 => Self::I16,
                IntKind::I32 => Self::I32,
                IntKind::I64 => Self::I64,
                IntKind::U8 => Self::U8,
                IntKind::U16 => Self::U16,
                IntKind::U32 => Self::U32,
                IntKind::U64 => Self::U64,
                IntKind::Isize => Self::Isize,
                IntKind::Usize => Self::Usize,
            },
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opcode {
    Nop = 0,
    Move = 1,
    Move2 = 2,
    Return = 3,
    Return0 = 4,
    Return1 = 5,
    LoadNil = 6,
    LoadBool = 7,
    LoadInt = 8,
    LoadFloat = 9,
    LoadString = 10,
    LoadHeapConst = 11,
    AddInt = 12,
    SubInt = 13,
    MulInt = 14,
    DivInt = 15,
    ModInt = 16,
    AddIntI = 17,
    MulIntI = 18,
    ModIntI = 19,
    AddMulInt = 20,
    Add2Int = 21,
    AddListInt = 22,
    SubListInt = 23,
    MinInt = 24,
    MaxInt = 25,
    MidInt = 26,
    AddFloat = 27,
    SubFloat = 28,
    MulFloat = 29,
    DivFloat = 30,
    ModFloat = 31,
    CmpInt = 32,
    CmpNeInt = 33,
    CmpLtInt = 34,
    CmpLeInt = 35,
    CmpGtInt = 36,
    CmpGeInt = 37,
    TestEqInt = 38,
    TestNeInt = 39,
    TestLtInt = 40,
    TestLeInt = 41,
    TestGtInt = 42,
    TestGeInt = 43,
    TestEqIntI = 44,
    TestNeIntI = 45,
    TestLtIntI = 46,
    TestLeIntI = 47,
    TestGtIntI = 48,
    TestGeIntI = 49,
    TestEqIntI2 = 50,
    Test = 51,
    Not = 52,
    IsNil = 53,
    IsList = 54,
    IsMap = 55,
    Jmp = 56,
    BrFalse = 57,
    BrTrue = 58,
    BrNil = 59,
    BrNotNil = 60,
    BrEqZeroInt = 61,
    BrNeZeroInt = 62,
    BrEqIntI4 = 63,
    BrNeIntI4 = 64,
    BrModEqZeroIntI4 = 65,
    BrModNeZeroIntI4 = 66,
    ForLoopI = 67,
    Call = 68,
    CallDirect = 69,
    CallNamed = 70,
    LoadFunction = 71,
    MakeClosure = 72,
    LoadCapture = 73,
    LoadCellVal = 74,
    StoreCellVal = 75,
    GetGlobal = 76,
    SetGlobal = 77,
    NewList = 78,
    NewMap = 79,
    NewRange = 80,
    NewObject = 81,
    GetIndex = 82,
    SetIndex = 83,
    GetIndexStrI = 84,
    SetIndexStrI = 85,
    GetFieldK = 86,
    SetFieldK = 87,
    GetList = 88,
    ListPush = 89,
    Len = 90,
    ToIter = 91,
    Contains = 92,
    SliceFrom = 93,
    MapRest = 94,
    ToString = 95,
    ConcatString = 96,
    ConcatN = 97,
    StringSplit = 98,
    ListJoin = 99,
    Raise = 100,
    TryBegin = 101,
    TryEnd = 102,
    Wide = 103,
    /// Boxing-free method call: `a` = window base (receiver at `a`, args at
    /// `[a+1, a+1+c)`, result written to `a`), `b` = method-name string
    /// constant index, `c` = positional argument count. Replaces the
    /// `GetGlobal __lk_call_method` + `NewList` + `Call` sequence for
    /// positional method calls whose name constant index fits in `b`.
    CallMethodK = 104,
    /// `A = B as <type encoded in C>` — see `CastTarget`.
    ///
    /// One opcode for every conversion rather than one per source/target pair:
    /// the source type is only known at runtime anyway, so a per-pair opcode
    /// would not save the dispatch on it.
    CastTo = 105,
    /// `A = -B`.
    ///
    /// Not `0 - B`: the two differ on floats, where `-0.0` is a value distinct
    /// from `0.0 - 0.0`, and negation is what the writer asked for.
    Neg = 106,
    /// `A = floor(B / C)` on two `Int`s — the fused form of
    /// `math.floor(a / b)`.
    ///
    /// Exists because `/` yields a `Float`, so this idiom is the only way to
    /// write integer division and it would otherwise cost a float divide plus
    /// a native call. Floor, not truncation: `math.floor(-7 / 2)` is `-4`.
    /// Non-`Int` operands divide as `f64` and floor the result, which is what
    /// `math.floor` would have answered.
    FloorDivInt = 107,
}

impl Opcode {
    /// Number of opcode slots available in the current 7-bit encoding.
    pub const COUNT: u8 = 128;

    #[inline]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Nop),
            1 => Some(Self::Move),
            2 => Some(Self::Move2),
            3 => Some(Self::Return),
            4 => Some(Self::Return0),
            5 => Some(Self::Return1),
            6 => Some(Self::LoadNil),
            7 => Some(Self::LoadBool),
            8 => Some(Self::LoadInt),
            9 => Some(Self::LoadFloat),
            10 => Some(Self::LoadString),
            11 => Some(Self::LoadHeapConst),
            12 => Some(Self::AddInt),
            13 => Some(Self::SubInt),
            14 => Some(Self::MulInt),
            15 => Some(Self::DivInt),
            16 => Some(Self::ModInt),
            17 => Some(Self::AddIntI),
            18 => Some(Self::MulIntI),
            19 => Some(Self::ModIntI),
            20 => Some(Self::AddMulInt),
            21 => Some(Self::Add2Int),
            22 => Some(Self::AddListInt),
            23 => Some(Self::SubListInt),
            24 => Some(Self::MinInt),
            25 => Some(Self::MaxInt),
            26 => Some(Self::MidInt),
            27 => Some(Self::AddFloat),
            28 => Some(Self::SubFloat),
            29 => Some(Self::MulFloat),
            30 => Some(Self::DivFloat),
            31 => Some(Self::ModFloat),
            32 => Some(Self::CmpInt),
            33 => Some(Self::CmpNeInt),
            34 => Some(Self::CmpLtInt),
            35 => Some(Self::CmpLeInt),
            36 => Some(Self::CmpGtInt),
            37 => Some(Self::CmpGeInt),
            38 => Some(Self::TestEqInt),
            39 => Some(Self::TestNeInt),
            40 => Some(Self::TestLtInt),
            41 => Some(Self::TestLeInt),
            42 => Some(Self::TestGtInt),
            43 => Some(Self::TestGeInt),
            44 => Some(Self::TestEqIntI),
            45 => Some(Self::TestNeIntI),
            46 => Some(Self::TestLtIntI),
            47 => Some(Self::TestLeIntI),
            48 => Some(Self::TestGtIntI),
            49 => Some(Self::TestGeIntI),
            50 => Some(Self::TestEqIntI2),
            51 => Some(Self::Test),
            52 => Some(Self::Not),
            53 => Some(Self::IsNil),
            54 => Some(Self::IsList),
            55 => Some(Self::IsMap),
            56 => Some(Self::Jmp),
            57 => Some(Self::BrFalse),
            58 => Some(Self::BrTrue),
            59 => Some(Self::BrNil),
            60 => Some(Self::BrNotNil),
            61 => Some(Self::BrEqZeroInt),
            62 => Some(Self::BrNeZeroInt),
            63 => Some(Self::BrEqIntI4),
            64 => Some(Self::BrNeIntI4),
            65 => Some(Self::BrModEqZeroIntI4),
            66 => Some(Self::BrModNeZeroIntI4),
            67 => Some(Self::ForLoopI),
            68 => Some(Self::Call),
            69 => Some(Self::CallDirect),
            70 => Some(Self::CallNamed),
            71 => Some(Self::LoadFunction),
            72 => Some(Self::MakeClosure),
            73 => Some(Self::LoadCapture),
            74 => Some(Self::LoadCellVal),
            75 => Some(Self::StoreCellVal),
            76 => Some(Self::GetGlobal),
            77 => Some(Self::SetGlobal),
            78 => Some(Self::NewList),
            79 => Some(Self::NewMap),
            80 => Some(Self::NewRange),
            81 => Some(Self::NewObject),
            82 => Some(Self::GetIndex),
            83 => Some(Self::SetIndex),
            84 => Some(Self::GetIndexStrI),
            85 => Some(Self::SetIndexStrI),
            86 => Some(Self::GetFieldK),
            87 => Some(Self::SetFieldK),
            88 => Some(Self::GetList),
            89 => Some(Self::ListPush),
            90 => Some(Self::Len),
            91 => Some(Self::ToIter),
            92 => Some(Self::Contains),
            93 => Some(Self::SliceFrom),
            94 => Some(Self::MapRest),
            95 => Some(Self::ToString),
            96 => Some(Self::ConcatString),
            97 => Some(Self::ConcatN),
            98 => Some(Self::StringSplit),
            99 => Some(Self::ListJoin),
            100 => Some(Self::Raise),
            101 => Some(Self::TryBegin),
            102 => Some(Self::TryEnd),
            103 => Some(Self::Wide),
            104 => Some(Self::CallMethodK),
            105 => Some(Self::CastTo),
            106 => Some(Self::Neg),
            107 => Some(Self::FloorDivInt),
            _ => None,
        }
    }

    /// Returns true if this opcode loads an immutable scalar constant and reads
    /// no registers. Container heap constants are excluded because reusing one
    /// handle across loop iterations can change mutation/identity semantics.
    #[inline]
    pub const fn is_scalar_const_load(self) -> bool {
        matches!(
            self,
            Self::LoadNil | Self::LoadBool | Self::LoadInt | Self::LoadFloat | Self::LoadString
        )
    }

    #[inline]
    pub const fn info(self) -> OpcodeInfo {
        OpcodeInfo {
            format: match self {
                Self::Jmp => InstrFormat::Sj,
                Self::Raise
                | Self::GetGlobal
                | Self::SetGlobal
                | Self::LoadInt
                | Self::LoadFloat
                | Self::LoadString
                | Self::LoadHeapConst
                | Self::LoadCapture
                | Self::LoadFunction
                | Self::CallNamed
                | Self::BrEqIntI4
                | Self::BrNeIntI4
                | Self::BrModEqZeroIntI4
                | Self::BrModNeZeroIntI4 => InstrFormat::Abx,
                Self::TryBegin
                | Self::BrFalse
                | Self::BrTrue
                | Self::BrNil
                | Self::BrNotNil
                | Self::BrEqZeroInt
                | Self::BrNeZeroInt => InstrFormat::AsBx,
                Self::TryEnd | Self::Wide => InstrFormat::Ax,
                _ => InstrFormat::Abc,
            },
        }
    }

    #[inline]
    pub const fn is_compare_test(self) -> bool {
        matches!(
            self,
            Self::TestEqInt
                | Self::TestNeInt
                | Self::TestLtInt
                | Self::TestLeInt
                | Self::TestGtInt
                | Self::TestGeInt
                | Self::TestEqIntI
                | Self::TestNeIntI
                | Self::TestLtIntI
                | Self::TestLeIntI
                | Self::TestGtIntI
                | Self::TestGeIntI
                | Self::TestEqIntI2
        )
    }

    #[inline]
    pub const fn is_int_immediate_compare_test(self) -> bool {
        matches!(
            self,
            Self::TestEqIntI
                | Self::TestNeIntI
                | Self::TestLtIntI
                | Self::TestLeIntI
                | Self::TestGtIntI
                | Self::TestGeIntI
        )
    }

    #[inline]
    pub const fn is_return(self) -> bool {
        matches!(self, Self::Return | Self::Return0 | Self::Return1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpcodeInfo {
    pub format: InstrFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instr(u32);

impl Instr {
    const OPCODE_BITS: u32 = 7;
    const OP_SHIFT: u32 = 0;
    const A_SHIFT: u32 = Self::OP_SHIFT + Self::OPCODE_BITS;
    const K_SHIFT: u32 = Self::A_SHIFT + 8;
    const B_SHIFT: u32 = Self::K_SHIFT + 1;
    const C_SHIFT: u32 = Self::B_SHIFT + 8;
    const AX_SHIFT: u32 = Self::A_SHIFT;
    const BX_SHIFT: u32 = Self::K_SHIFT;
    const OP_MASK: u32 = (1 << Self::OPCODE_BITS) - 1;
    const BYTE_MASK: u32 = 0xFF;
    const B_MASK: u32 = 0xFF;
    const C_MASK: u32 = 0xFF;
    const BX_MASK: u32 = 0xFFFF;
    const I4_BRANCH_IMMEDIATE_SHIFT: u32 = 12;
    const I4_BRANCH_OFFSET_MASK: u16 = 0x0FFF;
    const I4_BRANCH_OFFSET_BIAS: i32 = 1 << 11;
    const AX_MASK: u32 = (1 << 25) - 1;
    const SJ_MASK: u32 = Self::AX_MASK;
    const SBX_BIAS: i32 = (Self::BX_MASK as i32) >> 1;
    const SJ_BIAS: i32 = (Self::SJ_MASK as i32) >> 1;

    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }

    pub fn try_from_raw(raw: u32) -> Result<Self> {
        let opcode = ((raw >> Self::OP_SHIFT) & Self::OP_MASK) as u8;
        if Opcode::from_bits(opcode).is_none() {
            bail!("invalid Instr opcode bits: {opcode}");
        }
        Ok(Self(raw))
    }

    #[inline]
    pub fn opcode(self) -> Opcode {
        Opcode::from_bits(((self.0 >> Self::OP_SHIFT) & Self::OP_MASK) as u8)
            .expect("Instr opcode is validated at construction")
    }

    #[inline]
    pub fn format(self) -> InstrFormat {
        self.opcode().info().format
    }

    #[inline]
    pub const fn abc(op: Opcode, a: u8, b: u8, c: u8) -> Self {
        Self(
            ((op as u32) << Self::OP_SHIFT)
                | ((a as u32) << Self::A_SHIFT)
                | (((b as u32) & Self::B_MASK) << Self::B_SHIFT)
                | (((c as u32) & Self::C_MASK) << Self::C_SHIFT),
        )
    }

    #[inline]
    pub const fn abx(op: Opcode, a: u8, bx: u16) -> Self {
        Self(
            ((op as u32) << Self::OP_SHIFT)
                | ((a as u32) << Self::A_SHIFT)
                | (((bx as u32) & Self::BX_MASK) << Self::BX_SHIFT),
        )
    }

    #[inline]
    pub const fn branch_i4(op: Opcode, a: u8, immediate: u8, offset: i16) -> Self {
        let encoded = (offset as i32 + Self::I4_BRANCH_OFFSET_BIAS) as u16;
        debug_assert!(immediate <= 0x0F);
        debug_assert!(encoded <= Self::I4_BRANCH_OFFSET_MASK);
        Self::abx(
            op,
            a,
            ((immediate as u16) << Self::I4_BRANCH_IMMEDIATE_SHIFT) | (encoded & Self::I4_BRANCH_OFFSET_MASK),
        )
    }

    #[inline]
    pub const fn as_bx(op: Opcode, a: u8, sbx: i16) -> Self {
        let encoded = (sbx as i32 + Self::SBX_BIAS) as u32;
        debug_assert!(encoded <= Self::BX_MASK);
        Self(
            ((op as u32) << Self::OP_SHIFT)
                | ((a as u32) << Self::A_SHIFT)
                | ((encoded & Self::BX_MASK) << Self::BX_SHIFT),
        )
    }

    #[inline]
    pub const fn ax(op: Opcode, ax: u32) -> Self {
        debug_assert!(ax <= Self::AX_MASK);
        Self(((op as u32) << Self::OP_SHIFT) | ((ax & Self::AX_MASK) << Self::AX_SHIFT))
    }

    #[inline]
    pub const fn sj(op: Opcode, sj: i32) -> Self {
        let encoded = (sj + Self::SJ_BIAS) as u32;
        debug_assert!(encoded <= Self::SJ_MASK);
        Self(((op as u32) << Self::OP_SHIFT) | ((encoded & Self::SJ_MASK) << Self::AX_SHIFT))
    }

    #[inline]
    pub const fn a(self) -> u8 {
        ((self.0 >> Self::A_SHIFT) & Self::BYTE_MASK) as u8
    }

    #[inline]
    pub const fn b(self) -> u8 {
        ((self.0 >> Self::B_SHIFT) & Self::B_MASK) as u8
    }

    #[inline]
    pub const fn c(self) -> u8 {
        ((self.0 >> Self::C_SHIFT) & Self::C_MASK) as u8
    }

    #[inline]
    pub const fn sc(self) -> i8 {
        self.c() as i8
    }

    #[inline]
    pub const fn bx(self) -> u16 {
        ((self.0 >> Self::BX_SHIFT) & Self::BX_MASK) as u16
    }

    #[inline]
    pub const fn sbx(self) -> i16 {
        (self.bx() as i32 - Self::SBX_BIAS) as i16
    }

    #[inline]
    pub const fn branch_i4_immediate(self) -> u8 {
        (self.bx() >> Self::I4_BRANCH_IMMEDIATE_SHIFT) as u8
    }

    #[inline]
    pub const fn branch_i4_offset(self) -> i16 {
        ((self.bx() & Self::I4_BRANCH_OFFSET_MASK) as i32 - Self::I4_BRANCH_OFFSET_BIAS) as i16
    }

    #[inline]
    pub const fn ax_arg(self) -> u32 {
        (self.0 >> Self::AX_SHIFT) & Self::AX_MASK
    }

    #[inline]
    pub const fn sj_arg(self) -> i32 {
        self.ax_arg() as i32 - Self::SJ_BIAS
    }

    #[inline]
    pub const fn return_base(self) -> u8 {
        self.a()
    }

    #[inline]
    pub fn return_count(self) -> u8 {
        match self.opcode() {
            Opcode::Return0 => 0,
            Opcode::Return1 => 1,
            Opcode::Return => self.b(),
            _ => 0,
        }
    }

    pub fn disassemble(self) -> String {
        if matches!(
            self.opcode(),
            Opcode::BrEqIntI4 | Opcode::BrNeIntI4 | Opcode::BrModEqZeroIntI4 | Opcode::BrModNeZeroIntI4
        ) {
            return format!(
                "{:?} r{} {} {}",
                self.opcode(),
                self.a(),
                self.branch_i4_immediate(),
                self.branch_i4_offset()
            );
        }
        match self.format() {
            InstrFormat::Abc => format!("{:?} r{} r{} r{}", self.opcode(), self.a(), self.b(), self.c()),
            InstrFormat::Abx => format!("{:?} r{} #{}", self.opcode(), self.a(), self.bx()),
            InstrFormat::AsBx => format!("{:?} r{} {}", self.opcode(), self.a(), self.sbx()),
            InstrFormat::Ax => format!("{:?} #{}", self.opcode(), self.ax_arg()),
            InstrFormat::Sj => format!("{:?} {}", self.opcode(), self.sj_arg()),
        }
    }
}

pub fn encode_instr(code: &[Instr]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(code.len() * size_of::<u32>());
    for instr in code {
        bytes.extend_from_slice(&instr.raw().to_le_bytes());
    }
    bytes
}

pub fn decode_instr(bytes: &[u8]) -> Result<Vec<Instr>> {
    if !bytes.len().is_multiple_of(size_of::<u32>()) {
        bail!("Instr encoded length {} is not 4-byte aligned", bytes.len());
    }
    let mut instrs = Vec::with_capacity(bytes.len() / size_of::<u32>());
    for chunk in bytes.chunks_exact(size_of::<u32>()) {
        let raw = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        instrs.push(Instr::try_from_raw(raw)?);
    }
    Ok(instrs)
}

#[derive(Clone, Debug, Default)]
pub struct Function {
    pub consts: ConstPool,
    pub code: Vec<Instr>,
    pub performance: PerformanceFacts,
    pub register_count: u16,
    pub param_count: u16,
    pub positional_param_count: u16,
    pub param_names: Vec<Arc<str>>,
    pub capture_count: u16,
    /// Optional source name for this function (e.g. `foo` for `fn foo(){}`),
    /// kept purely for diagnostics / tracebacks. `None` for anonymous lambdas.
    /// The executor never reads it, so it is zero-cost on the hot path.
    pub debug_name: Option<Arc<str>>,
    /// The C symbol this function is exported under, from `#[export]` /
    /// `#[export("name")]`. `None` — the usual case — means the native backend
    /// gives it internal linkage under a generated name.
    ///
    /// The VM ignores this: an exported function is still an ordinary LK
    /// function to it. It matters to the AOT path, where a board's reset stub
    /// or interrupt vector has to be able to *name* the compiled code it calls.
    pub export_name: Option<Arc<str>>,
    /// The C symbol this function is *implemented by*, from `#[extern]` /
    /// `#[extern("name")]` — the mirror of `export_name`.
    ///
    /// The native backend turns calls to it into calls to that symbol and
    /// never emits the body. The body is not dead, though: it is what the
    /// interpreter runs, which is the only sensible thing for a function whose
    /// real implementation is outside the program.
    pub extern_name: Option<Arc<str>>,
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub functions: Vec<Function>,
    pub globals: Vec<GlobalSlot>,
    pub entry: u32,
    /// Static `trait`/`impl` declarations (see [`super::TypeInfo`]). Produced
    /// by the compiler, consumed by the back ends — none of them has to
    /// reconstruct it from bytecode.
    pub type_info: super::TypeInfo,
    /// Identity of this module as a *declarer of types* — see
    /// [`crate::val::TypeScope`].
    pub type_scope: crate::val::TypeScope,
}

impl Module {
    #[inline]
    pub fn single(function: Function) -> Self {
        Self {
            functions: vec![function],
            globals: Vec::new(),
            entry: 0,
            type_info: super::TypeInfo::default(),
            type_scope: crate::val::TypeScope::anonymous(),
        }
    }

    #[inline]
    pub fn entry_function(&self) -> Option<&Function> {
        self.functions.get(self.entry as usize)
    }
}

pub fn disassemble_function(function: &Function) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        ".function regs={} params={} captures={}",
        function.register_count, function.param_count, function.capture_count
    );
    for (pc, instr) in function.code.iter().enumerate() {
        let _ = writeln!(out, "{pc:04} {}", instr.disassemble());
    }
    out
}

pub fn disassemble_module(module: &Module) -> String {
    let mut out = String::new();
    let _ = writeln!(out, ".module entry={}", module.entry);
    if !module.globals.is_empty() {
        let _ = writeln!(out, ".globals");
        for (slot, global) in module.globals.iter().enumerate() {
            let _ = writeln!(out, "  g{slot} {}", global.name);
        }
    }
    for (index, function) in module.functions.iter().enumerate() {
        let _ = writeln!(out, ".fn {index}");
        out.push_str(&disassemble_function(function));
    }
    out
}

#[cfg(test)]
mod tests {

    use super::*;

    /// The opcode discriminants run 0..=N with no holes, and that is a
    /// **performance** property, not tidiness.
    ///
    /// Removing `LoadNative` (an opcode no production path ever emitted) left a
    /// hole at 72 and cost **9%** on the workload suite — measured three times
    /// either side: 1.075 / 1.086 / 1.089 against a 0.991 / 0.986 baseline.
    /// Renumbering the opcodes above it to close the hole put it back to
    /// 0.994 / 0.987. The dispatch `match` lowers to a jump table only while the
    /// discriminants are dense; one gap is enough to lose it.
    ///
    /// Nothing guarded this, and the next opcode removal would have paid the
    /// same 9% with no test and no reviewer able to see why. Note the cost is
    /// the *hole*, not the missing arm: the same removal with contiguous
    /// numbering is free.
    ///
    /// Renumbering changes the artifact encoding, so it comes with a
    /// `MODULE_ARTIFACT_VERSION` bump.
    #[test]
    fn opcodes_are_contiguous() {
        assert_contiguous("Opcode", |value| Opcode::from_bits(value).map(|op| op as u8));
        // Both of these are decoded from a byte on a dispatch path too, and the
        // rule is not about `Opcode` — it is about what a `match` on a dense
        // integer lowers to. Guarding only the one that was measured would be
        // guarding the incident rather than the property.
        assert_contiguous("InstrFormat", |value| InstrFormat::from_bits(value).map(|f| f as u8));
        assert_contiguous("CastTarget", |value| CastTarget::from_u8(value).map(|c| c as u8));
    }

    /// Every byte the decoder accepts forms `0..=N`, **and** decodes to the
    /// variant whose discriminant is that byte.
    ///
    /// The round trip is the load-bearing half. Each of these decoders is a
    /// hand-written `match` on literals, so it mirrors the discriminants rather
    /// than deriving from them — a first version of this test only checked
    /// which bytes the decoder accepted, which is the mirror and not the thing.
    /// It would have passed with `Sj = 40` and `4 => Some(Self::Sj)` side by
    /// side: contiguous decode, sparse enum, and the jump table gone.
    fn assert_contiguous(name: &str, decode: impl Fn(u8) -> Option<u8>) {
        let decoded: Vec<(u8, u8)> = (0u8..=255)
            .filter_map(|value| decode(value).map(|discriminant| (value, discriminant)))
            .collect();
        assert!(!decoded.is_empty(), "{name}: nothing decodes at all");
        let expected: Vec<(u8, u8)> = (0..decoded.len() as u8).map(|value| (value, value)).collect();
        assert_eq!(
            decoded,
            expected,
            "{name} must decode 0..={} onto the variants whose discriminants are those bytes — a \
             hole costs ~9% by breaking the dispatch jump table, and a decoder that disagrees with \
             the discriminants hides one",
            decoded.len() - 1
        );
    }

    #[test]
    fn abc_round_trips_opcode_format_and_registers() {
        let instr = Instr::abc(Opcode::AddInt, 1, 2, 255);

        assert_eq!(instr.opcode(), Opcode::AddInt);
        assert_eq!(instr.format(), InstrFormat::Abc);
        assert_eq!(instr.a(), 1);
        assert_eq!(instr.b(), 2);
        assert_eq!(instr.c(), 255);
    }

    #[test]
    fn abx_and_asbx_share_payload_layout() {
        let load = Instr::abx(Opcode::LoadString, 4, 12_345);
        let handler = Instr::as_bx(Opcode::TryBegin, 0, -123);

        assert_eq!(load.format(), InstrFormat::Abx);
        assert_eq!(load.a(), 4);
        assert_eq!(load.bx(), 12_345);
        assert_eq!(handler.format(), InstrFormat::AsBx);
        assert_eq!(handler.sbx(), -123);
    }

    #[test]
    fn i4_branch_packs_immediate_and_offset_readably() {
        let branch = Instr::branch_i4(Opcode::BrNeIntI4, 7, 3, -12);

        assert_eq!(branch.format(), InstrFormat::Abx);
        assert_eq!(branch.a(), 7);
        assert_eq!(branch.branch_i4_immediate(), 3);
        assert_eq!(branch.branch_i4_offset(), -12);
        assert_eq!(branch.disassemble(), "BrNeIntI4 r7 3 -12");

        let mod_branch = Instr::branch_i4(Opcode::BrModEqZeroIntI4, 4, 5, 9);
        assert_eq!(mod_branch.format(), InstrFormat::Abx);
        assert_eq!(mod_branch.branch_i4_immediate(), 5);
        assert_eq!(mod_branch.branch_i4_offset(), 9);
        assert_eq!(mod_branch.disassemble(), "BrModEqZeroIntI4 r4 5 9");
    }

    #[test]
    fn ax_and_sj_cover_wide_payloads() {
        let wide = Instr::ax(Opcode::Wide, 0x2A_BCDE);
        let jmp = Instr::sj(Opcode::Jmp, -20_000);

        assert_eq!(wide.format(), InstrFormat::Ax);
        assert_eq!(wide.ax_arg(), 0x2A_BCDE);
        assert_eq!(jmp.format(), InstrFormat::Sj);
        assert_eq!(jmp.sj_arg(), -20_000);
    }

    #[test]
    fn instr_encoder_decoder_round_trips_validated_words() {
        let code = vec![
            Instr::abc(Opcode::NewMap, 1, 2, 3),
            Instr::abc(Opcode::LoadCellVal, 2, 3, 0),
            Instr::abc(Opcode::StoreCellVal, 3, 4, 0),
            Instr::abx(Opcode::LoadString, 4, 12_345),
            Instr::abx(Opcode::LoadHeapConst, 5, 7),
            Instr::as_bx(Opcode::TryBegin, 6, 2),
            Instr::ax(Opcode::TryEnd, 0),
            Instr::sj(Opcode::Jmp, -20_000),
        ];

        let bytes = encode_instr(&code);
        let decoded = decode_instr(&bytes).expect("decode");

        assert_eq!(decoded, code);
    }

    #[test]
    fn cell_and_handler_instr_disassemble_with_expected_formats() {
        let load = Instr::abc(Opcode::LoadCellVal, 1, 2, 0);
        let store = Instr::abc(Opcode::StoreCellVal, 2, 3, 0);
        let begin = Instr::as_bx(Opcode::TryBegin, 4, 9);
        let end = Instr::ax(Opcode::TryEnd, 0);

        assert_eq!(load.format(), InstrFormat::Abc);
        assert_eq!(store.format(), InstrFormat::Abc);
        assert_eq!(begin.format(), InstrFormat::AsBx);
        assert_eq!(end.format(), InstrFormat::Ax);
        assert_eq!(load.disassemble(), "LoadCellVal r1 r2 r0");
        assert_eq!(store.disassemble(), "StoreCellVal r2 r3 r0");
        assert_eq!(begin.disassemble(), "TryBegin r4 9");
        assert_eq!(end.disassemble(), "TryEnd #0");
    }

    #[test]
    fn instr_decoder_rejects_unaligned_or_invalid_opcode_words() {
        let unaligned = [0_u8, 1, 2];
        assert!(decode_instr(&unaligned).is_err());

        let invalid_opcode = 127_u32.to_le_bytes();
        assert!(decode_instr(&invalid_opcode).is_err());
    }

    #[test]
    fn const_pool_pushes_and_reads_typed_pools() {
        let mut pool = ConstPool::default();

        let int = pool.push_int(42).expect("int");
        let float = pool.push_float(3.5).expect("float");
        let string = pool.push_string("short").expect("string");
        let heap_value = ConstHeapValue::LongString(Arc::<str>::from("longer-than-seven"));
        let heap = pool.push_heap_value(heap_value.clone()).expect("heap");

        assert_eq!(pool.push_int(42).expect("duplicate int"), int);
        assert_eq!(pool.push_float(3.5).expect("duplicate float"), float);
        assert_eq!(pool.push_string("short").expect("duplicate string"), string);
        assert_eq!(pool.push_heap_value(heap_value).expect("duplicate heap"), heap);
        assert_eq!(pool.int(int), Some(42));
        assert_eq!(pool.float(float), Some(3.5));
        assert_eq!(pool.string(string), Some("short"));
        assert!(matches!(
            pool.heap_value(heap),
            Some(ConstHeapValue::LongString(value)) if value.as_ref() == "longer-than-seven"
        ));
        assert_eq!(pool.ints.len(), 1);
        assert_eq!(pool.floats.len(), 1);
        assert_eq!(pool.strings.len(), 1);
        assert_eq!(pool.heap_values.len(), 1);
    }

    #[test]
    fn const_pool_heap_values_can_represent_nested_containers() {
        let mut entries = crate::util::value_map::value_map_new();
        entries.insert(
            RuntimeMapKey::ShortStr(ShortStr::new("name").expect("short")),
            ConstRuntimeValue::Heap(Box::new(ConstHeapValue::LongString(Arc::<str>::from(
                "longer-than-seven",
            )))),
        );
        let value = ConstHeapValue::List(vec![
            ConstRuntimeValue::Int(1),
            ConstRuntimeValue::Heap(Box::new(ConstHeapValue::Map(entries))),
        ]);
        let mut pool = ConstPool::default();

        let index = pool.push_heap_value(value).expect("heap const");

        assert!(matches!(
            pool.heap_value(index),
            Some(ConstHeapValue::List(values)) if values.len() == 2
        ));
    }

    #[test]
    fn disassembles_function_stably() {
        let function = Function {
            code: vec![
                Instr::abx(Opcode::LoadCapture, 0, 1),
                Instr::abc(Opcode::MakeClosure, 2, 3, 4),
                Instr::sj(Opcode::Jmp, -2),
            ],
            register_count: 5,
            param_count: 1,
            positional_param_count: 1,
            param_names: Vec::new(),
            capture_count: 2,
            ..Function::default()
        };

        let text = disassemble_function(&function);

        assert!(text.contains(".function regs=5 params=1 captures=2"));
        assert!(text.contains("0000 LoadCapture r0 #1"));
        assert!(text.contains("0001 MakeClosure r2 r3 r4"));
        assert!(text.contains("0002 Jmp -2"));
    }

    #[test]
    fn disassembles_module_metadata() {
        let module = Module {
            functions: vec![Function::default()],
            globals: vec![GlobalSlot {
                name: Arc::<str>::from("answer"),
            }],
            entry: 0,
            type_info: Default::default(),
            type_scope: Default::default(),
        };

        let text = disassemble_module(&module);

        assert!(text.contains(".module entry=0"));
        assert!(text.contains("g0 answer"));
        assert!(text.contains(".fn 0"));
    }
}

#[cfg(test)]
mod signed_zero_pool_tests {
    use super::*;

    /// A constant pool entry's identity is its bits, not `==`.
    ///
    /// `-0.0 == 0.0` is true and the two are different values, so pooling by
    /// equality made whichever literal a file wrote first swallow every later
    /// occurrence of the other: `println(-0.0); println(0.0);` printed `-0`
    /// twice, and the swallowed sign reached division —
    /// `println(1.0 / 0.0)` answered `-inf`. The answer depended on the
    /// spelling and position of an unrelated line in the same file.
    #[test]
    fn the_two_zeros_are_two_constants() {
        let mut pool = ConstPool::default();
        let negative = pool.push_float(-0.0).expect("pooled");
        let positive = pool.push_float(0.0).expect("pooled");
        assert_ne!(negative, positive, "-0.0 and 0.0 are different constants");
        assert_eq!(pool.floats.len(), 2);
        assert_eq!(pool.push_float(-0.0).expect("pooled"), negative, "and each still pools");
        assert_eq!(pool.push_float(0.0).expect("pooled"), positive);

        // The same rule one carrier deeper: a container constant holds these
        // values, so `[0.0]` and `[-0.0]` were the same pool entry too.
        let mut pool = ConstPool::default();
        let negative = pool
            .push_heap_value(ConstHeapValue::List(vec![ConstRuntimeValue::Float(-0.0)]))
            .expect("pooled");
        let positive = pool
            .push_heap_value(ConstHeapValue::List(vec![ConstRuntimeValue::Float(0.0)]))
            .expect("pooled");
        assert_ne!(negative, positive);
        assert_eq!(pool.heap_values.len(), 2);
    }
}
