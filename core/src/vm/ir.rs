//! Canonical 32-bit VM instruction model for the VM rewrite.
//!
//! Compiler and executor work should target this representation directly;
//! alternate instruction models must not be reintroduced.

use crate::util::fast_map::FastHashMap;
use std::{fmt::Write as _, mem::size_of, sync::Arc};

use anyhow::{Result, bail};

use crate::{
    val::{RuntimeMapKey, ShortStr},
    vm::analysis::{FunctionAnalysis, PerformanceFacts},
};

use super::runtime::NativeEntry;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalSlot {
    pub name: Arc<str>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConstPool {
    pub ints: Vec<i64>,
    pub floats: Vec<f64>,
    pub strings: Vec<String>,
    pub heap_values: Vec<ConstHeapValue>,
}

impl ConstPool {
    const MAX_ABX_CONSTS: usize = 1 << 16;

    pub fn push_int(&mut self, value: i64) -> Result<u16> {
        push_const(&mut self.ints, value, "int")
    }

    pub fn push_float(&mut self, value: f64) -> Result<u16> {
        push_const(&mut self.floats, value, "float")
    }

    pub fn push_string(&mut self, value: impl Into<String>) -> Result<u16> {
        push_const(&mut self.strings, value.into(), "string")
    }

    pub fn push_heap_value(&mut self, value: ConstHeapValue) -> Result<u16> {
        push_const(&mut self.heap_values, value, "heap value")
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
        self.strings.get(index as usize).map(String::as_str)
    }

    #[inline]
    pub fn heap_value(&self, index: u16) -> Option<&ConstHeapValue> {
        self.heap_values.get(index as usize)
    }
}

fn push_const<T: PartialEq>(values: &mut Vec<T>, value: T, name: &str) -> Result<u16> {
    if let Some(index) = values.iter().position(|existing| existing == &value) {
        return Ok(index as u16);
    }
    let index = values.len();
    if index >= ConstPool::MAX_ABX_CONSTS {
        bail!("Instr {name} const pool overflow");
    }
    values.push(value);
    Ok(index as u16)
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
    Map(FastHashMap<RuntimeMapKey, ConstRuntimeValue>),
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

#[repr(u8)]
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
    LoadNative = 72,
    MakeClosure = 73,
    LoadCapture = 74,
    LoadCellVal = 75,
    StoreCellVal = 76,
    GetGlobal = 77,
    SetGlobal = 78,
    NewList = 79,
    NewMap = 80,
    NewRange = 81,
    NewObject = 82,
    GetIndex = 83,
    SetIndex = 84,
    GetIndexStrI = 85,
    SetIndexStrI = 86,
    GetFieldK = 87,
    SetFieldK = 88,
    GetList = 89,
    ListPush = 90,
    Len = 91,
    ToIter = 92,
    Contains = 93,
    SliceFrom = 94,
    MapRest = 95,
    ToString = 96,
    ConcatString = 97,
    ConcatN = 98,
    StringSplit = 99,
    ListJoin = 100,
    Raise = 101,
    TryBegin = 102,
    TryEnd = 103,
    Wide = 104,
    /// Boxing-free method call: `a` = window base (receiver at `a`, args at
    /// `[a+1, a+1+c)`, result written to `a`), `b` = method-name string
    /// constant index, `c` = positional argument count. Replaces the
    /// `GetGlobal __lk_call_method` + `NewList` + `Call` sequence for
    /// positional method calls whose name constant index fits in `b`.
    CallMethodK = 105,
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
            72 => Some(Self::LoadNative),
            73 => Some(Self::MakeClosure),
            74 => Some(Self::LoadCapture),
            75 => Some(Self::LoadCellVal),
            76 => Some(Self::StoreCellVal),
            77 => Some(Self::GetGlobal),
            78 => Some(Self::SetGlobal),
            79 => Some(Self::NewList),
            80 => Some(Self::NewMap),
            81 => Some(Self::NewRange),
            82 => Some(Self::NewObject),
            83 => Some(Self::GetIndex),
            84 => Some(Self::SetIndex),
            85 => Some(Self::GetIndexStrI),
            86 => Some(Self::SetIndexStrI),
            87 => Some(Self::GetFieldK),
            88 => Some(Self::SetFieldK),
            89 => Some(Self::GetList),
            90 => Some(Self::ListPush),
            91 => Some(Self::Len),
            92 => Some(Self::ToIter),
            93 => Some(Self::Contains),
            94 => Some(Self::SliceFrom),
            95 => Some(Self::MapRest),
            96 => Some(Self::ToString),
            97 => Some(Self::ConcatString),
            98 => Some(Self::ConcatN),
            99 => Some(Self::StringSplit),
            100 => Some(Self::ListJoin),
            101 => Some(Self::Raise),
            102 => Some(Self::TryBegin),
            103 => Some(Self::TryEnd),
            104 => Some(Self::Wide),
            105 => Some(Self::CallMethodK),
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
                | Self::LoadNative
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
    pub analyses: Vec<FunctionAnalysis>,
    pub performance: PerformanceFacts,
    pub register_count: u16,
    pub param_count: u16,
    pub positional_param_count: u16,
    pub param_names: Vec<Arc<str>>,
    pub capture_count: u16,
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub functions: Vec<Function>,
    pub natives: Vec<NativeEntry>,
    pub globals: Vec<GlobalSlot>,
    pub entry: u32,
}

impl Module {
    #[inline]
    pub fn single(function: Function) -> Self {
        Self {
            functions: vec![function],
            natives: Vec::new(),
            globals: Vec::new(),
            entry: 0,
        }
    }

    #[inline]
    pub fn entry_function(&self) -> Option<&Function> {
        self.functions.get(self.entry as usize)
    }

    pub fn native_index(&self, name: &str) -> Option<usize> {
        self.natives.iter().position(|native| native.name == name)
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
    if !module.natives.is_empty() {
        let _ = writeln!(out, ".natives");
        for (slot, native) in module.natives.iter().enumerate() {
            let _ = writeln!(out, "  n{slot} {} arity={}", native.name, native.arity);
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
    use crate::util::fast_map::fast_hash_map_new;
    use crate::{val::RuntimeVal, vm::NativeFunction};

    use super::*;

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
        let mut entries = fast_hash_map_new();
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
            natives: vec![NativeEntry {
                name: "native_add".to_string(),
                arity: 2,
                function: NativeFunction::Plain(|_, _runtime| Ok(RuntimeVal::Nil)),
            }],
            globals: vec![GlobalSlot {
                name: Arc::<str>::from("answer"),
            }],
            entry: 0,
        };

        let text = disassemble_module(&module);

        assert!(text.contains(".module entry=0"));
        assert!(text.contains("g0 answer"));
        assert!(text.contains("n0 native_add arity=2"));
        assert!(text.contains(".fn 0"));
    }
}
