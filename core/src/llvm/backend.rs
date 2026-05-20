use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, anyhow};

use crate::{
    stmt::Program,
    val::Val,
    vm::{CaptureSpec, Function, Op, compile_program, rk_index, rk_is_const},
};

use super::{
    encoding,
    options::{LlvmBackendOptions, OptLevel},
    passes,
};

mod calls;
mod comparisons;
mod containers;
mod globals;
mod string_keys;
mod string_lengths;
mod support;
mod values;

use support::*;

pub type LlvmBackendError = anyhow::Error;

/// Metadata for an emitted LLVM module.
#[derive(Debug, Clone)]
pub struct LlvmModule {
    pub name: String,
    pub ir: String,
    pub target_triple: Option<String>,
}

/// Aggregates the raw IR plus (optional) optimised IR produced by `opt`.
#[derive(Debug, Clone)]
pub struct LlvmModuleArtifact {
    pub module: LlvmModule,
    pub optimised_ir: Option<String>,
    pub opt_level: OptLevel,
}

#[derive(Debug, Default)]
pub struct LlvmBackend {
    options: LlvmBackendOptions,
}

impl LlvmBackend {
    pub fn new(options: LlvmBackendOptions) -> Self {
        Self { options }
    }

    pub fn options(&self) -> &LlvmBackendOptions {
        &self.options
    }

    pub fn with_options(mut self, options: LlvmBackendOptions) -> Self {
        self.options = options;
        self
    }

    pub fn compile_program(&self, program: &Program) -> Result<LlvmModuleArtifact> {
        let lowered = compile_program(program);
        self.compile_function_with_name(&lowered, "lk_entry")
    }

    pub fn compile_function_with_name(&self, function: &Function, name: &str) -> Result<LlvmModuleArtifact> {
        let translator = FunctionTranslator::new(function, name, &self.options);
        let ir = translator.translate()?;

        let optimised = if self.options.run_optimizations {
            passes::run_opt(&ir, self.options.opt_level)?
        } else {
            None
        };

        Ok(LlvmModuleArtifact {
            module: LlvmModule {
                name: self.options.module_name.clone(),
                ir,
                target_triple: self.options.target_triple.clone(),
            },
            optimised_ir: optimised,
            opt_level: self.options.opt_level,
        })
    }
}

pub fn compile_program_to_llvm(program: &Program, options: LlvmBackendOptions) -> Result<LlvmModuleArtifact> {
    LlvmBackend::new(options).compile_program(program)
}

pub fn compile_function_to_llvm(
    function: &Function,
    name: &str,
    options: LlvmBackendOptions,
) -> Result<LlvmModuleArtifact> {
    LlvmBackend::new(options).compile_function_with_name(function, name)
}

struct FunctionTranslator<'a> {
    function: &'a Function,
    function_name: &'a str,
    options: &'a LlvmBackendOptions,
    writer: IrWriter,
    tmp_counter: usize,
    blocks: Vec<BlockRange>,
    block_index_by_start: BTreeMap<usize, usize>,
    runtime_helpers: BTreeSet<RuntimeHelper>,
    known_regs: Vec<Option<KnownReg>>,
    known_globals: BTreeMap<String, KnownReg>,
    string_constants: BTreeMap<u16, StringConstant>,
    anonymous_string_constants: Vec<StringConstant>,
    string_const_counter: usize,
    native_closures: BTreeMap<u16, NativeClosureBinding>,
    native_closure_ir: Vec<String>,
    specialized_native_closures: BTreeMap<String, String>,
    skipped_block_targets: BTreeMap<usize, String>,
    capture_specs: Option<&'a [CaptureSpec]>,
    initial_known_params: BTreeMap<usize, KnownReg>,
    integer_param_indices: BTreeSet<usize>,
    integer_regs: BTreeSet<u16>,
}

impl<'a> FunctionTranslator<'a> {
    fn new(function: &'a Function, function_name: &'a str, options: &'a LlvmBackendOptions) -> Self {
        let integer_param_indices = infer_integer_parameter_indices(function);
        let integer_regs = infer_integer_registers(function, &integer_param_indices);
        Self {
            function,
            function_name,
            options,
            writer: IrWriter::new(),
            tmp_counter: 0,
            blocks: Vec::new(),
            block_index_by_start: BTreeMap::new(),
            runtime_helpers: BTreeSet::new(),
            known_regs: vec![None; function.n_regs as usize],
            known_globals: BTreeMap::new(),
            string_constants: BTreeMap::new(),
            anonymous_string_constants: Vec::new(),
            string_const_counter: 0,
            native_closures: BTreeMap::new(),
            native_closure_ir: Vec::new(),
            specialized_native_closures: BTreeMap::new(),
            skipped_block_targets: BTreeMap::new(),
            capture_specs: None,
            initial_known_params: BTreeMap::new(),
            integer_param_indices,
            integer_regs,
        }
    }

    fn with_capture_specs(mut self, capture_specs: Option<&'a [CaptureSpec]>) -> Self {
        self.capture_specs = capture_specs;
        self
    }

