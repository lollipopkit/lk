#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    expr::{Expr, Pattern},
    operator::BinOp,
    stmt::ImportStmt,
    token::Span,
    val::Type,
};
use anyhow::Result;

/// Source attribute preserved for later derive/attribute/procedural macro expansion.
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub tokens: Vec<crate::token::Token>,
    pub span: Option<Span>,
}

/// A `for` loop's binding pattern.
#[derive(Debug, Clone, PartialEq)]
pub enum ForPattern {
    /// `for x in iter`
    Variable(String),
    /// `for _ in iter`
    Ignore,
    /// `for (a, b, c) in iter`
    Tuple(Vec<ForPattern>),
    /// `for [a, b] in iter`
    Array {
        patterns: Vec<ForPattern>,
        rest: Option<String>, // for [a, b, ..rest] or [a, b, ..]
    },
    /// `for {"k1": v1, "k2": v2} in iter` — string-literal keys only; a value
    /// position takes a name or a deeper pattern.
    Object(Vec<(String, ForPattern)>),
}

/// A named parameter, as a function declares it.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedParamDecl {
    pub name: String,
    /// `None` means unannotated, which is `Any`.
    pub type_annotation: Option<Type>,
    /// Used only when the call omits this parameter.
    pub default: Option<Expr>,
}

/// The statement AST.
///
/// The syntax:
/// program  ::= statement*
/// statement ::= import_stmt | if_stmt | while_stmt | let_stmt | assign_stmt | break_stmt | continue_stmt | return_stmt | fn_stmt | expr_stmt | block_stmt
/// import_stmt ::= 'use' import_spec ';'
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
    /// #[attr] item
    Attributed {
        attributes: Vec<Attribute>,
        item: Box<Stmt>,
    },
    /// use statement
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
    While {
        condition: Box<Expr>,
        body: Box<Stmt>,
    },
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
    /// `name = value;`
    Assign {
        name: String,
        value: Box<Expr>,
        span: Option<Span>,
    },
    /// `name op= value;` — `x += 5` and the rest.
    CompoundAssign {
        name: String,
        op: BinOp,
        value: Box<Expr>,
        span: Option<Span>,
    },
    /// `name := value;` — Go's short declaration.
    ///
    /// The same binding `let name = value` makes — both lower through
    /// `lower_define` — so it carries a span for the same reasons `Let` does:
    /// to place its type error, and to place the type hint an editor writes
    /// where the annotation would have gone.
    Define {
        name: String,
        value: Box<Expr>,
        span: Option<Span>,
    },
    /// break;
    /// `defer <statement>` — run it when the function *returns*, on every
    /// return path, in reverse order.
    ///
    /// **Not** when a raise unwinds past it. This comment used to say "whichever
    /// way", which the rewrite below cannot deliver and which
    /// [`crate::stmt::defer`] contradicts in the same words two files away — see
    /// there for the two measured attempts at the raise path and why each was
    /// reverted.
    ///
    /// Gone by the time anything but the parser sees it: a pass rewrites each
    /// function's body so the deferred statements appear before every `return`
    /// and at the end, in reverse order. That keeps it out of the type checker,
    /// the resolver, both compilers and both backends — a release that has to
    /// happen on every path is a *shape*, not a runtime mechanism, and the one
    /// thing worse than not having it would be having it in one backend.
    Defer {
        body: Box<Stmt>,
        span: Option<Span>,
    },

    Break,
    /// continue;
    Continue,
    /// return [expression];
    Return {
        value: Option<Box<Expr>>,
    },
    /// struct Name { field: Type, ... }
    Struct {
        name: String,
        fields: Vec<(String, Option<Type>)>,
    },
    /// type Alias = ExistingType;
    TypeAlias {
        name: String,
        target: Type,
    },
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
        /// Methods the trait wrote a *body* for, as the `Stmt::Function` an
        /// `impl` block would have held.
        ///
        /// A type that implements the trait and does not write the method gets
        /// this one, copied in by `stmt::trait_defaults` — so dispatch, the
        /// type checker and the AOT lowering never learn that defaults exist.
        /// Storing the whole function is what makes the copy exact: a signature
        /// alone loses the parameter *names* the body reads.
        default_methods: Vec<Stmt>,
    },
    /// impl Trait for Type { fn method(...) { body } }
    Impl {
        /// `None` for an inherent `impl Type { … }` — methods that belong to
        /// the type itself rather than to a trait it satisfies.
        trait_name: Option<String>,
        target_type: Type,
        /// Methods implemented in this block (as function statements)
        methods: Vec<Stmt>,
    },
    /// expression;
    ///
    /// Carries a span for the same reason `Let` does, and for one more: this is
    /// where a bare call statement lives, so it is where argument type errors
    /// are raised. It was the one statement variant with no position at all, and
    /// `TypeError::span` is filled by the enclosing statement on the way out —
    /// so `f("x");` reported "Argument 1 has the wrong type (expected Int, got
    /// String) at `x`" and nothing else. In a four-thousand-line program that is
    /// not a diagnostic; the same mistake in a `let` said `1:1-6`.
    Expr {
        value: Box<Expr>,
        span: Option<Span>,
    },
    /// { statements }
    Block {
        statements: Vec<Box<Stmt>>,
    },
    /// A placeholder the parser emits where a statement was expected.
    Empty,
}

impl Stmt {
    /// A bare expression statement whose position is not known yet.
    ///
    /// Most construction sites are desugarings and tests, which have no source
    /// text to point at; the parser fills the span in where the statement really
    /// was written. Having the constructor keeps those sites from each having to
    /// spell `span: None`, and keeps the field from drifting back to "there is
    /// no position here" by default in the one place that does have one.
    pub fn expr(value: Box<Expr>) -> Self {
        Self::Expr { value, span: None }
    }
}

/// A program: its statements.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub statements: Vec<Box<Stmt>>,
}

impl Program {
    pub fn new(statements: Vec<Box<Stmt>>) -> Result<Self> {
        Ok(Program { statements })
    }
}
