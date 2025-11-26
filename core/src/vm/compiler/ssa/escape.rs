use std::cmp::max;

use crate::vm::analysis::{EscapeClass, EscapeSummary};

use super::{SsaCallTarget, SsaFunction, SsaRvalue, SsaStatement, SsaTerminator, ValueId};

pub fn analyze(func: &SsaFunction) -> EscapeSummary {
    let capacity = value_capacity(func);
    if capacity == 0 {
        return EscapeSummary {
            return_class: EscapeClass::Trivial,
            escaping_values: Vec::new(),
        };
    }

    let mut classes = vec![EscapeClass::Trivial; capacity];
    let mut base_classes = vec![EscapeClass::Trivial; capacity];
    let return_values = collect_return_values(func);

    let mut changed = true;
    while changed {
        changed = false;

        for &ret in &return_values {
            if mark_value(&mut classes, ret, EscapeClass::Escapes) {
                changed = true;
            }
        }

        for block in &func.blocks {
            for stmt in &block.statements {
                let slot = stmt.result.index();
                let new_class = classify_rvalue(stmt, &classes);
                base_classes[slot] = base_classes[slot].join(new_class);
                if mark_slot(&mut classes, slot, new_class) {
                    changed = true;
                }

                if mark_call_inputs_escape(stmt, &mut classes) {
                    changed = true;
                }

                if classes[slot].is_escaping() && propagate_escape_to_operands(stmt, &mut classes) {
                    changed = true;
                }
            }
        }
    }

    let mut summary = EscapeSummary {
        return_class: EscapeClass::Trivial,
        escaping_values: Vec::new(),
    };

    for &ret in &return_values {
        let class = base_classes.get(ret.index()).copied().unwrap_or(EscapeClass::Trivial);
        summary.return_class = summary.return_class.join(class);
    }

    for (idx, class) in classes.into_iter().enumerate() {
        if class.is_escaping() {
            if matches!(base_classes.get(idx), Some(EscapeClass::Trivial)) {
                continue;
            }
            summary.mark_escaping(idx);
        }
    }

    summary
}

fn value_capacity(func: &SsaFunction) -> usize {
    let mut max_idx: Option<usize> = None;
    for block in &func.blocks {
        for stmt in &block.statements {
            max_idx = Some(max(max_idx.unwrap_or(0), stmt.result.index()));
            match &stmt.value {
                SsaRvalue::Unary { operand, .. } => {
                    max_idx = Some(max(max_idx.unwrap_or(0), operand.index()));
                }
                SsaRvalue::Binary { lhs, rhs, .. } => {
                    max_idx = Some(max(max_idx.unwrap_or(0), lhs.index()));
                    max_idx = Some(max(max_idx.unwrap_or(0), rhs.index()));
                }
                SsaRvalue::List(values) => {
                    for value in values {
                        max_idx = Some(max(max_idx.unwrap_or(0), value.index()));
                    }
                }
                SsaRvalue::Map(entries) => {
                    for (key, value) in entries {
                        max_idx = Some(max(max_idx.unwrap_or(0), key.index()));
                        max_idx = Some(max(max_idx.unwrap_or(0), value.index()));
                    }
                }
                SsaRvalue::StructLiteral { fields, .. } => {
                    for (_, value) in fields {
                        max_idx = Some(max(max_idx.unwrap_or(0), value.index()));
                    }
                }
                SsaRvalue::Call {
                    positional,
                    named,
                    target,
                } => {
                    for value in positional {
                        max_idx = Some(max(max_idx.unwrap_or(0), value.index()));
                    }
                    for (_, value) in named {
                        max_idx = Some(max(max_idx.unwrap_or(0), value.index()));
                    }
                    if let SsaCallTarget::Value(val) = target {
                        max_idx = Some(max(max_idx.unwrap_or(0), val.index()));
                    }
                }
                SsaRvalue::Phi { sources } => {
                    for operand in sources {
                        max_idx = Some(max(max_idx.unwrap_or(0), operand.value.index()));
                    }
                }
                SsaRvalue::Const(_) | SsaRvalue::Param(_) => {}
            }
        }

        if let Some(SsaTerminator::Return { value }) = &block.terminator {
            max_idx = Some(max(max_idx.unwrap_or(0), value.index()));
        }
    }

    max_idx.map(|idx| idx + 1).unwrap_or(0)
}

fn classify_rvalue(stmt: &SsaStatement, classes: &[EscapeClass]) -> EscapeClass {
    match &stmt.value {
        SsaRvalue::Const(_) => EscapeClass::Trivial,
        SsaRvalue::Param(_) => EscapeClass::Local,
        SsaRvalue::Unary { operand, .. } => classes.get(operand.index()).copied().unwrap_or(EscapeClass::Local),
        SsaRvalue::Binary { lhs, rhs, .. } => classes
            .get(lhs.index())
            .copied()
            .unwrap_or(EscapeClass::Local)
            .join(classes.get(rhs.index()).copied().unwrap_or(EscapeClass::Local)),
        SsaRvalue::List(values) => values.iter().fold(EscapeClass::Local, |acc, value| {
            acc.join(classes.get(value.index()).copied().unwrap_or(EscapeClass::Local))
        }),
        SsaRvalue::Map(entries) => entries.iter().fold(EscapeClass::Local, |acc, (key, value)| {
            let key_class = classes.get(key.index()).copied().unwrap_or(EscapeClass::Local);
            let value_class = classes.get(value.index()).copied().unwrap_or(EscapeClass::Local);
            acc.join(key_class).join(value_class)
        }),
        SsaRvalue::StructLiteral { fields, .. } => fields.iter().fold(EscapeClass::Local, |acc, (_, value)| {
            acc.join(classes.get(value.index()).copied().unwrap_or(EscapeClass::Local))
        }),
        SsaRvalue::Call { .. } => EscapeClass::Escapes,
        SsaRvalue::Phi { sources } => sources.iter().fold(EscapeClass::Trivial, |acc, operand| {
            acc.join(
                classes
                    .get(operand.value.index())
                    .copied()
                    .unwrap_or(EscapeClass::Local),
            )
        }),
    }
}

