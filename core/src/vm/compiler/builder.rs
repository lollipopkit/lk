use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use super::builder_support::collect_pattern_names;
use super::driver::Compiler;
use super::free_vars::FreeVarCollector;
use crate::resolve::slots::FunctionLayout;
use crate::{
    expr::Expr,
    stmt::{NamedParamDecl, Stmt},
    val::{ClosureCapture, ClosureInit, ClosureValue, Type, Val},
    vm::{
        Bc32Function, CaptureSpec, ClosureProto, Function, FunctionAnalysis, NamedParamLayoutEntry, Op, PatternBinding,
        PatternPlan, capture_names_from_specs, closure_code_cell, closure_empty_captures, closure_empty_closure_cell,
        closure_empty_env, closure_empty_upvalues, context::VmContext,
    },
};

pub(crate) struct FunctionBuilder {
    pub consts: Vec<Val>,
    pub code: Vec<Op>,
    pub n_regs: u16,
    pub vars: HashMap<String, u16>,
    pub protos: Vec<ClosureProto>,
    pub param_regs: Vec<u16>,
    pub named_param_regs: Vec<u16>,
    pub named_param_layout: Vec<NamedParamLayoutEntry>,
    pub pattern_plans: Vec<PatternPlan>,
    pub const_bindings: HashMap<String, Val>,
    pub const_scope_stack: Vec<Vec<String>>,
    pub const_env: VmContext,
    pub global_defs: HashSet<String>,
    pub export_toplevel_globals: bool,
    var_scope_stack: Vec<Vec<(String, Option<u16>)>>,
    pub capture_indices: HashMap<String, u16>,
    pub break_locations: Vec<usize>,
    pub continue_locations: Vec<usize>,
    pub loop_depth: usize,
    pub analysis: Option<FunctionAnalysis>,
    pub const_names: HashSet<String>,
    pub(crate) expr_type_hints: Option<HashMap<usize, Type>>,
    /// Registers known to hold Map values (set when initialized from {} or map exprs).
    /// Used to safely emit MapSet opcode in compile_method_call.
    pub(crate) map_locals: HashSet<u16>,
    /// Registers whose Map values are known to have a homogeneous value type.
    /// This feeds typed arithmetic after MapGet without assuming a concrete key exists.
    pub(crate) map_value_types: HashMap<u16, Type>,
    /// Empty Map registers can adopt the first known value type written through MapSet.
    pub(crate) map_value_adoptable: HashSet<u16>,
    /// Registers known to hold List values (set when initialized from [] or list exprs).
    pub(crate) list_locals: HashSet<u16>,
    /// Registers whose List values are known to have a homogeneous element type.
    /// Kept conservative around mutation and aliases.
    pub(crate) list_value_types: HashMap<u16, Type>,
    /// Registers whose List length is known at compile time.
    pub(crate) list_lengths: HashMap<u16, usize>,
    /// Empty List registers can adopt the first known element type written through ListPush.
    pub(crate) list_value_adoptable: HashSet<u16>,
    /// Registers known to currently hold Int values.
    /// This is a best-effort local fact used to select typed arithmetic opcodes
    /// in hot loops even when full type inference did not provide hints.
    pub(crate) int_regs: HashSet<u16>,
    /// Registers known to currently hold Float values.
    /// Kept separate from int facts so generic numeric lowering can choose
    /// float typed opcodes without relying on full type inference.
    pub(crate) float_regs: HashSet<u16>,
    /// Loop-invariant pure expressions already materialized for the current loop body.
    pub(crate) loop_invariant_expr_regs: Vec<(Expr, u16)>,
    pub(crate) inferred_function_param_types: HashMap<String, Vec<Option<Type>>>,
    pub(crate) inferred_function_return_types: HashMap<String, Option<Type>>,
}

impl FunctionBuilder {
    pub fn new() -> Self {
        Self::new_with_captures_and_global_exports(&[], true)
    }

    pub fn new_function_with_captures(captures: &[CaptureSpec]) -> Self {
        Self::new_with_captures_and_global_exports(captures, false)
    }

