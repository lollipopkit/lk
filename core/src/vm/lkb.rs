//! LKB bytecode module encapsulation and serialization.
//!
//! Provides a minimal container format for compiled `Function`s together with
//! module-level metadata and feature flags. The current format is intentionally
//! simple to keep the encoder/decoder easy to audit and evolve.

use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use arcstr::ArcStr;
use serde::{Deserialize, Serialize};

use crate::{
    util::fast_map::{FastHashMap, fast_hash_map_with_capacity},
    val::{Type, Val},
};

use super::alloc::{AllocationRegion, RegionPlan};
use super::analysis::{EscapeClass, EscapeSummary, FunctionAnalysis};
use super::bytecode::{CaptureSpec, ClosureProto, Function, NamedParamLayoutEntry, PatternPlan};
use op_codec::{decode_op, encode_op};

mod op_codec;

const MAGIC: [u8; 3] = *b"LKB";
pub const CURRENT_VERSION: u16 = 9;

/// Flags describing optimisation passes that were applied when emitting the module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModuleFlags(u32);

impl ModuleFlags {
    pub const NONE: ModuleFlags = ModuleFlags(0);
    pub const CONST_FOLDED: ModuleFlags = ModuleFlags(1 << 0);
    pub const TREE_SHAKEN: ModuleFlags = ModuleFlags(1 << 1);

    #[inline]
    pub const fn bits(self) -> u32 {
        self.0
    }

    #[inline]
    pub const fn from_bits(bits: u32) -> ModuleFlags {
        ModuleFlags(bits)
    }

    #[inline]
    pub const fn contains(self, other: ModuleFlags) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline]
    pub fn insert(&mut self, other: ModuleFlags) {
        self.0 |= other.0;
    }
}

/// Optional metadata describing the source that produced the bytecode module.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModuleMeta {
    /// Original source path or identifier (if available).
    pub source: Option<String>,
    /// Optional hex-encoded checksum of the source/plain AST.
    pub checksum: Option<String>,
    /// Additional string key/value annotations.
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}

impl ModuleMeta {
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.source.is_none() && self.checksum.is_none() && self.tags.is_empty()
    }
}

/// Complete LKB module payload (single entry function + optional metadata).
#[derive(Debug, Clone)]
pub struct BytecodeModule {
    pub version: u16,
    pub flags: ModuleFlags,
    pub entry: Function,
    pub meta: Option<ModuleMeta>,
    /// Additional modules bundled together with this entry.
    pub bundled_modules: Vec<BundledModule>,
    /// Raw debug blob (reserved for future source maps).
    pub debug: Option<Vec<u8>>,
}

impl BytecodeModule {
    pub fn new(entry: Function) -> Self {
        Self {
            version: CURRENT_VERSION,
            flags: ModuleFlags::NONE,
            entry,
            meta: None,
            bundled_modules: Vec::new(),
            debug: None,
        }
    }
}

/// A module embedded inside another LKB payload.
#[derive(Debug, Clone)]
pub struct BundledModule {
    /// Filesystem path used to resolve the module at runtime.
    pub path: String,
    /// Compiled bytecode for the bundled module.
    pub module: BytecodeModule,
}

/// Encode a module into the LKB binary representation.
pub fn encode_module(module: &BytecodeModule) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    write_u16(&mut out, module.version);
    write_u16(&mut out, 0); // reserved
    write_u32(&mut out, module.flags.bits());

    let func_payload = encode_function(&module.entry, module.version)?;
    write_section(&mut out, *b"FUNC", &func_payload);

    if let Some(meta) = module.meta.as_ref().filter(|m| !m.is_empty()) {
        let meta_payload = serde_json::to_vec(meta)?;
        write_section(&mut out, *b"META", &meta_payload);
    }

    if !module.bundled_modules.is_empty() {
        let bundled_payload = encode_bundled_modules(&module.bundled_modules)?;
        write_section(&mut out, *b"MODS", &bundled_payload);
    }

    if let Some(debug) = module.debug.as_ref() {
        write_section(&mut out, *b"DBG!", debug);
    }

    Ok(out)
}

