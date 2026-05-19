use crate::{
    expr::{Expr, Pattern},
    op::BinOp,
    stmt::{ForPattern, Stmt},
    val::Val,
    vm::Op,
};

use super::FunctionBuilder;

fn strip_trailing_increment(stmt: &Stmt, counter_name: &str) -> Stmt {
    match stmt {
        Stmt::Block { statements } => {
            if statements.len() <= 1 {
                Stmt::Block { statements: vec![] }
            } else {
                let mut trimmed = statements.clone();
                trimmed.pop();
                if let Some(last) = trimmed.pop() {
                    trimmed.push(Box::new(strip_trailing_increment(&last, counter_name)));
                }
                Stmt::Block { statements: trimmed }
            }
        }
        Stmt::Assign { name, .. } if name == counter_name => Stmt::Block { statements: vec![] },
        Stmt::CompoundAssign { name, .. } if name == counter_name => Stmt::Block { statements: vec![] },
        other => other.clone(),
    }
}

impl FunctionBuilder {
    pub(super) fn declare_for_pattern(&mut self, pattern: &ForPattern) {
        match pattern {
            ForPattern::Variable(name) => {
                self.define_scoped_var(name);
            }
            ForPattern::Ignore => {}
            ForPattern::Tuple(patterns) => {
                for pat in patterns {
                    self.declare_for_pattern(pat);
                }
            }
            ForPattern::Array { patterns, rest } => {
                for pat in patterns {
                    self.declare_for_pattern(pat);
                }
                if let Some(name) = rest {
                    self.define_scoped_var(name);
                }
            }
            ForPattern::Object(entries) => {
                for (_, pat) in entries {
                    self.declare_for_pattern(pat);
                }
            }
        }
    }

    pub(super) fn pattern_requires_check(pattern: &ForPattern) -> bool {
        !matches!(pattern, ForPattern::Variable(_) | ForPattern::Ignore)
    }

    pub(super) fn pattern_from_for(pattern: &ForPattern) -> Pattern {
        match pattern {
            ForPattern::Variable(name) => Pattern::Variable(name.clone()),
            ForPattern::Ignore => Pattern::Wildcard,
            ForPattern::Tuple(patterns) => Pattern::List {
                patterns: patterns.iter().map(Self::pattern_from_for).collect(),
                rest: None,
            },
            ForPattern::Array { patterns, rest } => Pattern::List {
                patterns: patterns.iter().map(Self::pattern_from_for).collect(),
                rest: rest.clone(),
            },
            ForPattern::Object(entries) => Pattern::Map {
                patterns: entries
                    .iter()
                    .map(|(k, p)| (k.clone(), Self::pattern_from_for(p)))
                    .collect(),
                rest: None,
            },
        }
    }

    /// Try to lower a simple `while (i < N) { body; i = i + 1; }` loop into a for-range loop.
    /// This enables BC32 packing and uses the efficient ForRangeState instead of Val-based
    /// comparison/increment for each iteration. Only applied when the body is simple enough
    /// that for-range overhead (3 words/tags per iteration) beats peephole-fused while (1 dispatch).
    pub(super) fn try_lower_while_to_for_range(&mut self, condition: &Expr, body: &Stmt) -> bool {
        let (counter_name, limit_val) = match condition {
            Expr::Bin(left, BinOp::Lt, right) => {
                let Expr::Var(name) = left.as_ref() else {
                    return false;
                };
                let Expr::Val(Val::Int(n)) = right.as_ref() else {
                    return false;
                };
                (name.as_str(), *n)
            }
            _ => return false,
        };

        let Some(counter_reg) = self.lookup(counter_name) else {
            return false;
        };

        fn body_ends_with_inc(s: &Stmt, counter_name: &str) -> bool {
            match s {
                Stmt::Block { statements } => statements.last().is_some_and(|s| body_ends_with_inc(s, counter_name)),
                Stmt::Assign { name, value, .. } => {
                    name == counter_name
                        && matches!(
                            value.as_ref(),
                            Expr::Bin(left, BinOp::Add, right)
                                if matches!(left.as_ref(), Expr::Var(n) if n == counter_name)
                                    && matches!(right.as_ref(), Expr::Val(Val::Int(1)))
                        )
                }
                Stmt::CompoundAssign { name, op, value, .. } => {
                    name == counter_name && matches!(op, BinOp::Add) && matches!(value.as_ref(), Expr::Val(Val::Int(1)))
                }
                _ => false,
            }
        }

        if !body_ends_with_inc(body, counter_name) {
            return false;
        }

        fn ops_in_body(s: &Stmt, counter_name: &str) -> usize {
            match s {
                Stmt::Block { statements } => statements.iter().map(|s| ops_in_body(s, counter_name)).sum(),
                Stmt::Assign { name, .. } | Stmt::CompoundAssign { name, .. } if name == counter_name => 0,
                Stmt::Expr(_) => 1,
                Stmt::Assign { .. } | Stmt::Let { .. } | Stmt::CompoundAssign { .. } => 1,
                _ => 8,
            }
        }
        if ops_in_body(body, counter_name) > 6 {
            return false;
        }

        let limit_reg = self.alloc();
        let limit_k = self.k(Val::Int(limit_val));
        self.emit(Op::LoadK(limit_reg, limit_k));

        let step_reg = self.alloc();

        self.emit(Op::ForRangePrep {
            idx: counter_reg,
            limit: limit_reg,
            step: step_reg,
            inclusive: false,
            explicit: false,
        });

        let guard_pos = self.code.len();
        self.emit(Op::ForRangeLoop {
            idx: counter_reg,
            limit: limit_reg,
            step: step_reg,
            inclusive: false,
            write_idx: true,
            ofs: 0,
        });

        let saved_breaks = std::mem::take(&mut self.break_locations);
        let saved_conts = std::mem::take(&mut self.continue_locations);
        self.loop_depth = self.loop_depth.saturating_add(1);

        let body_without_inc = strip_trailing_increment(body, counter_name);
        self.with_const_scope(|builder| builder.stmt(&body_without_inc));

        let pending_continues = std::mem::take(&mut self.continue_locations);
        let step_pos = self.code.len();
        self.emit(Op::ForRangeStep {
            idx: counter_reg,
            step: step_reg,
            back_ofs: 0,
        });

        let back = (guard_pos as isize - step_pos as isize) as i16;
        if let Op::ForRangeStep { back_ofs, .. } = &mut self.code[step_pos] {
            *back_ofs = back;
        }
        for loc in pending_continues {
            if let Some(Op::Continue(ofs)) = self.code.get_mut(loc) {
                *ofs = (step_pos as isize - loc as isize) as i16;
            }
        }

        let end_pos = self.code.len();
        if let Op::ForRangeLoop { ofs, .. } = &mut self.code[guard_pos] {
            *ofs = (end_pos as isize - guard_pos as isize) as i16;
        }
        let pending_breaks = std::mem::take(&mut self.break_locations);
        for loc in pending_breaks {
            if let Some(Op::Break(ofs)) = self.code.get_mut(loc) {
                *ofs = (end_pos as isize - loc as isize) as i16;
            }
        }

        self.loop_depth = self.loop_depth.saturating_sub(1);
        self.break_locations = saved_breaks;
        self.continue_locations = saved_conts;

        true
    }
}
