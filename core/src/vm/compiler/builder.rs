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
    var_scope_stack: Vec<Vec<(String, Option<u16>)>>,
    pub capture_indices: HashMap<String, u16>,
    pub break_locations: Vec<usize>,
    pub continue_locations: Vec<usize>,
    pub loop_depth: usize,
    pub analysis: Option<FunctionAnalysis>,
    pub const_names: HashSet<String>,
    expr_type_hints: Option<HashMap<usize, Type>>,
    /// Registers known to hold Map values (set when initialized from {} or map exprs).
    /// Used to safely emit MapSet opcode in compile_method_call.
    pub(crate) map_locals: HashSet<u16>,
    /// Registers known to hold List values (set when initialized from [] or list exprs).
    pub(crate) list_locals: HashSet<u16>,
    /// Registers known to currently hold Int values.
    /// This is a best-effort local fact used to select typed arithmetic opcodes
    /// in hot loops even when full type inference did not provide hints.
    pub(crate) int_regs: HashSet<u16>,
    /// Loop-invariant pure expressions already materialized for the current loop body.
    pub(crate) loop_invariant_expr_regs: Vec<(Expr, u16)>,
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
            map_locals: HashSet::new(),
            list_locals: HashSet::new(),
            int_regs: HashSet::new(),
            loop_invariant_expr_regs: Vec::new(),
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

        if self.expr_known_int(left) && self.expr_known_int(right) {
            return ArithFlavor::Int;
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

    fn expr_known_int(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Val(Val::Int(_)) => true,
            Expr::Var(name) => {
                self.lookup(name).is_some_and(|reg| self.int_regs.contains(&reg))
                    || self
                        .lookup_const(name)
                        .is_some_and(|value| matches!(value, Val::Int(_)))
            }
            Expr::Paren(inner) => self.expr_known_int(inner),
            Expr::Bin(left, op, right) if !matches!(op, BinOp::Div) && op.is_arith() => {
                self.expr_known_int(left) && self.expr_known_int(right)
            }
            _ => false,
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

        // Peephole: fuse CmpLtImm + JmpFalse into CmpLtImmJmp
        super::peephole::peephole_fuse_cmp_jmp(&mut f.code);

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

    fn update_int_reg_facts(&mut self, op: &Op) {
        match *op {
            Op::LoadK(dst, kidx) => match self.consts.get(kidx as usize) {
                Some(Val::Int(_)) => {
                    self.int_regs.insert(dst);
                    self.list_locals.remove(&dst);
                    self.map_locals.remove(&dst);
                }
                Some(Val::List(_)) => {
                    self.int_regs.remove(&dst);
                    self.list_locals.insert(dst);
                    self.map_locals.remove(&dst);
                }
                Some(Val::Map(_)) => {
                    self.int_regs.remove(&dst);
                    self.list_locals.remove(&dst);
                    self.map_locals.insert(dst);
                }
                _ => {
                    self.int_regs.remove(&dst);
                    self.list_locals.remove(&dst);
                    self.map_locals.remove(&dst);
                }
            },
            Op::Move(dst, src) | Op::StoreLocal(dst, src) => {
                if self.int_regs.contains(&src) {
                    self.int_regs.insert(dst);
                } else {
                    self.int_regs.remove(&dst);
                }
                if self.list_locals.contains(&src) {
                    self.list_locals.insert(dst);
                } else {
                    self.list_locals.remove(&dst);
                }
                if self.map_locals.contains(&src) {
                    self.map_locals.insert(dst);
                } else {
                    self.map_locals.remove(&dst);
                }
            }
            Op::AddInt(dst, _, _)
            | Op::SubInt(dst, _, _)
            | Op::MulInt(dst, _, _)
            | Op::ModInt(dst, _, _)
            | Op::AddIntImm(dst, _, _)
            | Op::AddIntImmJmp { r: dst, .. }
            | Op::AddRangeCountImm { target: dst, .. }
            | Op::Len { dst, .. }
            | Op::ListLen { dst, .. }
            | Op::MapLen { dst, .. }
            | Op::StrLen { dst, .. } => {
                self.int_regs.insert(dst);
            }
            Op::ForRangePrep { step, .. } => {
                self.int_regs.insert(step);
            }
            Op::ForRangeLoop {
                idx, write_idx: true, ..
            }
            | Op::RangeLoopI {
                idx, write_idx: true, ..
            } => {
                self.int_regs.insert(idx);
            }
            Op::Add(dst, _, _)
            | Op::StrConcatKnownCap(dst, _, _)
            | Op::Sub(dst, _, _)
            | Op::Mul(dst, _, _)
            | Op::Div(dst, _, _)
            | Op::Mod(dst, _, _)
            | Op::AddFloat(dst, _, _)
            | Op::SubFloat(dst, _, _)
            | Op::MulFloat(dst, _, _)
            | Op::DivFloat(dst, _, _)
            | Op::ModFloat(dst, _, _)
            | Op::LoadGlobal(dst, _)
            | Op::LoadCapture { dst, .. }
            | Op::Access(dst, _, _)
            | Op::AccessK(dst, _, _)
            | Op::IndexK(dst, _, _)
            | Op::ListIndexI(dst, _, _)
            | Op::ListSetI { dst, .. }
            | Op::StrIndexI(dst, _, _)
            | Op::ContainsK(dst, _, _)
            | Op::MapHas(dst, _, _)
            | Op::MapGetInterned(dst, _, _)
            | Op::MapGetDynamic(dst, _, _)
            | Op::MapHasK(dst, _, _)
            | Op::MakeClosure { dst, .. }
            | Op::Call { base: dst, retc: 1, .. }
            | Op::CallExact { base: dst, retc: 1, .. }
            | Op::CallClosureExact { base: dst, retc: 1, .. }
            | Op::CallNativeFast { base: dst, retc: 1, .. }
            | Op::CallNamed {
                base_pos: dst, retc: 1, ..
            }
            | Op::CallNamedFallback {
                base_pos: dst, retc: 1, ..
            }
            | Op::ToStr(dst, _)
            | Op::ToBool(dst, _)
            | Op::Not(dst, _)
            | Op::CmpEq(dst, _, _)
            | Op::CmpNe(dst, _, _)
            | Op::CmpLt(dst, _, _)
            | Op::CmpLe(dst, _, _)
            | Op::CmpGt(dst, _, _)
            | Op::CmpGe(dst, _, _)
            | Op::CmpI { dst, .. }
            | Op::CmpEqImm(dst, _, _)
            | Op::CmpNeImm(dst, _, _)
            | Op::CmpLtImm(dst, _, _)
            | Op::CmpLeImm(dst, _, _)
            | Op::CmpGtImm(dst, _, _)
            | Op::CmpGeImm(dst, _, _)
            | Op::In(dst, _, _)
            | Op::NullishPick { dst, .. }
            | Op::JmpFalseSet { dst, .. }
            | Op::JmpTrueSet { dst, .. }
            | Op::ToIter { dst, .. }
            | Op::ListSlice { dst, .. }
            | Op::PatternMatch { dst, .. } => {
                self.int_regs.remove(&dst);
                self.list_locals.remove(&dst);
                self.map_locals.remove(&dst);
            }
            Op::BuildList { dst, .. } => {
                self.int_regs.remove(&dst);
                self.list_locals.insert(dst);
                self.map_locals.remove(&dst);
            }
            Op::BuildMap { dst, .. } => {
                self.int_regs.remove(&dst);
                self.list_locals.remove(&dst);
                self.map_locals.insert(dst);
            }
            Op::ListFoldAdd { acc, .. } | Op::MapValuesFoldAdd { acc, .. } => {
                self.int_regs.remove(&acc);
            }
            Op::MapSetMove { key, val, .. } => {
                self.int_regs.remove(&key);
                self.int_regs.remove(&val);
                self.list_locals.remove(&key);
                self.list_locals.remove(&val);
                self.map_locals.remove(&key);
                self.map_locals.remove(&val);
            }
            Op::Call { base, retc, .. }
            | Op::CallExact { base, retc, .. }
            | Op::CallClosureExact { base, retc, .. }
            | Op::CallNativeFast { base, retc, .. }
                if retc > 1 =>
            {
                for reg in base..base.saturating_add(retc as u16) {
                    self.int_regs.remove(&reg);
                }
            }
            Op::CallNamed { base_pos, retc, .. } if retc > 1 => {
                for reg in base_pos..base_pos.saturating_add(retc as u16) {
                    self.int_regs.remove(&reg);
                }
            }
            Op::CallNamedFallback { base_pos, retc, .. } if retc > 1 => {
                for reg in base_pos..base_pos.saturating_add(retc as u16) {
                    self.int_regs.remove(&reg);
                }
            }
            _ => {}
        }
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
        named_params: &[NamedParamDecl],
        body: &Stmt,
        register_const: bool,
    ) -> u16 {
        let dst = self.alloc();
        self.emit_function_closure_into(dst, name, params, named_params, body, register_const);
        dst
    }

    pub fn emit_function_closure_into(
        &mut self,
        dst: u16,
        name: Option<&str>,
        params: &[String],
        named_params: &[NamedParamDecl],
        body: &Stmt,
        register_const: bool,
    ) {
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

        let func = Arc::new(compiled);
        self.protos.push(ClosureProto {
            self_name: name.map(|n| n.to_string()),
            params: Arc::new(params.to_vec()),
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
        let body_stmt = Stmt::Expr(Box::new(body.clone()));
        let captures = self.collect_captures(None, params, &[], &body_stmt);
        if !captures.is_empty() {
            return false;
        }

        let func_val = Val::Closure(Arc::new(ClosureValue::new(ClosureInit {
            params: Arc::new(params.to_vec()),
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
