use crate::compat::collections::HashMap;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::rc::Rc;

use anyhow::{Result, anyhow, bail};

use crate::{
    expr::Expr,
    stmt::{Program, Stmt},
    syntax::{ParseOptions, parse_program_source},
};

use super::{
    CompiledFunction, Compiler, Function, FunctionSignature, HashSet, Module, collect_function_inline_bodies,
    collect_function_machine_returns, collect_function_names, collect_function_signatures,
    collect_function_visible_let_names, collect_global_names_with_external, collect_impl_method_names,
    collect_struct_field_machine_widths, collect_top_level_data_global_names, collect_top_level_machine_widths,
    export_name_from_attributes, extern_name_from_attributes, function_frame_params, global_slots_from_names,
    item_without_attributes,
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
        Self::compile_module_with_globals(program, core::iter::empty::<&str>())
    }

    pub fn compile_module_with_globals<I, S>(program: &Program, external_globals: I) -> Result<Module>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::compile_module_with_globals_and_data(program, external_globals, core::iter::empty::<&str>())
    }

    /// As [`Self::compile_module_with_globals`], with the subset of
    /// `external_globals` that hold *user data* rather than imported module
    /// objects.
    ///
    /// Only a host that keeps bindings alive across compilations knows this —
    /// in-tree that is the REPL, whose `xs` from a previous line is an external
    /// global indistinguishable from `math` without being told. Getting it
    /// wrong is not a missed optimisation: `xs.len()` compiles to an index read
    /// keyed by the string `"len"` and fails at run time.
    pub fn compile_module_with_globals_and_data<I, S, D, T>(
        program: &Program,
        external_globals: I,
        external_data_globals: D,
    ) -> Result<Module>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
        D: IntoIterator<Item = T>,
        T: AsRef<str>,
    {
        let external_data_globals = external_data_globals
            .into_iter()
            .map(|name| name.as_ref().to_owned())
            .collect::<Vec<_>>();
        let function_names = Rc::new(collect_function_names(program)?);
        let function_signatures = Rc::new(collect_function_signatures(program)?);
        let function_bodies = Rc::new(collect_function_inline_bodies(program)?);
        let native_names = Rc::new(HashMap::new());
        let global_names = Rc::new(collect_global_names_with_external(program, external_globals)?);
        let user_let_globals = Rc::new(collect_function_visible_let_names(program));
        let mut data_globals = collect_top_level_data_global_names(program);
        data_globals.extend(external_data_globals);
        let data_globals = Rc::new(data_globals);
        let machine_returns = Rc::new(collect_function_machine_returns(program));
        let struct_widths = Rc::new(collect_struct_field_machine_widths(program));
        let impl_methods = Rc::new(collect_impl_method_names(program));
        let global_widths = Rc::new(collect_top_level_machine_widths(program));
        let mut module = Module {
            functions: vec![Function::default(); function_names.len() + 1],
            globals: global_slots_from_names(&global_names),
            entry: 0,
            type_info: crate::vm::TypeInfo::default(),
            // Stamped by the caller, which knows what file this is
            // (`compile_program_module_with_ctx`); the compiler does not.
            type_scope: crate::val::TypeScope::anonymous(),
        };

        let mut entry = Self::with_names(
            function_names.clone(),
            function_signatures.clone(),
            function_bodies.clone(),
            native_names.clone(),
            global_names.clone(),
            true,
        );
        entry.user_let_globals = user_let_globals.clone();
        entry.top_level_data_globals = data_globals.clone();
        entry.function_machine_returns = machine_returns.clone();
        entry.struct_field_machine_widths = struct_widths.clone();
        entry.impl_method_names = impl_methods.clone();
        entry.global_machine_widths = global_widths.clone();
        entry.dynamic_function_base = module.functions.len() as u32;
        // As in `compile_function_body`: method-name constants first, so a
        // `CallMethodK`'s 8-bit name index does not run out on a top level that
        // also names structs and fields.
        for method in crate::stmt::init_order::method_names_called_at_top_level(program) {
            entry.push_string(&method)?;
        }
        entry.lower_program_statements(program)?;
        module.type_info = core::mem::take(&mut entry.type_info);
        module.functions[0] = entry.finish()?;
        module.functions.extend(entry.pending_functions);

        for stmt in &program.statements {
            if let Stmt::Function {
                name,
                params,
                param_types,
                named_params,
                body,
                ..
            } = item_without_attributes(stmt)
            {
                let function_index = *function_names
                    .get(name)
                    .ok_or_else(|| anyhow!("Compiler missing function index for `{name}`"))?;
                let mut compiled = Self::compile_function_body(
                    params,
                    param_types,
                    named_params,
                    body,
                    function_names.clone(),
                    function_signatures.clone(),
                    function_bodies.clone(),
                    native_names.clone(),
                    global_names.clone(),
                    user_let_globals.clone(),
                    data_globals.clone(),
                    machine_returns.clone(),
                    struct_widths.clone(),
                    impl_methods.clone(),
                    global_widths.clone(),
                    HashMap::new(),
                    module.functions.len() as u32,
                )?;
                compiled.function.debug_name = Some(alloc::sync::Arc::<str>::from(name.as_str()));
                compiled.function.export_name = export_name_from_attributes(stmt, name)?;
                compiled.function.extern_name = extern_name_from_attributes(stmt, name)?;
                module.functions[function_index as usize] = compiled.function;
                module.functions.append(&mut compiled.pending_functions);
            }
        }

        // Needs the whole function table, so it cannot happen in
        // `lower_impl_decl`: a method may call a function compiled after it.
        Self::record_impl_method_global_use(&mut module);

        // The load-time bytecode verifier (`vm::verify`) must accept every
        // module this compiler emits; running it here in debug builds turns the
        // whole test suite into a guard against both compiler-invariant
        // regressions and verifier false rejections.
        #[cfg(debug_assertions)]
        super::super::verify::verify_module(&module)?;

        Ok(module)
    }

    /// Records, per impl method, how its reachable subtree uses module globals.
    ///
    /// The walk itself is [`crate::vm::analysis::function_global_use`] — one
    /// implementation, because the same question is asked at run time when a
    /// function value crosses a module boundary, and two walks that disagreed
    /// would let a function that writes a global cross anyway.
    fn record_impl_method_global_use(module: &mut Module) {
        let facts: Vec<(u32, bool, Vec<u16>)> = module
            .type_info
            .impls
            .iter()
            .flat_map(|decl| decl.methods.iter())
            .map(|method| {
                let (writes, reads) =
                    crate::vm::analysis::function_global_use(module, method.function).writes_and_reads();
                (method.function, writes, reads)
            })
            .collect();
        let mut facts = facts.into_iter();
        for decl in &mut module.type_info.impls {
            for method in &mut decl.methods {
                let (function, writes, reads) = facts.next().expect("one fact per method, in the same order");
                debug_assert_eq!(function, method.function);
                method.writes_globals = writes;
                method.reads_globals = reads;
            }
        }
    }

    pub fn compile_source(source: &str) -> Result<Function> {
        let program = parse_program_source(source, ParseOptions::default())?;
        Ok(Self::compile_module(&program)?.functions.swap_remove(0))
    }

    pub fn compile_source_module(source: &str) -> Result<Module> {
        let program = parse_program_source(source, ParseOptions::default())?;
        Self::compile_module(&program)
    }

    pub(super) fn with_names(
        function_names: Rc<HashMap<String, u32>>,
        function_signatures: Rc<HashMap<String, FunctionSignature>>,
        function_bodies: Rc<HashMap<String, super::support::FunctionInlineBody>>,
        native_names: Rc<HashMap<String, u32>>,
        global_names: Rc<HashMap<String, u32>>,
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

    #[allow(clippy::too_many_arguments)]
    pub(super) fn compile_function_body(
        params: &[String],
        param_types: &[Option<crate::val::Type>],
        named_params: &[crate::stmt::NamedParamDecl],
        body: &Stmt,
        function_names: Rc<HashMap<String, u32>>,
        function_signatures: Rc<HashMap<String, FunctionSignature>>,
        function_bodies: Rc<HashMap<String, super::support::FunctionInlineBody>>,
        native_names: Rc<HashMap<String, u32>>,
        global_names: Rc<HashMap<String, u32>>,
        user_let_globals: Rc<HashSet<String>>,
        top_level_data_globals: Rc<HashSet<String>>,
        machine_returns: Rc<HashMap<String, super::RegisterWidth>>,
        struct_widths: Rc<HashMap<String, HashMap<String, super::RegisterWidth>>>,
        impl_methods: Rc<HashSet<String>>,
        global_widths: Rc<HashMap<String, super::RegisterWidth>>,
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
        compiler.user_let_globals = user_let_globals;
        compiler.top_level_data_globals = top_level_data_globals;
        compiler.function_machine_returns = machine_returns;
        compiler.struct_field_machine_widths = struct_widths;
        compiler.impl_method_names = impl_methods;
        compiler.global_machine_widths = global_widths;
        compiler.capture_names = capture_names;
        compiler.dynamic_function_base = dynamic_function_base;
        // Said at the declaration, like the closure form: a call passes at most
        // `MAX_CALL_ARGUMENTS`, so more parameters than that means a function
        // nothing can call. Reported before this as a register overflow at the
        // *call*, which pointed at the wrong line and offered advice about a
        // body that was not the problem.
        // `params`, not `frame_params`: the limit is the *positional* count,
        // which a call names in 7 bits. Named parameters ride a wider field and
        // are bounded by the register file instead (`MAX_STRUCT_FIELDS`) — a
        // 200-field struct's generated constructor is 200 named parameters, and
        // counting those here refused a struct literal that works.
        if params.len() > crate::vm::compiler::MAX_CALL_ARGUMENTS {
            bail!(
                "this function declares {} positional parameters, and {} is the most a call can pass, so it \
                 could never be called. Take a list or a map instead",
                params.len(),
                crate::vm::compiler::MAX_CALL_ARGUMENTS
            );
        }
        compiler.function.param_count = frame_params.len() as u16;
        compiler.function.positional_param_count = params.len() as u16;
        compiler.function.param_names = Vec::with_capacity(frame_params.len());
        for name in &frame_params {
            compiler
                .function
                .param_names
                .push(alloc::sync::Arc::<str>::from(name.as_str()));
        }
        // Method-name constants first, before anything in the body can take a
        // low index (see `stmt::init_order::method_names_called`).
        for method in crate::stmt::init_order::method_names_called(body) {
            compiler.push_string(&method)?;
        }
        compiler.function.capture_count = compiler.capture_names.len() as u16;
        compiler.next_reg = compiler.function.param_count;
        compiler.peak_reg = compiler.function.param_count;
        for (index, param) in frame_params.iter().enumerate() {
            compiler.insert_local(param.clone(), index as u16);
        }
        // A parameter's declared width is a width the body can rely on.
        //
        // Without this, every machine-integer rule stopped at the function
        // boundary: `fn f(a: u8) -> u8 { return a + 1; }` answered 256, and
        // `fn f(a: u64, b: u64) { return a > b; }` compared two addresses
        // *signed*. All of it — the wrap, the unsigned compare, the logical
        // shift, the unsigned divide — is chosen from `machine_regs`, and a
        // parameter register was never in it. The rules held for an annotated
        // `let` and for an `as` cast, which is why every test and every driver
        // that casts on entry looked right.
        //
        // No differential test could see it: the fact is missing in the
        // compiler, so both backends are handed the same wrong instruction.
        for (index, declared) in param_types.iter().enumerate() {
            if let Some(width) = declared.as_ref().and_then(crate::vm::compiler::register_width_of)
                && index < params.len()
            {
                compiler.machine_regs.insert(index as u16, width);
            }
        }
        // Named parameters carry their own annotations and sit after the
        // positional ones in the frame, in `function_frame_params` order.
        for (offset, named) in named_params.iter().enumerate() {
            if let Some(width) = named
                .type_annotation
                .as_ref()
                .and_then(crate::vm::compiler::register_width_of)
            {
                let index = params.len() + offset;
                if index < frame_params.len() {
                    compiler.machine_regs.insert(index as u16, width);
                }
            }
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
