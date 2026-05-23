//! Canonical 32-bit VM instruction model for the VM rewrite.
//!
//! This module is intentionally independent from the legacy `Op` enum. New
//! compiler and executor work should target this representation directly.

use std::{collections::BTreeMap, fmt::Write as _, mem::size_of, sync::Arc};

use anyhow::{Result, bail};

use crate::{
    val::{RuntimeMapKey, ShortStr},
    vm::analysis::FunctionAnalysis,
};

use super::runtime32::NativeEntry32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalSlot32 {
    pub name: Arc<str>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConstPool32 {
    pub ints: Vec<i64>,
    pub floats: Vec<f64>,
    pub strings: Vec<String>,
    pub heap_values: Vec<ConstHeapValue32>,
}

impl ConstPool32 {
    const MAX_ABX_CONSTS: usize = 1 << 15;

    pub fn push_int(&mut self, value: i64) -> Result<u16> {
        push_const32(&mut self.ints, value, "int")
    }

    pub fn push_float(&mut self, value: f64) -> Result<u16> {
        push_const32(&mut self.floats, value, "float")
    }

    pub fn push_string(&mut self, value: impl Into<String>) -> Result<u16> {
        push_const32(&mut self.strings, value.into(), "string")
    }

    pub fn push_heap_value(&mut self, value: ConstHeapValue32) -> Result<u16> {
        push_const32(&mut self.heap_values, value, "heap value")
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
    pub fn heap_value(&self, index: u16) -> Option<&ConstHeapValue32> {
        self.heap_values.get(index as usize)
    }
}

fn push_const32<T>(values: &mut Vec<T>, value: T, name: &str) -> Result<u16> {
    let index = values.len();
    if index >= ConstPool32::MAX_ABX_CONSTS {
        bail!("Instr32 {name} const pool overflow");
    }
    values.push(value);
    Ok(index as u16)
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConstRuntimeValue32 {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    ShortStr(ShortStr),
    Heap(Box<ConstHeapValue32>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConstHeapValue32 {
    LongString(Arc<str>),
    List(Vec<ConstRuntimeValue32>),
    Map(BTreeMap<RuntimeMapKey, ConstRuntimeValue32>),
    UpvalCell(Box<ConstRuntimeValue32>),
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
pub enum Opcode32 {
    Nop = 0,
    Move = 1,
    LoadNil = 2,
    LoadBool = 3,
    LoadInt = 4,
    LoadFloat = 5,
    LoadString = 6,
    LoadHeapConst = 52,
    LoadCapture = 30,
    LoadCellVal = 53,
    StoreCellVal = 54,
    LoadFunction = 28,
    LoadNative = 29,
    MakeClosure = 31,
    AddInt = 7,
    SubInt = 8,
    MulInt = 9,
    DivInt = 32,
    ModInt = 10,
    AddFloat = 11,
    SubFloat = 37,
    MulFloat = 38,
    DivFloat = 39,
    ModFloat = 40,
    NewRange = 41,
    Len = 42,
    ToIter = 43,
    NewObject = 44,
    Contains = 45,
    SliceFrom = 46,
    MapRest = 47,
    Raise = 48,
    TryBegin = 55,
    TryEnd = 56,
    IsList = 49,
    IsMap = 50,
    CallNamed = 51,
    Not = 33,
    IsNil = 34,
    ToString = 35,
    ConcatString = 36,
    CmpInt = 12,
    Test = 13,
    Jmp = 14,
    Call = 15,
    Return = 16,
    GetGlobal = 17,
    SetGlobal = 18,
    NewList = 19,
    NewMap = 20,
    GetIndex = 21,
    SetIndex = 22,
    CmpNeInt = 23,
    CmpLtInt = 24,
    CmpLeInt = 25,
    CmpGtInt = 26,
    CmpGeInt = 27,
    Extra = 62,
    Wide = 63,
}

impl Opcode32 {
    #[inline]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Nop),
            1 => Some(Self::Move),
            2 => Some(Self::LoadNil),
            3 => Some(Self::LoadBool),
            4 => Some(Self::LoadInt),
            5 => Some(Self::LoadFloat),
            6 => Some(Self::LoadString),
            52 => Some(Self::LoadHeapConst),
            30 => Some(Self::LoadCapture),
            53 => Some(Self::LoadCellVal),
            54 => Some(Self::StoreCellVal),
            28 => Some(Self::LoadFunction),
            29 => Some(Self::LoadNative),
            31 => Some(Self::MakeClosure),
            7 => Some(Self::AddInt),
            8 => Some(Self::SubInt),
            9 => Some(Self::MulInt),
            32 => Some(Self::DivInt),
            10 => Some(Self::ModInt),
            11 => Some(Self::AddFloat),
            37 => Some(Self::SubFloat),
            38 => Some(Self::MulFloat),
            39 => Some(Self::DivFloat),
            40 => Some(Self::ModFloat),
            41 => Some(Self::NewRange),
            42 => Some(Self::Len),
            43 => Some(Self::ToIter),
            44 => Some(Self::NewObject),
            45 => Some(Self::Contains),
            46 => Some(Self::SliceFrom),
            47 => Some(Self::MapRest),
            48 => Some(Self::Raise),
            55 => Some(Self::TryBegin),
            56 => Some(Self::TryEnd),
            49 => Some(Self::IsList),
            50 => Some(Self::IsMap),
            51 => Some(Self::CallNamed),
            33 => Some(Self::Not),
            34 => Some(Self::IsNil),
            35 => Some(Self::ToString),
            36 => Some(Self::ConcatString),
            12 => Some(Self::CmpInt),
            13 => Some(Self::Test),
            14 => Some(Self::Jmp),
            15 => Some(Self::Call),
            16 => Some(Self::Return),
            17 => Some(Self::GetGlobal),
            18 => Some(Self::SetGlobal),
            19 => Some(Self::NewList),
            20 => Some(Self::NewMap),
            21 => Some(Self::GetIndex),
            22 => Some(Self::SetIndex),
            23 => Some(Self::CmpNeInt),
            24 => Some(Self::CmpLtInt),
            25 => Some(Self::CmpLeInt),
            26 => Some(Self::CmpGtInt),
            27 => Some(Self::CmpGeInt),
            62 => Some(Self::Extra),
            63 => Some(Self::Wide),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instr32(u32);

impl Instr32 {
    const OPCODE_BITS: u32 = 6;
    const FORMAT_BITS: u32 = 3;
    const OP_SHIFT: u32 = 0;
    const FORMAT_SHIFT: u32 = Self::OP_SHIFT + Self::OPCODE_BITS;
    const A_SHIFT: u32 = Self::FORMAT_SHIFT + Self::FORMAT_BITS;
    const B_SHIFT: u32 = Self::A_SHIFT + 8;
    const C_SHIFT: u32 = Self::B_SHIFT + 8; // B is now 8 bits wide
    const AX_SHIFT: u32 = Self::A_SHIFT;
    const OP_MASK: u32 = (1 << Self::OPCODE_BITS) - 1;
    const FORMAT_MASK: u32 = (1 << Self::FORMAT_BITS) - 1;
    const BYTE_MASK: u32 = 0xFF;
    const B_MASK: u32 = 0xFF;
    const C_MASK: u32 = 0x7F;
    const BX_MASK: u32 = 0x7FFF;
    const AX_MASK: u32 = (1 << 23) - 1;
    const SJ_MASK: u32 = Self::AX_MASK;
    const SBX_BIAS: i32 = (Self::BX_MASK as i32) >> 1;
    const SJ_BIAS: i32 = (Self::SJ_MASK as i32) >> 1;

    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }

    pub fn try_from_raw(raw: u32) -> Result<Self> {
        let opcode = ((raw >> Self::OP_SHIFT) & Self::OP_MASK) as u8;
        if Opcode32::from_bits(opcode).is_none() {
            bail!("invalid Instr32 opcode bits: {opcode}");
        }
        let format = ((raw >> Self::FORMAT_SHIFT) & Self::FORMAT_MASK) as u8;
        if InstrFormat::from_bits(format).is_none() {
            bail!("invalid Instr32 format bits: {format}");
        }
        Ok(Self(raw))
    }

    #[inline]
    pub fn opcode(self) -> Opcode32 {
        Opcode32::from_bits(((self.0 >> Self::OP_SHIFT) & Self::OP_MASK) as u8)
            .expect("Instr32 opcode is validated at construction")
    }

    #[inline]
    pub fn format(self) -> InstrFormat {
        InstrFormat::from_bits(((self.0 >> Self::FORMAT_SHIFT) & Self::FORMAT_MASK) as u8)
            .expect("Instr32 format is validated at construction")
    }

    #[inline]
    pub const fn abc(op: Opcode32, a: u8, b: u8, c: u8) -> Self {
        debug_assert!(c < 128);
        Self(
            ((op as u32) << Self::OP_SHIFT)
                | ((InstrFormat::Abc as u32) << Self::FORMAT_SHIFT)
                | ((a as u32) << Self::A_SHIFT)
                | (((b as u32) & Self::B_MASK) << Self::B_SHIFT)
                | (((c as u32) & Self::C_MASK) << Self::C_SHIFT),
        )
    }

    #[inline]
    pub const fn abx(op: Opcode32, a: u8, bx: u16) -> Self {
        debug_assert!(bx < (1 << 15));
        Self(
            ((op as u32) << Self::OP_SHIFT)
                | ((InstrFormat::Abx as u32) << Self::FORMAT_SHIFT)
                | ((a as u32) << Self::A_SHIFT)
                | (((bx as u32) & Self::BX_MASK) << Self::B_SHIFT),
        )
    }

    #[inline]
    pub const fn as_bx(op: Opcode32, a: u8, sbx: i16) -> Self {
        let encoded = (sbx as i32 + Self::SBX_BIAS) as u32;
        debug_assert!(encoded <= Self::BX_MASK);
        Self(
            ((op as u32) << Self::OP_SHIFT)
                | ((InstrFormat::AsBx as u32) << Self::FORMAT_SHIFT)
                | ((a as u32) << Self::A_SHIFT)
                | ((encoded & Self::BX_MASK) << Self::B_SHIFT),
        )
    }

    #[inline]
    pub const fn ax(op: Opcode32, ax: u32) -> Self {
        debug_assert!(ax <= Self::AX_MASK);
        Self(
            ((op as u32) << Self::OP_SHIFT)
                | ((InstrFormat::Ax as u32) << Self::FORMAT_SHIFT)
                | ((ax & Self::AX_MASK) << Self::AX_SHIFT),
        )
    }

    #[inline]
    pub const fn sj(op: Opcode32, sj: i32) -> Self {
        let encoded = (sj + Self::SJ_BIAS) as u32;
        debug_assert!(encoded <= Self::SJ_MASK);
        Self(
            ((op as u32) << Self::OP_SHIFT)
                | ((InstrFormat::Sj as u32) << Self::FORMAT_SHIFT)
                | ((encoded & Self::SJ_MASK) << Self::AX_SHIFT),
        )
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
    pub const fn bx(self) -> u16 {
        ((self.0 >> Self::B_SHIFT) & Self::BX_MASK) as u16
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

pub fn encode_instr32(code: &[Instr32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(code.len() * size_of::<u32>());
    for instr in code {
        bytes.extend_from_slice(&instr.raw().to_le_bytes());
    }
    bytes
}

pub fn decode_instr32(bytes: &[u8]) -> Result<Vec<Instr32>> {
    if bytes.len() % size_of::<u32>() != 0 {
        bail!("Instr32 bytecode length {} is not 4-byte aligned", bytes.len());
    }
    bytes
        .chunks_exact(size_of::<u32>())
        .map(|chunk| {
            let raw = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            Instr32::try_from_raw(raw)
        })
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct Function32 {
    pub consts: ConstPool32,
    pub code: Vec<Instr32>,
    pub analyses: Vec<FunctionAnalysis>,
    pub register_count: u16,
    pub param_count: u16,
    pub positional_param_count: u16,
    pub param_names: Vec<Arc<str>>,
    pub capture_count: u16,
}

#[derive(Clone, Debug, Default)]
pub struct Module32 {
    pub functions: Vec<Function32>,
    pub natives: Vec<NativeEntry32>,
    pub globals: Vec<GlobalSlot32>,
    pub entry: u32,
}

impl Module32 {
    #[inline]
    pub fn single(function: Function32) -> Self {
        Self {
            functions: vec![function],
            natives: Vec::new(),
            globals: Vec::new(),
            entry: 0,
        }
    }

    #[inline]
    pub fn entry_function(&self) -> Option<&Function32> {
        self.functions.get(self.entry as usize)
    }

    pub fn native_index(&self, name: &str) -> Option<usize> {
        self.natives.iter().position(|native| native.name == name)
    }
}

pub fn disassemble_function32(function: &Function32) -> String {
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

pub fn disassemble_module32(module: &Module32) -> String {
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
        out.push_str(&disassemble_function32(function));
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::{val::RuntimeVal, vm::NativeFunction32};

    use super::*;

    #[test]
    fn abc_round_trips_opcode_format_and_registers() {
        let instr = Instr32::abc(Opcode32::AddInt, 1, 2, 3);

        assert_eq!(instr.opcode(), Opcode32::AddInt);
        assert_eq!(instr.format(), InstrFormat::Abc);
        assert_eq!(instr.a(), 1);
        assert_eq!(instr.b(), 2);
        assert_eq!(instr.c(), 3);
    }

    #[test]
    fn abx_and_asbx_share_payload_layout() {
        let load = Instr32::abx(Opcode32::LoadString, 4, 12_345);
        let jump = Instr32::as_bx(Opcode32::Jmp, 0, -123);

        assert_eq!(load.format(), InstrFormat::Abx);
        assert_eq!(load.a(), 4);
        assert_eq!(load.bx(), 12_345);
        assert_eq!(jump.format(), InstrFormat::AsBx);
        assert_eq!(jump.sbx(), -123);
    }

    #[test]
    fn ax_and_sj_cover_wide_payloads() {
        let extra = Instr32::ax(Opcode32::Extra, 0x2A_BCDE);
        let jmp = Instr32::sj(Opcode32::Jmp, -20_000);

        assert_eq!(extra.format(), InstrFormat::Ax);
        assert_eq!(extra.ax_arg(), 0x2A_BCDE);
        assert_eq!(jmp.format(), InstrFormat::Sj);
        assert_eq!(jmp.sj_arg(), -20_000);
    }

    #[test]
    fn instr32_encoder_decoder_round_trips_validated_words() {
        let code = vec![
            Instr32::abc(Opcode32::NewMap, 1, 2, 3),
            Instr32::abc(Opcode32::LoadCellVal, 2, 3, 0),
            Instr32::abc(Opcode32::StoreCellVal, 3, 4, 0),
            Instr32::abx(Opcode32::LoadString, 4, 12_345),
            Instr32::abx(Opcode32::LoadHeapConst, 5, 7),
            Instr32::as_bx(Opcode32::TryBegin, 6, 2),
            Instr32::ax(Opcode32::TryEnd, 0),
            Instr32::sj(Opcode32::Jmp, -20_000),
        ];

        let bytes = encode_instr32(&code);
        let decoded = decode_instr32(&bytes).expect("decode");

        assert_eq!(decoded, code);
    }

    #[test]
    fn cell_and_handler_instr32_disassemble_with_expected_formats() {
        let load = Instr32::abc(Opcode32::LoadCellVal, 1, 2, 0);
        let store = Instr32::abc(Opcode32::StoreCellVal, 2, 3, 0);
        let begin = Instr32::as_bx(Opcode32::TryBegin, 4, 9);
        let end = Instr32::ax(Opcode32::TryEnd, 0);

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
    fn instr32_decoder_rejects_unaligned_or_unknown_words() {
        let unaligned = [0_u8, 1, 2];
        assert!(decode_instr32(&unaligned).is_err());

        let invalid_opcode = 125_u32.to_le_bytes();
        assert!(decode_instr32(&invalid_opcode).is_err());
    }

    #[test]
    fn const_pool32_pushes_and_reads_typed_pools() {
        let mut pool = ConstPool32::default();

        let int = pool.push_int(42).expect("int");
        let float = pool.push_float(3.5).expect("float");
        let string = pool.push_string("short").expect("string");
        let heap = pool
            .push_heap_value(ConstHeapValue32::LongString(Arc::<str>::from("longer-than-seven")))
            .expect("heap");

        assert_eq!(pool.int(int), Some(42));
        assert_eq!(pool.float(float), Some(3.5));
        assert_eq!(pool.string(string), Some("short"));
        assert!(matches!(
            pool.heap_value(heap),
            Some(ConstHeapValue32::LongString(value)) if value.as_ref() == "longer-than-seven"
        ));
        assert_eq!(pool.ints.len(), 1);
        assert_eq!(pool.heap_values.len(), 1);
    }

    #[test]
    fn const_pool32_heap_values_can_represent_nested_containers() {
        let mut entries = BTreeMap::new();
        entries.insert(
            RuntimeMapKey::ShortStr(ShortStr::new("name").expect("short")),
            ConstRuntimeValue32::Heap(Box::new(ConstHeapValue32::LongString(Arc::<str>::from(
                "longer-than-seven",
            )))),
        );
        let value = ConstHeapValue32::List(vec![
            ConstRuntimeValue32::Int(1),
            ConstRuntimeValue32::Heap(Box::new(ConstHeapValue32::Map(entries))),
        ]);
        let mut pool = ConstPool32::default();

        let index = pool.push_heap_value(value).expect("heap const");

        assert!(matches!(
            pool.heap_value(index),
            Some(ConstHeapValue32::List(values)) if values.len() == 2
        ));
    }

    #[test]
    fn disassembles_function32_stably() {
        let function = Function32 {
            code: vec![
                Instr32::abx(Opcode32::LoadCapture, 0, 1),
                Instr32::abc(Opcode32::MakeClosure, 2, 3, 4),
                Instr32::sj(Opcode32::Jmp, -2),
            ],
            register_count: 5,
            param_count: 1,
            positional_param_count: 1,
            param_names: Vec::new(),
            capture_count: 2,
            ..Function32::default()
        };

        let text = disassemble_function32(&function);

        assert!(text.contains(".function regs=5 params=1 captures=2"));
        assert!(text.contains("0000 LoadCapture r0 #1"));
        assert!(text.contains("0001 MakeClosure r2 r3 r4"));
        assert!(text.contains("0002 Jmp -2"));
    }

    #[test]
    fn disassembles_module32_metadata() {
        let module = Module32 {
            functions: vec![Function32::default()],
            natives: vec![NativeEntry32 {
                name: "native_add".to_string(),
                arity: 2,
                function: NativeFunction32::Plain(|_, _runtime| Ok(RuntimeVal::Nil)),
            }],
            globals: vec![GlobalSlot32 {
                name: Arc::<str>::from("answer"),
            }],
            entry: 0,
        };

        let text = disassemble_module32(&module);

        assert!(text.contains(".module entry=0"));
        assert!(text.contains("g0 answer"));
        assert!(text.contains("n0 native_add arity=2"));
        assert!(text.contains(".fn 0"));
    }
}