    fn new_with_captures_and_global_exports(captures: &[CaptureSpec], export_toplevel_globals: bool) -> Self {
        let mut builder = Self {
            consts: Vec::new(),
            code: Vec::new(),
            n_regs: 0,
            vars: HashMap::new(),
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            const_bindings: HashMap::new(),
            const_scope_stack: vec![Vec::new()],
            const_env: VmContext::new(),
            global_defs: HashSet::new(),
            export_toplevel_globals,
            var_scope_stack: Vec::new(),
            capture_indices: HashMap::new(),
            break_locations: Vec::new(),
            continue_locations: Vec::new(),
            loop_depth: 0,
            analysis: None,
            const_names: HashSet::new(),
            expr_type_hints: None,
            map_locals: HashSet::new(),
            map_value_types: HashMap::new(),
            map_value_adoptable: HashSet::new(),
            list_locals: HashSet::new(),
            list_value_types: HashMap::new(),
            list_lengths: HashMap::new(),
            list_value_adoptable: HashSet::new(),
            int_regs: HashSet::new(),
            float_regs: HashSet::new(),
            loop_invariant_expr_regs: Vec::new(),
            inferred_function_param_types: HashMap::new(),
            inferred_function_return_types: HashMap::new(),
        };
        for (idx, cap) in captures.iter().enumerate() {
            let name = match cap {
                CaptureSpec::Register { name, .. } | CaptureSpec::Const { name, .. } | CaptureSpec::Global { name } => {
                    name
                }
            };
            builder.capture_indices.insert(name.clone(), idx as u16);
        }
        builder
    }

    pub fn set_expr_type_hints(&mut self, hints: HashMap<usize, Type>) {
        self.expr_type_hints = Some(hints);
    }

    pub fn set_inferred_function_param_types(&mut self, inferred: HashMap<String, Vec<Option<Type>>>) {
        self.inferred_function_param_types = inferred;
    }

    pub fn set_inferred_function_return_types(&mut self, inferred: HashMap<String, Option<Type>>) {
        self.inferred_function_return_types = inferred;
    }

    pub fn set_analysis(&mut self, analysis: Option<FunctionAnalysis>) {
        self.analysis = analysis;
    }

    pub fn apply_slot_layout(&mut self, layout: &FunctionLayout) {
        self.n_regs = self.n_regs.max(layout.total_locals);
        for decl in &layout.decls {
            self.vars.insert(decl.name.clone(), decl.index);
        }
    }

    pub fn build_named_param_layout(&mut self, named_params: &[NamedParamDecl]) {
        self.named_param_layout.clear();
        if named_params.is_empty() {
            return;
        }
        for (idx, decl) in named_params.iter().enumerate() {
            let const_idx = self.k(Val::from_str(decl.name.as_str()));
            let dest = self.named_param_regs.get(idx).copied().unwrap_or(0);
            let default_index = decl.default.as_ref().map(|_| idx as u16);
            self.named_param_layout.push(NamedParamLayoutEntry {
                name_const_idx: const_idx,
                dest_reg: dest,
                default_index,
            });
        }
    }

    pub fn finish(self) -> Function {
        let mut f = Function {
            consts: self.consts,
            code: self.code,
            n_regs: self.n_regs,
            protos: self.protos,
            param_regs: self.param_regs,
            named_param_regs: self.named_param_regs,
            named_param_layout: self.named_param_layout,
            pattern_plans: self.pattern_plans,
            code32: None,
            bc32_decoded: None,
            analysis: self.analysis,
        };

        // Peephole: fuse common compare/branch and presence-check patterns.
        super::peephole::peephole_fuse_cmp_jmp_with_consts(&mut f.code, &f.consts);

        if let Some(packed) = Bc32Function::try_from_function(&f) {
            let decoded = packed.decoded;
            f.code32 = Some(packed.code32);
            f.bc32_decoded = decoded;
        }

        f
    }

    pub fn emit(&mut self, op: Op) {
        self.update_int_reg_facts(&op);
        self.code.push(op);
    }

    pub(crate) fn emit_positional_call(&mut self, f: u16, base: u16, argc: u8, retc: u8, known_callee: Option<&Val>) {
        let op = match known_callee {
            Some(Val::Closure(closure)) if closure.named_params.is_empty() && closure.params.len() == argc as usize => {
                Op::CallClosureExact { f, base, argc, retc }
            }
            Some(Val::RustFunction(_) | Val::RustFastFunction(_)) => Op::CallNativeFast { f, base, argc, retc },
            _ => Op::Call { f, base, argc, retc },
        };
        self.emit(op);
    }