    fn with_initial_known_params(mut self, params: BTreeMap<usize, KnownReg>) -> Self {
        self.initial_known_params = params;
        self
    }

    fn translate(mut self) -> Result<String> {
        if self.function.code.is_empty() {
            return Err(anyhow!("cannot compile empty function to LLVM IR"));
        }

        self.prepare_native_closures()?;
        self.build_blocks()?;
        self.write_function()?;

        let function_ir = self.writer.finish();

        let mut module = String::new();
        module.push_str(&format!("; ModuleID = '{}'\n", self.options.module_name));
        module.push_str(&format!("source_filename = \"{}\"\n", self.options.module_name));
        if let Some(triple) = &self.options.target_triple {
            let escaped_triple = triple.replace('"', "\\\"");
            module.push_str(&format!("target triple = \"{}\"\n", escaped_triple));
        }
        module.push('\n');

        for const_data in self
            .string_constants
            .values()
            .chain(self.anonymous_string_constants.iter())
        {
            module.push_str(&format!(
                "@{} = private constant [{} x i8] c\"{}\"\n",
                const_data.label, const_data.array_len, const_data.encoded
            ));
        }
        if !self.string_constants.is_empty() || !self.anonymous_string_constants.is_empty() {
            module.push('\n');
        }

        for helper in &self.runtime_helpers {
            module.push_str(helper.declaration());
            module.push('\n');
        }
        if !self.runtime_helpers.is_empty() {
            module.push('\n');
        }

        module.push_str(&function_ir);
        for ir in &self.native_closure_ir {
            module.push('\n');
            module.push_str(ir);
            module.push('\n');
        }

        Ok(module)
    }

    fn prepare_native_closures(&mut self) -> Result<()> {
        for (idx, proto) in self.function.protos.iter().enumerate() {
            let captures_are_global = proto
                .captures
                .iter()
                .all(|capture| matches!(capture, crate::vm::CaptureSpec::Global { .. }));
            if !captures_are_global {
                continue;
            }
            if !proto.named_params.is_empty() || !proto.default_funcs.is_empty() {
                continue;
            }
            let Some(func) = proto.func.as_ref() else {
                continue;
            };
            let symbol = format!("{}_proto_{}", self.function_name, idx);
            let translator =
                function_translator_with_captures(func, &symbol, self.options, Some(proto.captures.as_ref()));
            let ir = translator
                .translate()
                .with_context(|| format!("compile native closure proto {}", idx))?;
            self.merge_runtime_helpers_from_ir(&ir);
            self.native_closures.insert(
                idx as u16,
                NativeClosureBinding {
                    symbol,
                    proto_index: idx as u16,
                    arity: proto.params.len(),
                    integer_params: infer_integer_parameter_indices(func),
                },
            );
            self.native_closure_ir.push(strip_nested_module_header(&ir));
        }
        Ok(())
    }

    fn merge_runtime_helpers_from_ir(&mut self, ir: &str) {
        for helper in RuntimeHelper::ALL {
            let needle = format!("@{}(", helper.symbol());
            if ir.contains(&needle) {
                self.runtime_helpers.insert(helper);
            }
        }
    }

    fn build_blocks(&mut self) -> Result<()> {
        let mut starts: BTreeSet<usize> = BTreeSet::new();
        starts.insert(0);

        for (idx, op) in self.function.code.iter().enumerate() {
            match op {
                Op::Jmp(ofs)
                | Op::Break(ofs)
                | Op::Continue(ofs)
                | Op::AddIntImmJmp { ofs, .. }
                | Op::ForRangeStep { back_ofs: ofs, .. } => {
                    let target = Self::compute_target(idx, *ofs, self.function.code.len())?;
                    starts.insert(target);
                    starts.insert(idx + 1);
                }
                Op::JmpFalse(_, ofs)
                | Op::BoolBranch(_, ofs)
                | Op::CmpLtImmJmp { ofs, .. }
                | Op::CmpLeImmJmp { ofs, .. }
                | Op::CmpEqImmJmp { ofs, .. }
                | Op::CmpGtImmJmp { ofs, .. }
                | Op::CmpGeImmJmp { ofs, .. }
                | Op::CmpNeImmJmp { ofs, .. }
                | Op::CmpIntJmp { ofs, .. } => {
                    let target = Self::compute_target(idx, *ofs, self.function.code.len())?;
                    starts.insert(target);
                    starts.insert(idx + 1);
                }
                Op::JmpIfNil(_, ofs) | Op::JmpIfNotNil(_, ofs) => {
                    let target = Self::compute_target(idx, *ofs, self.function.code.len())?;
                    starts.insert(target);
                    starts.insert(idx + 1);
                }
                Op::JmpFalseSet { ofs, .. }
                | Op::JmpTrueSet { ofs, .. }
                | Op::NullishPick { ofs, .. }
                | Op::ForRangeLoop { ofs, .. }
                | Op::RangeLoopI { ofs, .. } => {
                    let target = Self::compute_target(idx, *ofs, self.function.code.len())?;
                    if *ofs != 0 {
                        starts.insert(idx);
                    }
                    starts.insert(target);
                    starts.insert(idx + 1);
                }
                Op::Ret { .. } => {
                    starts.insert(idx + 1);
                }
                _ => {}
            }
        }

        let mut sorted: Vec<usize> = starts.into_iter().collect();
        sorted.sort();
        let mut block_id = 0usize;
        for (i, start) in sorted.iter().copied().enumerate() {
            if start >= self.function.code.len() {
                continue;
            }
            let end = sorted
                .iter()
                .copied()
                .skip(i + 1)
                .find(|candidate| *candidate > start)
                .unwrap_or(self.function.code.len());
            let label = format!("block{block_id}");
            self.block_index_by_start.insert(start, block_id);
            self.blocks.push(BlockRange { start, end, label });
            block_id += 1;
        }

        if self.blocks.is_empty() {
            return Err(anyhow!("bytecode did not produce any basic blocks"));
        }

        // Add sentinel return block so fallthroughs can target the function end.
        let sentinel_index = self.blocks.len();
        self.block_index_by_start
            .insert(self.function.code.len(), sentinel_index);
        self.blocks.push(BlockRange {
            start: self.function.code.len(),
            end: self.function.code.len(),
            label: DEFAULT_RETURN_LABEL.to_string(),
        });

        Ok(())
    }