fn mark_call_inputs_escape(stmt: &SsaStatement, classes: &mut [EscapeClass]) -> bool {
    match &stmt.value {
        SsaRvalue::Call {
            target,
            positional,
            named,
        } => {
            let mut changed = false;
            if let SsaCallTarget::Value(value) = target {
                changed |= mark_value(classes, *value, EscapeClass::Escapes);
            }
            for arg in positional {
                changed |= mark_value(classes, *arg, EscapeClass::Escapes);
            }
            for (_, value) in named {
                changed |= mark_value(classes, *value, EscapeClass::Escapes);
            }
            changed
        }
        _ => false,
    }
}

fn propagate_escape_to_operands(stmt: &SsaStatement, classes: &mut [EscapeClass]) -> bool {
    match &stmt.value {
        SsaRvalue::Const(_) | SsaRvalue::Param(_) => false,
        SsaRvalue::Unary { operand, .. } => mark_value(classes, *operand, EscapeClass::Escapes),
        SsaRvalue::Binary { lhs, rhs, .. } => {
            mark_value(classes, *lhs, EscapeClass::Escapes) | mark_value(classes, *rhs, EscapeClass::Escapes)
        }
        SsaRvalue::List(values) => {
            let mut changed = false;
            for value in values {
                if mark_value(classes, *value, EscapeClass::Escapes) {
                    changed = true;
                }
            }
            changed
        }
        SsaRvalue::Map(entries) => {
            let mut changed = false;
            for (key, value) in entries {
                if mark_value(classes, *key, EscapeClass::Escapes) {
                    changed = true;
                }
                if mark_value(classes, *value, EscapeClass::Escapes) {
                    changed = true;
                }
            }
            changed
        }
        SsaRvalue::StructLiteral { fields, .. } => {
            let mut changed = false;
            for (_, value) in fields {
                if mark_value(classes, *value, EscapeClass::Escapes) {
                    changed = true;
                }
            }
            changed
        }
        SsaRvalue::Call {
            target,
            positional,
            named,
        } => {
            let mut changed = false;
            if let SsaCallTarget::Value(value) = target
                && mark_value(classes, *value, EscapeClass::Escapes)
            {
                changed = true;
            }
            for arg in positional {
                if mark_value(classes, *arg, EscapeClass::Escapes) {
                    changed = true;
                }
            }
            for (_, value) in named {
                if mark_value(classes, *value, EscapeClass::Escapes) {
                    changed = true;
                }
            }
            changed
        }
        SsaRvalue::Phi { sources } => {
            let mut changed = false;
            for operand in sources {
                if mark_value(classes, operand.value, EscapeClass::Escapes) {
                    changed = true;
                }
            }
            changed
        }
    }
}

fn collect_return_values(func: &SsaFunction) -> Vec<ValueId> {
    let mut values = Vec::new();
    for block in &func.blocks {
        if let Some(SsaTerminator::Return { value }) = &block.terminator {
            values.push(*value);
        }
    }
    values
}

fn mark_value(classes: &mut [EscapeClass], value: ValueId, target: EscapeClass) -> bool {
    let slot = value.index();
    if let Some(class) = classes.get_mut(slot) {
        let joined = class.join(target);
        if *class != joined {
            *class = joined;
            return true;
        }
    }
    false
}

fn mark_slot(classes: &mut [EscapeClass], slot: usize, target: EscapeClass) -> bool {
    if let Some(class) = classes.get_mut(slot) {
        let joined = class.join(target);
        if *class != joined {
            *class = joined;
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::super::lower_expr_to_ssa;
    use super::*;
    use crate::expr::Expr;
    use crate::val::Val;

    #[test]
    fn constant_expression_stays_trivial() {
        let expr = Expr::Val(Val::Int(42));
        let func = lower_expr_to_ssa(&expr).expect("lowering");
        let summary = analyze(&func);
        assert_eq!(summary.return_class, EscapeClass::Trivial);
        assert!(summary.escaping_values.is_empty());
    }

    #[test]
    fn returning_list_marks_escape() {
        let expr = Expr::List(vec![Box::new(Expr::Val(Val::Int(1)))]);
        let func = lower_expr_to_ssa(&expr).expect("lowering");
        let summary = analyze(&func);
        assert_eq!(summary.return_class, EscapeClass::Escapes);
        assert!(!summary.escaping_values.is_empty());
    }

    #[test]
    fn call_arguments_escape() {
        let expr = Expr::Call("foo".to_string(), vec![Box::new(Expr::Var("x".to_string()))]);
        let func = lower_expr_to_ssa(&expr).expect("lowering");
        let summary = analyze(&func);
        assert_eq!(summary.return_class, EscapeClass::Escapes);
        assert!(!summary.escaping_values.is_empty());
    }

    #[test]
    fn phi_joins_escape_information() {
        let expr = Expr::Conditional(
            Box::new(Expr::Var("flag".to_string())),
            Box::new(Expr::List(vec![Box::new(Expr::Val(Val::Int(1)))])),
            Box::new(Expr::Val(Val::Int(0))),
        );
        let func = lower_expr_to_ssa(&expr).expect("lowering");
        let summary = analyze(&func);
        assert_eq!(summary.return_class, EscapeClass::Escapes);
    }
}
