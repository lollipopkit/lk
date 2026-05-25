use crate::vm::{ConstHeapValue32Data, Function32Data, Instr32, Opcode32};

use super::{
    scalar_block_helpers::{static_callable_value, static_string_i64_map_supported},
    straightline_value::{
        NativeBuiltin, NativeStraightlineValue, NativeTextPart, native_static_global, native_static_index,
        native_static_list_from_values, native_static_set_index, native_straightline_heap_const_value,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NativeScalarKind {
    I64,
    F64,
    Bool,
    Nil,
    StrPtr,
    MaybeI64,
}

pub(super) struct NativeScalarFacts {
    registers_before: Vec<Vec<Option<NativeScalarKind>>>,
    globals_before: Vec<Vec<Option<NativeScalarKind>>>,
}

impl NativeScalarFacts {
    pub(super) fn register_kind_before(&self, pc: usize, reg: u8) -> Option<NativeScalarKind> {
        self.registers_before
            .get(pc)
            .and_then(|kinds| kinds.get(reg as usize))
            .copied()
            .flatten()
    }

    pub(super) fn global_kind_before(&self, pc: usize, slot: u16) -> Option<NativeScalarKind> {
        self.globals_before
            .get(pc)
            .and_then(|kinds| kinds.get(slot as usize))
            .copied()
            .flatten()
    }

    pub(super) fn global_kinds_before(&self, pc: usize) -> Option<&[Option<NativeScalarKind>]> {
        self.globals_before.get(pc).map(Vec::as_slice)
    }
}

impl NativeScalarKind {
    pub(super) const fn llvm_type(self) -> &'static str {
        match self {
            Self::F64 => "double",
            Self::StrPtr => "ptr",
            Self::I64 | Self::Bool | Self::Nil | Self::MaybeI64 => "i64",
        }
    }

    pub(super) const fn is_numeric(self) -> bool {
        matches!(self, Self::I64 | Self::F64)
    }
}

pub(super) fn native_scalar_block_facts_with_statics_and_functions(
    register_count: usize,
    global_count: usize,
    global_names: &[String],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    code: &[Instr32],
    functions: Option<&[Function32Data]>,
) -> Option<NativeScalarFacts> {
    native_scalar_block_facts_with_initial(
        register_count,
        global_count,
        global_names,
        strings,
        heap_values,
        code,
        vec![None; register_count],
        vec![None; register_count],
        vec![None; global_count],
        vec![None; global_count],
        functions,
        &[],
        0,
    )
}

pub(super) fn native_scalar_block_facts_with_initial(
    register_count: usize,
    global_count: usize,
    global_names: &[String],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    code: &[Instr32],
    mut kinds: Vec<Option<NativeScalarKind>>,
    mut static_values: Vec<Option<NativeStraightlineValue>>,
    mut global_kinds: Vec<Option<NativeScalarKind>>,
    mut static_globals: Vec<Option<NativeStraightlineValue>>,
    functions: Option<&[Function32Data]>,
    static_captures: &[NativeStraightlineValue],
    depth: usize,
) -> Option<NativeScalarFacts> {
    if kinds.len() != register_count
        || static_values.len() != register_count
        || global_kinds.len() != global_count
        || static_globals.len() != global_count
    {
        return None;
    }
    let mut registers_before = Vec::with_capacity(code.len());
    let mut globals_before = Vec::with_capacity(code.len());
    for instr in code.iter().copied() {
        registers_before.push(kinds.clone());
        globals_before.push(global_kinds.clone());
        match instr.opcode() {
            Opcode32::Nop | Opcode32::Jmp => {}
            Opcode32::LoadNil => {
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::Nil) {
                    return None;
                }
            }
            Opcode32::LoadInt => {
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64) {
                    return None;
                }
            }
            Opcode32::LoadFloat => {
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::F64) {
                    return None;
                }
            }
            Opcode32::LoadBool => {
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::Bool) {
                    return None;
                }
            }
            Opcode32::LoadString => {
                let Some(value) = strings.get(instr.bx() as usize) else {
                    return None;
                };
                if !set_static_value(
                    &mut kinds,
                    &mut static_values,
                    instr.a(),
                    Some(NativeScalarKind::StrPtr),
                    NativeStraightlineValue::String {
                        symbol: String::new(),
                        value: value.clone(),
                        len: value.chars().count(),
                        key_kind: super::straightline_value::native_runtime_string_key_kind(value),
                    },
                ) {
                    return None;
                }
            }
            Opcode32::LoadHeapConst => {
                let Some(value) = heap_values.get(instr.bx() as usize) else {
                    return None;
                };
                let Some(value) = native_static_heap_const_value(value) else {
                    return None;
                };
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), value.0, value.1) {
                    return None;
                }
            }
            Opcode32::LoadFunction => {
                if !set_static_value(
                    &mut kinds,
                    &mut static_values,
                    instr.a(),
                    None,
                    NativeStraightlineValue::Function(instr.bx()),
                ) {
                    return None;
                }
            }
            Opcode32::LoadCapture => {
                let value = static_captures.get(instr.bx() as usize)?.clone();
                let kind = static_value_scalar_kind(&value);
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                    return None;
                }
            }
            Opcode32::MakeClosure => {
                let value = static_callable_value(functions?, instr, &static_values)?;
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                    return None;
                }
            }
            Opcode32::Move => {
                if let Some(value) = static_kind(&static_values, instr.b()) {
                    let kind = native_kind(&kinds, instr.b());
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                        return None;
                    }
                    continue;
                }
                let Some(kind) = native_kind(&kinds, instr.b()) else {
                    return None;
                };
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                    return None;
                }
            }
            Opcode32::AddFloat | Opcode32::SubFloat | Opcode32::MulFloat | Opcode32::DivFloat | Opcode32::ModFloat => {
                if native_kind(&kinds, instr.b()) != Some(NativeScalarKind::F64)
                    || native_kind(&kinds, instr.c()) != Some(NativeScalarKind::F64)
                    || !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::F64)
                {
                    return None;
                }
            }
            Opcode32::AddInt | Opcode32::SubInt | Opcode32::MulInt | Opcode32::DivInt | Opcode32::ModInt => {
                let Some(lhs) = native_kind(&kinds, instr.b()) else {
                    return None;
                };
                let Some(rhs) = native_kind(&kinds, instr.c()) else {
                    return None;
                };
                let lhs_is_i64 = matches!(lhs, NativeScalarKind::I64 | NativeScalarKind::MaybeI64);
                let rhs_is_i64 = matches!(rhs, NativeScalarKind::I64 | NativeScalarKind::MaybeI64);
                if (!lhs.is_numeric() && lhs != NativeScalarKind::MaybeI64)
                    || (!rhs.is_numeric() && rhs != NativeScalarKind::MaybeI64)
                {
                    return None;
                }
                let out = if lhs_is_i64 && rhs_is_i64 {
                    NativeScalarKind::I64
                } else if lhs == NativeScalarKind::F64 || rhs == NativeScalarKind::F64 {
                    NativeScalarKind::F64
                } else {
                    NativeScalarKind::I64
                };
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), out) {
                    return None;
                }
            }
            Opcode32::CmpInt
            | Opcode32::CmpNeInt
            | Opcode32::CmpLtInt
            | Opcode32::CmpLeInt
            | Opcode32::CmpGtInt
            | Opcode32::CmpGeInt => {
                let Some(lhs) = native_kind(&kinds, instr.b()) else {
                    return None;
                };
                let Some(rhs) = native_kind(&kinds, instr.c()) else {
                    return None;
                };
                if (lhs != rhs || !lhs.is_numeric()) && !matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt)
                {
                    return None;
                }
                if matches!(instr.opcode(), Opcode32::CmpInt | Opcode32::CmpNeInt)
                    && lhs != rhs
                    && (lhs.is_numeric() || rhs.is_numeric())
                {
                    return None;
                }
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::Bool) {
                    return None;
                }
            }
            Opcode32::Test => {
                if native_kind(&kinds, instr.a()).is_none() {
                    return None;
                }
            }
            Opcode32::Not => {
                let Some(kind) = native_kind(&kinds, instr.b()) else {
                    return None;
                };
                if !matches!(kind, NativeScalarKind::Bool | NativeScalarKind::Nil)
                    || !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::Bool)
                {
                    return None;
                }
            }
            Opcode32::IsNil => {
                if native_kind(&kinds, instr.b()).is_none()
                    || !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::Bool)
                {
                    return None;
                }
            }
            Opcode32::ToString => {
                let Some(value) = static_kind(&static_values, instr.b())
                    .or_else(|| native_kind(&kinds, instr.b()).and_then(|kind| dynamic_text_part(kind, instr.b())))
                else {
                    return None;
                };
                let Some(text) = text_value_from_native(value) else {
                    return None;
                };
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, text) {
                    return None;
                }
            }
            Opcode32::ConcatString => {
                let Some(lhs) = static_kind(&static_values, instr.b())
                    .or_else(|| native_kind(&kinds, instr.b()).and_then(|kind| dynamic_text_part(kind, instr.b())))
                else {
                    return None;
                };
                let Some(rhs) = static_kind(&static_values, instr.c())
                    .or_else(|| native_kind(&kinds, instr.c()).and_then(|kind| dynamic_text_part(kind, instr.c())))
                else {
                    return None;
                };
                let Some(text) = concat_text_values(lhs, rhs) else {
                    return None;
                };
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, text) {
                    return None;
                }
            }
            Opcode32::Len => {
                let Some(target) = static_kind(&static_values, instr.b()) else {
                    return None;
                };
                if !native_dynamic_text_len_supported(&target)
                    || !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64)
                {
                    return None;
                }
            }
            Opcode32::StringSplit => {
                let Some(target) = static_kind(&static_values, instr.b()) else {
                    return None;
                };
                let Some(delimiter) = static_kind(&static_values, instr.c()) else {
                    return None;
                };
                let (NativeStraightlineValue::Text(text), NativeStraightlineValue::String { value: delimiter, .. }) =
                    (target, delimiter)
                else {
                    return None;
                };
                if !delimiter.is_ascii()
                    || !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        None,
                        NativeStraightlineValue::DynamicSplitText { text, delimiter },
                    )
                {
                    return None;
                }
            }
            Opcode32::ListJoin => {
                let Some(target) = static_kind(&static_values, instr.b()) else {
                    return None;
                };
                let Some(delimiter) = static_kind(&static_values, instr.c()) else {
                    return None;
                };
                if let (
                    NativeStraightlineValue::DynamicTextList { id },
                    NativeStraightlineValue::String { value: delimiter, .. },
                ) = (&target, &delimiter)
                {
                    if !delimiter.is_ascii()
                        || !set_static_value(
                            &mut kinds,
                            &mut static_values,
                            instr.a(),
                            None,
                            NativeStraightlineValue::DynamicJoinedText {
                                id: *id,
                                delimiter_len: delimiter.len(),
                            },
                        )
                    {
                        return None;
                    }
                    continue;
                }
                let (
                    NativeStraightlineValue::DynamicSplitText {
                        text,
                        delimiter: split_delimiter,
                    },
                    NativeStraightlineValue::String {
                        value: join_delimiter, ..
                    },
                ) = (target, delimiter)
                else {
                    return None;
                };
                if split_delimiter != join_delimiter
                    || !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        None,
                        NativeStraightlineValue::Text(text),
                    )
                {
                    return None;
                }
            }
            Opcode32::StringStartsWith => {
                let Some(prefix) = static_kind(&static_values, instr.c()) else {
                    return None;
                };
                let NativeStraightlineValue::String { value: prefix, .. } = prefix else {
                    return None;
                };
                if let Some(NativeStraightlineValue::String { value: target, .. }) =
                    static_kind(&static_values, instr.b())
                {
                    let value = i64::from(target.starts_with(&prefix)).to_string();
                    if !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        Some(NativeScalarKind::Bool),
                        NativeStraightlineValue::Bool(value),
                    ) {
                        return None;
                    }
                } else if native_kind(&kinds, instr.b()) == Some(NativeScalarKind::StrPtr) {
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::Bool) {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            Opcode32::GetGlobal => {
                if let Some(value) = global_names
                    .get(instr.bx() as usize)
                    .and_then(|name| native_static_global(name))
                {
                    if !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        static_value_scalar_kind(&value),
                        value,
                    ) {
                        return None;
                    }
                    continue;
                }
                if let Some(value) = static_global(&static_globals, instr.bx()) {
                    if !set_static_value(
                        &mut kinds,
                        &mut static_values,
                        instr.a(),
                        static_value_scalar_kind(&value),
                        value,
                    ) {
                        return None;
                    }
                    continue;
                }
                let Some(kind) = native_global_kind(&global_kinds, instr.bx()) else {
                    return None;
                };
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                    return None;
                }
            }
            Opcode32::SetGlobal => {
                if let Some(value) = static_kind(&static_values, instr.a()) {
                    if !set_static_global(&mut static_globals, &mut global_kinds, instr.bx(), value) {
                        return None;
                    }
                } else {
                    let Some(kind) = native_kind(&kinds, instr.a()) else {
                        return None;
                    };
                    if !set_native_global_kind(&mut global_kinds, &mut static_globals, instr.bx(), kind) {
                        return None;
                    }
                }
            }
            Opcode32::GetIndex => {
                let Some(target) = static_kind(&static_values, instr.b()) else {
                    return None;
                };
                if matches!(target, NativeStraightlineValue::Text(_)) {
                    if native_kind(&kinds, instr.c()) != Some(NativeScalarKind::I64)
                        || !set_static_value(
                            &mut kinds,
                            &mut static_values,
                            instr.a(),
                            None,
                            NativeStraightlineValue::DynamicTextChar,
                        )
                    {
                        return None;
                    }
                } else if matches!(target, NativeStraightlineValue::DynamicIntList { .. }) {
                    if native_kind(&kinds, instr.c()) != Some(NativeScalarKind::I64)
                        || !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::I64)
                    {
                        return None;
                    }
                } else if matches!(target, NativeStraightlineValue::DynamicStringIntMap { .. }) {
                    let Some(key) = static_kind(&static_values, instr.c()) else {
                        return None;
                    };
                    if !native_string_int_map_key_supported(&key) {
                        return None;
                    }
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::MaybeI64) {
                        return None;
                    }
                } else if let NativeStraightlineValue::Map { entries, .. } = &target
                    && native_kind(&kinds, instr.c()) == Some(NativeScalarKind::StrPtr)
                    && static_string_i64_map_supported(entries)
                {
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), NativeScalarKind::MaybeI64) {
                        return None;
                    }
                } else {
                    let Some(key) = static_kind(&static_values, instr.c()) else {
                        return None;
                    };
                    let Some(value) = native_static_index(target, key, String::new()) else {
                        return None;
                    };
                    let kind = static_value_scalar_kind(&value);
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), kind, value) {
                        return None;
                    }
                }
            }
            Opcode32::SetIndex => {
                let Some(target) = static_kind(&static_values, instr.a()) else {
                    return None;
                };
                let Some(key) = static_kind(&static_values, instr.b()) else {
                    return None;
                };
                if matches!(target, NativeStraightlineValue::DynamicStringIntMap { .. }) {
                    if !native_string_int_map_key_supported(&key)
                        || native_kind(&kinds, instr.c()) != Some(NativeScalarKind::I64)
                    {
                        return None;
                    }
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, target) {
                        return None;
                    }
                } else {
                    let Some(value) = static_kind(&static_values, instr.c()) else {
                        return None;
                    };
                    let Some(value) = native_static_set_index(target, key, value) else {
                        return None;
                    };
                    if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                        return None;
                    }
                }
            }
            Opcode32::ListPush => {
                let Some(target) = static_kind(&static_values, instr.a()) else {
                    return None;
                };
                match target {
                    NativeStraightlineValue::DynamicIntList { id } => {
                        if native_kind(&kinds, instr.b()) == Some(NativeScalarKind::I64) {
                            if !set_static_value(
                                &mut kinds,
                                &mut static_values,
                                instr.a(),
                                None,
                                NativeStraightlineValue::DynamicIntList { id },
                            ) {
                                return None;
                            }
                        } else {
                            let Some(value) = static_kind(&static_values, instr.b()) else {
                                return None;
                            };
                            if !native_dynamic_text_len_supported(&value)
                                || !set_static_value(
                                    &mut kinds,
                                    &mut static_values,
                                    instr.a(),
                                    None,
                                    NativeStraightlineValue::DynamicTextList { id },
                                )
                            {
                                return None;
                            }
                        }
                    }
                    NativeStraightlineValue::DynamicTextList { id } => {
                        let Some(value) = static_kind(&static_values, instr.b()) else {
                            return None;
                        };
                        if !native_dynamic_text_len_supported(&value)
                            || !set_static_value(
                                &mut kinds,
                                &mut static_values,
                                instr.a(),
                                None,
                                NativeStraightlineValue::DynamicTextList { id },
                            )
                        {
                            return None;
                        }
                    }
                    _ => return None,
                }
            }
            Opcode32::NewList => {
                let start = instr.b() as usize;
                let end = start.checked_add(instr.c() as usize)?;
                let Some(values) = static_values.get(start..end) else {
                    return None;
                };
                let Some(values) = values.iter().cloned().collect::<Option<Vec<_>>>() else {
                    return None;
                };
                let Some(value) = native_static_list_from_values(&values, String::new()) else {
                    return None;
                };
                if !set_static_value(&mut kinds, &mut static_values, instr.a(), None, value) {
                    return None;
                }
            }
            Opcode32::Call => {
                if instr.a() != instr.b() {
                    return None;
                }
                let Some(target) = static_kind(&static_values, instr.b()) else {
                    return None;
                };
                if let Some((function_index, captures)) = static_call_target(&target) {
                    let function_index = u8::try_from(function_index).ok()?;
                    let direct_instr = Instr32::abc(Opcode32::CallDirect, instr.a(), function_index, instr.c());
                    let kind = native_direct_call_return_kind(
                        functions?,
                        direct_instr,
                        &kinds,
                        &static_values,
                        &global_kinds,
                        &static_globals,
                        global_count,
                        global_names,
                        &captures,
                        depth,
                    )?;
                    if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                        return None;
                    }
                    continue;
                }
                let start = instr.b() as usize + 1;
                let end = start.checked_add(instr.c() as usize)?;
                let Some(args) = static_values.get(start..end) else {
                    return None;
                };
                let Some(args) = args.iter().cloned().collect::<Option<Vec<_>>>() else {
                    return None;
                };
                let Some(kind) = native_builtin_return_kind(target, &args) else {
                    return None;
                };
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                    return None;
                }
            }
            Opcode32::CallDirect => {
                let Some(kind) = native_direct_call_return_kind(
                    functions?,
                    instr,
                    &kinds,
                    &static_values,
                    &global_kinds,
                    &static_globals,
                    global_count,
                    global_names,
                    &[],
                    depth,
                ) else {
                    return None;
                };
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                    return None;
                }
            }
            Opcode32::CallNamed => {
                let Some(target) = static_kind(&static_values, instr.a()) else {
                    return None;
                };
                let Some((function_index, captures)) = static_call_target(&target) else {
                    return None;
                };
                let function = functions?.get(function_index as usize)?;
                let args = native_named_call_args(
                    function,
                    &kinds,
                    &static_values,
                    instr.a(),
                    instr.bx() & 0x7f,
                    instr.bx() >> 7,
                )?;
                let kind = native_static_function_return_kind(
                    functions?,
                    function_index as usize,
                    &args,
                    &captures,
                    &global_kinds,
                    &static_globals,
                    global_count,
                    global_names,
                    depth,
                )?;
                if !set_native_kind(&mut kinds, &mut static_values, instr.a(), kind) {
                    return None;
                }
            }
            Opcode32::Return => {
                if instr.b() > 1 {
                    return None;
                }
                if instr.b() == 1 && native_kind(&kinds, instr.a()).is_none() {
                    return None;
                }
            }
            _ => {
                return None;
            }
        }
    }
    Some(NativeScalarFacts {
        registers_before,
        globals_before,
    })
}