/// Decode an LKB binary payload back into a module.
pub fn decode_module(bytes: &[u8]) -> Result<BytecodeModule> {
    ensure!(bytes.len() >= MAGIC.len() + 8, "module too small");
    ensure!(bytes.starts_with(&MAGIC), "invalid LKB magic");

    let mut cursor = MAGIC.len();
    let version = read_u16(bytes, &mut cursor)?;
    let _reserved = read_u16(bytes, &mut cursor)?;
    let flags_bits = read_u32(bytes, &mut cursor)?;

    ensure!(
        version <= CURRENT_VERSION,
        "unsupported LKB version {} (reader supports <= {})",
        version,
        CURRENT_VERSION
    );

    let mut entry: Option<Function> = None;
    let mut meta: Option<ModuleMeta> = None;
    let mut debug: Option<Vec<u8>> = None;
    let mut bundled_modules: Vec<BundledModule> = Vec::new();

    while cursor < bytes.len() {
        let tag = read_tag(bytes, &mut cursor)?;
        let len = read_u32(bytes, &mut cursor)? as usize;
        ensure!(cursor + len <= bytes.len(), "section overruns payload");
        let payload = &bytes[cursor..cursor + len];
        cursor += len;

        match &tag {
            b"FUNC" => {
                ensure!(entry.is_none(), "duplicate FUNC section");
                entry = Some(decode_function(payload, version)?);
            }
            b"META" => {
                ensure!(meta.is_none(), "duplicate META section");
                meta = Some(serde_json::from_slice(payload)?);
            }
            b"MODS" => {
                ensure!(bundled_modules.is_empty(), "duplicate MODS section");
                bundled_modules = decode_bundled_modules(payload)?;
            }
            b"DBG!" => {
                ensure!(debug.is_none(), "duplicate DBG! section");
                debug = Some(payload.to_vec());
            }
            _ => {
                // Unknown sections are skipped for forward compatibility.
            }
        }
    }

    ensure!(cursor == bytes.len(), "extra data at end of module");
    let entry = entry.ok_or_else(|| anyhow::anyhow!("missing FUNC section"))?;

    Ok(BytecodeModule {
        version,
        flags: ModuleFlags::from_bits(flags_bits),
        entry,
        meta,
        bundled_modules,
        debug,
    })
}

fn write_section(out: &mut Vec<u8>, tag: [u8; 4], payload: &[u8]) {
    out.extend_from_slice(&tag);
    write_u32(out, payload.len() as u32);
    out.extend_from_slice(payload);
}

fn encode_bundled_modules(modules: &[BundledModule]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_u32(&mut out, modules.len() as u32);
    for bundled in modules {
        write_str(&mut out, &bundled.path);
        let bytes = encode_module(&bundled.module)?;
        write_u32(&mut out, bytes.len() as u32);
        out.extend_from_slice(&bytes);
    }
    Ok(out)
}

fn decode_bundled_modules(bytes: &[u8]) -> Result<Vec<BundledModule>> {
    let mut cursor = 0usize;
    let count = read_u32(bytes, &mut cursor)? as usize;
    let mut modules = Vec::with_capacity(count);
    for _ in 0..count {
        let path = read_string(bytes, &mut cursor)?;
        let len = read_u32(bytes, &mut cursor)? as usize;
        ensure!(cursor + len <= bytes.len(), "embedded module overruns payload");
        let nested = decode_module(&bytes[cursor..cursor + len])?;
        cursor += len;
        modules.push(BundledModule { path, module: nested });
    }
    ensure!(cursor == bytes.len(), "unexpected trailing data in MODS section");
    Ok(modules)
}

