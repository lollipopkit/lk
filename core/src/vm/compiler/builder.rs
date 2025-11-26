use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use super::driver::Compiler;
use super::free_vars::FreeVarCollector;
use crate::resolve::slots::FunctionLayout;
use crate::{
    expr::Expr,
    op::BinOp,
    stmt::{NamedParamDecl, Stmt},
    typ::{NumericClass, NumericHierarchy},
    val::{ClosureCapture, ClosureInit, ClosureValue, Type, Val},
    vm::{
        Bc32Function, CaptureSpec, ClosureProto, Function, FunctionAnalysis, NamedParamLayoutEntry, Op, PatternBinding,
        PatternPlan, context::VmContext,
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
    var_scope_stack: Vec<Vec<(String, Option<u16>)>>,
    pub capture_indices: HashMap<String, u16>,
    pub break_locations: Vec<usize>,
    pub continue_locations: Vec<usize>,
    pub loop_depth: usize,
    pub analysis: Option<FunctionAnalysis>,
    pub const_names: HashSet<String>,
    expr_type_hints: Option<HashMap<usize, Type>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArithFlavor {
    Int,
    Float,
    Any,
}

fn collect_pattern_names(pattern: &crate::expr::Pattern, out: &mut Vec<String>) {
    use crate::expr::Pattern;
    match pattern {
        Pattern::Variable(name) => out.push(name.clone()),
        Pattern::List { patterns, rest } => {
            for sub in patterns {
                collect_pattern_names(sub, out);
            }
            if let Some(rest_name) = rest {
                out.push(rest_name.clone());
            }
        }
        Pattern::Map { patterns, rest } => {
            for (_, sub) in patterns {
                collect_pattern_names(sub, out);
            }
            if let Some(rest_name) = rest {
                out.push(rest_name.clone());
            }
        }
        Pattern::Or(alternatives) => {
            for alt in alternatives {
                collect_pattern_names(alt, out);
            }
        }
        Pattern::Guard { pattern, .. } => {
            collect_pattern_names(pattern, out);
        }
        Pattern::Literal(_) | Pattern::Wildcard | Pattern::Range { .. } => {}
    }
}

impl FunctionBuilder {
    pub fn new() -> Self {
        Self::new_with_captures(&[])
    }

    pub fn new_with_captures(captures: &[CaptureSpec]) -> Self {
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
            var_scope_stack: Vec::new(),
            capture_indices: HashMap::new(),
            break_locations: Vec::new(),
            continue_locations: Vec::new(),
            loop_depth: 0,
            analysis: None,
            const_names: HashSet::new(),
            expr_type_hints: None,
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

    fn expr_type_hint(&self, expr: &Expr) -> Option<&Type> {
        let key = expr as *const Expr as usize;
        self.expr_type_hints.as_ref().and_then(|map| map.get(&key))
    }

    fn numeric_class_hint(&self, expr: &Expr) -> Option<NumericClass> {
        self.expr_type_hint(expr).and_then(NumericHierarchy::classify)
    }

    pub(crate) fn select_arith_flavor(&self, op: &BinOp, left: &Expr, right: &Expr, whole: &Expr) -> ArithFlavor {
        if matches!(op, BinOp::Div) {
            return ArithFlavor::Float;
        }

        let result_class = self.numeric_class_hint(whole);
        let left_class = self.numeric_class_hint(left);
        let right_class = self.numeric_class_hint(right);

        match result_class {
            Some(NumericClass::Float) => ArithFlavor::Float,
            Some(NumericClass::Int) => {
                if left_class == Some(NumericClass::Int) && right_class == Some(NumericClass::Int) {
                    ArithFlavor::Int
                } else {
                    ArithFlavor::Any
                }
            }
            Some(NumericClass::Boxed) | None => {
                if left_class == Some(NumericClass::Float) || right_class == Some(NumericClass::Float) {
                    ArithFlavor::Float
                } else if left_class == Some(NumericClass::Int) && right_class == Some(NumericClass::Int) {
                    ArithFlavor::Int
                } else {
                    ArithFlavor::Any
                }
            }
        }
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
            let const_idx = self.k(Val::Str(decl.name.clone().into()));
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

        if let Some(packed) = Bc32Function::try_from_function(&f) {
            let decoded = packed.decoded;
            f.code32 = Some(packed.code32);
            f.bc32_decoded = decoded;
        }

        f
    }

    pub fn emit(&mut self, op: Op) {
        self.code.push(op);
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

    pub fn emit_function_closure(
        &mut self,
        name: Option<&str>,
        params: &[String],
        named_params: &[NamedParamDecl],
        body: &Stmt,
        register_const: bool,
    ) -> u16 {
        if register_const && let Some(func_name) = name {
            self.register_function_const_env(func_name, params, named_params, body);
        }

        let proto_idx = self.protos.len() as u16;
        let captures = self.collect_captures(name, params, named_params, body);
        let compiled = Compiler::new().compile_function_with_captures(params, named_params, body, &captures);
        let default_funcs: Vec<Option<Function>> = named_params
            .iter()
            .map(|decl| {
                decl.default.as_ref().map(|expr| {
                    Compiler::new().compile_default_expr_with_captures(params, named_params, expr, &captures)
                })
            })
            .collect();

        self.protos.push(ClosureProto {
            self_name: name.map(|n| n.to_string()),
            params: params.to_vec(),
            named_params: named_params.to_vec(),
            default_funcs,
            func: Some(Box::new(compiled)),
            body: body.clone(),
            captures,
        });

        let dst = self.alloc();
        self.emit(Op::MakeClosure { dst, proto: proto_idx });
        dst
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
            let msg_idx = self.k(Val::Str(msg.into()));
            self.emit(Op::Raise { err_kidx: msg_idx });
        } else {
            self.emit(Op::StoreLocal(idx, src));
        }
    }

    fn register_function_const_env(
        &mut self,
        name: &str,
        params: &[String],
        named_params: &[NamedParamDecl],
        body: &Stmt,
    ) {
        let mut func_val = Val::Closure(Arc::new(ClosureValue::new(ClosureInit {
            params: Arc::new(params.to_vec()),
            named_params: Arc::new(named_params.to_vec()),
            body: Arc::new(body.clone()),
            env: Arc::new(self.const_env.clone()),
            upvalues: Arc::new(Vec::<Val>::new()),
            captures: ClosureCapture::empty(),
            capture_specs: Arc::new(Vec::new()),
            default_funcs: Arc::new(Vec::new()),
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
}