    pub(crate) fn reserve_call_window(&mut self, argc: usize, retc: u8) -> u16 {
        let base = self.n_regs;
        let slots = argc.max(retc as usize);
        for _ in 0..slots {
            let _ = self.alloc();
        }
        base
    }

    pub(crate) fn emit_known_or_global_callable(&mut self, name: &str, known_callee: Option<&Val>) -> u16 {
        let dst = self.alloc();
        if let Some(value @ Val::Closure(_)) = known_callee {
            let kidx = self.k(value.clone());
            self.emit(Op::LoadK(dst, kidx));
        } else {
            let kidx = self.k(Val::from_str(name));
            self.emit(Op::LoadGlobal(dst, kidx));
        }
        dst
    }

    pub fn alloc(&mut self) -> u16 {
        let r = self.n_regs;
        self.n_regs = self.n_regs.saturating_add(1);
        r
    }

    pub fn k(&mut self, v: Val) -> u16 {
        if let Some((i, _)) = self.consts.iter().enumerate().find(|(_, x)| *x == &v) {
            i as u16
        } else {
            self.consts.push(v);
            (self.consts.len() - 1) as u16
        }
    }

    pub fn get_or_define(&mut self, name: &str) -> u16 {
        if let Some(&i) = self.vars.get(name) {
            i
        } else {
            let idx = self.alloc();
            self.vars.insert(name.to_string(), idx);
            idx
        }
    }

    pub fn lookup(&self, name: &str) -> Option<u16> {
        self.vars.get(name).copied()
    }

    pub fn register_pattern_plan(&mut self, pattern: &crate::expr::Pattern) -> u16 {
        let mut names = Vec::new();
        collect_pattern_names(pattern, &mut names);
        let mut seen = HashSet::new();
        let mut bindings = Vec::new();
        for name in names {
            if seen.insert(name.clone()) {
                let reg = self.get_or_define(&name);
                bindings.push(PatternBinding { name, reg });
            }
        }
        let idx = self.pattern_plans.len();
        self.pattern_plans.push(PatternPlan {
            pattern: pattern.clone(),
            bindings,
        });
        idx as u16
    }

    pub fn register_scoped_pattern_plan(&mut self, pattern: &crate::expr::Pattern) -> u16 {
        let mut names = Vec::new();
        collect_pattern_names(pattern, &mut names);
        let mut seen = HashSet::new();
        let mut bindings = Vec::new();
        for name in names {
            if seen.insert(name.clone()) {
                let reg = self.define_scoped_var(&name);
                bindings.push(PatternBinding { name, reg });
            }
        }
        let idx = self.pattern_plans.len();
        self.pattern_plans.push(PatternPlan {
            pattern: pattern.clone(),
            bindings,
        });
        idx as u16
    }

    pub fn push_var_scope(&mut self) {
        self.var_scope_stack.push(Vec::new());
    }

    pub fn pop_var_scope(&mut self) {
        if let Some(entries) = self.var_scope_stack.pop() {
            for (name, prev) in entries.into_iter().rev() {
                match prev {
                    Some(idx) => {
                        self.vars.insert(name, idx);
                    }
                    None => {
                        self.vars.remove(&name);
                    }
                }
            }
        }
    }

    pub fn define_scoped_var(&mut self, name: &str) -> u16 {
        debug_assert!(
            self.var_scope_stack.last().is_some(),
            "define_scoped_var called without an active var scope"
        );
        let reg = self.alloc();
        let prev = self.vars.insert(name.to_string(), reg);
        if let Some(scope) = self.var_scope_stack.last_mut() {
            scope.push((name.to_string(), prev));
        }
        reg
    }

    /// Define a variable that already has a register assigned (e.g., from expr result).
    /// Records the scope stack entry so the variable is properly unwound.
    pub fn define_var_as(&mut self, name: &str, reg: u16) {
        let prev = self.vars.insert(name.to_string(), reg);
        if let Some(scope) = self.var_scope_stack.last_mut() {
            scope.push((name.to_string(), prev));
        }
    }

    #[inline]
    pub fn var_scope_depth(&self) -> usize {
        self.var_scope_stack.len()
    }

    pub fn emit_function_closure(
        &mut self,
        name: Option<&str>,
        params: &[String],
        param_types: &[Option<Type>],
        named_params: &[NamedParamDecl],
        body: &Stmt,
        register_const: bool,
    ) -> u16 {
        let dst = self.alloc();
        self.emit_function_closure_into(dst, name, params, param_types, named_params, body, register_const);
        dst
    }

