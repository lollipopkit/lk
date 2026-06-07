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
    AddInt = 1,
    SubInt = 2,
    MulInt = 3,
    DivInt = 4,
    ModInt = 5,
    AddFloat = 6,
    SubFloat = 7,
    MulFloat = 8,
    DivFloat = 9,
    ModFloat = 10,
    CmpInt = 11,
    CmpNeInt = 12,
    CmpLtInt = 13,
    CmpLeInt = 14,
    CmpGtInt = 15,
    CmpGeInt = 16,
    Test = 17,
    Jmp = 18,
    Call = 19,
    Return = 20,
    Move = 21,
    LoadNil = 22,
    LoadBool = 23,
    LoadInt = 24,
    LoadFloat = 25,
    LoadString = 26,
    LoadHeapConst = 27,
    LoadCapture = 28,
    LoadCellVal = 29,
    StoreCellVal = 30,
    GetGlobal = 31,
    SetGlobal = 32,
    NewList = 33,
    NewMap = 34,
    GetIndex = 35,
    SetIndex = 36,
    ListPush = 37,
    LoadFunction = 38,
    LoadNative = 39,
    MakeClosure = 40,
    CallDirect = 41,
    CallNamed = 42,
    Not = 43,
    IsNil = 44,
    IsList = 45,
    IsMap = 46,
    ToString = 47,
    ConcatString = 48,
    StringStartsWith = 49,
    StringSplit = 50,
    ListJoin = 51,
    NewRange = 52,
    Len = 53,
    ToIter = 54,
    NewObject = 55,
    Contains = 56,
    SliceFrom = 57,
    MapRest = 58,
    Raise = 59,
    TryBegin = 60,
    TryEnd = 61,
    ForLoopI = 62,
    Wide = 63,
    AddIntI = 64,
    BrFalse = 65,
    BrTrue = 66,
    BrNil = 67,
    BrNotNil = 68,
    TestEqInt = 69,
    TestNeInt = 70,
    TestLtInt = 71,
    TestLeInt = 72,
    TestGtInt = 73,
    TestGeInt = 74,
    GetFieldK = 75,
    SetFieldK = 76,
}

impl Opcode {
    /// Number of opcode slots available in the current 7-bit encoding.
    pub const COUNT: u8 = 128;

    #[inline]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Nop),
            1 => Some(Self::AddInt),
            2 => Some(Self::SubInt),
            3 => Some(Self::MulInt),
            4 => Some(Self::DivInt),
            5 => Some(Self::ModInt),
            6 => Some(Self::AddFloat),
            7 => Some(Self::SubFloat),
            8 => Some(Self::MulFloat),
            9 => Some(Self::DivFloat),
            10 => Some(Self::ModFloat),
            11 => Some(Self::CmpInt),
            12 => Some(Self::CmpNeInt),
            13 => Some(Self::CmpLtInt),
            14 => Some(Self::CmpLeInt),
            15 => Some(Self::CmpGtInt),
            16 => Some(Self::CmpGeInt),
            17 => Some(Self::Test),
            18 => Some(Self::Jmp),
            19 => Some(Self::Call),
            20 => Some(Self::Return),
            21 => Some(Self::Move),
            22 => Some(Self::LoadNil),
            23 => Some(Self::LoadBool),
            24 => Some(Self::LoadInt),
            25 => Some(Self::LoadFloat),
            26 => Some(Self::LoadString),
            27 => Some(Self::LoadHeapConst),
            28 => Some(Self::LoadCapture),
            29 => Some(Self::LoadCellVal),
            30 => Some(Self::StoreCellVal),
            31 => Some(Self::GetGlobal),
            32 => Some(Self::SetGlobal),
            33 => Some(Self::NewList),
            34 => Some(Self::NewMap),
            35 => Some(Self::GetIndex),
            36 => Some(Self::SetIndex),
            37 => Some(Self::ListPush),
            38 => Some(Self::LoadFunction),
            39 => Some(Self::LoadNative),
            40 => Some(Self::MakeClosure),
            41 => Some(Self::CallDirect),
            42 => Some(Self::CallNamed),
            43 => Some(Self::Not),
            44 => Some(Self::IsNil),
            45 => Some(Self::IsList),
            46 => Some(Self::IsMap),
            47 => Some(Self::ToString),
            48 => Some(Self::ConcatString),
            49 => Some(Self::StringStartsWith),
            50 => Some(Self::StringSplit),
            51 => Some(Self::ListJoin),
            52 => Some(Self::NewRange),
            53 => Some(Self::Len),
            54 => Some(Self::ToIter),
            55 => Some(Self::NewObject),
            56 => Some(Self::Contains),
            57 => Some(Self::SliceFrom),
            58 => Some(Self::MapRest),
            59 => Some(Self::Raise),
            60 => Some(Self::TryBegin),
            61 => Some(Self::TryEnd),
            62 => Some(Self::ForLoopI),
            63 => Some(Self::Wide),
            64 => Some(Self::AddIntI),
            65 => Some(Self::BrFalse),
            66 => Some(Self::BrTrue),
            67 => Some(Self::BrNil),
            68 => Some(Self::BrNotNil),
            69 => Some(Self::TestEqInt),
            70 => Some(Self::TestNeInt),
            71 => Some(Self::TestLtInt),
            72 => Some(Self::TestLeInt),
            73 => Some(Self::TestGtInt),
            74 => Some(Self::TestGeInt),
            75 => Some(Self::GetFieldK),
            76 => Some(Self::SetFieldK),
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
                | Self::CallNamed => InstrFormat::Abx,
                Self::TryBegin | Self::BrFalse | Self::BrTrue | Self::BrNil | Self::BrNotNil => InstrFormat::AsBx,
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
        )
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
    pub const fn ax_arg(self) -> u32 {
        (self.0 >> Self::AX_SHIFT) & Self::AX_MASK
    }

    #[inline]
    pub const fn sj_arg(self) -> i32 {
        self.ax_arg() as i32 - Self::SJ_BIAS
    }

    pub fn disassemble(self) -> String {
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
    if bytes.len() % size_of::<u32>() != 0 {
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
