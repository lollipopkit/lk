use std::collections::HashMap;

mod escape;
pub mod pipeline;

use crate::{
    expr::Expr,
    op::{BinOp, UnaryOp},
    val::Val,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueId(u32);

impl ValueId {
    fn new(raw: u32) -> Self {
        Self(raw)
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(u32);

impl BlockId {
    const ENTRY: BlockId = BlockId(0);

    pub const fn entry() -> Self {
        BlockId::ENTRY
    }

    fn new(raw: u32) -> Self {
        BlockId(raw)
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParamId(pub usize);

#[derive(Debug, Clone)]
pub struct SsaFunction {
    pub entry: BlockId,
    pub blocks: Vec<SsaBlock>,
    pub params: Vec<String>,
}

impl SsaFunction {
    pub fn block(&self, id: BlockId) -> Option<&SsaBlock> {
        self.blocks.get(id.index())
    }
}

#[derive(Debug, Clone)]
pub struct SsaBlock {
    pub id: BlockId,
    pub statements: Vec<SsaStatement>,
    pub terminator: Option<SsaTerminator>,
}

impl SsaBlock {
    fn new(id: BlockId) -> Self {
        Self {
            id,
            statements: Vec::new(),
            terminator: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SsaStatement {
    pub result: ValueId,
    pub value: SsaRvalue,
}

#[derive(Debug, Clone)]
pub struct PhiOperand {
    pub block: BlockId,
    pub value: ValueId,
}

#[derive(Debug, Clone)]
pub enum SsaRvalue {
    Const(Val),
    Param(ParamId),
    Binary {
        op: BinOp,
        lhs: ValueId,
        rhs: ValueId,
    },
    Unary {
        op: UnaryOp,
        operand: ValueId,
    },
    List(Vec<ValueId>),
    Map(Vec<(ValueId, ValueId)>),
    StructLiteral {
        name: String,
        fields: Vec<(String, ValueId)>,
    },
    Call {
        target: SsaCallTarget,
        positional: Vec<ValueId>,
        named: Vec<(String, ValueId)>,
    },
    Phi {
        sources: Vec<PhiOperand>,
    },
}

#[derive(Debug, Clone)]
pub enum SsaTerminator {
    Return {
        value: ValueId,
    },
    Branch {
        cond: ValueId,
        then_block: BlockId,
        else_block: BlockId,
    },
    Jump {
        target: BlockId,
    },
    Unreachable,
}

#[derive(Debug, Clone)]
pub enum SsaCallTarget {
    Named(String),
    Value(ValueId),
}

#[derive(Debug)]
pub struct SsaLoweringError {
    msg: String,
}

impl std::fmt::Display for SsaLoweringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SSA lowering failed: {}", self.msg)
    }
}

impl std::error::Error for SsaLoweringError {}

pub fn lower_expr_to_ssa(expr: &Expr) -> Result<SsaFunction, SsaLoweringError> {
    let mut ctx = LoweringContext::new();
    let value = ctx.lower_expr(expr)?;
    ctx.finish_return(value);
    Ok(ctx.finish())
}

struct LoweringContext {
    blocks: Vec<SsaBlock>,
    params: Vec<String>,
    param_indices: HashMap<String, ParamId>,
    next_value: u32,
    current_block: BlockId,
}

#[derive(Debug, Clone, Copy)]
enum ShortCircuitKind {
    And,
    Or,
}

impl LoweringContext {
    fn new() -> Self {
        Self {
            blocks: vec![SsaBlock::new(BlockId::ENTRY)],
            params: Vec::new(),
            param_indices: HashMap::new(),
            next_value: 0,
            current_block: BlockId::ENTRY,
        }
    }

    fn current_block_mut(&mut self) -> &mut SsaBlock {
        let idx = self.current_block.index();
        self.blocks
            .get_mut(idx)
            .expect("lowering context always has an active block")
    }

    fn current_block_id(&self) -> BlockId {
        self.current_block
    }

    fn switch_to_block(&mut self, id: BlockId) {
        self.current_block = id;
    }

    fn create_block(&mut self) -> BlockId {
        let id = BlockId::new(self.blocks.len() as u32);
        self.blocks.push(SsaBlock::new(id));
        id
    }

    fn block_mut(&mut self, id: BlockId) -> &mut SsaBlock {
        self.blocks
            .get_mut(id.index())
            .expect("block should exist in SSA function")
    }

    fn set_terminator(&mut self, block: BlockId, terminator: SsaTerminator) {
        let block_ref = self.block_mut(block);
        if block_ref.terminator.is_some() {
            panic!("attempted to reassign terminator for block {:?}", block);
        }
        block_ref.terminator = Some(terminator);
    }

    fn ensure_jump(&mut self, block: BlockId, target: BlockId) {
        if self.blocks[block.index()].terminator.is_none() {
            self.blocks[block.index()].terminator = Some(SsaTerminator::Jump { target });
        }
    }

    fn alloc_value(&mut self) -> ValueId {
        let id = ValueId::new(self.next_value);
        self.next_value += 1;
        id
    }

    fn lower_expr(&mut self, expr: &Expr) -> Result<ValueId, SsaLoweringError> {
        match expr {
            Expr::Val(v) => Ok(self.emit_const(v.clone())),
            Expr::Var(name) => Ok(self.emit_param(name.clone())),
            Expr::Unary(op, inner) => {
                let operand = self.lower_expr(inner)?;
                Ok(self.emit_unary(op.clone(), operand))
            }
            Expr::Bin(lhs, op, rhs) => {
                let lhs_val = self.lower_expr(lhs)?;
                let rhs_val = self.lower_expr(rhs)?;
                Ok(self.emit_binary(op.clone(), lhs_val, rhs_val))
            }
            Expr::Conditional(cond, then_expr, else_expr) => self.lower_conditional(cond, then_expr, else_expr),
            Expr::And(lhs, rhs) => self.lower_short_circuit(lhs, rhs, ShortCircuitKind::And),
            Expr::Or(lhs, rhs) => self.lower_short_circuit(lhs, rhs, ShortCircuitKind::Or),
            Expr::Call(name, args) => {
                let positional = self.lower_expr_list(args)?;
                Ok(self.emit_call(SsaCallTarget::Named(name.clone()), positional, Vec::new()))
            }
            Expr::CallExpr(callee, args) => {
                let callee_val = self.lower_expr(callee)?;
                let positional = self.lower_expr_list(args)?;
                Ok(self.emit_call(SsaCallTarget::Value(callee_val), positional, Vec::new()))
            }
            Expr::CallNamed(callee, positional_args, named_args) => {
                let callee_val = self.lower_expr(callee)?;
                let positional = self.lower_expr_list(positional_args)?;
                let named = self.lower_named_args(named_args)?;
                Ok(self.emit_call(SsaCallTarget::Value(callee_val), positional, named))
            }
            Expr::List(items) => {
                let values = self.lower_expr_list(items)?;
                Ok(self.emit_list(values))
            }
            Expr::Map(entries) => {
                let entries = self.lower_map_entries(entries)?;
                Ok(self.emit_map(entries))
            }
            Expr::StructLiteral { name, fields } => {
                let fields = self.lower_struct_fields(fields)?;
                Ok(self.emit_struct_literal(name.clone(), fields))
            }
            Expr::Paren(inner) => self.lower_expr(inner),
            other => Err(SsaLoweringError {
                msg: format!("unsupported expression form for SSA lowering: {other:?}"),
            }),
        }
    }

    fn lower_conditional(
        &mut self,
        cond: &Expr,
        then_expr: &Expr,
        else_expr: &Expr,
    ) -> Result<ValueId, SsaLoweringError> {
        let cond_val = self.lower_expr(cond)?;
        let pivot_block = self.current_block_id();
        let then_block = self.create_block();
        let else_block = self.create_block();
        let merge_block = self.create_block();

        self.set_terminator(
            pivot_block,
            SsaTerminator::Branch {
                cond: cond_val,
                then_block,
                else_block,
            },
        );

        self.switch_to_block(then_block);
        let then_val = self.lower_expr(then_expr)?;
        self.ensure_jump(then_block, merge_block);

        self.switch_to_block(else_block);
        let else_val = self.lower_expr(else_expr)?;
        self.ensure_jump(else_block, merge_block);

        self.switch_to_block(merge_block);
        Ok(self.emit_phi(vec![(then_block, then_val), (else_block, else_val)]))
    }

    fn lower_short_circuit(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
        kind: ShortCircuitKind,
    ) -> Result<ValueId, SsaLoweringError> {
        let lhs_val = self.lower_expr(lhs)?;
        let pivot_block = self.current_block_id();
        let rhs_block = self.create_block();
        let merge_block = self.create_block();
        let (then_block, else_block) = match kind {
            ShortCircuitKind::And => (rhs_block, merge_block),
            ShortCircuitKind::Or => (merge_block, rhs_block),
        };

        self.set_terminator(
            pivot_block,
            SsaTerminator::Branch {
                cond: lhs_val,
                then_block,
                else_block,
            },
        );

        self.switch_to_block(rhs_block);
        let rhs_val = self.lower_expr(rhs)?;
        self.ensure_jump(rhs_block, merge_block);

        self.switch_to_block(merge_block);
        let sources = match kind {
            ShortCircuitKind::And => vec![(rhs_block, rhs_val), (pivot_block, lhs_val)],
            ShortCircuitKind::Or => vec![(pivot_block, lhs_val), (rhs_block, rhs_val)],
        };
        Ok(self.emit_phi(sources))
    }

    fn lower_expr_list(&mut self, exprs: &[Box<Expr>]) -> Result<Vec<ValueId>, SsaLoweringError> {
        exprs
            .iter()
            .map(|expr| self.lower_expr(expr))
            .collect::<Result<Vec<_>, _>>()
    }

    fn lower_named_args(
        &mut self,
        named_args: &[(String, Box<Expr>)],
    ) -> Result<Vec<(String, ValueId)>, SsaLoweringError> {
        let mut lowered = Vec::with_capacity(named_args.len());
        for (name, expr) in named_args {
            let value = self.lower_expr(expr)?;
            lowered.push((name.clone(), value));
        }
        Ok(lowered)
    }

    fn lower_map_entries(
        &mut self,
        entries: &[(Box<Expr>, Box<Expr>)],
    ) -> Result<Vec<(ValueId, ValueId)>, SsaLoweringError> {
        let mut lowered = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            let key_id = self.lower_expr(key)?;
            let value_id = self.lower_expr(value)?;
            lowered.push((key_id, value_id));
        }
        Ok(lowered)
    }

    fn lower_struct_fields(
        &mut self,
        fields: &[(String, Box<Expr>)],
    ) -> Result<Vec<(String, ValueId)>, SsaLoweringError> {
        let mut lowered = Vec::with_capacity(fields.len());
        for (name, expr) in fields {
            let value = self.lower_expr(expr)?;
            lowered.push((name.clone(), value));
        }
        Ok(lowered)
    }

    fn emit_const(&mut self, value: Val) -> ValueId {
        let id = self.alloc_value();
        self.current_block_mut().statements.push(SsaStatement {
            result: id,
            value: SsaRvalue::Const(value),
        });
        id
    }

    fn emit_param(&mut self, name: String) -> ValueId {
        let param = match self.param_indices.get(&name) {
            Some(id) => *id,
            None => {
                let id = ParamId(self.params.len());
                self.params.push(name.clone());
                self.param_indices.insert(name.clone(), id);
                id
            }
        };
        let id = self.alloc_value();
        self.current_block_mut().statements.push(SsaStatement {
            result: id,
            value: SsaRvalue::Param(param),
        });
        id
    }

    fn emit_unary(&mut self, op: UnaryOp, operand: ValueId) -> ValueId {
        let id = self.alloc_value();
        self.current_block_mut().statements.push(SsaStatement {
            result: id,
            value: SsaRvalue::Unary { op, operand },
        });
        id
    }

    fn emit_binary(&mut self, op: BinOp, lhs: ValueId, rhs: ValueId) -> ValueId {
        let id = self.alloc_value();
        self.current_block_mut().statements.push(SsaStatement {
            result: id,
            value: SsaRvalue::Binary { op, lhs, rhs },
        });
        id
    }

    fn emit_list(&mut self, elements: Vec<ValueId>) -> ValueId {
        let id = self.alloc_value();
        self.current_block_mut().statements.push(SsaStatement {
            result: id,
            value: SsaRvalue::List(elements),
        });
        id
    }

    fn emit_map(&mut self, entries: Vec<(ValueId, ValueId)>) -> ValueId {
        let id = self.alloc_value();
        self.current_block_mut().statements.push(SsaStatement {
            result: id,
            value: SsaRvalue::Map(entries),
        });
        id
    }

    fn emit_struct_literal(&mut self, name: String, fields: Vec<(String, ValueId)>) -> ValueId {
        let id = self.alloc_value();
        self.current_block_mut().statements.push(SsaStatement {
            result: id,
            value: SsaRvalue::StructLiteral { name, fields },
        });
        id
    }

    fn emit_call(&mut self, target: SsaCallTarget, positional: Vec<ValueId>, named: Vec<(String, ValueId)>) -> ValueId {
        let id = self.alloc_value();
        self.current_block_mut().statements.push(SsaStatement {
            result: id,
            value: SsaRvalue::Call {
                target,
                positional,
                named,
            },
        });
        id
    }

    fn emit_phi(&mut self, sources: Vec<(BlockId, ValueId)>) -> ValueId {
        let operands = sources
            .into_iter()
            .map(|(block, value)| PhiOperand { block, value })
            .collect();
        let id = self.alloc_value();
        self.current_block_mut().statements.push(SsaStatement {
            result: id,
            value: SsaRvalue::Phi { sources: operands },
        });
        id
    }

    fn finish_return(&mut self, value: ValueId) {
        let block = self.current_block_id();
        self.set_terminator(block, SsaTerminator::Return { value });
    }

    fn finish(self) -> SsaFunction {
        SsaFunction {
            entry: BlockId::ENTRY,
            blocks: self.blocks,
            params: self.params,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expr::Expr, op::BinOp, val::Val};

    #[test]
    fn lowers_simple_binary_expression() {
        let expr = Expr::Bin(
            Box::new(Expr::Var("a".into())),
            BinOp::Add,
            Box::new(Expr::Val(Val::Int(1))),
        );

        let func = lower_expr_to_ssa(&expr).expect("lowering should succeed");
        assert_eq!(func.entry, BlockId::ENTRY);
        assert_eq!(func.params, vec!["a".to_string()]);
        assert_eq!(func.blocks.len(), 1);

        let block = &func.blocks[0];
        assert_eq!(block.statements.len(), 3);

        let const_stmt = block
            .statements
            .iter()
            .find(|stmt| matches!(stmt.value, SsaRvalue::Const(_)));
        assert!(const_stmt.is_some(), "expected constant statement");

        let param_stmt = block
            .statements
            .iter()
            .find(|stmt| matches!(stmt.value, SsaRvalue::Param(_)));
        assert!(param_stmt.is_some(), "expected parameter statement");

        let bin_stmt = block
            .statements
            .iter()
            .find(|stmt| matches!(stmt.value, SsaRvalue::Binary { .. }));
        assert!(bin_stmt.is_some(), "expected binary statement");

        match block.terminator.as_ref() {
            Some(SsaTerminator::Return { value }) => {
                assert_eq!(value.index(), (block.statements.len() - 1));
            }
            other => panic!("expected return terminator, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unsupported_expression() {
        let expr = Expr::NullishCoalescing(Box::new(Expr::Var("lhs".into())), Box::new(Expr::Var("rhs".into())));
        let err = lower_expr_to_ssa(&expr).expect_err("nullish coalescing lowering is not yet supported");
        assert!(
            err.to_string().contains("unsupported expression"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn lowers_conditional_expression_into_multiple_blocks() {
        let expr = Expr::Conditional(
            Box::new(Expr::Var("flag".into())),
            Box::new(Expr::Val(Val::Int(1))),
            Box::new(Expr::Val(Val::Int(2))),
        );

        let func = lower_expr_to_ssa(&expr).expect("lowering should succeed");
        assert_eq!(func.params, vec!["flag".to_string()]);
        assert_eq!(func.blocks.len(), 4);

        let entry = &func.blocks[BlockId::entry().index()];
        let (then_block, else_block) = match entry.terminator.as_ref() {
            Some(SsaTerminator::Branch {
                cond,
                then_block,
                else_block,
            }) => {
                assert_eq!(cond.index(), 0);
                (then_block, else_block)
            }
            other => panic!("expected branch terminator, got {other:?}"),
        };

        let then_block_data = &func.blocks[then_block.index()];
        assert!(matches!(
            then_block_data.statements.first().map(|stmt| &stmt.value),
            Some(SsaRvalue::Const(Val::Int(1)))
        ));

        let else_block_data = &func.blocks[else_block.index()];
        assert!(matches!(
            else_block_data.statements.first().map(|stmt| &stmt.value),
            Some(SsaRvalue::Const(Val::Int(2)))
        ));

        let merge_block = func.blocks.last().expect("expected merge block");
        assert_eq!(merge_block.statements.len(), 1);
        match &merge_block.statements[0].value {
            SsaRvalue::Phi { sources } => {
                assert_eq!(sources.len(), 2);
                let mut source_blocks: Vec<_> = sources.iter().map(|operand| operand.block.index()).collect();
                source_blocks.sort_unstable();
                assert_eq!(source_blocks, vec![then_block.index(), else_block.index()]);
            }
            other => panic!("expected phi in merge block, got {other:?}"),
        }
        assert!(matches!(merge_block.terminator, Some(SsaTerminator::Return { .. })));
    }

    #[test]
    fn lowers_named_call_with_list_and_map_arguments() {
        let expr = Expr::Call(
            "combine".into(),
            vec![
                Box::new(Expr::List(vec![
                    Box::new(Expr::Val(Val::Int(1))),
                    Box::new(Expr::Val(Val::Int(2))),
                ])),
                Box::new(Expr::Map(vec![(
                    Box::new(Expr::Val(Val::Int(0))),
                    Box::new(Expr::Val(Val::Int(42))),
                )])),
            ],
        );

        let func = lower_expr_to_ssa(&expr).expect("lowering should succeed");
        let entry = &func.blocks[BlockId::entry().index()];

        let list_stmt = entry
            .statements
            .iter()
            .find(|stmt| matches!(stmt.value, SsaRvalue::List(_)))
            .expect("expected list statement");
        if let SsaRvalue::List(elements) = &list_stmt.value {
            assert_eq!(elements.len(), 2);
        }

        let map_stmt = entry
            .statements
            .iter()
            .find(|stmt| matches!(stmt.value, SsaRvalue::Map(_)))
            .expect("expected map statement");
        if let SsaRvalue::Map(entries) = &map_stmt.value {
            assert_eq!(entries.len(), 1);
        }

        let call_stmt = entry
            .statements
            .iter()
            .find(|stmt| matches!(stmt.value, SsaRvalue::Call { .. }))
            .expect("expected call statement");
        if let SsaRvalue::Call {
            target,
            positional,
            named,
        } = &call_stmt.value
        {
            match target {
                SsaCallTarget::Named(name) => assert_eq!(name, "combine"),
                other => panic!("expected named call target, got {other:?}"),
            }
            assert_eq!(positional.len(), 2);
            assert!(named.is_empty());
        }
    }

    #[test]
    fn lowers_short_circuit_and_expression() {
        let expr = Expr::And(Box::new(Expr::Var("lhs".into())), Box::new(Expr::Var("rhs".into())));

        let func = lower_expr_to_ssa(&expr).expect("lowering should succeed");
        assert_eq!(func.params, vec!["lhs".to_string(), "rhs".to_string()]);
        assert!(func.blocks.len() >= 3, "expected entry, rhs, and merge blocks");

        let entry = &func.blocks[BlockId::entry().index()];
        let (rhs_block, merge_block) = match entry.terminator.as_ref() {
            Some(SsaTerminator::Branch {
                cond,
                then_block,
                else_block,
            }) => {
                assert_eq!(cond.index(), 0);
                (then_block, else_block)
            }
            other => panic!("expected branch terminator, got {other:?}"),
        };

        let rhs_block_data = &func.blocks[rhs_block.index()];
        assert!(
            rhs_block_data
                .statements
                .iter()
                .any(|stmt| matches!(stmt.value, SsaRvalue::Param(ParamId(1)))),
            "rhs block should contain rhs param read"
        );

        let merge_block_data = &func.blocks[merge_block.index()];
        let phi_stmt = merge_block_data
            .statements
            .iter()
            .find(|stmt| matches!(stmt.value, SsaRvalue::Phi { .. }))
            .expect("expected phi statement in merge block");
        if let SsaRvalue::Phi { sources } = &phi_stmt.value {
            assert_eq!(sources.len(), 2);
            let mut source_blocks: Vec<_> = sources.iter().map(|operand| operand.block.index()).collect();
            source_blocks.sort_unstable();
            assert_eq!(source_blocks, vec![BlockId::entry().index(), rhs_block.index()]);
        }

        assert!(matches!(
            merge_block_data.terminator,
            Some(SsaTerminator::Return { .. })
        ));
    }
}