    fn compute_target(current: usize, ofs: i16, len: usize) -> Result<usize> {
        let target = current as isize + ofs as isize;
        if target < 0 || target as usize > len {
            return Err(anyhow!("branch at {} jumps to out-of-range target {}", current, target));
        }
        Ok(target as usize)
    }

    fn compute_back_target(current: usize, ofs: i16, len: usize) -> Result<usize> {
        let target = current as isize + ofs as isize;
        if target < 0 || target as usize > len {
            return Err(anyhow!(
                "back-edge at {} jumps to out-of-range target {}",
                current,
                target
            ));
        }
        Ok(target as usize)
    }

    fn write_function(&mut self) -> Result<()> {
        self.write_function_signature();
        self.write_entry_block()?;
        for idx in 0..self.blocks.len() {
            self.translate_block(idx)?;
        }
        self.writer.dedent();
        self.writer.line("}");
        Ok(())
    }

    fn write_function_signature(&mut self) {
        let args = (0..self.function.param_regs.len())
            .map(|idx| format!("i64 %arg{idx}"))
            .collect::<Vec<_>>()
            .join(", ");
        self.writer
            .line(format!("define i64 @{}({}) {{", self.function_name, args));
        self.writer.indent();
    }

    fn write_entry_block(&mut self) -> Result<()> {
        self.writer.raw_line("entry:");
        for reg in 0..self.function.n_regs {
            self.writer.line(format!("%r{reg} = alloca i64, align 8"));
            self.writer
                .line(format!("store i64 {}, i64* %r{reg}, align 8", encoding::NIL_LITERAL));
        }
        for (idx, reg) in self.function.param_regs.iter().copied().enumerate() {
            self.writer.line(format!("store i64 %arg{idx}, i64* %r{reg}, align 8"));
            if let Some(known) = self.initial_known_params.get(&idx).cloned() {
                self.set_known(reg, Some(known));
            } else if self.integer_param_indices.contains(&idx) {
                self.set_known(reg, Some(KnownReg::Int));
            }
        }
        if !self.blocks.is_empty() {
            self.writer.line(format!("br label %{}", self.blocks[0].label));
        } else {
            self.writer.line(format!("ret i64 {}", encoding::NIL_LITERAL));
        }
        Ok(())
    }