fn encode_function(func: &Function, version: u16) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_u16(&mut out, func.n_regs);
    ensure!(
        func.param_regs.len() <= u16::MAX as usize,
        "parameter register list too long"
    );
    write_u16(&mut out, func.param_regs.len() as u16);
    for reg in &func.param_regs {
        write_u16(&mut out, *reg);
    }
    if version >= 5 {
        ensure!(
            func.named_param_regs.len() <= u16::MAX as usize,
            "named parameter register list too long"
        );
        write_u16(&mut out, func.named_param_regs.len() as u16);
        for reg in &func.named_param_regs {
            write_u16(&mut out, *reg);
        }
        ensure!(
            func.named_param_layout.len() <= u16::MAX as usize,
            "named parameter layout too long"
        );
        write_u16(&mut out, func.named_param_layout.len() as u16);
        for entry in &func.named_param_layout {
            write_u16(&mut out, entry.name_const_idx);
            write_u16(&mut out, entry.dest_reg);
            let default_idx = entry.default_index.map_or(u16::MAX, |idx| idx);
            write_u16(&mut out, default_idx);
        }
    }

    ensure!(func.consts.len() <= u32::MAX as usize, "constant pool too large");
    write_u32(&mut out, func.consts.len() as u32);
    for val in &func.consts {
        encode_val(&mut out, val)?;
    }

    ensure!(func.code.len() <= u32::MAX as usize, "instruction stream too large");
    write_u32(&mut out, func.code.len() as u32);
    for op in &func.code {
        encode_op(&mut out, op)?;
    }

    if version >= 6 {
        ensure!(
            func.pattern_plans.len() <= u16::MAX as usize,
            "pattern plan table too large"
        );
        write_u16(&mut out, func.pattern_plans.len() as u16);
        for plan in &func.pattern_plans {
            let payload = serde_json::to_vec(plan)?;
            ensure!(payload.len() <= u32::MAX as usize, "pattern plan payload too large");
            write_u32(&mut out, payload.len() as u32);
            out.extend_from_slice(&payload);
        }
    }

    if version >= 7 {
        if let Some(analysis) = func.analysis.as_ref() {
            write_u8(&mut out, 1);
            encode_analysis(&mut out, analysis)?;
        } else {
            write_u8(&mut out, 0);
        }
    }

    // Encode nested closure prototypes for version >= 2
    if version >= 2 {
        ensure!(func.protos.len() <= u16::MAX as usize, "too many nested closures");
        write_u16(&mut out, func.protos.len() as u16);
        for proto in &func.protos {
            // params
            ensure!(
                proto.params.len() <= u16::MAX as usize,
                "too many params in nested closure"
            );
            write_u16(&mut out, proto.params.len() as u16);
            for p in proto.params.iter() {
                write_str(&mut out, p);
            }
            if version >= 9 {
                let payload = serde_json::to_vec(proto.param_types.as_ref())?;
                ensure!(
                    payload.len() <= u32::MAX as usize,
                    "nested closure param type payload too large"
                );
                write_u32(&mut out, payload.len() as u32);
                out.extend_from_slice(&payload);
            }
            if version >= 3 {
                if let Some(name) = &proto.self_name {
                    write_u8(&mut out, 1);
                    write_str(&mut out, name);
                } else {
                    write_u8(&mut out, 0);
                }
            }
            if version >= 4 {
                ensure!(
                    proto.captures.len() <= u16::MAX as usize,
                    "too many captures in nested closure"
                );
                write_u16(&mut out, proto.captures.len() as u16);
                for cap in proto.captures.iter() {
                    match cap {
                        CaptureSpec::Register { name, src } => {
                            write_u8(&mut out, 0);
                            write_u16(&mut out, *src);
                            write_str(&mut out, name);
                        }
                        CaptureSpec::Const { name, kidx } => {
                            write_u8(&mut out, 1);
                            write_u16(&mut out, *kidx);
                            write_str(&mut out, name);
                        }
                        CaptureSpec::Global { name } => {
                            write_u8(&mut out, 2);
                            write_str(&mut out, name);
                        }
                    }
                }
            }
            // nested function payload
            let nested_fun = if let Some(f) = &proto.func {
                f.as_ref()
            } else {
                // Fallback: compile from body if needed
                let compiled = crate::vm::Compiler::new().compile_function_with_param_types_and_captures(
                    proto.params.as_ref(),
                    proto.param_types.as_ref(),
                    proto.named_params.as_ref(),
                    proto.body.as_ref(),
                    proto.captures.as_ref(),
                );
                // allocate to keep alive
                // We’ll encode directly from compiled temporary
                // by shadowing the reference below
                // slightly wasteful but fine for rare fallback
                // Use a local to appease borrow checker
                // Encode using current version to keep format consistent
                // Note: we can't return a ref to temporary; instead, encode directly here
                let bytes = encode_function(&compiled, version)?;
                write_u32(&mut out, bytes.len() as u32);
                out.extend_from_slice(&bytes);
                continue;
            };
            let nested_bytes = encode_function(nested_fun, version)?;
            write_u32(&mut out, nested_bytes.len() as u32);
            out.extend_from_slice(&nested_bytes);
        }
    }

    Ok(out)
}

