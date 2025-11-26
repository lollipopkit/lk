use crate::{
    expr::{Expr, Pattern},
    op::BinOp,
    stmt::{
        ImportStmt,
        import::{collect_program_imports, execute_imports},
    },
    token::Span,
    typ::TypeChecker,
    val::{Type, Val},
    vm::{Vm, VmContext, compile_program},
};
use anyhow::Result;

/// For 循环的模式匹配 (类似 Rust 的 Pattern)
#[derive(Debug, Clone, PartialEq)]
pub enum ForPattern {
    /// 简单变量绑定：for x in iter
    Variable(String),
    /// 忽略模式：for _ in iter
    Ignore,
    /// 元组解构：for (a, b, c) in iter
    Tuple(Vec<ForPattern>),
    /// 数组解构：for [a, b] in iter
    Array {
        patterns: Vec<ForPattern>,
        rest: Option<String>, // for [a, b, ..rest] or [a, b, ..]
    },
    /// 对象解构：for {"k1": v1, "k2": v2} in iter
    /// 仅支持字符串字面量作为键，值位置可以是变量或更深的模式（递归支持）
    Object(Vec<(String, ForPattern)>),
}

/// 具名参数声明（用于函数定义）
#[derive(Debug, Clone, PartialEq)]
pub struct NamedParamDecl {
    pub name: String,
    /// 可选类型注解（None 表示未注解，按 Any 处理）
    pub type_annotation: Option<Type>,
    /// 可选默认值表达式（仅在调用省略该具名参数时使用）
    pub default: Option<Expr>,
}

/// Statement AST 节点类型定义
///
/// 语法设计：
/// program  ::= statement*
/// statement ::= import_stmt | if_stmt | while_stmt | let_stmt | assign_stmt | break_stmt | continue_stmt | return_stmt | fn_stmt | expr_stmt | block_stmt
/// import_stmt ::= 'import' import_spec ';'
/// if_stmt  ::= 'if' '(' expr ')' statement ['else' statement]
/// while_stmt ::= 'while' '(' expr ')' statement
/// let_stmt ::= 'let' id [':' type] '=' expr ';'
/// assign_stmt ::= id '=' expr ';'
/// break_stmt ::= 'break' ';'
/// continue_stmt ::= 'continue' ';'
/// return_stmt ::= 'return' [expr] ';'
/// fn_stmt ::= 'fn' id '(' [id {',' id}] ')' block_stmt
/// expr_stmt ::= expr ';'
/// block_stmt ::= '{' statement* '}'
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// import statement
    Import(ImportStmt),
    /// if (condition) then_stmt [else else_stmt]
    If {
        condition: Box<Expr>,
        then_stmt: Box<Stmt>,
        else_stmt: Option<Box<Stmt>>,
    },
    /// if let pattern = expression { then_stmt } [else else_stmt]
    IfLet {
        pattern: Pattern,
        value: Box<Expr>,
        then_stmt: Box<Stmt>,
        else_stmt: Option<Box<Stmt>>,
    },
    /// while (condition) body
    While { condition: Box<Expr>, body: Box<Stmt> },
    /// while let pattern = expression { body }
    WhileLet {
        pattern: Pattern,
        value: Box<Expr>,
        body: Box<Stmt>,
    },
    /// for pattern in iterable { body }
    For {
        pattern: ForPattern,
        iterable: Box<Expr>,
        body: Box<Stmt>,
    },
    /// let pattern [: type] = value; (supports both single variables and destructuring patterns)
    Let {
        pattern: Pattern,
        type_annotation: Option<Type>,
        value: Box<Expr>,
        span: Option<Span>,
        is_const: bool,
    },
    /// name = value; (赋值语句)
    Assign {
        name: String,
        value: Box<Expr>,
        span: Option<Span>,
    },
    /// name op= value; (复合赋值语句, 如 x += 5)
    CompoundAssign {
        name: String,
        op: BinOp,
        value: Box<Expr>,
        span: Option<Span>,
    },
    /// name = value; (变量定义，类似 Go 的短声明)
    Define { name: String, value: Box<Expr> },
    /// break;
    Break,
    /// continue;
    Continue,
    /// return [expression];
    Return { value: Option<Box<Expr>> },
    /// struct Name { field: Type, ... }
    Struct {
        name: String,
        fields: Vec<(String, Option<Type>)>,
    },
    /// type Alias = ExistingType;
    TypeAlias { name: String, target: Type },
    /// fn name(param1[: type], ...) [-> type] { body }
    Function {
        name: String,
        params: Vec<String>,
        /// Parameter types aligned with params; None when unannotated
        param_types: Vec<Option<Type>>,
        /// Named parameters (appear as a block in parameter list: {x: T, y: ?U = default})
        named_params: Vec<NamedParamDecl>,
        /// Optional declared return type
        return_type: Option<Type>,
        body: Box<Stmt>,
    },
    /// trait Name { fn method(params) -> Type; ... }
    Trait {
        name: String,
        /// Method signatures indexed by method name
        methods: Vec<(String, Type)>,
    },
    /// impl Trait for Type { fn method(...) { body } }
    Impl {
        trait_name: String,
        target_type: Type,
        /// Methods implemented in this block (as function statements)
        methods: Vec<Stmt>,
    },
    /// expression;
    Expr(Box<Expr>),
    /// { statements }
    Block { statements: Vec<Box<Stmt>> },
    /// 空语句 (用于处理解析时的占位)
    Empty,
}

/// 程序结构 - 包含语句列表
#[derive(Debug, Clone)]
pub struct Program {
    pub statements: Vec<Box<Stmt>>,
}

impl Program {
    pub fn new(statements: Vec<Box<Stmt>>) -> Result<Self> {
        Ok(Program { statements })
    }

    pub fn execute(&self) -> Result<Val> {
        let mut vm = Vm::new();
        let mut ctx = VmContext::new();
        self.execute_with_vm(&mut vm, &mut ctx)
    }

    pub fn execute_with_ctx(&self, ctx: &mut VmContext) -> Result<Val> {
        let mut vm = Vm::new();
        self.execute_with_vm(&mut vm, ctx)
    }

    pub fn execute_with_vm(&self, vm: &mut Vm, ctx: &mut VmContext) -> Result<Val> {
        let mut type_checker = TypeChecker::new();
        self.type_check(&mut type_checker)?;
        let imports = collect_program_imports(self);
        let resolver = ctx.resolver().clone();
        execute_imports(&imports, resolver.as_ref(), ctx)?;
        let function = compile_program(self);
        vm.exec_with(&function, ctx, None)
    }
}