fn native_kind(kinds: &[Option<NativeScalarKind>], reg: u8) -> Option<NativeScalarKind> {
    kinds.get(reg as usize).copied().flatten()
}

fn static_kind(values: &[Option<NativeStraightlineValue>], reg: u8) -> Option<NativeStraightlineValue> {
    values.get(reg as usize).cloned().flatten()
}

fn static_call_target(value: &NativeStraightlineValue) -> Option<(u16, Vec<NativeStraightlineValue>)> {
    match value {
        NativeStraightlineValue::Function(function_index) => Some((*function_index, Vec::new())),
        NativeStraightlineValue::Closure {
            function_index,
            captures,
        } => Some((*function_index, captures.clone())),
        _ => None,
    }
}

fn static_global(values: &[Option<NativeStraightlineValue>], slot: u16) -> Option<NativeStraightlineValue> {
    values.get(slot as usize).cloned().flatten()
}

fn native_arg_value(
    kinds: &[Option<NativeScalarKind>],
    values: &[Option<NativeStraightlineValue>],
    reg: usize,
) -> Option<NativeStraightlineValue> {
    if let Some(value) = values.get(reg).cloned().flatten() {
        return Some(value);
    }
    let kind = kinds.get(reg).copied().flatten()?;
    let value = format!("%call_arg_r{reg}");
    match kind {
        NativeScalarKind::I64 | NativeScalarKind::MaybeI64 => Some(NativeStraightlineValue::I64(value)),
        NativeScalarKind::F64 => Some(NativeStraightlineValue::F64(value)),
        NativeScalarKind::Bool => Some(NativeStraightlineValue::Bool(value)),
        NativeScalarKind::Nil => Some(NativeStraightlineValue::Nil),
        NativeScalarKind::StrPtr => Some(NativeStraightlineValue::StringPtr(value)),
    }
}