fn decode_function(bytes: &[u8], version: u16) -> Result<Function> {
    let mut cursor = 0usize;
    ensure!(bytes.len() >= 6, "function payload too small");
    let n_regs = read_u16(bytes, &mut cursor)?;
    let param_len = read_u16(bytes, &mut cursor)? as usize;
    let mut param_regs = Vec::with_capacity(param_len);
    for _ in 0..param_len {
        param_regs.push(read_u16(bytes, &mut cursor)?);
    }
    let mut named_param_regs = Vec::new();
    let mut named_param_layout = Vec::new();
    if version >= 5 {
        let count = read_u16(bytes, &mut cursor)? as usize;
        named_param_regs.reserve(count);
        for _ in 0..count {
            named_param_regs.push(read_u16(bytes, &mut cursor)?);
        }
        let layout_count = read_u16(bytes, &mut cursor)? as usize;
        named_param_layout.reserve(layout_count);
        for _ in 0..layout_count {
            let name_const_idx = read_u16(bytes, &mut cursor)?;
            let dest_reg = read_u16(bytes, &mut cursor)?;
            let default_raw = read_u16(bytes, &mut cursor)?;
            let default_index = if default_raw == u16::MAX {
                None
            } else {
                Some(default_raw)
            };
            named_param_layout.push(NamedParamLayoutEntry {
                name_const_idx,
                dest_reg,
                default_index,
            });
        }
    }

    let const_len = read_u32(bytes, &mut cursor)? as usize;
    let mut consts = Vec::with_capacity(const_len);
    for _ in 0..const_len {
        consts.push(decode_val(bytes, &mut cursor)?);
    }

    let code_len = read_u32(bytes, &mut cursor)? as usize;
    let mut code = Vec::with_capacity(code_len);
    for _ in 0..code_len {
        code.push(decode_op(bytes, &mut cursor)?);
    }

    let mut pattern_plans = Vec::new();
    if version >= 6 {
        let count = read_u16(bytes, &mut cursor)? as usize;
        pattern_plans.reserve(count);
        for _ in 0..count {
            let len = read_u32(bytes, &mut cursor)? as usize;
            ensure!(
                cursor + len <= bytes.len(),
                "pattern plan payload overruns function section"
            );
            let slice = &bytes[cursor..cursor + len];
            cursor += len;
            let plan: PatternPlan = serde_json::from_slice(slice)?;
            pattern_plans.push(plan);
        }
    }

    let mut analysis = None;
    if version >= 7 {
        let has_analysis = read_u8(bytes, &mut cursor)? != 0;
        if has_analysis {
            analysis = Some(decode_analysis(bytes, &mut cursor)?);
        }
    }

    // In version >= 2, we may have trailing prototype sections.
    let mut protos: Vec<ClosureProto> = Vec::new();
    if version >= 2 && cursor < bytes.len() {
        let n = read_u16(bytes, &mut cursor)? as usize;
        protos.reserve(n);
        for _ in 0..n {
            // Read param names
            let pcount = read_u16(bytes, &mut cursor)? as usize;
            let mut params = Vec::with_capacity(pcount);
            for _ in 0..pcount {
                let s = read_string(bytes, &mut cursor)?;
                params.push(s);
            }
            let param_types: Vec<Option<Type>> = if version >= 9 {
                let payload_len = read_u32(bytes, &mut cursor)? as usize;
                ensure!(
                    cursor + payload_len <= bytes.len(),
                    "nested closure param type payload overruns function"
                );
                let parsed = serde_json::from_slice(&bytes[cursor..cursor + payload_len])?;
                cursor += payload_len;
                parsed
            } else {
                Vec::new()
            };
            let self_name = if version >= 3 {
                let has_name = read_u8(bytes, &mut cursor)? != 0;
                if has_name {
                    Some(read_string(bytes, &mut cursor)?)
                } else {
                    None
                }
            } else {
                None
            };
            let captures = if version >= 4 {
                let count = read_u16(bytes, &mut cursor)? as usize;
                let mut caps = Vec::with_capacity(count);
                for _ in 0..count {
                    let tag = read_u8(bytes, &mut cursor)?;
                    match tag {
                        0 => {
                            let src = read_u16(bytes, &mut cursor)?;
                            let name = read_string(bytes, &mut cursor)?;
                            caps.push(CaptureSpec::Register { name, src });
                        }
                        1 => {
                            let kidx = read_u16(bytes, &mut cursor)?;
                            let name = read_string(bytes, &mut cursor)?;
                            caps.push(CaptureSpec::Const { name, kidx });
                        }
                        2 => {
                            let name = read_string(bytes, &mut cursor)?;
                            caps.push(CaptureSpec::Global { name });
                        }
                        other => bail!("unknown capture tag {}", other),
                    }
                }
                caps
            } else {
                Vec::new()
            };
            // Read nested function payload
            let sz = read_u32(bytes, &mut cursor)? as usize;
            ensure!(cursor + sz <= bytes.len(), "nested function overruns payload");
            let nested = decode_function(&bytes[cursor..cursor + sz], version)?;
            cursor += sz;
            let func = Arc::new(nested);
            protos.push(ClosureProto {
                self_name,
                params: Arc::new(params),
                param_types: Arc::new(param_types),
                named_params: Arc::new(Vec::new()),
                default_funcs: Arc::new(Vec::new()),
                func: Some(Arc::clone(&func)),
                body: Arc::new(crate::stmt::Stmt::Empty),
                capture_names: crate::vm::capture_names_from_specs(&captures),
                captures: Arc::new(captures),
                code: crate::vm::closure_code_cell(Some(&func)),
                empty_env: crate::vm::closure_empty_env(),
                empty_upvalues: crate::vm::closure_empty_upvalues(),
                empty_captures: crate::vm::closure_empty_captures(),
                empty_closure: crate::vm::closure_empty_closure_cell(),
            });
        }
    }
    ensure!(cursor == bytes.len(), "unexpected trailing bytes in function section");

    let mut function = Function {
        consts,
        code,
        n_regs,
        protos,
        param_regs,
        named_param_regs,
        named_param_layout,
        pattern_plans,
        code32: None,
        bc32_decoded: None,
        analysis,
    };

    {
        if let Some(packed) = crate::vm::Bc32Function::try_from_function(&function) {
            function.bc32_decoded = packed.decoded;
            function.code32 = Some(packed.code32);
        }
    }

    Ok(function)
}

