use super::ssa::pipeline;
use crate::resolve::slots::SlotResolver;
use crate::{
    expr::Expr,
    stmt::{NamedParamDecl, Program, Stmt},
    typ::TypeChecker,
    val::Val,
    vm::{CaptureSpec, Function, Op},
};

use super::builder::FunctionBuilder;

/// A simple single-function compiler that lowers a subset of Stmt/Expr to register bytecode.
/// - Locals are assigned sequential indices at first definition and never deallocated.
/// - Control flow supports if/while/return and expression statements.
pub struct Compiler;

impl Compiler {
    pub fn new() -> Self {
        Self
    }

    /// Compile a single expression into a self-contained function.
    pub fn compile_expr(&self, expr: &Expr) -> Function {
        let mut checker = TypeChecker::new();
        let _ = checker.infer_resolved_type(expr);
        let mut b = FunctionBuilder::new();
        let hints = checker.take_expr_types();
        if !hints.is_empty() {
            b.set_expr_type_hints(hints);
        }
        {
            let analysis = pipeline::analyze_expr(expr);
            b.set_analysis(analysis);
        }
        let dst = b.expr(expr);
        b.emit(Op::Ret { base: dst, retc: 1 });
        b.finish()
    }

    /// Compile a statement into a function. Returns from explicit `return`;
    /// if no return occurs, returns Nil.
    pub fn compile_stmt(&self, stmt: &Stmt) -> Function {
        let mut checker = TypeChecker::new();
        let _ = stmt.type_check(&mut checker);
        let mut b = FunctionBuilder::new();
        let hints = checker.take_expr_types();
        if !hints.is_empty() {
            b.set_expr_type_hints(hints);
        }
        b.stmt(stmt);
        let k = b.k(Val::Nil);
        let r0 = b.alloc();
        b.emit(Op::LoadK(r0, k));
        b.emit(Op::Ret { base: r0, retc: 1 });
        b.finish()
    }

    /// Compile a function body with positional and named parameter declarations.
    /// Parameters are treated as locals with preassigned registers so the VM can seed
    /// values directly into the appropriate slots.
    pub fn compile_function(&self, params: &[String], named_params: &[NamedParamDecl], body: &Stmt) -> Function {
        self.compile_function_with_captures(params, named_params, body, &[])
    }

    pub fn compile_function_with_captures(
        &self,
        params: &[String],
        named_params: &[NamedParamDecl],
        body: &Stmt,
        captures: &[CaptureSpec],
    ) -> Function {
        let mut checker = TypeChecker::new();
        let _ = body.type_check(&mut checker);
        let mut b = FunctionBuilder::new_with_captures(captures);
        let hints = checker.take_expr_types();
        if !hints.is_empty() {
            b.set_expr_type_hints(hints);
        }
        let slot_layout = {
            let mut resolver = SlotResolver::new();
            resolver.resolve_function_slots(params, named_params, body)
        };
        b.apply_slot_layout(&slot_layout);
        // Prebind params to local registers and record their register indices
        b.param_regs.reserve(params.len());
        for p in params {
            let idx = b.get_or_define(p);
            b.param_regs.push(idx);
        }
        b.named_param_regs.reserve(named_params.len());
        for decl in named_params {
            let idx = b.get_or_define(&decl.name);
            b.named_param_regs.push(idx);
        }
        b.build_named_param_layout(named_params);
        {
            for (param, &reg) in params.iter().zip(b.param_regs.iter()) {
                if let Some(expected) = slot_layout
                    .decls
                    .iter()
                    .find(|decl| decl.name == *param && decl.is_param)
                    .map(|decl| decl.index)
                {
                    debug_assert_eq!(
                        reg, expected,
                        "slot resolver allocated index {expected} for param {param}, but builder used {reg}"
                    );
                }
            }
            for (decl, &reg) in named_params.iter().zip(b.named_param_regs.iter()) {
                if let Some(expected) = slot_layout
                    .decls
                    .iter()
                    .find(|d| d.name == decl.name && d.is_param)
                    .map(|d| d.index)
                {
                    debug_assert_eq!(
                        reg, expected,
                        "slot resolver index mismatch for named param {}",
                        decl.name
                    );
                }
            }
        }
        // Body: closures with expression bodies should return the expression's value.
        match body {
            Stmt::Expr(e) => {
                let r = b.expr(e);
                b.emit(Op::Ret { base: r, retc: 1 });
            }
            other => {
                b.stmt(other);
                // Default return Nil if no explicit return executed
                let k = b.k(Val::Nil);
                let r0 = b.alloc();
                b.emit(Op::LoadK(r0, k));
                b.emit(Op::Ret { base: r0, retc: 1 });
            }
        }
        b.finish()
    }

    /// Compile a default argument expression into a thunk that expects the outer
    /// function's parameters to be pre-seeded in the same register layout.
    pub fn compile_default_expr(&self, params: &[String], named_params: &[NamedParamDecl], expr: &Expr) -> Function {
        self.compile_default_expr_with_captures(params, named_params, expr, &[])
    }

    pub fn compile_default_expr_with_captures(
        &self,
        params: &[String],
        named_params: &[NamedParamDecl],
        expr: &Expr,
        captures: &[CaptureSpec],
    ) -> Function {
        let mut checker = TypeChecker::new();
        let _ = checker.infer_resolved_type(expr);
        let mut b = FunctionBuilder::new_with_captures(captures);
        let hints = checker.take_expr_types();
        if !hints.is_empty() {
            b.set_expr_type_hints(hints);
        }
        {
            let analysis = pipeline::analyze_expr(expr);
            b.set_analysis(analysis);
        }
        b.param_regs.reserve(params.len() + named_params.len());
        b.named_param_regs.reserve(named_params.len());
        for p in params {
            let idx = b.get_or_define(p);
            b.param_regs.push(idx);
        }
        for decl in named_params {
            let idx = b.get_or_define(&decl.name);
            b.param_regs.push(idx);
            b.named_param_regs.push(idx);
        }
        b.build_named_param_layout(named_params);
        let reg = b.expr(expr);
        b.emit(Op::Ret { base: reg, retc: 1 });
        b.finish()
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}

/// Compile a full program into a VM-ready function containing the top-level statements.
pub fn compile_program(program: &Program) -> Function {
    let block = Stmt::Block {
        statements: program
            .statements
            .iter()
            .map(|stmt| Box::new((**stmt).clone()))
            .collect(),
    };
    Compiler::new().compile_stmt(&block)
}
