use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{Result, anyhow};

use crate::{
    stmt::Program,
    val::Val,
    vm::{Function, Op, compile_program},
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
        self.compile_function_with_name(&lowered, "lkr_entry")
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

struct FunctionTranslator<'a> {
    function: &'a Function,
    function_name: &'a str,
    options: &'a LlvmBackendOptions,
    writer: IrWriter,
    tmp_counter: usize,
    blocks: Vec<BlockRange>,
    block_index_by_start: BTreeMap<usize, usize>,
    runtime_helpers: BTreeSet<RuntimeHelper>,
    string_constants: BTreeMap<u16, StringConstant>,
    string_const_counter: usize,
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
            string_constants: BTreeMap::new(),
            string_const_counter: 0,
        }
    }

    fn translate(mut self) -> Result<String> {
        if self.function.code.is_empty() {
            return Err(anyhow!("cannot compile empty function to LLVM IR"));
        }

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

        for const_data in self.string_constants.values() {
            module.push_str(&format!(
                "@{} = private constant [{} x i8] c\"{}\"\n",
                const_data.label, const_data.array_len, const_data.encoded
            ));
        }
        if !self.string_constants.is_empty() {
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

        Ok(module)
    }

    fn build_blocks(&mut self) -> Result<()> {
        let mut starts: BTreeSet<usize> = BTreeSet::new();
        starts.insert(0);

        for (idx, op) in self.function.code.iter().enumerate() {
            match op {
                Op::Jmp(ofs) | Op::Break(ofs) | Op::Continue(ofs) | Op::ForRangeStep { back_ofs: ofs, .. } => {
                    let target = Self::compute_target(idx, *ofs, self.function.code.len())?;
                    starts.insert(target);
                    starts.insert(idx + 1);
                }
                Op::JmpFalse(_, ofs) => {
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
        self.writer.line(format!("define i64 @{}() {{", self.function_name));
        self.writer.indent();
    }

    fn write_entry_block(&mut self) -> Result<()> {
        self.writer.raw_line("entry:");
        for reg in 0..self.function.n_regs {
            self.writer.line(format!("%r{reg} = alloca i64, align 8"));
            self.writer
                .line(format!("store i64 {}, i64* %r{reg}, align 8", encoding::NIL_LITERAL));
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
                Op::Sub(dst, a, b) => self.emit_binary(*dst, *a, *b, "sub")?,
                Op::Mul(dst, a, b) => self.emit_binary(*dst, *a, *b, "mul")?,
                Op::Div(dst, a, b) => self.emit_binary(*dst, *a, *b, "sdiv")?,
                Op::Mod(dst, a, b) => self.emit_binary(*dst, *a, *b, "srem")?,
                Op::AddInt(dst, a, b) => self.emit_binary(*dst, *a, *b, "add")?,
                Op::AddIntImm(dst, a, imm) => self.emit_add_int_imm(*dst, *a, *imm)?,
                Op::SubInt(dst, a, b) => self.emit_binary(*dst, *a, *b, "sub")?,
                Op::MulInt(dst, a, b) => self.emit_binary(*dst, *a, *b, "mul")?,
                Op::ModInt(dst, a, b) => self.emit_binary(*dst, *a, *b, "srem")?,
                Op::AddFloat(dst, a, b) => self.emit_float_binary(*dst, *a, *b, "fadd")?,
                Op::SubFloat(dst, a, b) => self.emit_float_binary(*dst, *a, *b, "fsub")?,
                Op::MulFloat(dst, a, b) => self.emit_float_binary(*dst, *a, *b, "fmul")?,
                Op::DivFloat(dst, a, b) => self.emit_float_binary(*dst, *a, *b, "fdiv")?,
                Op::ModFloat(dst, a, b) => self.emit_float_binary(*dst, *a, *b, "frem")?,
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
                Op::DefineGlobal(kidx, src) => self.emit_define_global(*kidx, *src)?,
                Op::BuildList { dst, base, len } => self.emit_build_list(*dst, *base, *len)?,
                Op::Call { f, base, argc, retc } => self.emit_call(*f, *base, *argc, *retc)?,
                Op::Access(dst, base, field) => self.emit_access(*dst, *base, *field)?,
                Op::AccessK(dst, base, kidx) => self.emit_access_const(*dst, *base, *kidx)?,
                Op::Index { dst, base, idx } => self.emit_index(*dst, *base, *idx)?,
                Op::IndexK(dst, base, kidx) => self.emit_index_const(*dst, *base, *kidx)?,
                Op::Len { dst, src } => self.emit_len(*dst, *src)?,
                Op::ToIter { dst, src } => self.emit_to_iter(*dst, *src)?,
                Op::BuildMap { dst, base, len } => self.emit_build_map(*dst, *base, *len)?,
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

    fn store_reg(&mut self, reg: u16, value: impl AsRef<str>) -> Result<()> {
        self.ensure_reg(reg)?;
        self.writer
            .line(format!("store i64 {}, i64* %r{reg}, align 8", value.as_ref()));
        Ok(())
    }

    fn store_bool(&mut self, reg: u16, value: bool) -> Result<()> {
        self.store_reg(reg, encoding::bool_literal(value))
    }

    fn store_double(&mut self, reg: u16, value: impl AsRef<str>) -> Result<()> {
        let bits = self.fresh("fcast");
        self.writer
            .line(format!("{bits} = bitcast double {} to i64", value.as_ref()));
        self.store_reg(reg, &bits)?;
        Ok(())
    }

    fn load_reg_as_double(&mut self, reg: u16) -> Result<String> {
        let raw = self.load_reg(reg)?;
        let dbl = self.fresh("asdouble");
        self.writer.line(format!("{dbl} = bitcast i64 {raw} to double"));
        Ok(dbl)
    }

    fn emit_float_binary(&mut self, dst: u16, a: u16, b: u16, op: &str) -> Result<()> {
        let lhs = self.load_reg_as_double(a)?;
        let rhs = self.load_reg_as_double(b)?;
        let tmp = self.fresh(op);
        self.writer.line(format!("{tmp} = {op} double {lhs}, {rhs}"));
        self.store_double(dst, &tmp)?;
        Ok(())
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
        self.string_constants.entry(kidx).or_insert_with(|| {
            let label = format!(".str{}", self.string_const_counter);
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
        })
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
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        match val {
            Val::Int(_) | Val::Bool(_) | Val::Nil => {
                let encoded = encoding::encode_immediate(val)?;
                self.store_reg(dst, encoded.to_string())?;
            }
            Val::Float(f) => {
                let literal = Self::format_double(*f);
                let tmp = self.fresh("constf");
                self.writer.line(format!("{tmp} = bitcast double {literal} to i64"));
                self.store_reg(dst, &tmp)?;
            }
            Val::Str(s) => {
                let handle = self.intern_string_constant(kidx, s.as_ref())?;
                self.store_reg(dst, &handle)?;
            }
            other => {
                return Err(anyhow!(
                    "unsupported constant {:?} in LLVM backend; only Int/Float/Bool/Str/Nil are accepted",
                    other
                ));
            }
        }
        Ok(())
    }

    fn format_double(value: f64) -> String {
        let bits = value.to_bits();
        format!("0x{:016X}", bits)
    }

    fn emit_copy(&mut self, dst: u16, src: u16) -> Result<()> {
        let value = self.load_reg(src)?;
        self.store_reg(dst, &value)?;
        Ok(())
    }

    fn emit_store_local(&mut self, idx: u16, src: u16) -> Result<()> {
        let value = self.load_reg(src)?;
        self.store_reg(idx, &value)?;
        Ok(())
    }

    fn emit_load_local(&mut self, dst: u16, idx: u16) -> Result<()> {
        let value = self.load_reg(idx)?;
        self.store_reg(dst, &value)?;
        Ok(())
    }

    fn emit_binary(&mut self, dst: u16, a: u16, b: u16, op: &str) -> Result<()> {
        let lhs = self.load_reg(a)?;
        let rhs = self.load_reg(b)?;
        let tmp = self.fresh(op);
        self.writer.line(format!("{tmp} = {op} i64 {lhs}, {rhs}"));
        self.store_reg(dst, &tmp)?;
        Ok(())
    }

    fn emit_add_int_imm(&mut self, dst: u16, src: u16, imm: i16) -> Result<()> {
        let lhs = self.load_reg(src)?;
        let literal = encoding::encode_immediate(&Val::Int(imm as i64))?;
        let tmp = self.fresh("addi");
        self.writer.line(format!("{tmp} = add i64 {lhs}, {literal}"));
        self.store_reg(dst, &tmp)?;
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

    fn emit_add_value(&mut self, dst: u16, a: u16, b: u16) -> Result<()> {
        let lhs = self.load_reg(a)?;
        let rhs = self.load_reg(b)?;
        self.require_helper(RuntimeHelper::AddValue);
        let tmp = self.fresh("addval");
        self.writer.line(format!(
            "{tmp} = call i64 @{}(i64 {lhs}, i64 {rhs})",
            RuntimeHelper::AddValue.symbol()
        ));
        self.store_reg(dst, &tmp)?;
        Ok(())
    }

    fn emit_compare(&mut self, dst: u16, a: u16, b: u16, op: &str) -> Result<()> {
        let lhs = self.load_reg(a)?;
        let rhs = self.load_reg(b)?;
        let tmp = self.fresh("cmp");
        self.writer.line(format!("{tmp} = icmp {op} i64 {lhs}, {rhs}"));
        let select = self.fresh("boolsel");
        self.writer.line(format!(
            "{select} = select i1 {tmp}, i64 {true_val}, i64 {false_val}",
            true_val = encoding::BOOL_TRUE_VALUE,
            false_val = encoding::BOOL_FALSE_VALUE
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
        let name_str = match name {
            Val::Str(s) => s.as_ref(),
            other => {
                return Err(anyhow!("LoadGlobal expects string constant; found {:?}", other));
            }
        };
        let handle = self.intern_string_constant(kidx, name_str)?;
        self.require_helper(RuntimeHelper::LoadGlobal);
        let global = self.fresh("loadglobal");
        self.writer.line(format!(
            "{global} = call i64 @{}(i64 {handle})",
            RuntimeHelper::LoadGlobal.symbol()
        ));
        self.store_reg(dst, &global)?;
        Ok(())
    }

    fn emit_define_global(&mut self, kidx: u16, src: u16) -> Result<()> {
        let name = self
            .function
            .consts
            .get(kidx as usize)
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        let name_str = match name {
            Val::Str(s) => s.as_ref(),
            other => {
                return Err(anyhow!("DefineGlobal expects string constant; found {:?}", other));
            }
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
        Ok(())
    }

    fn emit_call(&mut self, rf: u16, base: u16, argc: u8, retc: u8) -> Result<()> {
        if retc > 1 {
            return Err(anyhow!("multiple return values are not supported by the LLVM backend"));
        }
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

        self.require_helper(RuntimeHelper::Call);
        let result = self.fresh("callres");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {func}, i64* {args}, i64 {argc}, i64 {retc})",
            RuntimeHelper::Call.symbol(),
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
        let name_str = match name {
            Val::Str(s) => s.as_ref(),
            other => {
                return Err(anyhow!("AccessK expects string constant; found {:?}", other));
            }
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
    Call,
    Access,
    Index,
    In,
    Len,
    ListSlice,
    ToIter,
    AddValue,
}

impl RuntimeHelper {
    fn symbol(self) -> &'static str {
        match self {
            RuntimeHelper::InternString => "lkr_rt_intern_string",
            RuntimeHelper::ToString => "lkr_rt_to_string",
            RuntimeHelper::LoadGlobal => "lkr_rt_load_global",
            RuntimeHelper::DefineGlobal => "lkr_rt_define_global",
            RuntimeHelper::BuildList => "lkr_rt_build_list",
            RuntimeHelper::BuildMap => "lkr_rt_build_map",
            RuntimeHelper::Call => "lkr_rt_call",
            RuntimeHelper::Access => "lkr_rt_access",
            RuntimeHelper::Index => "lkr_rt_index",
            RuntimeHelper::In => "lkr_rt_in",
            RuntimeHelper::Len => "lkr_rt_len",
            RuntimeHelper::ListSlice => "lkr_rt_list_slice",
            RuntimeHelper::ToIter => "lkr_rt_to_iter",
            RuntimeHelper::AddValue => "lkr_rt_add",
        }
    }

    fn declaration(self) -> &'static str {
        match self {
            RuntimeHelper::InternString => "declare i64 @lkr_rt_intern_string(i8*, i64)",
            RuntimeHelper::ToString => "declare i64 @lkr_rt_to_string(i64)",
            RuntimeHelper::LoadGlobal => "declare i64 @lkr_rt_load_global(i64)",
            RuntimeHelper::DefineGlobal => "declare void @lkr_rt_define_global(i64, i64)",
            RuntimeHelper::BuildList => "declare i64 @lkr_rt_build_list(i64*, i64)",
            RuntimeHelper::Call => "declare i64 @lkr_rt_call(i64, i64*, i64, i64)",
            RuntimeHelper::BuildMap => "declare i64 @lkr_rt_build_map(i64*, i64)",
            RuntimeHelper::Access => "declare i64 @lkr_rt_access(i64, i64)",
            RuntimeHelper::Index => "declare i64 @lkr_rt_index(i64, i64)",
            RuntimeHelper::In => "declare i64 @lkr_rt_in(i64, i64)",
            RuntimeHelper::Len => "declare i64 @lkr_rt_len(i64)",
            RuntimeHelper::ListSlice => "declare i64 @lkr_rt_list_slice(i64, i64)",
            RuntimeHelper::ToIter => "declare i64 @lkr_rt_to_iter(i64)",
            RuntimeHelper::AddValue => "declare i64 @lkr_rt_add(i64, i64)",
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
