use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{Context, Result, anyhow};
use arcstr::ArcStr;

use crate::{
    stmt::Program,
    util::fast_map::FastHashMap,
    val::Val,
    vm::{CaptureSpec, Function, Op, compile_program, rk_index, rk_is_const},
};

use super::{
    encoding,
    options::{LlvmBackendOptions, OptLevel},
    passes,
};

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

fn strip_nested_module_header(ir: &str) -> String {
    ir.lines()
        .filter(|line| {
            !line.starts_with("; ModuleID")
                && !line.starts_with("source_filename")
                && !line.starts_with("target triple")
                && !line.starts_with("declare ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn function_translator_with_captures<'a>(
    function: &'a Function,
    name: &'a str,
    options: &'a LlvmBackendOptions,
    capture_specs: Option<&'a [CaptureSpec]>,
) -> FunctionTranslator<'a> {
    FunctionTranslator::new(function, name, options).with_capture_specs(capture_specs)
}

struct BlockRange {
    start: usize,
    end: usize,
    label: String,
}

const DEFAULT_RETURN_LABEL: &str = "block_return_default";

struct ForRangeLoopParams {
    block_idx: usize,
    instr_idx: usize,
    idx: u16,
    limit: u16,
    step: u16,
    inclusive: bool,
    ofs: i16,
}

#[derive(Clone)]
enum KnownReg {
    Global(String),
    StringHandle(String),
    List { base: u16, len: u16 },
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
    string_constants: BTreeMap<u16, StringConstant>,
    anonymous_string_constants: Vec<StringConstant>,
    string_const_counter: usize,
    native_closures: BTreeMap<u16, NativeClosureBinding>,
    native_closure_ir: Vec<String>,
    capture_specs: Option<&'a [CaptureSpec]>,
}

#[derive(Clone)]
struct NativeClosureBinding {
    symbol: String,
    arity: usize,
}

impl<'a> FunctionTranslator<'a> {
    fn new(function: &'a Function, function_name: &'a str, options: &'a LlvmBackendOptions) -> Self {
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
            string_constants: BTreeMap::new(),
            anonymous_string_constants: Vec::new(),
            string_const_counter: 0,
            native_closures: BTreeMap::new(),
            native_closure_ir: Vec::new(),
            capture_specs: None,
        }
    }

    fn with_capture_specs(mut self, capture_specs: Option<&'a [CaptureSpec]>) -> Self {
        self.capture_specs = capture_specs;
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
                    arity: proto.params.len(),
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
                Op::JmpFalse(_, ofs) | Op::CmpLtImmJmp { ofs, .. } => {
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
                | Op::ForRangeLoop { ofs, .. } => {
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
        if block.start == self.function.code.len() {
            self.writer.raw_line(format!("{}:", block.label));
            self.writer.line(format!("ret i64 {}", encoding::NIL_LITERAL));
            return Ok(());
        }
        self.writer.raw_line(format!("{}:", block.label));
        let mut terminated = false;
        for instr_idx in block.start..block.end {
            let op = &self.function.code[instr_idx];
            match op {
                Op::LoadK(dst, kidx) => self.emit_load_const(*dst, *kidx)?,
                Op::Move(dst, src) => self.emit_copy(*dst, *src)?,
                Op::StoreLocal(idx, src) => self.emit_store_local(*idx, *src)?,
                Op::LoadLocal(dst, idx) => self.emit_load_local(*dst, *idx)?,
                Op::Add(dst, a, b) => self.emit_add_value(*dst, *a, *b)?,
                Op::Sub(dst, a, b) => self.emit_value_binary(*dst, *a, *b, RuntimeHelper::SubValue)?,
                Op::Mul(dst, a, b) => self.emit_value_binary(*dst, *a, *b, RuntimeHelper::MulValue)?,
                Op::Div(dst, a, b) => self.emit_value_binary(*dst, *a, *b, RuntimeHelper::DivValue)?,
                Op::Mod(dst, a, b) => self.emit_value_binary(*dst, *a, *b, RuntimeHelper::ModValue)?,
                Op::AddInt(dst, a, b) => self.emit_binary(*dst, *a, *b, "add")?,
                Op::AddIntImm(dst, a, imm) => self.emit_add_int_imm(*dst, *a, *imm)?,
                Op::SubInt(dst, a, b) => self.emit_binary(*dst, *a, *b, "sub")?,
                Op::MulInt(dst, a, b) => self.emit_binary(*dst, *a, *b, "mul")?,
                Op::ModInt(dst, a, b) => self.emit_binary(*dst, *a, *b, "srem")?,
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
                Op::ListPush { list, val } => self.emit_list_push(*list, *val)?,
                Op::Call { f, base, argc, retc } => self.emit_call(*f, *base, *argc, *retc)?,
                Op::Access(dst, base, field) => self.emit_access(*dst, *base, *field)?,
                Op::AccessK(dst, base, kidx) => self.emit_access_const(*dst, *base, *kidx)?,
                Op::Index { dst, base, idx } => self.emit_index(*dst, *base, *idx)?,
                Op::IndexK(dst, base, kidx) => self.emit_index_const(*dst, *base, *kidx)?,
                Op::Len { dst, src } => self.emit_len(*dst, *src)?,
                Op::ToIter { dst, src } => self.emit_to_iter(*dst, *src)?,
                Op::BuildMap { dst, base, len } => self.emit_build_map(*dst, *base, *len)?,
                Op::MapSet { map, key, val } | Op::MapSetMove { map, key, val } => {
                    self.emit_map_set(*map, *key, *val)?
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
                Op::JmpFalse(reg, ofs) => {
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
                    self.emit_cmp_lt_imm_jmp(block_idx, instr_idx, *r, *imm, *ofs)?;
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

    fn fresh(&mut self, prefix: &str) -> String {
        let tmp = format!("%{}_{}", prefix, self.tmp_counter);
        self.tmp_counter += 1;
        tmp
    }

    fn fresh_label(&mut self, prefix: &str) -> String {
        let label = format!("{}_{}", prefix, self.tmp_counter);
        self.tmp_counter += 1;
        label
    }

    fn set_known(&mut self, reg: u16, value: Option<KnownReg>) {
        if let Some(slot) = self.known_regs.get_mut(reg as usize) {
            *slot = value;
        }
    }

    fn known(&self, reg: u16) -> Option<&KnownReg> {
        self.known_regs.get(reg as usize).and_then(Option::as_ref)
    }

    fn ensure_reg(&self, reg: u16) -> Result<()> {
        if reg as usize >= self.function.n_regs as usize {
            Err(anyhow!("register {} out of bounds", reg))
        } else {
            Ok(())
        }
    }

    fn load_reg(&mut self, reg: u16) -> Result<String> {
        self.ensure_reg(reg)?;
        let tmp = self.fresh("load");
        self.writer.line(format!("{tmp} = load i64, i64* %r{reg}, align 8"));
        Ok(tmp)
    }

    fn load_rk(&mut self, operand: u16) -> Result<String> {
        if rk_is_const(operand) {
            self.load_const_value(rk_index(operand))
        } else {
            self.load_reg(operand)
        }
    }

    fn load_const_value(&mut self, kidx: u16) -> Result<String> {
        let val = self
            .function
            .consts
            .get(kidx as usize)
            .cloned()
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        match &val {
            Val::Int(_) | Val::Bool(_) | Val::Nil => Ok(encoding::encode_immediate(&val)?.to_string()),
            Val::Float(f) => {
                let literal = Self::format_double(*f);
                Ok(self.emit_float_value(&literal))
            }
            val if val.as_str().is_some() => self.intern_string_constant(kidx, val.as_str().unwrap()),
            Val::List(items) => self.emit_const_list(items),
            Val::Map(map) => self.emit_const_map(map),
            other => Err(anyhow!(
                "unsupported constant {:?} in LLVM backend; only primitive/List/Map constants are accepted",
                other
            )),
        }
    }

    fn store_reg(&mut self, reg: u16, value: impl AsRef<str>) -> Result<()> {
        self.ensure_reg(reg)?;
        self.writer
            .line(format!("store i64 {}, i64* %r{reg}, align 8", value.as_ref()));
        self.set_known(reg, None);
        Ok(())
    }

    fn store_bool(&mut self, reg: u16, value: bool) -> Result<()> {
        self.store_reg(reg, encoding::bool_literal(value))
    }

    fn emit_float_value(&mut self, literal: &str) -> String {
        self.require_helper(RuntimeHelper::MakeFloat);
        let tmp = self.fresh("constf");
        self.writer.line(format!(
            "{tmp} = call i64 @{}(double {literal})",
            RuntimeHelper::MakeFloat.symbol()
        ));
        tmp
    }

    fn require_helper(&mut self, helper: RuntimeHelper) {
        self.runtime_helpers.insert(helper);
    }

    fn intern_string_constant(&mut self, kidx: u16, value: &str) -> Result<String> {
        let const_data = self.ensure_string_constant(kidx, value).clone();
        let ptr = self.emit_string_pointer(&const_data);
        self.require_helper(RuntimeHelper::InternString);
        let handle = self.fresh("conststr");
        self.writer.line(format!(
            "{handle} = call i64 @{}(i8* {ptr}, i64 {})",
            RuntimeHelper::InternString.symbol(),
            const_data.len
        ));
        Ok(handle)
    }

    fn intern_anonymous_string(&mut self, value: &str) -> Result<String> {
        let const_data = self.make_string_constant(value);
        self.anonymous_string_constants.push(const_data.clone());
        let ptr = self.emit_string_pointer(&const_data);
        self.require_helper(RuntimeHelper::InternString);
        let handle = self.fresh("conststr");
        self.writer.line(format!(
            "{handle} = call i64 @{}(i8* {ptr}, i64 {})",
            RuntimeHelper::InternString.symbol(),
            const_data.len
        ));
        Ok(handle)
    }

    fn emit_string_pointer(&mut self, const_data: &StringConstant) -> String {
        let ptr = self.fresh("strptr");
        self.writer.line(format!(
            "{ptr} = getelementptr inbounds [{len} x i8], [{len} x i8]* @{label}, i64 0, i64 0",
            len = const_data.array_len,
            label = const_data.label
        ));
        ptr
    }

    fn ensure_string_constant(&mut self, kidx: u16, value: &str) -> &StringConstant {
        if !self.string_constants.contains_key(&kidx) {
            let const_data = self.make_string_constant(value);
            self.string_constants.insert(kidx, const_data);
        }
        self.string_constants.get(&kidx).expect("string constant inserted")
    }

    fn make_string_constant(&mut self, value: &str) -> StringConstant {
        let function = self
            .function_name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let label = format!(".{function}.str{}", self.string_const_counter);
        self.string_const_counter += 1;
        let bytes = value.as_bytes();
        let len = bytes.len();
        let encoded = Self::encode_string_literal(bytes);
        StringConstant {
            label,
            encoded,
            len,
            array_len: len + 1,
        }
    }

    fn encode_string_literal(bytes: &[u8]) -> String {
        let mut encoded = String::with_capacity(bytes.len() * 4 + 4);
        for &b in bytes {
            let _ = write!(&mut encoded, "\\{:02X}", b);
        }
        encoded.push_str("\\00");
        encoded
    }

    fn emit_load_const(&mut self, dst: u16, kidx: u16) -> Result<()> {
        let val = self
            .function
            .consts
            .get(kidx as usize)
            .cloned()
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        match &val {
            Val::Int(_) | Val::Bool(_) | Val::Nil => {
                let encoded = encoding::encode_immediate(&val)?;
                self.store_reg(dst, encoded.to_string())?;
            }
            Val::Float(f) => {
                let literal = Self::format_double(*f);
                let tmp = self.emit_float_value(&literal);
                self.store_reg(dst, &tmp)?;
            }
            val if val.as_str().is_some() => {
                let handle = self.intern_string_constant(kidx, val.as_str().unwrap())?;
                self.store_reg(dst, &handle)?;
                self.set_known(dst, Some(KnownReg::StringHandle(handle)));
            }
            Val::List(items) => {
                let handle = self.emit_const_list(items)?;
                self.store_reg(dst, &handle)?;
                self.set_known(dst, None);
            }
            Val::Map(map) => {
                let handle = self.emit_const_map(map)?;
                self.store_reg(dst, &handle)?;
                self.set_known(dst, None);
            }
            other => {
                return Err(anyhow!(
                    "unsupported constant {:?} in LLVM backend; only primitive/List/Map constants are accepted",
                    other
                ));
            }
        }
        Ok(())
    }

    fn emit_const_value(&mut self, val: &Val) -> Result<String> {
        match val {
            Val::Int(_) | Val::Bool(_) | Val::Nil => Ok(encoding::encode_immediate(val)?.to_string()),
            Val::Float(f) => {
                let literal = Self::format_double(*f);
                Ok(self.emit_float_value(&literal))
            }
            val if val.as_str().is_some() => self.intern_anonymous_string(val.as_str().unwrap()),
            Val::List(items) => self.emit_const_list(items),
            Val::Map(map) => self.emit_const_map(map),
            other => Err(anyhow!("unsupported nested constant {:?} in LLVM backend", other)),
        }
    }

    fn emit_const_list(&mut self, items: &[Val]) -> Result<String> {
        self.require_helper(RuntimeHelper::BuildList);
        if items.is_empty() {
            let list = self.fresh("constlist");
            self.writer.line(format!(
                "{list} = call i64 @{}(i64* null, i64 0)",
                RuntimeHelper::BuildList.symbol()
            ));
            return Ok(list);
        }
        let len = items.len();
        let stack_guard = self.fresh("stacksp");
        self.writer.line(format!("{stack_guard} = call i8* @llvm.stacksave()"));
        let array = self.fresh("constlistbuf");
        self.writer.line(format!("{array} = alloca [{len} x i64], align 8"));
        for (idx, item) in items.iter().enumerate() {
            let value = self.emit_const_value(item)?;
            let slot = self.fresh("constlistelt");
            self.writer.line(format!(
                "{slot} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 {idx}"
            ));
            self.writer.line(format!("store i64 {value}, i64* {slot}, align 8"));
        }
        let ptr = self.fresh("constlistptr");
        self.writer.line(format!(
            "{ptr} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 0"
        ));
        let list = self.fresh("constlist");
        self.writer.line(format!(
            "{list} = call i64 @{}(i64* {ptr}, i64 {len})",
            RuntimeHelper::BuildList.symbol()
        ));
        self.writer
            .line(format!("call void @llvm.stackrestore(i8* {stack_guard})"));
        Ok(list)
    }

    fn emit_const_map(&mut self, map: &FastHashMap<ArcStr, Val>) -> Result<String> {
        self.require_helper(RuntimeHelper::BuildMap);
        if map.is_empty() {
            let out = self.fresh("constmap");
            self.writer.line(format!(
                "{out} = call i64 @{}(i64* null, i64 0)",
                RuntimeHelper::BuildMap.symbol()
            ));
            return Ok(out);
        }
        let len = map.len();
        let total = len * 2;
        let stack_guard = self.fresh("stacksp");
        self.writer.line(format!("{stack_guard} = call i8* @llvm.stacksave()"));
        let array = self.fresh("constmapbuf");
        self.writer.line(format!("{array} = alloca [{total} x i64], align 8"));
        for (idx, (key, value)) in map.iter().enumerate() {
            let key_value = self.intern_anonymous_string(key.as_str())?;
            let val_value = self.emit_const_value(value)?;
            for (offset, raw) in [(idx * 2, key_value), (idx * 2 + 1, val_value)] {
                let slot = self.fresh("constmapelt");
                self.writer.line(format!(
                    "{slot} = getelementptr inbounds [{total} x i64], [{total} x i64]* {array}, i64 0, i64 {offset}"
                ));
                self.writer.line(format!("store i64 {raw}, i64* {slot}, align 8"));
            }
        }
        let ptr = self.fresh("constmapptr");
        self.writer.line(format!(
            "{ptr} = getelementptr inbounds [{total} x i64], [{total} x i64]* {array}, i64 0, i64 0"
        ));
        let out = self.fresh("constmap");
        self.writer.line(format!(
            "{out} = call i64 @{}(i64* {ptr}, i64 {len})",
            RuntimeHelper::BuildMap.symbol()
        ));
        self.writer
            .line(format!("call void @llvm.stackrestore(i8* {stack_guard})"));
        Ok(out)
    }

    fn format_double(value: f64) -> String {
        let bits = value.to_bits();
        format!("0x{:016X}", bits)
    }

    fn emit_copy(&mut self, dst: u16, src: u16) -> Result<()> {
        let known = self.known(src).cloned();
        let value = self.load_reg(src)?;
        self.store_reg(dst, &value)?;
        self.set_known(dst, known);
        Ok(())
    }

    fn emit_store_local(&mut self, idx: u16, src: u16) -> Result<()> {
        let value = self.load_reg(src)?;
        self.store_reg(idx, &value)?;
        Ok(())
    }

    fn emit_load_local(&mut self, dst: u16, idx: u16) -> Result<()> {
        let known = self.known(idx).cloned();
        let value = self.load_reg(idx)?;
        self.store_reg(dst, &value)?;
        self.set_known(dst, known);
        Ok(())
    }

    fn emit_binary(&mut self, dst: u16, a: u16, b: u16, op: &str) -> Result<()> {
        let lhs = self.load_rk(a)?;
        let rhs = self.load_rk(b)?;
        let tmp = self.fresh(op);
        self.writer.line(format!("{tmp} = {op} i64 {lhs}, {rhs}"));
        self.store_reg(dst, &tmp)?;
        Ok(())
    }

    fn emit_add_int_imm(&mut self, dst: u16, src: u16, imm: i16) -> Result<()> {
        let lhs = self.load_reg(src)?;
        self.require_helper(RuntimeHelper::AddValue);
        let tmp = self.fresh("addi");
        self.writer.line(format!(
            "{tmp} = call i64 @{}(i64 {lhs}, i64 {})",
            RuntimeHelper::AddValue.symbol(),
            imm as i64
        ));
        self.store_reg(dst, &tmp)?;
        Ok(())
    }

    fn emit_add_int_imm_jmp(&mut self, instr_idx: usize, r: u16, imm: i16, ofs: i16) -> Result<()> {
        let lhs = self.load_reg(r)?;
        let tmp = self.fresh("addijmp");
        self.writer.line(format!("{tmp} = add i64 {lhs}, {}", imm as i64));
        self.store_reg(r, &tmp)?;
        let target = Self::compute_target(instr_idx, ofs, self.function.code.len())?;
        if let Some(Op::ForRangeLoop { idx, step, .. }) = self.function.code.get(target) {
            let current = self.load_reg(*idx)?;
            let step_val = self.load_reg(*step)?;
            let next = self.fresh("forjmp_next");
            self.writer.line(format!("{next} = add i64 {current}, {step_val}"));
            self.store_reg(*idx, &next)?;
        }
        let label = self.block_label_for_index(target)?;
        self.writer.line(format!("br label %{}", label));
        Ok(())
    }

    fn emit_cmp_int_imm(&mut self, dst: u16, src: u16, imm: i16, op: &str) -> Result<()> {
        let lhs = self.load_reg(src)?;
        let literal = encoding::encode_immediate(&Val::Int(imm as i64))?;
        let tmp = self.fresh("cmpimm");
        self.writer.line(format!("{tmp} = icmp {op} i64 {lhs}, {literal}"));
        let select = self.fresh("boolsel");
        self.writer.line(format!(
            "{select} = select i1 {tmp}, i64 {true_val}, i64 {false_val}",
            true_val = encoding::BOOL_TRUE_VALUE,
            false_val = encoding::BOOL_FALSE_VALUE
        ));
        self.store_reg(dst, &select)?;
        Ok(())
    }

    fn emit_cmp_lt_imm_jmp(&mut self, block_idx: usize, instr_idx: usize, r: u16, imm: i16, ofs: i16) -> Result<()> {
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
        let is_lt = self.fresh("cmpimm_lt");
        self.writer
            .line(format!("{is_lt} = icmp slt i64 {value}, {}", imm as i64));
        let not_sentinel = self.fresh("cmpimm_not_sentinel");
        self.writer.line(format!("{not_sentinel} = xor i1 {is_sentinel}, true"));
        let is_int_lt = self.fresh("cmpimm_int_lt");
        self.writer
            .line(format!("{is_int_lt} = and i1 {is_lt}, {not_sentinel}"));
        let should_jump = self.fresh("cmpimm_jump");
        self.writer.line(format!("{should_jump} = xor i1 {is_int_lt}, true"));
        self.writer.line(format!(
            "br i1 {should_jump}, label %{}, label %{}",
            target_label, fallthrough
        ));
        Ok(())
    }

    fn emit_add_value(&mut self, dst: u16, a: u16, b: u16) -> Result<()> {
        self.emit_value_binary(dst, a, b, RuntimeHelper::AddValue)
    }

    fn emit_value_binary(&mut self, dst: u16, a: u16, b: u16, helper: RuntimeHelper) -> Result<()> {
        let lhs = self.load_rk(a)?;
        let rhs = self.load_rk(b)?;
        self.require_helper(helper);
        let tmp = self.fresh(helper.temp_prefix());
        self.writer
            .line(format!("{tmp} = call i64 @{}(i64 {lhs}, i64 {rhs})", helper.symbol()));
        self.store_reg(dst, &tmp)?;
        Ok(())
    }

    fn emit_compare(&mut self, dst: u16, a: u16, b: u16, op: &str) -> Result<()> {
        let lhs = self.load_rk(a)?;
        let rhs = self.load_rk(b)?;
        let code = match op {
            "eq" => 0,
            "ne" => 1,
            "slt" => 2,
            "sle" => 3,
            "sgt" => 4,
            "sge" => 5,
            _ => return Err(anyhow!("unsupported LLVM compare op {op}")),
        };
        self.require_helper(RuntimeHelper::Compare);
        let select = self.fresh("cmpval");
        self.writer.line(format!(
            "{select} = call i64 @{}(i64 {lhs}, i64 {rhs}, i64 {code})",
            RuntimeHelper::Compare.symbol()
        ));
        self.store_reg(dst, &select)?;
        Ok(())
    }

    fn emit_to_bool(&mut self, dst: u16, src: u16) -> Result<()> {
        let value = self.load_reg(src)?;
        let is_false = self.fresh("isfalse");
        self.writer.line(format!(
            "{is_false} = icmp eq i64 {value}, {false_val}",
            false_val = encoding::BOOL_FALSE_VALUE
        ));
        let is_nil = self.fresh("isnil");
        self.writer.line(format!(
            "{is_nil} = icmp eq i64 {value}, {nil_val}",
            nil_val = encoding::NIL_VALUE
        ));
        let falsy = self.fresh("falsy");
        self.writer.line(format!("{falsy} = or i1 {is_false}, {is_nil}"));
        let result = self.fresh("tobool");
        self.writer.line(format!(
            "{result} = select i1 {falsy}, i64 {false_val}, i64 {true_val}",
            false_val = encoding::BOOL_FALSE_VALUE,
            true_val = encoding::BOOL_TRUE_VALUE
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    fn emit_not(&mut self, dst: u16, src: u16) -> Result<()> {
        let value = self.load_reg(src)?;
        let is_false = self.fresh("not_is_false");
        self.writer.line(format!(
            "{is_false} = icmp eq i64 {value}, {false_val}",
            false_val = encoding::BOOL_FALSE_VALUE
        ));
        let result = self.fresh("not");
        self.writer.line(format!(
            "{result} = select i1 {is_false}, i64 {true_val}, i64 {false_val}",
            true_val = encoding::BOOL_TRUE_VALUE,
            false_val = encoding::BOOL_FALSE_VALUE
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    fn emit_to_str(&mut self, dst: u16, src: u16) -> Result<()> {
        let value = self.load_reg(src)?;
        self.require_helper(RuntimeHelper::ToString);
        let tmp = self.fresh("tostr");
        self.writer.line(format!(
            "{tmp} = call i64 @{}(i64 {value})",
            RuntimeHelper::ToString.symbol()
        ));
        self.store_reg(dst, &tmp)?;
        Ok(())
    }

    fn emit_load_global(&mut self, dst: u16, kidx: u16) -> Result<()> {
        let name = self
            .function
            .consts
            .get(kidx as usize)
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        let name_str = match name.as_str() {
            Some(s) => s,
            None => return Err(anyhow!("LoadGlobal expects string constant; found {:?}", name)),
        };
        if matches!(name_str, "__lk_call_method" | "__lk_call_method_named") {
            self.store_reg(dst, encoding::NIL_LITERAL)?;
            self.set_known(dst, Some(KnownReg::Global(name_str.to_string())));
            return Ok(());
        }
        let handle = self.intern_string_constant(kidx, name_str)?;
        self.require_helper(RuntimeHelper::LoadGlobal);
        let global = self.fresh("loadglobal");
        self.writer.line(format!(
            "{global} = call i64 @{}(i64 {handle})",
            RuntimeHelper::LoadGlobal.symbol()
        ));
        self.store_reg(dst, &global)?;
        self.set_known(dst, Some(KnownReg::Global(name_str.to_string())));
        Ok(())
    }

    fn emit_load_capture(&mut self, dst: u16, idx: u16) -> Result<()> {
        let specs = self
            .capture_specs
            .ok_or_else(|| anyhow!("LoadCapture c{} has no capture metadata in LLVM backend", idx))?;
        let spec = specs
            .get(idx as usize)
            .ok_or_else(|| anyhow!("capture index {} out of range in LLVM backend", idx))?;
        match spec {
            CaptureSpec::Global { name } => {
                let handle = self.intern_anonymous_string(name.as_str())?;
                self.require_helper(RuntimeHelper::LoadGlobal);
                let global = self.fresh("loadcapture_global");
                self.writer.line(format!(
                    "{global} = call i64 @{}(i64 {handle})",
                    RuntimeHelper::LoadGlobal.symbol()
                ));
                self.store_reg(dst, &global)?;
                self.set_known(dst, Some(KnownReg::Global(name.clone())));
                Ok(())
            }
            CaptureSpec::Const { kidx, .. } => {
                let value = self.load_const_value(*kidx)?;
                self.store_reg(dst, &value)?;
                Ok(())
            }
            CaptureSpec::Register { name, .. } => Err(anyhow!(
                "unsupported register capture `{}` in LLVM native closure p{}",
                name,
                idx
            )),
        }
    }

    fn emit_define_global(&mut self, kidx: u16, src: u16) -> Result<()> {
        let name = self
            .function
            .consts
            .get(kidx as usize)
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        let name_str = match name.as_str() {
            Some(s) => s,
            None => return Err(anyhow!("DefineGlobal expects string constant; found {:?}", name)),
        };
        let handle = self.intern_string_constant(kidx, name_str)?;
        let value = self.load_reg(src)?;
        self.require_helper(RuntimeHelper::DefineGlobal);
        self.writer.line(format!(
            "call void @{}(i64 {handle}, i64 {value})",
            RuntimeHelper::DefineGlobal.symbol()
        ));
        Ok(())
    }

    fn emit_build_list(&mut self, dst: u16, base: u16, len: u16) -> Result<()> {
        if len == 0 {
            self.require_helper(RuntimeHelper::BuildList);
            let list = self.fresh("list");
            self.writer.line(format!(
                "{list} = call i64 @{}(i64* null, i64 0)",
                RuntimeHelper::BuildList.symbol()
            ));
            self.store_reg(dst, &list)?;
            self.set_known(dst, Some(KnownReg::List { base, len }));
            return Ok(());
        }

        let base_idx = base as usize;
        let len_usize = len as usize;
        if base_idx + len_usize > self.function.n_regs as usize {
            return Err(anyhow!("BuildList reads out of bounds registers"));
        }

        let stack_guard = self.fresh("stacksp");
        self.writer.line(format!("{stack_guard} = call i8* @llvm.stacksave()"));
        let array = self.fresh("listbuf");
        self.writer
            .line(format!("{array} = alloca [{len} x i64], align 8", len = len));
        for i in 0..len_usize {
            let value = self.load_reg(base + i as u16)?;
            let slot = self.fresh("listelt");
            self.writer.line(format!(
                "{slot} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 {idx}",
                len = len,
                idx = i
            ));
            self.writer.line(format!("store i64 {value}, i64* {slot}, align 8"));
        }

        let ptr = self.fresh("listptr");
        self.writer.line(format!(
            "{ptr} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 0",
            len = len
        ));
        self.require_helper(RuntimeHelper::BuildList);
        let list = self.fresh("list");
        self.writer.line(format!(
            "{list} = call i64 @{}(i64* {ptr}, i64 {len})",
            RuntimeHelper::BuildList.symbol(),
            len = len
        ));
        self.writer
            .line(format!("call void @llvm.stackrestore(i8* {stack_guard})"));
        self.store_reg(dst, &list)?;
        self.set_known(dst, Some(KnownReg::List { base, len }));
        Ok(())
    }

    fn emit_list_push(&mut self, list: u16, val: u16) -> Result<()> {
        let list_value = self.load_reg(list)?;
        let item_value = self.load_reg(val)?;
        self.require_helper(RuntimeHelper::ListPush);
        let updated = self.fresh("listpush");
        self.writer.line(format!(
            "{updated} = call i64 @{}(i64 {list_value}, i64 {item_value})",
            RuntimeHelper::ListPush.symbol()
        ));
        self.store_reg(list, &updated)?;
        self.set_known(list, None);
        Ok(())
    }

    fn emit_call(&mut self, rf: u16, base: u16, argc: u8, retc: u8) -> Result<()> {
        if retc > 1 {
            return Err(anyhow!("multiple return values are not supported by the LLVM backend"));
        }
        if self.try_emit_method_call(rf, base, argc, retc)? {
            return Ok(());
        }
        let use_native_call = matches!(
            self.known(rf),
            Some(KnownReg::Global(name)) if matches!(name.as_str(), "print" | "println" | "panic")
        );
        let func = self.load_reg(rf)?;

        let (args_expr, len, stack_restore) = if argc == 0 {
            (String::from("null"), 0usize, None)
        } else {
            let len = argc as usize;
            let base_idx = base as usize;
            if base_idx + len > self.function.n_regs as usize {
                return Err(anyhow!("Call reads out of bounds registers"));
            }
            let stack_guard = self.fresh("stacksp");
            self.writer.line(format!("{stack_guard} = call i8* @llvm.stacksave()"));
            let array = self.fresh("callargs");
            self.writer.line(format!("{array} = alloca [{len} x i64], align 8"));
            for i in 0..len {
                let value = self.load_reg(base + i as u16)?;
                let slot = self.fresh("callarg");
                self.writer.line(format!(
                    "{slot} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 {idx}",
                    len = len,
                    idx = i
                ));
                self.writer.line(format!("store i64 {value}, i64* {slot}, align 8"));
            }
            let ptr = self.fresh("callargv");
            self.writer.line(format!(
                "{ptr} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 0",
                len = len
            ));
            (ptr, len, Some(stack_guard))
        };

        let helper = if use_native_call {
            RuntimeHelper::CallNative
        } else {
            RuntimeHelper::Call
        };
        self.require_helper(helper);
        let result = self.fresh("callres");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {func}, i64* {args}, i64 {argc}, i64 {retc})",
            helper.symbol(),
            args = args_expr,
            argc = len,
            retc = retc
        ));
        if let Some(stack_guard) = stack_restore {
            self.writer
                .line(format!("call void @llvm.stackrestore(i8* {stack_guard})"));
        }
        if retc == 1 {
            self.store_reg(base, &result)?;
        }
        Ok(())
    }

    fn try_emit_method_call(&mut self, rf: u16, base: u16, argc: u8, retc: u8) -> Result<bool> {
        if argc != 3 {
            return Ok(false);
        }
        if !matches!(self.known(rf), Some(KnownReg::Global(name)) if name == "__lk_call_method") {
            return Ok(false);
        }
        let Some(KnownReg::StringHandle(method_handle)) = self.known(base + 1).cloned() else {
            return Ok(false);
        };
        let Some(KnownReg::List { base: args_base, len }) = self.known(base + 2).cloned() else {
            return Ok(false);
        };

        let receiver = self.load_reg(base)?;
        let (args_expr, stack_restore) = self.emit_arg_array(args_base, len)?;
        self.require_helper(RuntimeHelper::CallMethod);
        let result = self.fresh("methodres");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {receiver}, i64 {method}, i64* {args}, i64 {argc}, i64 {retc})",
            RuntimeHelper::CallMethod.symbol(),
            method = method_handle,
            args = args_expr,
            argc = len
        ));
        if let Some(stack_guard) = stack_restore {
            self.writer
                .line(format!("call void @llvm.stackrestore(i8* {stack_guard})"));
        }
        if retc == 1 {
            self.store_reg(base, &result)?;
        }
        Ok(true)
    }

    fn emit_arg_array(&mut self, base: u16, len: u16) -> Result<(String, Option<String>)> {
        if len == 0 {
            return Ok((String::from("null"), None));
        }
        let len_usize = len as usize;
        let base_idx = base as usize;
        if base_idx + len_usize > self.function.n_regs as usize {
            return Err(anyhow!("Call reads out of bounds registers"));
        }
        let stack_guard = self.fresh("stacksp");
        self.writer.line(format!("{stack_guard} = call i8* @llvm.stacksave()"));
        let array = self.fresh("callargs");
        self.writer.line(format!("{array} = alloca [{len} x i64], align 8"));
        for i in 0..len_usize {
            let value = self.load_reg(base + i as u16)?;
            let slot = self.fresh("callarg");
            self.writer.line(format!(
                "{slot} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 {idx}",
                len = len,
                idx = i
            ));
            self.writer.line(format!("store i64 {value}, i64* {slot}, align 8"));
        }
        let ptr = self.fresh("callargv");
        self.writer.line(format!(
            "{ptr} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 0",
            len = len
        ));
        Ok((ptr, Some(stack_guard)))
    }

    fn emit_build_map(&mut self, dst: u16, base: u16, len: u16) -> Result<()> {
        if len == 0 {
            self.require_helper(RuntimeHelper::BuildMap);
            let map = self.fresh("map");
            self.writer.line(format!(
                "{map} = call i64 @{}(i64* null, i64 0)",
                RuntimeHelper::BuildMap.symbol()
            ));
            self.store_reg(dst, &map)?;
            return Ok(());
        }

        let pair_count = len as usize;
        let base_idx = base as usize;
        if base_idx + pair_count * 2 > self.function.n_regs as usize {
            return Err(anyhow!("BuildMap reads out of bounds registers"));
        }

        let stack_guard = self.fresh("stacksp");
        self.writer.line(format!("{stack_guard} = call i8* @llvm.stacksave()"));
        let array = self.fresh("mapbuf");
        let total = pair_count * 2;
        self.writer.line(format!("{array} = alloca [{total} x i64], align 8"));
        for i in 0..pair_count {
            let key = self.load_reg(base + (2 * i) as u16)?;
            let val = self.load_reg(base + (2 * i + 1) as u16)?;

            let key_slot = self.fresh("mapkey");
            self.writer.line(format!(
                "{key_slot} = getelementptr inbounds [{total} x i64], [{total} x i64]* {array}, i64 0, i64 {idx}",
                total = total,
                idx = 2 * i
            ));
            self.writer.line(format!("store i64 {key}, i64* {key_slot}, align 8"));

            let val_slot = self.fresh("mapval");
            self.writer.line(format!(
                "{val_slot} = getelementptr inbounds [{total} x i64], [{total} x i64]* {array}, i64 0, i64 {idx}",
                total = total,
                idx = 2 * i + 1
            ));
            self.writer.line(format!("store i64 {val}, i64* {val_slot}, align 8"));
        }

        let ptr = self.fresh("mapptr");
        self.writer.line(format!(
            "{ptr} = getelementptr inbounds [{total} x i64], [{total} x i64]* {array}, i64 0, i64 0",
            total = total
        ));

        self.require_helper(RuntimeHelper::BuildMap);
        let map = self.fresh("map");
        self.writer.line(format!(
            "{map} = call i64 @{}(i64* {ptr}, i64 {len})",
            RuntimeHelper::BuildMap.symbol(),
            len = pair_count
        ));
        self.writer
            .line(format!("call void @llvm.stackrestore(i8* {stack_guard})"));
        self.store_reg(dst, &map)?;
        Ok(())
    }

    fn emit_map_set(&mut self, map: u16, key: u16, val: u16) -> Result<()> {
        let map_value = self.load_reg(map)?;
        let key_value = self.load_reg(key)?;
        let val_value = self.load_reg(val)?;
        self.require_helper(RuntimeHelper::MapSet);
        let updated = self.fresh("mapset");
        self.writer.line(format!(
            "{updated} = call i64 @{}(i64 {map_value}, i64 {key_value}, i64 {val_value})",
            RuntimeHelper::MapSet.symbol()
        ));
        self.store_reg(map, &updated)?;
        self.set_known(map, None);
        Ok(())
    }

    fn emit_make_closure(&mut self, dst: u16, proto: u16) -> Result<()> {
        let binding = self
            .native_closures
            .get(&proto)
            .cloned()
            .ok_or_else(|| anyhow!("unsupported closure proto in LLVM backend: p{}", proto))?;
        self.require_helper(RuntimeHelper::MakeAotFunction);
        let closure = self.fresh("aotclosure");
        let params = std::iter::repeat_n("i64", binding.arity).collect::<Vec<_>>().join(", ");
        self.writer.line(format!(
            "{closure} = call i64 @{}(i8* bitcast (i64 ({params})* @{} to i8*), i64 {})",
            RuntimeHelper::MakeAotFunction.symbol(),
            binding.symbol,
            binding.arity
        ));
        self.store_reg(dst, &closure)?;
        self.set_known(dst, None);
        Ok(())
    }

    fn emit_list_slice(&mut self, dst: u16, src: u16, start: u16) -> Result<()> {
        let list = self.load_reg(src)?;
        let start_idx = self.load_reg(start)?;
        self.require_helper(RuntimeHelper::ListSlice);
        let result = self.fresh("listslice");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {list}, i64 {start})",
            RuntimeHelper::ListSlice.symbol(),
            list = list,
            start = start_idx
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    fn emit_access(&mut self, dst: u16, base: u16, field: u16) -> Result<()> {
        let base_val = self.load_reg(base)?;
        let key = self.load_reg(field)?;
        self.require_helper(RuntimeHelper::Access);
        let result = self.fresh("access");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {base}, i64 {key})",
            RuntimeHelper::Access.symbol(),
            base = base_val,
            key = key
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    fn emit_access_const(&mut self, dst: u16, base: u16, kidx: u16) -> Result<()> {
        let name = self
            .function
            .consts
            .get(kidx as usize)
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        let name_str = match name.as_str() {
            Some(s) => s,
            None => return Err(anyhow!("AccessK expects string constant; found {:?}", name)),
        };
        let key = self.intern_string_constant(kidx, name_str)?;
        self.emit_access_with_key(dst, base, key.as_str())
    }

    fn emit_access_with_key(&mut self, dst: u16, base: u16, key: &str) -> Result<()> {
        let base_val = self.load_reg(base)?;
        self.require_helper(RuntimeHelper::Access);
        let result = self.fresh("access");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {base}, i64 {key})",
            RuntimeHelper::Access.symbol(),
            base = base_val,
            key = key
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    fn emit_index(&mut self, dst: u16, base: u16, idx: u16) -> Result<()> {
        let base_val = self.load_reg(base)?;
        let index_val = self.load_reg(idx)?;
        self.require_helper(RuntimeHelper::Index);
        let result = self.fresh("index");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {base}, i64 {index})",
            RuntimeHelper::Index.symbol(),
            base = base_val,
            index = index_val
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    fn emit_index_const(&mut self, dst: u16, base: u16, kidx: u16) -> Result<()> {
        let value = self
            .function
            .consts
            .get(kidx as usize)
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        let literal = match value {
            Val::Int(i) => i.to_string(),
            other => {
                return Err(anyhow!("IndexK expects integer constant; found {:?}", other));
            }
        };
        let base_val = self.load_reg(base)?;
        self.require_helper(RuntimeHelper::Index);
        let result = self.fresh("index");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {base}, i64 {literal})",
            RuntimeHelper::Index.symbol(),
            base = base_val,
            literal = literal
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    fn emit_in(&mut self, dst: u16, needle: u16, haystack: u16) -> Result<()> {
        let needle_val = self.load_reg(needle)?;
        let haystack_val = self.load_reg(haystack)?;
        self.require_helper(RuntimeHelper::In);
        let result = self.fresh("in");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {needle}, i64 {haystack})",
            RuntimeHelper::In.symbol(),
            needle = needle_val,
            haystack = haystack_val
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    fn emit_len(&mut self, dst: u16, src: u16) -> Result<()> {
        let value = self.load_reg(src)?;
        self.require_helper(RuntimeHelper::Len);
        let result = self.fresh("len");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {value})",
            RuntimeHelper::Len.symbol()
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    fn emit_to_iter(&mut self, dst: u16, src: u16) -> Result<()> {
        let value = self.load_reg(src)?;
        self.require_helper(RuntimeHelper::ToIter);
        let result = self.fresh("toiter");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {value})",
            RuntimeHelper::ToIter.symbol()
        ));
        self.store_reg(dst, &result)?;
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

        let target = Self::compute_back_target(instr_idx, back_ofs, self.function.code.len())?;
        let label = self.block_label_for_index(target)?;
        self.writer.line(format!("br label %{}", label));
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RuntimeHelper {
    InternString,
    ToString,
    LoadGlobal,
    DefineGlobal,
    BuildList,
    BuildMap,
    ListPush,
    MapSet,
    MakeAotFunction,
    Call,
    CallNative,
    CallMethod,
    Access,
    Index,
    In,
    Len,
    ListSlice,
    ToIter,
    MakeFloat,
    Compare,
    AddValue,
    SubValue,
    MulValue,
    DivValue,
    ModValue,
}

impl RuntimeHelper {
    const ALL: [RuntimeHelper; 25] = [
        RuntimeHelper::InternString,
        RuntimeHelper::ToString,
        RuntimeHelper::LoadGlobal,
        RuntimeHelper::DefineGlobal,
        RuntimeHelper::BuildList,
        RuntimeHelper::BuildMap,
        RuntimeHelper::ListPush,
        RuntimeHelper::MapSet,
        RuntimeHelper::MakeAotFunction,
        RuntimeHelper::Call,
        RuntimeHelper::CallNative,
        RuntimeHelper::CallMethod,
        RuntimeHelper::Access,
        RuntimeHelper::Index,
        RuntimeHelper::In,
        RuntimeHelper::Len,
        RuntimeHelper::ListSlice,
        RuntimeHelper::ToIter,
        RuntimeHelper::MakeFloat,
        RuntimeHelper::Compare,
        RuntimeHelper::AddValue,
        RuntimeHelper::SubValue,
        RuntimeHelper::MulValue,
        RuntimeHelper::DivValue,
        RuntimeHelper::ModValue,
    ];

    fn symbol(self) -> &'static str {
        match self {
            RuntimeHelper::InternString => "lk_rt_intern_string",
            RuntimeHelper::ToString => "lk_rt_to_string",
            RuntimeHelper::LoadGlobal => "lk_rt_load_global",
            RuntimeHelper::DefineGlobal => "lk_rt_define_global",
            RuntimeHelper::BuildList => "lk_rt_build_list",
            RuntimeHelper::BuildMap => "lk_rt_build_map",
            RuntimeHelper::ListPush => "lk_rt_list_push",
            RuntimeHelper::MapSet => "lk_rt_map_set",
            RuntimeHelper::MakeAotFunction => "lk_rt_make_aot_function",
            RuntimeHelper::Call => "lk_rt_call",
            RuntimeHelper::CallNative => "lk_rt_call_native",
            RuntimeHelper::CallMethod => "lk_rt_call_method",
            RuntimeHelper::Access => "lk_rt_access",
            RuntimeHelper::Index => "lk_rt_index",
            RuntimeHelper::In => "lk_rt_in",
            RuntimeHelper::Len => "lk_rt_len",
            RuntimeHelper::ListSlice => "lk_rt_list_slice",
            RuntimeHelper::ToIter => "lk_rt_to_iter",
            RuntimeHelper::MakeFloat => "lk_rt_float",
            RuntimeHelper::Compare => "lk_rt_cmp",
            RuntimeHelper::AddValue => "lk_rt_add",
            RuntimeHelper::SubValue => "lk_rt_sub",
            RuntimeHelper::MulValue => "lk_rt_mul",
            RuntimeHelper::DivValue => "lk_rt_div",
            RuntimeHelper::ModValue => "lk_rt_mod",
        }
    }

    fn temp_prefix(self) -> &'static str {
        match self {
            RuntimeHelper::AddValue => "addval",
            RuntimeHelper::SubValue => "subval",
            RuntimeHelper::MulValue => "mulval",
            RuntimeHelper::DivValue => "divval",
            RuntimeHelper::ModValue => "modval",
            _ => "rtval",
        }
    }

    fn declaration(self) -> &'static str {
        match self {
            RuntimeHelper::InternString => "declare i64 @lk_rt_intern_string(i8*, i64)",
            RuntimeHelper::ToString => "declare i64 @lk_rt_to_string(i64)",
            RuntimeHelper::LoadGlobal => "declare i64 @lk_rt_load_global(i64)",
            RuntimeHelper::DefineGlobal => "declare void @lk_rt_define_global(i64, i64)",
            RuntimeHelper::BuildList => "declare i64 @lk_rt_build_list(i64*, i64)",
            RuntimeHelper::ListPush => "declare i64 @lk_rt_list_push(i64, i64)",
            RuntimeHelper::MapSet => "declare i64 @lk_rt_map_set(i64, i64, i64)",
            RuntimeHelper::MakeAotFunction => "declare i64 @lk_rt_make_aot_function(i8*, i64)",
            RuntimeHelper::Call => "declare i64 @lk_rt_call(i64, i64*, i64, i64)",
            RuntimeHelper::CallNative => "declare i64 @lk_rt_call_native(i64, i64*, i64, i64)",
            RuntimeHelper::CallMethod => "declare i64 @lk_rt_call_method(i64, i64, i64*, i64, i64)",
            RuntimeHelper::BuildMap => "declare i64 @lk_rt_build_map(i64*, i64)",
            RuntimeHelper::Access => "declare i64 @lk_rt_access(i64, i64)",
            RuntimeHelper::Index => "declare i64 @lk_rt_index(i64, i64)",
            RuntimeHelper::In => "declare i64 @lk_rt_in(i64, i64)",
            RuntimeHelper::Len => "declare i64 @lk_rt_len(i64)",
            RuntimeHelper::ListSlice => "declare i64 @lk_rt_list_slice(i64, i64)",
            RuntimeHelper::ToIter => "declare i64 @lk_rt_to_iter(i64)",
            RuntimeHelper::MakeFloat => "declare i64 @lk_rt_float(double)",
            RuntimeHelper::Compare => "declare i64 @lk_rt_cmp(i64, i64, i64)",
            RuntimeHelper::AddValue => "declare i64 @lk_rt_add(i64, i64)",
            RuntimeHelper::SubValue => "declare i64 @lk_rt_sub(i64, i64)",
            RuntimeHelper::MulValue => "declare i64 @lk_rt_mul(i64, i64)",
            RuntimeHelper::DivValue => "declare i64 @lk_rt_div(i64, i64)",
            RuntimeHelper::ModValue => "declare i64 @lk_rt_mod(i64, i64)",
        }
    }
}

#[derive(Debug, Clone)]
struct StringConstant {
    label: String,
    encoded: String,
    len: usize,
    array_len: usize,
}

struct IrWriter {
    buf: String,
    indent: usize,
}

impl IrWriter {
    fn new() -> Self {
        Self {
            buf: String::new(),
            indent: 0,
        }
    }

    fn indent(&mut self) {
        self.indent += 1;
    }

    fn dedent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
    }

    fn line<S: AsRef<str>>(&mut self, line: S) {
        let line = line.as_ref();
        if !line.is_empty() {
            for _ in 0..self.indent {
                self.buf.push_str("  ");
            }
        }
        self.buf.push_str(line);
        self.buf.push('\n');
    }

    fn raw_line<S: AsRef<str>>(&mut self, line: S) {
        self.buf.push_str(line.as_ref());
        self.buf.push('\n');
    }

    fn finish(self) -> String {
        self.buf
    }
}