    fn translate_block(&mut self, block_idx: usize) -> Result<()> {
        let block = &self.blocks[block_idx];
        let block_start = block.start;
        let block_end = block.end;
        let block_label = block.label.clone();
        if block_start == self.function.code.len() {
            self.writer.raw_line(format!("{}:", block_label));
            self.writer.line(format!("ret i64 {}", encoding::NIL_LITERAL));
            return Ok(());
        }
        self.writer.raw_line(format!("{}:", block_label));
        if let Some(target) = self.skipped_block_targets.get(&block_idx) {
            self.writer.line(format!("br label %{}", target));
            return Ok(());
        }
        let mut terminated = false;
        for instr_idx in block_start..block_end {
            let op = &self.function.code[instr_idx];
            match op {
                _ if self.try_emit_map_nil_counter_update_pattern(block_idx, instr_idx)? => {
                    terminated = true;
                    break;
                }
                Op::LoadK(dst, kidx) => self.emit_load_const(instr_idx, block_end, *dst, *kidx)?,
                Op::Move(dst, src) => self.emit_copy(*dst, *src)?,
                Op::StoreLocal(idx, src) => self.emit_store_local(*idx, *src)?,
                Op::LoadLocal(dst, idx) => self.emit_load_local(*dst, *idx)?,
                Op::Add(dst, a, b) => self.emit_add_value(instr_idx, block_end, *dst, *a, *b)?,
                Op::StrConcatKnownCap(dst, a, b) => self.emit_add_value(instr_idx, block_end, *dst, *a, *b)?,
                Op::StrConcatToStr(dst, lhs, src) => {
                    self.emit_str_concat_to_str(instr_idx, block_end, *dst, *lhs, *src)?
                }
                Op::Sub(dst, a, b) => self.emit_value_binary(*dst, *a, *b, RuntimeHelper::SubValue)?,
                Op::Mul(dst, a, b) => self.emit_value_binary(*dst, *a, *b, RuntimeHelper::MulValue)?,
                Op::Div(dst, a, b) => self.emit_value_binary(*dst, *a, *b, RuntimeHelper::DivValue)?,
                Op::Mod(dst, a, b) => self.emit_value_binary(*dst, *a, *b, RuntimeHelper::ModValue)?,
                Op::AddInt(dst, a, b) => self.emit_int_binary(*dst, *a, *b, "add", RuntimeHelper::AddValue)?,
                Op::AddIntImm(dst, a, imm) => self.emit_add_int_imm(*dst, *a, *imm)?,
                Op::SubInt(dst, a, b) => self.emit_int_binary(*dst, *a, *b, "sub", RuntimeHelper::SubValue)?,
                Op::MulInt(dst, a, b) => self.emit_int_binary(*dst, *a, *b, "mul", RuntimeHelper::MulValue)?,
                Op::ModInt(dst, a, b) => self.emit_int_binary(*dst, *a, *b, "srem", RuntimeHelper::ModValue)?,
                Op::AddFloat(dst, a, b) => self.emit_value_binary(*dst, *a, *b, RuntimeHelper::AddValue)?,
                Op::SubFloat(dst, a, b) => self.emit_value_binary(*dst, *a, *b, RuntimeHelper::SubValue)?,
                Op::MulFloat(dst, a, b) => self.emit_value_binary(*dst, *a, *b, RuntimeHelper::MulValue)?,
                Op::DivFloat(dst, a, b) => self.emit_binary(*dst, *a, *b, "sdiv")?,
                Op::ModFloat(dst, a, b) => self.emit_value_binary(*dst, *a, *b, RuntimeHelper::ModValue)?,
                Op::CmpEq(dst, a, b) => self.emit_compare(*dst, *a, *b, "eq")?,
                Op::CmpNe(dst, a, b) => self.emit_compare(*dst, *a, *b, "ne")?,
                Op::CmpLt(dst, a, b) => self.emit_compare(*dst, *a, *b, "slt")?,
                Op::CmpLe(dst, a, b) => self.emit_compare(*dst, *a, *b, "sle")?,
                Op::CmpGt(dst, a, b) => self.emit_compare(*dst, *a, *b, "sgt")?,
                Op::CmpGe(dst, a, b) => self.emit_compare(*dst, *a, *b, "sge")?,
                Op::CmpI { dst, a, b, kind } => self.emit_int_compare_kind(*dst, *a, *b, *kind)?,
                Op::CmpEqImm(dst, a, imm) => self.emit_cmp_int_imm(*dst, *a, *imm, "eq")?,
                Op::CmpNeImm(dst, a, imm) => self.emit_cmp_int_imm(*dst, *a, *imm, "ne")?,
                Op::CmpLtImm(dst, a, imm) => self.emit_cmp_int_imm(*dst, *a, *imm, "slt")?,
                Op::CmpLeImm(dst, a, imm) => self.emit_cmp_int_imm(*dst, *a, *imm, "sle")?,
                Op::CmpGtImm(dst, a, imm) => self.emit_cmp_int_imm(*dst, *a, *imm, "sgt")?,
                Op::CmpGeImm(dst, a, imm) => self.emit_cmp_int_imm(*dst, *a, *imm, "sge")?,
                Op::In(dst, a, b) => self.emit_in(*dst, *a, *b)?,
                Op::ToBool(dst, src) => self.emit_to_bool(*dst, *src)?,
                Op::Not(dst, src) => self.emit_not(*dst, *src)?,
                Op::ToStr(dst, src) => self.emit_to_str(*dst, *src)?,
                Op::LoadGlobal(dst, kidx) => self.emit_load_global(*dst, *kidx)?,
                Op::LoadCapture { dst, idx } => self.emit_load_capture(*dst, *idx)?,
                Op::DefineGlobal(kidx, src) => self.emit_define_global(*kidx, *src)?,
                Op::BuildList { dst, base, len } => self.emit_build_list(*dst, *base, *len)?,
                Op::ListPush { list, val } | Op::ListPushMove { list, val } => self.emit_list_push(*list, *val)?,
                Op::Call { f, base, argc, retc }
                | Op::CallExact { f, base, argc, retc }
                | Op::CallClosureExact { f, base, argc, retc }
                | Op::CallNativeFast { f, base, argc, retc } => self.emit_call(*f, *base, *argc, *retc)?,
                Op::CallMethod0 { dst, receiver, method } => self.emit_call_method0(*dst, *receiver, *method)?,
                Op::CallGlobalMethod0 { dst, receiver, method } => {
                    self.emit_call_global_method0(*dst, *receiver, *method)?
                }
                Op::Access(dst, base, field) => {
                    self.emit_access_or_defer_value(instr_idx, block_end, *dst, *base, *field)?
                }
                Op::AccessK(dst, base, kidx) => self.emit_access_const(*dst, *base, *kidx)?,
                Op::Index { dst, base, idx } => {
                    self.emit_index_or_defer_len(instr_idx, block_end, *dst, *base, *idx)?
                }
                Op::IndexK(dst, base, kidx) => self.emit_index_const(*dst, *base, *kidx)?,
                Op::Len { dst, src } | Op::ListLen { dst, src } | Op::MapLen { dst, src } | Op::StrLen { dst, src } => {
                    self.emit_len(*dst, *src)?
                }
                Op::Floor { dst, src } => self.emit_floor(*dst, *src)?,
                Op::FloorDivImm { dst, src, imm } => self.emit_floor_div_imm(*dst, *src, *imm)?,
                Op::StartsWithK(dst, src, kidx) => {
                    self.emit_string_predicate_k(*dst, *src, *kidx, RuntimeHelper::StartsWith)?
                }
                Op::ContainsK(dst, src, kidx) => {
                    self.emit_string_predicate_k(*dst, *src, *kidx, RuntimeHelper::Contains)?
                }
                Op::ToIter { dst, src } => self.emit_to_iter(*dst, *src)?,
                Op::BuildMap { dst, base, len } => self.emit_build_map(*dst, *base, *len)?,
                Op::MapHas(dst, map, key) => self.emit_map_has(*dst, *map, *key)?,
                Op::MapHasK(dst, map, kidx) => self.emit_map_has_const(*dst, *map, *kidx)?,
                Op::MapGetInterned(dst, map, kidx) => {
                    self.emit_map_get_const_str(instr_idx, block_end, *dst, *map, *kidx)?
                }
                Op::MapGetDynamic(dst, map, key) => {
                    if !self.emit_map_get_dynamic(instr_idx, block_end, *dst, *map, *key)? {
                        self.emit_access_or_defer_value(instr_idx, block_end, *dst, *map, *key)?;
                    }
                }
                Op::MapSet { map, key, val } | Op::MapSetMove { map, key, val } => {
                    self.emit_map_set(*map, *key, *val)?
                }
                Op::MapSetInterned(map, kidx, val) | Op::MapSetInternedMove(map, kidx, val) => {
                    self.emit_map_set_const(*map, *kidx, *val)?
                }
                Op::MakeClosure { dst, proto } => self.emit_make_closure(*dst, *proto)?,
                Op::ListSlice { dst, src, start } => self.emit_list_slice(*dst, *src, *start)?,
                Op::Jmp(ofs) => {
                    let target = Self::compute_target(instr_idx, *ofs, self.function.code.len())?;
                    let label = self.block_label_for_index(target)?;
                    self.writer.line(format!("br label %{}", label));
                    terminated = true;
                    break;
                }
                Op::Break(ofs) | Op::Continue(ofs) => {
                    let target = Self::compute_target(instr_idx, *ofs, self.function.code.len())?;
                    let label = self.block_label_for_index(target)?;
                    self.writer.line(format!("br label %{}", label));
                    terminated = true;
                    break;
                }
                Op::JmpFalse(reg, ofs) | Op::BoolBranch(reg, ofs) => {
                    let target = Self::compute_target(instr_idx, *ofs, self.function.code.len())?;
                    let label = self.block_label_for_index(target)?;
                    let fallthrough = self
                        .blocks
                        .get(block_idx + 1)
                        .map(|b| b.label.clone())
                        .unwrap_or_else(|| DEFAULT_RETURN_LABEL.to_string());
                    let cond = self.load_reg(*reg)?;
                    let is_false = self.fresh("isfalse");
                    self.writer.line(format!(
                        "{is_false} = icmp eq i64 {cond}, {false_val}",
                        false_val = encoding::BOOL_FALSE_VALUE
                    ));
                    let is_nil = self.fresh("isnil");
                    self.writer.line(format!(
                        "{is_nil} = icmp eq i64 {cond}, {nil_val}",
                        nil_val = encoding::NIL_VALUE
                    ));
                    let falsey = self.fresh("falsey");
                    self.writer.line(format!("{falsey} = or i1 {is_false}, {is_nil}"));
                    self.writer
                        .line(format!("br i1 {falsey}, label %{}, label %{}", label, fallthrough));
                    terminated = true;
                    break;
                }
                Op::CmpLtImmJmp { r, imm, ofs } => {
                    self.emit_cmp_imm_jmp(block_idx, instr_idx, *r, *imm, *ofs, "slt")?;
                    terminated = true;
                    break;
                }
                Op::CmpLeImmJmp { r, imm, ofs } => {
                    self.emit_cmp_imm_jmp(block_idx, instr_idx, *r, *imm, *ofs, "sle")?;
                    terminated = true;
                    break;
                }
                Op::CmpEqImmJmp { r, imm, ofs } => {
                    self.emit_cmp_imm_jmp(block_idx, instr_idx, *r, *imm, *ofs, "eq")?;
                    terminated = true;
                    break;
                }
                Op::CmpGtImmJmp { r, imm, ofs } => {
                    self.emit_cmp_imm_jmp(block_idx, instr_idx, *r, *imm, *ofs, "sgt")?;
                    terminated = true;
                    break;
                }
                Op::CmpGeImmJmp { r, imm, ofs } => {
                    self.emit_cmp_imm_jmp(block_idx, instr_idx, *r, *imm, *ofs, "sge")?;
                    terminated = true;
                    break;
                }
                Op::CmpNeImmJmp { r, imm, ofs } => {
                    self.emit_cmp_imm_jmp(block_idx, instr_idx, *r, *imm, *ofs, "ne")?;
                    terminated = true;
                    break;
                }
                Op::CmpIntJmp { kind, a, b, ofs } => {
                    self.emit_cmp_int_jmp(block_idx, instr_idx, *a, *b, *kind, *ofs)?;
                    terminated = true;
                    break;
                }
                Op::AddIntImmJmp { r, imm, ofs } => {
                    self.emit_add_int_imm_jmp(instr_idx, *r, *imm, *ofs)?;
                    terminated = true;
                    break;
                }
                Op::JmpIfNil(reg, ofs) => {
                    let target = Self::compute_target(instr_idx, *ofs, self.function.code.len())?;
                    let label = self.block_label_for_index(target)?;
                    let fallthrough = self
                        .blocks
                        .get(block_idx + 1)
                        .map(|b| b.label.clone())
                        .unwrap_or_else(|| DEFAULT_RETURN_LABEL.to_string());
                    let value = self.load_reg(*reg)?;
                    let is_nil = self.fresh("isnil");
                    self.writer.line(format!(
                        "{is_nil} = icmp eq i64 {value}, {nil_val}",
                        nil_val = encoding::NIL_VALUE
                    ));
                    self.writer
                        .line(format!("br i1 {is_nil}, label %{}, label %{}", label, fallthrough));
                    terminated = true;
                    break;
                }
                Op::JmpIfNotNil(reg, ofs) => {
                    let target = Self::compute_target(instr_idx, *ofs, self.function.code.len())?;
                    let label = self.block_label_for_index(target)?;
                    let fallthrough = self
                        .blocks
                        .get(block_idx + 1)
                        .map(|b| b.label.clone())
                        .unwrap_or_else(|| DEFAULT_RETURN_LABEL.to_string());
                    let value = self.load_reg(*reg)?;
                    let is_not_nil = self.fresh("isnotnil");
                    self.writer.line(format!(
                        "{is_not_nil} = icmp ne i64 {value}, {nil_val}",
                        nil_val = encoding::NIL_VALUE
                    ));
                    self.writer
                        .line(format!("br i1 {is_not_nil}, label %{}, label %{}", label, fallthrough));
                    terminated = true;
                    break;
                }
                Op::NullishPick { l, dst, ofs } => {
                    let target = Self::compute_target(instr_idx, *ofs, self.function.code.len())?;
                    let label = self.block_label_for_index(target)?;
                    let fallthrough = self
                        .blocks
                        .get(block_idx + 1)
                        .map(|b| b.label.clone())
                        .unwrap_or_else(|| DEFAULT_RETURN_LABEL.to_string());
                    let value = self.load_reg(*l)?;
                    let is_nil = self.fresh("isnil");
                    let taken_label = self.fresh_label("nullish_taken");
                    self.writer.line(format!(
                        "{is_nil} = icmp eq i64 {value}, {nil_val}",
                        nil_val = encoding::NIL_VALUE
                    ));
                    self.writer.line(format!(
                        "br i1 {is_nil}, label %{}, label %{}",
                        fallthrough, taken_label
                    ));
                    self.writer.raw_line(format!("{taken_label}:"));
                    self.store_reg(*dst, &value)?;
                    self.writer.line(format!("br label %{}", label));
                    terminated = true;
                    break;
                }
                Op::JmpFalseSet { r, dst, ofs } => {
                    let target = Self::compute_target(instr_idx, *ofs, self.function.code.len())?;
                    let label = self.block_label_for_index(target)?;
                    let fallthrough = self
                        .blocks
                        .get(block_idx + 1)
                        .map(|b| b.label.clone())
                        .unwrap_or_else(|| DEFAULT_RETURN_LABEL.to_string());
                    let value = self.load_reg(*r)?;
                    let is_false = self.fresh("isfalse");
                    let taken_label = self.fresh_label("and_false");
                    self.writer.line(format!(
                        "{is_false} = icmp eq i64 {value}, {false_val}",
                        false_val = encoding::BOOL_FALSE_VALUE
                    ));
                    let is_nil = self.fresh("isnil");
                    self.writer.line(format!(
                        "{is_nil} = icmp eq i64 {value}, {nil_val}",
                        nil_val = encoding::NIL_VALUE
                    ));
                    let should_take = self.fresh("and_should_take");
                    self.writer.line(format!("{should_take} = or i1 {is_false}, {is_nil}"));
                    self.writer.line(format!(
                        "br i1 {should_take}, label %{}, label %{}",
                        taken_label, fallthrough
                    ));
                    self.writer.raw_line(format!("{taken_label}:"));
                    self.store_bool(*dst, false)?;
                    self.writer.line(format!("br label %{}", label));
                    terminated = true;
                    break;
                }
                Op::JmpTrueSet { r, dst, ofs } => {
                    let target = Self::compute_target(instr_idx, *ofs, self.function.code.len())?;
                    let label = self.block_label_for_index(target)?;
                    let fallthrough = self
                        .blocks
                        .get(block_idx + 1)
                        .map(|b| b.label.clone())
                        .unwrap_or_else(|| DEFAULT_RETURN_LABEL.to_string());
                    let value = self.load_reg(*r)?;
                    let is_false = self.fresh("istrue_false");
                    self.writer.line(format!(
                        "{is_false} = icmp eq i64 {value}, {false_val}",
                        false_val = encoding::BOOL_FALSE_VALUE
                    ));
                    let is_nil = self.fresh("istrue_nil");
                    self.writer.line(format!(
                        "{is_nil} = icmp eq i64 {value}, {nil_val}",
                        nil_val = encoding::NIL_VALUE
                    ));
                    let falsy = self.fresh("istrue_falsy");
                    self.writer.line(format!("{falsy} = or i1 {is_false}, {is_nil}"));
                    let is_true = self.fresh("istrue");
                    self.writer.line(format!("{is_true} = xor i1 {falsy}, true"));
                    let taken_label = self.fresh_label("or_true");
                    self.writer.line(format!(
                        "br i1 {is_true}, label %{}, label %{}",
                        taken_label, fallthrough
                    ));
                    self.writer.raw_line(format!("{taken_label}:"));
                    self.store_bool(*dst, true)?;
                    self.writer.line(format!("br label %{}", label));
                    terminated = true;
                    break;
                }
                Op::Ret { base, retc } => {
                    if *retc == 0 {
                        self.writer.line(format!("ret i64 {}", encoding::NIL_LITERAL));
                    } else if *retc == 1 {
                        let value = self.load_reg(*base)?;
                        self.writer.line(format!("ret i64 {value}"));
                    } else {
                        return Err(anyhow!("multiple return values are not supported by the LLVM backend"));
                    }
                    terminated = true;
                    break;
                }
                Op::ForRangePrep {
                    idx,
                    limit,
                    step,
                    inclusive: _,
                    explicit,
                } => {
                    self.emit_for_range_prep(*idx, *limit, *step, *explicit)?;
                }
                Op::ForRangeLoop {
                    idx,
                    limit,
                    step,
                    inclusive,
                    write_idx: _,
                    ofs,
                }
                | Op::RangeLoopI {
                    idx,
                    limit,
                    step,
                    inclusive,
                    write_idx: _,
                    ofs,
                } => {
                    let guard_params = ForRangeLoopParams {
                        block_idx,
                        instr_idx,
                        idx: *idx,
                        limit: *limit,
                        step: *step,
                        inclusive: *inclusive,
                        ofs: *ofs,
                    };
                    self.emit_for_range_loop(guard_params)?;
                    terminated = true;
                    break;
                }
                Op::ForRangeStep { idx, step, back_ofs } => {
                    self.emit_for_range_step(instr_idx, *idx, *step, *back_ofs)?;
                    terminated = true;
                    break;
                }
                other => {
                    return Err(anyhow!("unsupported opcode in LLVM backend: {:?}", other));
                }
            }
        }

        if !terminated {
            if let Some(next) = self.blocks.get(block_idx + 1) {
                self.writer.line(format!("br label %{}", next.label));
            } else {
                self.writer.line(format!("ret i64 {}", encoding::NIL_LITERAL));
            }
        }
        Ok(())
    }

