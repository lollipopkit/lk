use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

use crate::{
    expr::Expr,
    stmt::{Program, Stmt, StmtParser},
    token::Tokenizer,
};

use super::{
    CompiledFunction, Compiler, Function, FunctionSignature, Module, NativeEntry, collect_function_inline_bodies,
    collect_function_names, collect_function_signatures, collect_global_names_with_external, collect_native_names,
    function_frame_params, global_slots_from_names,
};

impl Compiler {
    pub fn compile_expr(expr: &Expr) -> Result<Function> {
        let mut compiler = Self::default();
        let result = compiler.lower_expr(expr)?;
        compiler.emit_return(result)?;
        compiler.finish()
    }

    pub fn compile_program(program: &Program) -> Result<Function> {
        let mut compiler = Self::default();
        compiler.lower_program_statements(program)?;
        compiler.finish()
    }

    pub fn compile_module(program: &Program) -> Result<Module> {
        Self::compile_module_with_natives(program, Vec::new())
    }

    pub fn compile_module_with_natives(program: &Program, natives: Vec<NativeEntry>) -> Result<Module> {
        Self::compile_module_with_natives_and_globals(program, natives, std::iter::empty::<&str>())
    }

    pub fn compile_module_with_natives_and_globals<I, S>(
        program: &Program,
        natives: Vec<NativeEntry>,
        external_globals: I,
    ) -> Result<Module>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let function_names = collect_function_names(program)?;
        let function_signatures = collect_function_signatures(program)?;
        let function_bodies = collect_function_inline_bodies(program)?;
        let native_names = collect_native_names(&natives)?;
        let global_names = collect_global_names_with_external(program, external_globals)?;
        let mut module = Module {
            functions: vec![Function::default(); function_names.len() + 1],
            natives,
            globals: global_slots_from_names(&global_names),
            entry: 0,
        };

        let mut entry = Self::with_names(
            function_names.clone(),
            function_signatures.clone(),
            function_bodies.clone(),
            native_names.clone(),
            global_names.clone(),
            true,
        );
        entry.dynamic_function_base = module.functions.len() as u32;
        entry.lower_program_statements(program)?;
        module.functions[0] = entry.finish()?;
        module.functions.extend(entry.pending_functions);

        for stmt in &program.statements {
            if let Stmt::Function {
                name,
                params,
                named_params,
                body,
                ..
            } = stmt.as_ref()
            {
                let function_index = *function_names
                    .get(name)
                    .ok_or_else(|| anyhow!("Compiler missing function index for `{name}`"))?;
                let mut compiled = Self::compile_function_body(
                    params,
                    named_params,
                    body,
                    function_names.clone(),
                    function_signatures.clone(),
                    function_bodies.clone(),
                    native_names.clone(),
                    global_names.clone(),
                    HashMap::new(),
                    module.functions.len() as u32,
                )?;
                module.functions[function_index as usize] = compiled.function;
                module.functions.append(&mut compiled.pending_functions);
            }
        }

        Ok(module)
    }

    pub fn compile_source(source: &str) -> Result<Function> {
        let tokens = Tokenizer::tokenize(source)?;
        let program = StmtParser::new(&tokens).parse_program()?;
        Self::compile_program(&program)
    }

    pub fn compile_source_module(source: &str) -> Result<Module> {
        Self::compile_source_module_with_natives(source, Vec::new())
    }

    pub fn compile_source_module_with_natives(source: &str, natives: Vec<NativeEntry>) -> Result<Module> {
        let tokens = Tokenizer::tokenize(source)?;
        let program = StmtParser::new(&tokens).parse_program()?;
        Self::compile_module_with_natives(&program, natives)
    }

    pub(super) fn with_names(
        function_names: HashMap<String, u32>,
        function_signatures: HashMap<String, FunctionSignature>,
        function_bodies: HashMap<String, super::support::FunctionInlineBody>,
        native_names: HashMap<String, u32>,
        global_names: HashMap<String, u32>,
        top_level: bool,
    ) -> Self {
        Self {
            function_names,
            function_signatures,
            function_bodies,
            native_names,
            global_names,
            top_level,
            ..Self::default()
        }
    }

    pub(super) fn compile_function_body(
        params: &[String],
        named_params: &[crate::stmt::NamedParamDecl],
        body: &Stmt,
        function_names: HashMap<String, u32>,
        function_signatures: HashMap<String, FunctionSignature>,
        function_bodies: HashMap<String, super::support::FunctionInlineBody>,
        native_names: HashMap<String, u32>,
        global_names: HashMap<String, u32>,
        capture_names: HashMap<String, u16>,
        dynamic_function_base: u32,
    ) -> Result<CompiledFunction> {
        let frame_params = function_frame_params(params, named_params);
        if frame_params.len() > u16::MAX as usize {
            bail!("Compiler function has too many params: {}", frame_params.len());
        }
        let mut compiler = Self::with_names(
            function_names,
            function_signatures,
            function_bodies,
            native_names,
            global_names,
            false,
        );
        compiler.capture_names = capture_names;
        compiler.dynamic_function_base = dynamic_function_base;
        compiler.function.param_count = frame_params.len() as u16;
        compiler.function.positional_param_count = params.len() as u16;
        compiler.function.param_names = Vec::with_capacity(frame_params.len());
        for name in &frame_params {
            compiler
                .function
                .param_names
                .push(std::sync::Arc::<str>::from(name.as_str()));
        }
        compiler.function.capture_count = compiler.capture_names.len() as u16;
        compiler.next_reg = compiler.function.param_count;
        compiler.peak_reg = compiler.function.param_count;
        for (index, param) in frame_params.iter().enumerate() {
            compiler.insert_local(param.clone(), index as u16);
        }
        compiler.lower_stmt(body)?;
        if !compiler.emitted_return {
            compiler.emit_empty_return();
        }
        Ok(CompiledFunction {
            function: compiler.finish()?,
            pending_functions: compiler.pending_functions,
        })
    }

    pub(super) fn lower_program_statements(&mut self, program: &Program) -> Result<()> {
        self.lower_stmt_sequence(&program.statements)?;
        if !self.emitted_return {
            self.emit_empty_return();
        }
        Ok(())
    }
}
