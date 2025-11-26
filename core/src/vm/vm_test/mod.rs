pub(super) use crate::{
    expr::{Expr, Pattern, SelectCase, SelectPattern},
    op::{BinOp, UnaryOp},
    stmt::{ForPattern, NamedParamDecl, Program, Stmt, stmt_parser::StmtParser},
    token::Tokenizer,
    typ::TypeChecker,
    val::{Type, Val},
    vm::{self, Compiler, Vm, VmContext, bytecode::Op, with_current_vm_ctx},
};

pub(super) fn exec_with_new_vm(fun: &vm::bytecode::Function) -> Val {
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();
    vm.exec(fun, &mut ctx).unwrap()
}

mod bytecode;
mod control_flow;
mod functions;
mod inline_cache;
mod native;
mod region_plan;
mod semantics;