    pub fn emit_function_closure_into(
        &mut self,
        dst: u16,
        name: Option<&str>,
        params: &[String],
        param_types: &[Option<Type>],
        named_params: &[NamedParamDecl],
        body: &Stmt,
        register_const: bool,
    ) {
        if register_const && let Some(func_name) = name {
            self.register_function_const_env(func_name, params, param_types, named_params, body);
        }

        let proto_idx = self.protos.len() as u16;
        let captures = self.collect_captures(name, params, named_params, body);
        let compiled = Compiler::new().compile_function_with_param_types_and_captures(
            params,
            param_types,
            named_params,
            body,
            &captures,
        );
        let default_funcs: Vec<Option<Function>> = named_params
            .iter()
            .map(|decl| {
                decl.default.as_ref().map(|expr| {
                    Compiler::new().compile_default_expr_with_param_types_and_captures(
                        params,
                        param_types,
                        named_params,
                        expr,
                        &captures,
                    )
                })
            })
            .collect();

        let func = Arc::new(compiled);
        self.protos.push(ClosureProto {
            self_name: name.map(|n| n.to_string()),
            params: Arc::new(params.to_vec()),
            param_types: Arc::new(param_types.to_vec()),
            named_params: Arc::new(named_params.to_vec()),
            default_funcs: Arc::new(default_funcs),
            func: Some(Arc::clone(&func)),
            body: Arc::new(body.clone()),
            capture_names: capture_names_from_specs(&captures),
            captures: Arc::new(captures),
            code: closure_code_cell(Some(&func)),
            empty_env: closure_empty_env(),
            empty_upvalues: closure_empty_upvalues(),
            empty_captures: closure_empty_captures(),
            empty_closure: closure_empty_closure_cell(),
        });

        self.emit(Op::MakeClosure { dst, proto: proto_idx });
    }

    pub(crate) fn effective_function_param_types(
        &self,
        name: &str,
        params: &[String],
        declared: &[Option<Type>],
    ) -> Vec<Option<Type>> {
        let mut effective = declared.to_vec();
        if effective.len() < params.len() {
            effective.resize(params.len(), None);
        }
        if let Some(inferred) = self.inferred_function_param_types.get(name) {
            for (idx, inferred_ty) in inferred.iter().enumerate().take(params.len()) {
                if effective[idx].is_none()
                    && let Some(ty) = inferred_ty
                {
                    effective[idx] = Some(ty.clone());
                }
            }
        }
        effective
    }

    pub fn collect_captures(
        &mut self,
        self_name: Option<&str>,
        params: &[String],
        named_params: &[NamedParamDecl],
        body: &Stmt,
    ) -> Vec<CaptureSpec> {
        let mut collector = FreeVarCollector::new();
        if let Some(name) = self_name {
            collector.declare(name);
        }
        for param in params {
            collector.declare(param);
        }
        for decl in named_params {
            collector.declare(&decl.name);
            if let Some(default) = &decl.default {
                collector.visit_expr(default);
            }
        }
        collector.visit_stmt(body);
        collector
            .into_sorted_vec()
            .into_iter()
            .map(|name| {
                if let Some(&reg) = self.vars.get(&name) {
                    if self.global_defs.contains(&name) {
                        CaptureSpec::Global { name }
                    } else {
                        CaptureSpec::Register { name, src: reg }
                    }
                } else if let Some(val) = self.const_bindings.get(&name) {
                    let kidx = self.k(val.clone());
                    CaptureSpec::Const { name, kidx }
                } else {
                    CaptureSpec::Global { name }
                }
            })
            .collect()
    }

    pub(crate) fn record_const_pattern_names(&mut self, pattern: &crate::expr::Pattern) {
        let mut names = Vec::new();
        collect_pattern_names(pattern, &mut names);
        for name in names {
            self.const_names.insert(name);
        }
    }

    pub(crate) fn store_named(&mut self, name: &str, idx: u16, src: u16) {
        if self.const_names.contains(name) {
            let msg = format!("Cannot assign to const variable '{}'", name);
            let msg_idx = self.k(Val::from_str(msg.as_str()));
            self.emit(Op::Raise { err_kidx: msg_idx });
        } else {
            self.emit(Op::StoreLocal(idx, src));
            if self.should_export_global_write(name) {
                let kname = self.k(Val::from_str(name));
                self.emit(Op::DefineGlobal(kname, idx));
            }
        }
    }