    fn block_label_for_index(&self, index: usize) -> Result<String> {
        let block_idx = self
            .block_index_by_start
            .range(..=index)
            .next_back()
            .map(|(_, idx)| *idx)
            .ok_or_else(|| anyhow!("no block found for instruction index {}", index))?;
        Ok(self.blocks[block_idx].label.clone())
    }

    fn emit_add_int_imm_jmp(&mut self, instr_idx: usize, r: u16, imm: i16, ofs: i16) -> Result<()> {
        let lhs = self.load_reg(r)?;
        let tmp = self.fresh("addijmp");
        self.writer.line(format!("{tmp} = add i64 {lhs}, {}", imm as i64));
        self.store_reg(r, &tmp)?;
        self.set_known(r, Some(KnownReg::Int));
        let target = Self::compute_target(instr_idx, ofs, self.function.code.len())?;
        if let Some(Op::ForRangeLoop { idx, step, .. } | Op::RangeLoopI { idx, step, .. }) =
            self.function.code.get(target)
        {
            let current = self.load_reg(*idx)?;
            let step_val = self.load_reg(*step)?;
            let next = self.fresh("forjmp_next");
            self.writer.line(format!("{next} = add i64 {current}, {step_val}"));
            self.store_reg(*idx, &next)?;
            self.set_known(*idx, Some(KnownReg::Int));
        }
        let label = self.block_label_for_index(target)?;
        self.writer.line(format!("br label %{}", label));
        Ok(())
    }