fn native_named_call_args(
    function: &Function32Data,
    kinds: &[Option<NativeScalarKind>],
    values: &[Option<NativeStraightlineValue>],
    callee: u8,
    positional_count: u16,
    named_count: u16,
) -> Option<Vec<NativeStraightlineValue>> {
    let total_count = function.param_count as usize;
    let positional_count = positional_count as usize;
    let named_count = named_count as usize;
    if function.param_names.len() != total_count
        || function.positional_param_count as usize != positional_count
        || positional_count.checked_add(named_count)? != total_count
    {
        return None;
    }

    let positional_start = callee as usize + 1;
    let positional_end = positional_start.checked_add(positional_count)?;
    let named_start = positional_end;
    let named_width = named_count.checked_mul(2)?;
    let named_end = named_start.checked_add(named_width)?;
    if named_end > values.len() || named_end > kinds.len() {
        return None;
    }

    let mut args = vec![None; total_count];
    for (offset, slot) in args[..positional_count].iter_mut().enumerate() {
        *slot = Some(native_arg_value(kinds, values, positional_start + offset)?);
    }

    let mut seen = vec![false; named_count];
    for pair_start in (named_start..named_end).step_by(2) {
        let Some(NativeStraightlineValue::String { value: name, .. }) = values[pair_start].clone() else {
            return None;
        };
        let offset = function.param_names[positional_count..]
            .iter()
            .position(|param| param == &name)?;
        if std::mem::replace(&mut seen[offset], true) {
            return None;
        }
        args[positional_count + offset] = Some(native_arg_value(kinds, values, pair_start + 1)?);
    }

    if seen.iter().any(|seen| !seen) {
        return None;
    }
    args.into_iter().collect()
}

