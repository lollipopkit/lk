use crate::compat::collections::HashMap;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

use anyhow::{Result, anyhow, bail};

use crate::{
    expr::Expr,
    stmt::{Program, Stmt},
    syntax::{ParseOptions, parse_program_source},
};

use super::{
    CompiledFunction, Compiler, Function, FunctionSignature, HashSet, Module, NativeEntry,
    collect_function_inline_bodies, collect_function_machine_returns, collect_function_names,
    collect_function_signatures, collect_function_visible_let_names, collect_global_names_with_external,
    collect_impl_method_names, collect_native_names, collect_struct_field_machine_widths,
    collect_top_level_machine_widths, export_name_from_attributes, extern_name_from_attributes, function_frame_params,
    global_slots_from_names, item_without_attributes,
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
        Self::compile_module_with_natives_and_globals(program, natives, core::iter::empty::<&str>())
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
        let user_let_globals = collect_function_visible_let_names(program);
        let machine_returns = collect_function_machine_returns(program);
        let struct_widths = collect_struct_field_machine_widths(program);
        let impl_methods = collect_impl_method_names(program);
        let global_widths = collect_top_level_machine_widths(program);
        let mut module = Module {
            functions: vec![Function::default(); function_names.len() + 1],
            natives,
            globals: global_slots_from_names(&global_names),
            entry: 0,
            type_info: crate::vm::TypeInfo::default(),
            // Stamped by the caller, which knows what file this is
            // (`compile_program_module_with_ctx`); the compiler does not.
            type_scope: crate::vm::TypeScope::anonymous(),
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
        entry.function_machine_returns = machine_returns.clone();
        entry.struct_field_machine_widths = struct_widths.clone();
        entry.impl_method_names = impl_methods.clone();
        entry.global_machine_widths = global_widths.clone();
        entry.dynamic_function_base = module.functions.len() as u32;
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

    /// Records, per impl method, how its reachable subtree uses module globals
    /// (see [`crate::vm::ImplMethod::writes_globals`] and
    /// [`reads_globals`](crate::vm::ImplMethod::reads_globals)).
    ///
    /// Reachability follows `CallDirect` and `MakeClosure`, the two opcodes that
    /// name a function index statically — the same edges the AOT hybrid prescan
    /// walks. An indirect call (a closure through a register, a builtin loaded
    /// into one, a method dispatch) is *not* followed, so it counts as
    /// `writes_globals`: that keeps the read list complete for every method the
    /// flag clears, which is what a cross-module dispatch relies on.
    fn record_impl_method_global_use(module: &mut Module) {
        use super::super::ir::Opcode;

        /// A call this walk cannot follow to a named function index.
        fn is_opaque_call(op: Opcode) -> bool {
            matches!(op, Opcode::Call | Opcode::CallNamed | Opcode::CallMethodK)
        }

        let walk = |root: u32| -> (bool, Vec<u16>) {
            let mut reads: Vec<u16> = Vec::new();
            let mut seen = vec![false; module.functions.len()];
            let mut stack = vec![root as usize];
            while let Some(index) = stack.pop() {
                if index >= module.functions.len() || core::mem::replace(&mut seen[index], true) {
                    continue;
                }
                for instr in &module.functions[index].code {
                    match instr.opcode() {
                        Opcode::SetGlobal => return (true, Vec::new()),
                        op if is_opaque_call(op) => return (true, Vec::new()),
                        Opcode::GetGlobal => reads.push(instr.bx()),
                        Opcode::CallDirect | Opcode::MakeClosure => stack.push(instr.b() as usize),
                        _ => {}
                    }
                }
            }
            reads.sort_unstable();
            reads.dedup();
            (false, reads)
        };

        for decl in &mut module.type_info.impls {
            for method in &mut decl.methods {
                let (writes_globals, reads_globals) = walk(method.function);
                method.writes_globals = writes_globals;
                method.reads_globals = reads_globals;
            }
        }
    }

    pub fn compile_source(source: &str) -> Result<Function> {
        let program = parse_program_source(source, ParseOptions::default())?;
        Ok(Self::compile_module(&program)?.functions.swap_remove(0))
    }

    pub fn compile_source_module(source: &str) -> Result<Module> {
        Self::compile_source_module_with_natives(source, Vec::new())
    }

    pub fn compile_source_module_with_natives(source: &str, natives: Vec<NativeEntry>) -> Result<Module> {
        let program = parse_program_source(source, ParseOptions::default())?;
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

    #[allow(clippy::too_many_arguments)]
    pub(super) fn compile_function_body(
        params: &[String],
        param_types: &[Option<crate::val::Type>],
        named_params: &[crate::stmt::NamedParamDecl],
        body: &Stmt,
        function_names: HashMap<String, u32>,
        function_signatures: HashMap<String, FunctionSignature>,
        function_bodies: HashMap<String, super::support::FunctionInlineBody>,
        native_names: HashMap<String, u32>,
        global_names: HashMap<String, u32>,
        user_let_globals: HashSet<String>,
        machine_returns: HashMap<String, crate::val::IntKind>,
        struct_widths: HashMap<String, HashMap<String, crate::val::IntKind>>,
        impl_methods: HashSet<String>,
        global_widths: HashMap<String, crate::val::IntKind>,
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
        compiler.function_machine_returns = machine_returns;
        compiler.struct_field_machine_widths = struct_widths;
        compiler.impl_method_names = impl_methods;
        compiler.global_machine_widths = global_widths;
        compiler.capture_names = capture_names;
        compiler.dynamic_function_base = dynamic_function_base;
        compiler.function.param_count = frame_params.len() as u16;
        compiler.function.positional_param_count = params.len() as u16;
        compiler.function.param_names = Vec::with_capacity(frame_params.len());
        for name in &frame_params {
            compiler
                .function
                .param_names
                .push(alloc::sync::Arc::<str>::from(name.as_str()));
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
            if let Some(crate::val::Type::MachineInt(kind)) = declared
                && index < params.len()
            {
                compiler.machine_regs.insert(index as u16, *kind);
            }
        }
        // Named parameters carry their own annotations and sit after the
        // positional ones in the frame, in `function_frame_params` order.
        for (offset, named) in named_params.iter().enumerate() {
            if let Some(crate::val::Type::MachineInt(kind)) = &named.type_annotation {
                let index = params.len() + offset;
                if index < frame_params.len() {
                    compiler.machine_regs.insert(index as u16, *kind);
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