    fn emit_cmp_imm_jmp(
        &mut self,
        block_idx: usize,
        instr_idx: usize,
        r: u16,
        imm: i16,
        ofs: i16,
        op: &str,
    ) -> Result<()> {
        let target = Self::compute_target(instr_idx, ofs, self.function.code.len())?;
        let target_label = self.block_label_for_index(target)?;
        let fallthrough = self
            .blocks
            .get(block_idx + 1)
            .map(|b| b.label.clone())
            .unwrap_or_else(|| DEFAULT_RETURN_LABEL.to_string());
        let value = self.load_reg(r)?;
        let is_sentinel = self.fresh("cmpimm_sentinel");
        self.writer.line(format!(
            "{is_sentinel} = icmp sle i64 {value}, {sentinel_max}",
            sentinel_max = encoding::BOOL_TRUE_VALUE
        ));
        let cmp = self.fresh("cmpimm");
        self.writer
            .line(format!("{cmp} = icmp {op} i64 {value}, {}", imm as i64));
        let not_sentinel = self.fresh("cmpimm_not_sentinel");
        self.writer.line(format!("{not_sentinel} = xor i1 {is_sentinel}, true"));
        let is_int_match = self.fresh("cmpimm_int_match");
        self.writer
            .line(format!("{is_int_match} = and i1 {cmp}, {not_sentinel}"));
        let should_jump = self.fresh("cmpimm_jump");
        self.writer.line(format!("{should_jump} = xor i1 {is_int_match}, true"));
        self.writer.line(format!(
            "br i1 {should_jump}, label %{}, label %{}",
            target_label, fallthrough
        ));
        Ok(())
    }