fn set_native_kind(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    reg: u8,
    kind: NativeScalarKind,
) -> bool {
    let Some(slot) = kinds.get_mut(reg as usize) else {
        return false;
    };
    *slot = Some(kind);
    if let Some(value) = static_values.get_mut(reg as usize) {
        *value = None;
    }
    true
}

fn set_static_value(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    reg: u8,
    scalar_kind: Option<NativeScalarKind>,
    value: NativeStraightlineValue,
) -> bool {
    let Some(slot) = static_values.get_mut(reg as usize) else {
        return false;
    };
    *slot = Some(value);
    if let Some(kind) = kinds.get_mut(reg as usize) {
        *kind = scalar_kind;
    }
    true
}

fn native_global_kind(kinds: &[Option<NativeScalarKind>], slot: u16) -> Option<NativeScalarKind> {
    kinds.get(slot as usize).copied().flatten()
}

fn set_native_global_kind(
    kinds: &mut [Option<NativeScalarKind>],
    static_globals: &mut [Option<NativeStraightlineValue>],
    slot: u16,
    kind: NativeScalarKind,
) -> bool {
    let index = slot as usize;
    let Some(slot) = kinds.get_mut(index) else {
        return false;
    };
    *slot = Some(kind);
    if let Some(value) = static_globals.get_mut(index) {
        *value = None;
    }
    true
}