fn encode_analysis(out: &mut Vec<u8>, analysis: &FunctionAnalysis) -> Result<()> {
    let class_tag = match analysis.escape.return_class {
        EscapeClass::Trivial => 0u8,
        EscapeClass::Local => 1u8,
        EscapeClass::Escapes => 2u8,
    };
    write_u8(out, class_tag);

    ensure!(
        analysis.escape.escaping_values.len() <= u32::MAX as usize,
        "escaping value list too large"
    );
    write_u32(out, analysis.escape.escaping_values.len() as u32);
    for &value in &analysis.escape.escaping_values {
        ensure!(value <= u32::MAX as usize, "escaping value index too large");
        write_u32(out, value as u32);
    }

    ensure!(
        analysis.region_plan.values.len() <= u32::MAX as usize,
        "region plan value list too large"
    );
    write_u32(out, analysis.region_plan.values.len() as u32);
    for region in &analysis.region_plan.values {
        let tag = encode_region_tag(*region);
        write_u8(out, tag);
    }

    let return_region_tag = encode_region_tag(analysis.region_plan.return_region);
    write_u8(out, return_region_tag);

    Ok(())
}

fn decode_analysis(bytes: &[u8], cursor: &mut usize) -> Result<FunctionAnalysis> {
    let class_tag = read_u8(bytes, cursor)?;
    let return_class = decode_escape_class(class_tag)?;

    let escaping_len = read_u32(bytes, cursor)? as usize;
    let mut escaping_values = Vec::with_capacity(escaping_len);
    for _ in 0..escaping_len {
        escaping_values.push(read_u32(bytes, cursor)? as usize);
    }

    let values_len = read_u32(bytes, cursor)? as usize;
    let mut regions = Vec::with_capacity(values_len);
    for _ in 0..values_len {
        let tag = read_u8(bytes, cursor)?;
        regions.push(decode_region_tag(tag)?);
    }

    let return_region_tag = read_u8(bytes, cursor)?;
    let return_region = decode_region_tag(return_region_tag)?;

    Ok(FunctionAnalysis {
        ssa: None,
        escape: EscapeSummary {
            return_class,
            escaping_values,
        },
        region_plan: Arc::new(RegionPlan {
            values: regions,
            return_region,
        }),
    })
}