    fn emit_for_range_prep(&mut self, idx: u16, limit: u16, step: u16, explicit: bool) -> Result<()> {
        if !explicit {
            let start = self.load_reg(idx)?;
            let lim = self.load_reg(limit)?;
            let is_ascending = self.fresh("forprep_cmp");
            self.writer
                .line(format!("{is_ascending} = icmp sle i64 {start}, {lim}"));
            let chosen_step = self.fresh("forprep_step");
            self.writer
                .line(format!("{chosen_step} = select i1 {is_ascending}, i64 1, i64 -1"));
            self.store_reg(step, &chosen_step)?;
            self.set_known(step, Some(KnownReg::Int));
        }
        Ok(())
    }

    fn emit_for_range_loop(&mut self, params: ForRangeLoopParams) -> Result<()> {
        let ForRangeLoopParams {
            block_idx,
            instr_idx,
            idx,
            limit,
            step,
            inclusive,
            ofs,
        } = params;
        let target = Self::compute_target(instr_idx, ofs, self.function.code.len())?;
        let exit_label = self.block_label_for_index(target)?;
        let fallthrough = self
            .blocks
            .get(block_idx + 1)
            .map(|b| b.label.clone())
            .unwrap_or_else(|| DEFAULT_RETURN_LABEL.to_string());
        let idx_val = self.load_reg(idx)?;
        let limit_val = self.load_reg(limit)?;
        let step_val = self.load_reg(step)?;

        let is_positive = self.fresh("forstep_pos");
        self.writer.line(format!("{is_positive} = icmp sgt i64 {step_val}, 0"));

        let cond_pos = self.fresh("forguard_pos");
        if inclusive {
            self.writer
                .line(format!("{cond_pos} = icmp sle i64 {idx_val}, {limit_val}"));
        } else {
            self.writer
                .line(format!("{cond_pos} = icmp slt i64 {idx_val}, {limit_val}"));
        }

        let cond_neg = self.fresh("forguard_neg");
        if inclusive {
            self.writer
                .line(format!("{cond_neg} = icmp sge i64 {idx_val}, {limit_val}"));
        } else {
            self.writer
                .line(format!("{cond_neg} = icmp sgt i64 {idx_val}, {limit_val}"));
        }

        let cont = self.fresh("forguard_cont");
        self.writer.line(format!(
            "{cont} = select i1 {is_positive}, i1 {cond_pos}, i1 {cond_neg}"
        ));
        self.writer
            .line(format!("br i1 {cont}, label %{}, label %{}", fallthrough, exit_label));
        Ok(())
    }

    fn emit_for_range_step(&mut self, instr_idx: usize, idx: u16, step: u16, back_ofs: i16) -> Result<()> {
        let current = self.load_reg(idx)?;
        let step_val = self.load_reg(step)?;
        let next = self.fresh("forstep_next");
        self.writer.line(format!("{next} = add i64 {current}, {step_val}"));
        self.store_reg(idx, &next)?;
        self.set_known(idx, Some(KnownReg::Int));

        let target = Self::compute_back_target(instr_idx, back_ofs, self.function.code.len())?;
        let label = self.block_label_for_index(target)?;
        self.writer.line(format!("br label %{}", label));
        Ok(())
    }
}