fn set_static_global(
    static_globals: &mut [Option<NativeStraightlineValue>],
    kinds: &mut [Option<NativeScalarKind>],
    slot: u16,
    value: NativeStraightlineValue,
) -> bool {
    let Some(static_slot) = static_globals.get_mut(slot as usize) else {
        return false;
    };
    *static_slot = Some(value);
    if let Some(kind) = kinds.get_mut(slot as usize) {
        *kind = None;
    }
    true
}

fn native_builtin_return_kind(
    target: NativeStraightlineValue,
    args: &[NativeStraightlineValue],
) -> Option<NativeScalarKind> {
    match target {
        NativeStraightlineValue::Builtin(NativeBuiltin::OsClock) if args.is_empty() => Some(NativeScalarKind::F64),
        NativeStraightlineValue::Builtin(NativeBuiltin::OsEpoch) if args.is_empty() => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreCallMethod) => native_core_method_return_kind(args),
        NativeStraightlineValue::Builtin(NativeBuiltin::Print | NativeBuiltin::Println) if args.len() <= 1 => {
            Some(NativeScalarKind::Nil)
        }
        _ => None,
    }
}

fn native_core_method_return_kind(args: &[NativeStraightlineValue]) -> Option<NativeScalarKind> {
    let [
        NativeStraightlineValue::Module(super::straightline_value::NativeModule::OsEnv),
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
    else {
        return None;
    };
    if method == "get" && elements.len() == 2 {
        Some(NativeScalarKind::StrPtr)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn native_static_function_return_kind(
    functions: &[Function32Data],
    function_index: usize,
    args: &[NativeStraightlineValue],
    captures: &[NativeStraightlineValue],
    global_kinds: &[Option<NativeScalarKind>],
    static_globals: &[Option<NativeStraightlineValue>],
    global_count: usize,
    global_names: &[String],
    depth: usize,
) -> Option<NativeScalarKind> {
    if depth >= 8 {
        return None;
    }
    let callee = functions.get(function_index)?;
    if callee.capture_count as usize != captures.len() || callee.param_count as usize != args.len() {
        return None;
    }
    let code = callee
        .code
        .iter()
        .copied()
        .map(Instr32::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let mut callee_kinds = vec![None; callee.register_count as usize];
    let mut callee_static_values = vec![None; callee.register_count as usize];
    for (arg, value) in args.iter().cloned().enumerate() {
        *callee_kinds.get_mut(arg)? = static_value_scalar_kind(&value);
        *callee_static_values.get_mut(arg)? = Some(value);
    }
    let facts = native_scalar_block_facts_with_initial(
        callee.register_count as usize,
        global_count,
        global_names,
        &callee.consts.strings,
        &callee.consts.heap_values,
        &code,
        callee_kinds,
        callee_static_values,
        global_kinds.to_vec(),
        static_globals.to_vec(),
        Some(functions),
        captures,
        depth + 1,
    )?;
    native_return_kind_from_facts(&code, &facts)
}

#[allow(clippy::too_many_arguments)]
fn native_direct_call_return_kind(
    functions: &[Function32Data],
    instr: Instr32,
    caller_kinds: &[Option<NativeScalarKind>],
    caller_static_values: &[Option<NativeStraightlineValue>],
    global_kinds: &[Option<NativeScalarKind>],
    static_globals: &[Option<NativeStraightlineValue>],
    global_count: usize,
    global_names: &[String],
    captures: &[NativeStraightlineValue],
    depth: usize,
) -> Option<NativeScalarKind> {
    if depth >= 8 {
        return None;
    }
    let callee = functions.get(instr.b() as usize)?;
    if callee.capture_count as usize != captures.len() || instr.c() as u16 != callee.param_count {
        return None;
    }
    let code = callee
        .code
        .iter()
        .copied()
        .map(Instr32::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let mut callee_kinds = vec![None; callee.register_count as usize];
    let mut callee_static_values = vec![None; callee.register_count as usize];
    for arg in 0..instr.c() as usize {
        let caller_reg = instr.a() as usize + 1 + arg;
        let static_value = caller_static_values.get(caller_reg).cloned().flatten();
        if let Some(kind) = caller_kinds.get(caller_reg).copied().flatten() {
            *callee_kinds.get_mut(arg)? = Some(kind);
        } else if static_value.is_none() {
            return None;
        }
        *callee_static_values.get_mut(arg)? = static_value;
    }
    let facts = native_scalar_block_facts_with_initial(
        callee.register_count as usize,
        global_count,
        global_names,
        &callee.consts.strings,
        &callee.consts.heap_values,
        &code,
        callee_kinds,
        callee_static_values,
        global_kinds.to_vec(),
        static_globals.to_vec(),
        Some(functions),
        captures,
        depth + 1,
    )?;
    native_return_kind_from_facts(&code, &facts)
}

fn native_return_kind_from_facts(code: &[Instr32], facts: &NativeScalarFacts) -> Option<NativeScalarKind> {
    let mut return_kind = None;
    for (pc, instr) in code.iter().copied().enumerate() {
        if instr.opcode() != Opcode32::Return {
            continue;
        }
        if instr.b() != 1 {
            return None;
        }
        let kind = facts.register_kind_before(pc, instr.a())?;
        if return_kind.replace(kind).is_some_and(|previous| previous != kind) {
            return None;
        }
    }
    return_kind
}

fn native_static_heap_const_value(
    value: &ConstHeapValue32Data,
) -> Option<(Option<NativeScalarKind>, NativeStraightlineValue)> {
    match value {
        ConstHeapValue32Data::LongString(value) => Some((
            Some(NativeScalarKind::StrPtr),
            NativeStraightlineValue::String {
                symbol: String::new(),
                value: value.clone(),
                len: value.chars().count(),
                key_kind: super::straightline_value::native_runtime_string_key_kind(value),
            },
        )),
        ConstHeapValue32Data::List(values) if values.is_empty() => {
            Some((None, NativeStraightlineValue::DynamicIntList { id: 0 }))
        }
        ConstHeapValue32Data::Map(values) if values.is_empty() => {
            Some((None, NativeStraightlineValue::DynamicStringIntMap { id: 0 }))
        }
        ConstHeapValue32Data::List(_) | ConstHeapValue32Data::Map(_) | ConstHeapValue32Data::UpvalCell(_) => {
            Some((None, native_straightline_heap_const_value(0, 0, value)?))
        }
    }
}

fn static_value_scalar_kind(value: &NativeStraightlineValue) -> Option<NativeScalarKind> {
    match value {
        NativeStraightlineValue::I64(_) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::F64(_) => Some(NativeScalarKind::F64),
        NativeStraightlineValue::Bool(_) => Some(NativeScalarKind::Bool),
        NativeStraightlineValue::Nil => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::String { .. } | NativeStraightlineValue::StringPtr(_) => {
            Some(NativeScalarKind::StrPtr)
        }
        _ => None,
    }
}

fn dynamic_text_part(kind: NativeScalarKind, reg: u8) -> Option<NativeStraightlineValue> {
    let slot = format!("%call_arg_r{reg}");
    let part = match kind {
        NativeScalarKind::I64 => NativeTextPart::I64(slot),
        NativeScalarKind::F64 => NativeTextPart::F64(slot),
        NativeScalarKind::Bool => NativeTextPart::Bool(slot),
        NativeScalarKind::Nil => NativeTextPart::Nil,
        NativeScalarKind::StrPtr => NativeTextPart::StrPtr(slot),
        NativeScalarKind::MaybeI64 => return None,
    };
    Some(NativeStraightlineValue::Text(vec![part]))
}

fn text_value_from_native(value: NativeStraightlineValue) -> Option<NativeStraightlineValue> {
    let parts = match value {
        NativeStraightlineValue::Text(parts) => parts,
        NativeStraightlineValue::I64(value) => vec![NativeTextPart::I64(value)],
        NativeStraightlineValue::F64(value) => vec![NativeTextPart::F64(value)],
        NativeStraightlineValue::Bool(value) => vec![NativeTextPart::Bool(value)],
        NativeStraightlineValue::Nil => vec![NativeTextPart::Nil],
        NativeStraightlineValue::StringPtr(value) => vec![NativeTextPart::StrPtr(value)],
        NativeStraightlineValue::String { symbol, value, .. } => vec![NativeTextPart::String { symbol, value }],
        _ => return None,
    };
    Some(NativeStraightlineValue::Text(parts))
}

fn concat_text_values(lhs: NativeStraightlineValue, rhs: NativeStraightlineValue) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::Text(mut lhs) = text_value_from_native(lhs)? else {
        return None;
    };
    let NativeStraightlineValue::Text(rhs) = text_value_from_native(rhs)? else {
        return None;
    };
    lhs.extend(rhs);
    Some(NativeStraightlineValue::Text(lhs))
}

fn native_string_int_map_key_supported(value: &NativeStraightlineValue) -> bool {
    let NativeStraightlineValue::Text(parts) = value else {
        return matches!(value, NativeStraightlineValue::String { value, .. } if value.is_ascii());
    };
    let Some((last, prefix)) = parts.split_last() else {
        return false;
    };
    matches!(last, NativeTextPart::I64(_))
        && !prefix.is_empty()
        && prefix.iter().all(|part| matches!(part, NativeTextPart::String { .. }))
}

fn native_dynamic_text_len_supported(value: &NativeStraightlineValue) -> bool {
    match value {
        NativeStraightlineValue::String { value, .. } => value.is_ascii(),
        NativeStraightlineValue::DynamicJoinedText { .. } => true,
        NativeStraightlineValue::DynamicTextChar => true,
        NativeStraightlineValue::Text(parts) => parts.iter().all(|part| match part {
            NativeTextPart::String { value, .. } => value.is_ascii(),
            NativeTextPart::I64(_) => true,
            _ => false,
        }),
        _ => false,
    }
}