#[inline]
fn encode_region_tag(region: AllocationRegion) -> u8 {
    match region {
        AllocationRegion::ThreadLocal => 0,
        AllocationRegion::Heap => 1,
    }
}

fn decode_region_tag(tag: u8) -> Result<AllocationRegion> {
    Ok(match tag {
        0 => AllocationRegion::ThreadLocal,
        1 => AllocationRegion::Heap,
        other => bail!("unknown allocation region tag {}", other),
    })
}

fn decode_escape_class(tag: u8) -> Result<EscapeClass> {
    Ok(match tag {
        0 => EscapeClass::Trivial,
        1 => EscapeClass::Local,
        2 => EscapeClass::Escapes,
        other => bail!("unknown escape class tag {}", other),
    })
}

fn encode_val(out: &mut Vec<u8>, val: &Val) -> Result<()> {
    match val {
        Val::Nil => {
            write_u8(out, 0);
        }
        Val::Bool(b) => {
            write_u8(out, 1);
            write_u8(out, if *b { 1 } else { 0 });
        }
        Val::Int(i) => {
            write_u8(out, 2);
            write_i64(out, *i);
        }
        Val::Float(f) => {
            write_u8(out, 3);
            write_f64(out, *f);
        }
        Val::ShortStr(s) => {
            write_u8(out, 4);
            write_str(out, s.as_str());
        }
        Val::Str(s) => {
            write_u8(out, 4);
            write_str(out, s.as_str());
        }
        Val::List(items) => {
            write_u8(out, 5);
            ensure!(items.len() <= u32::MAX as usize, "list too large");
            write_u32(out, items.len() as u32);
            for item in items.iter() {
                encode_val(out, item)?;
            }
        }
        Val::Map(map) => {
            write_u8(out, 6);
            ensure!(map.len() <= u32::MAX as usize, "map too large");
            let mut entries: Vec<(String, &Val)> = map.iter().map(|(k, v)| (k.as_str().to_string(), v)).collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            write_u32(out, entries.len() as u32);
            for (key, value) in entries {
                write_str(out, &key);
                encode_val(out, value)?;
            }
        }
        unsupported => {
            bail!("cannot encode constant {:?} into LKB", unsupported.type_name());
        }
    }
    Ok(())
}