    fn register_function_const_env(
        &mut self,
        name: &str,
        params: &[String],
        param_types: &[Option<Type>],
        named_params: &[NamedParamDecl],
        body: &Stmt,
    ) {
        let mut func_val = Val::Closure(Arc::new(ClosureValue::new(ClosureInit {
            params: Arc::new(params.to_vec()),
            param_types: Arc::new(param_types.to_vec()),
            named_params: Arc::new(named_params.to_vec()),
            body: Arc::new(body.clone()),
            env: Arc::new(self.const_env.clone()),
            upvalues: Arc::new(Vec::<Val>::new()),
            captures: ClosureCapture::empty(),
            capture_specs: Arc::new(Vec::new()),
            default_funcs: Arc::new(Vec::new()),
            code: Arc::new(once_cell::sync::OnceCell::new()),
            debug_name: Some(name.to_string()),
            debug_location: None,
        })));
        if let Val::Closure(closure_arc) = &mut func_val
            && let Some(closure) = Arc::get_mut(closure_arc)
            && let Some(env_mut) = Arc::get_mut(&mut closure.env)
        {
            let env_ptr: *mut VmContext = env_mut;
            let clone_for_env = func_val.clone();
            unsafe {
                (*env_ptr).set(name.to_string(), clone_for_env);
            }
        }
        self.const_env.set(name.to_string(), func_val);
    }

    pub(crate) fn register_closure_const_env(&mut self, name: &str, params: &[String], body: &Expr) -> bool {
        let body_stmt = match body {
            Expr::Block(statements) => Stmt::Block {
                statements: statements.clone(),
            },
            _ => Stmt::Expr(Box::new(body.clone())),
        };
        let captures = self.collect_captures(None, params, &[], &body_stmt);
        if !captures.is_empty() {
            return false;
        }

        let func_val = Val::Closure(Arc::new(ClosureValue::new(ClosureInit {
            params: Arc::new(params.to_vec()),
            param_types: Arc::new(Vec::new()),
            named_params: Arc::new(Vec::new()),
            body: Arc::new(body_stmt),
            env: Arc::new(self.const_env.clone()),
            upvalues: Arc::new(Vec::<Val>::new()),
            captures: ClosureCapture::empty(),
            capture_specs: Arc::new(Vec::new()),
            default_funcs: Arc::new(Vec::new()),
            code: Arc::new(once_cell::sync::OnceCell::new()),
            debug_name: Some(name.to_string()),
            debug_location: None,
        })));
        self.const_env.define(name.to_string(), func_val);
        true
    }

    /// Check if an expression contains a function call or other effects that make
    /// it unsafe to keep the result in the expression's own register.
    pub(crate) fn expr_contains_call(e: &Expr) -> bool {
        match e {
            Expr::Call(_, _) => true,
            Expr::CallNamed(_, _, _) => true,
            Expr::CallExpr(_, _) => true,
            Expr::Bin(l, _, r) => Self::expr_contains_call(l) || Self::expr_contains_call(r),
            Expr::Unary(_, inner) => Self::expr_contains_call(inner),
            Expr::Paren(inner) => Self::expr_contains_call(inner),
            Expr::Access(obj, field) => Self::expr_contains_call(obj) || Self::expr_contains_call(field),
            Expr::OptionalAccess(obj, field) => Self::expr_contains_call(obj) || Self::expr_contains_call(field),
            Expr::List(elems) => elems.iter().any(|e| Self::expr_contains_call(e)),
            Expr::Map(pairs) => pairs
                .iter()
                .any(|(k, v)| Self::expr_contains_call(k) || Self::expr_contains_call(v)),
            Expr::Conditional(c, t, e) => {
                Self::expr_contains_call(c) || Self::expr_contains_call(t) || Self::expr_contains_call(e)
            }
            Expr::And(l, r) => Self::expr_contains_call(l) || Self::expr_contains_call(r),
            Expr::Or(l, r) => Self::expr_contains_call(l) || Self::expr_contains_call(r),
            Expr::NullishCoalescing(l, r) => Self::expr_contains_call(l) || Self::expr_contains_call(r),
            Expr::Select { .. } => true,
            Expr::Closure { .. } => true,
            Expr::StructLiteral { .. } => true,
            Expr::Match { .. } => true,
            Expr::TemplateString(_) => true,
            _ => false,
        }
    }
}