fn decode_val(bytes: &[u8], cursor: &mut usize) -> Result<Val> {
    let tag = read_u8(bytes, cursor)?;
    Ok(match tag {
        0 => Val::Nil,
        1 => {
            let b = read_u8(bytes, cursor)?;
            Val::Bool(b != 0)
        }
        2 => Val::Int(read_i64(bytes, cursor)?),
        3 => Val::Float(read_f64(bytes, cursor)?),
        4 => {
            let s = read_string(bytes, cursor)?;
            Val::from_str(s.as_str())
        }
        5 => {
            let len = read_u32(bytes, cursor)? as usize;
            let mut items = Vec::with_capacity(len);
            for _ in 0..len {
                items.push(decode_val(bytes, cursor)?);
            }
            Val::List(items.into())
        }
        6 => {
            let len = read_u32(bytes, cursor)? as usize;
            let mut map: FastHashMap<ArcStr, Val> = fast_hash_map_with_capacity(len);
            for _ in 0..len {
                let key = read_string(bytes, cursor)?;
                let value = decode_val(bytes, cursor)?;
                map.insert(Val::intern_str(key.as_str()), value);
            }
            Val::Map(Arc::new(map))
        }
        other => bail!("unknown value tag {}", other),
    })
}

pub(super) fn write_u8(out: &mut Vec<u8>, value: u8) {
    out.push(value);
}

pub(super) fn write_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn write_i16(out: &mut Vec<u8>, value: i16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_f64(out: &mut Vec<u8>, value: f64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_str(out: &mut Vec<u8>, value: &str) {
    write_u32(out, value.len() as u32);
    out.extend_from_slice(value.as_bytes());
}

pub(super) fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8> {
    if *cursor >= bytes.len() {
        bail!("unexpected end of input while reading u8");
    }
    let value = bytes[*cursor];
    *cursor += 1;
    Ok(value)
}

pub(super) fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16> {
    if *cursor + 2 > bytes.len() {
        bail!("unexpected end of input while reading u16");
    }
    let mut buf = [0u8; 2];
    buf.copy_from_slice(&bytes[*cursor..*cursor + 2]);
    *cursor += 2;
    Ok(u16::from_le_bytes(buf))
}

pub(super) fn read_i16(bytes: &[u8], cursor: &mut usize) -> Result<i16> {
    read_u16(bytes, cursor).map(|v| v as i16)
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32> {
    if *cursor + 4 > bytes.len() {
        bail!("unexpected end of input while reading u32");
    }
    let mut buf = [0u8; 4];
    buf.copy_from_slice(&bytes[*cursor..*cursor + 4]);
    *cursor += 4;
    Ok(u32::from_le_bytes(buf))
}

fn read_i64(bytes: &[u8], cursor: &mut usize) -> Result<i64> {
    if *cursor + 8 > bytes.len() {
        bail!("unexpected end of input while reading i64");
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[*cursor..*cursor + 8]);
    *cursor += 8;
    Ok(i64::from_le_bytes(buf))
}

fn read_f64(bytes: &[u8], cursor: &mut usize) -> Result<f64> {
    if *cursor + 8 > bytes.len() {
        bail!("unexpected end of input while reading f64");
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[*cursor..*cursor + 8]);
    *cursor += 8;
    Ok(f64::from_le_bytes(buf))
}

fn read_string(bytes: &[u8], cursor: &mut usize) -> Result<String> {
    let len = read_u32(bytes, cursor)? as usize;
    if *cursor + len > bytes.len() {
        bail!("unexpected end of input while reading string");
    }
    let slice = &bytes[*cursor..*cursor + len];
    *cursor += len;
    String::from_utf8(slice.to_vec()).context("invalid UTF-8 in string constant")
}

fn read_tag(bytes: &[u8], cursor: &mut usize) -> Result<[u8; 4]> {
    if *cursor + 4 > bytes.len() {
        bail!("unexpected end of input while reading section tag");
    }
    let mut tag = [0u8; 4];
    tag.copy_from_slice(&bytes[*cursor..*cursor + 4]);
    *cursor += 4;
    Ok(tag)
}

#[cfg(test)]
mod tests;
