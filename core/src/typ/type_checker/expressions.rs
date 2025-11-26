use super::{NamedParamSig, TypeChecker};
use crate::expr::{Expr, SelectCase, SelectPattern, TemplateStringPart};
use crate::op::{BinOp, UnaryOp};
use crate::typ::{NumericClass, NumericHierarchy};
use crate::val::{FunctionNamedParamType, Type, Val};
use anyhow::Result;
use std::collections::HashMap;

impl TypeChecker {
    fn enforce_int_type(&mut self, expr: &Expr, ty: Type, context: &str) -> Result<()> {
        let resolved = self.resolve_aliases(&ty);
        match resolved {
            Type::Int => Ok(()),
            Type::Variable(_) => {
                self.inference_engine.add_constraint(ty, Type::Int);
                Ok(())
            }
            Type::Any => {
                if self.strict_any() {
                    Err(Self::type_err(
                        &format!("{context} must be Int"),
                        Some(Type::Int),
                        Some(Type::Any),
                        Some(expr.clone()),
                    ))
                } else {
                    Ok(())
                }
            }
            other => Err(Self::type_err(
                &format!("{context} must be Int"),
                Some(Type::Int),
                Some(other),
                Some(expr.clone()),
            )),
        }
    }

    /// Type check an expression and record its inferred type
    pub fn check_expr(&mut self, expr: &Expr) -> Result<Type> {
        let ty = self.check_expr_inner(expr)?;
        self.record_expr_type(expr, &ty);
        Ok(ty)
    }

    /// Internal expression checker without recording.
    fn check_expr_inner(&mut self, expr: &Expr) -> Result<Type> {
        match expr {
            // Literals (via Val enum)
            Expr::Val(val) => self.check_literal(val),

            // Variables
            Expr::Var(name) => self.check_identifier(name),

            // Binary operations
            Expr::Bin(_, _, _) => self.check_binary_op_iter(expr),
            Expr::And(left, right) => self.check_logical_op(left, right, Type::Bool),
            Expr::Or(left, right) => self.check_logical_op(left, right, Type::Bool),

            // Unary operations
            Expr::Unary(op, expr) => self.check_unary_op(op, expr),

            // Collections
            Expr::List(items) => self.check_list(&items.iter().map(|i| i.as_ref().clone()).collect::<Vec<_>>()),
            Expr::Map(pairs) => self.check_map(
                &pairs
                    .iter()
                    .map(|(k, v)| (k.as_ref().clone(), v.as_ref().clone()))
                    .collect::<Vec<_>>(),
            ),
            Expr::StructLiteral { name, fields } => {
                // If struct is known, enforce field presence and types; otherwise, accept as named type
                if let Some(sd) = self.registry.get_struct(name) {
                    let schema = sd.fields.clone();
                    // Provided -> check existence and type
                    for (fname, fexpr) in fields {
                        let expected = schema.get(fname).cloned();
                        let at = self.check_expr(fexpr)?;
                        if let Some(expected) = expected {
                            if !self.is_assignable(&at, &expected) {
                                return Err(Self::type_err(
                                    &format!("Field '{}' type mismatch in struct '{}'", fname, name),
                                    Some(expected.clone()),
                                    Some(at),
                                    Some(fexpr.as_ref().clone()),
                                ));
                            }
                        } else {
                            return Err(Self::type_err(
                                &format!("Unknown field '{}' for struct '{}'", fname, name),
                                None,
                                None,
                                None,
                            ));
                        }
                    }
                    // Missing required fields (non-optional)
                    use Type as T;
                    for (req_name, req_ty) in &schema {
                        let provided = fields.iter().any(|(n, _)| n == req_name);
                        if !provided {
                            let optional = matches!(req_ty, T::Optional(_))
                                || matches!(req_ty, T::Union(v) if v.contains(&T::Nil));
                            if !optional {
                                return Err(Self::type_err(
                                    &format!("Missing required field '{}' for struct '{}'", req_name, name),
                                    None,
                                    None,
                                    None,
                                ));
                            }
                        }
                    }
                }
                Ok(Type::Named(name.clone()))
            }

            // Access operations
            Expr::Access(expr, field) => self.check_access(expr, field),
            Expr::NullishCoalescing(expr, default) => self.check_nullish_coalescing(expr, default),
            Expr::OptionalAccess(expr, field) => self.check_optional_chaining(expr, field),
            Expr::Conditional(cond, then_expr, else_expr) => {
                // condition must be Bool
                let cond_ty = self.check_expr(cond)?;
                if cond_ty != Type::Bool {
                    return Err(Self::type_err(
                        "Ternary condition must be Bool",
                        Some(Type::Bool),
                        Some(cond_ty),
                        Some(*cond.clone()),
                    ));
                }
                let then_ty = self.check_expr(then_expr)?;
                let else_ty = self.check_expr(else_expr)?;
                // unify then/else types; return the unified type (prefer then_ty)
                self.inference_engine.add_constraint(then_ty.clone(), else_ty.clone());
                Ok(then_ty)
            }
            // Functions - handle both Call (string name) and CallExpr (expression)
            Expr::Call(func, args) => {
                // For Call with string name, create a variable expression for the function
                let func_expr = Expr::Var(func.clone());
                self.check_function_call(&func_expr, &args.iter().map(|a| a.as_ref().clone()).collect::<Vec<_>>())
            }
            Expr::CallExpr(func_expr, args) => {
                self.check_function_call(func_expr, &args.iter().map(|a| a.as_ref().clone()).collect::<Vec<_>>())
            }
            Expr::CallNamed(callee, pos_args, named_args) => {
                // Struct constructor sugar: TypeName(field: expr, ...)
                if let Expr::Var(name) = callee.as_ref()
                    && let Some(sd) = self.registry.get_struct(name)
                {
                    let schema = sd.fields.clone();
                    if !pos_args.is_empty() {
                        return Err(Self::type_err(
                            &format!("Struct constructor '{}' does not accept positional arguments", name),
                            None,
                            None,
                            None,
                        ));
                    }
                    use std::collections::HashSet;
                    let mut seen_names: HashSet<&str> = HashSet::with_capacity(named_args.len());
                    // Check named arguments
                    for (n, e) in named_args {
                        let key = n.as_str();
                        if !seen_names.insert(key) {
                            return Err(Self::type_err(
                                &format!("Duplicate field '{}' for struct '{}'", n, name),
                                None,
                                None,
                                Some(e.as_ref().clone()),
                            ));
                        }
                        // Unknown field
                        if !schema.contains_key(n) {
                            return Err(Self::type_err(
                                &format!("Unknown field '{}' for struct '{}'", n, name),
                                None,
                                None,
                                Some(e.as_ref().clone()),
                            ));
                        }
                        let at = self.check_expr(e)?;
                        if let Some(expected) = schema.get(n)
                            && !self.is_assignable(&at, expected)
                        {
                            return Err(Self::type_err(
                                &format!("Field '{}' type mismatch in struct '{}'", n, name),
                                Some(expected.clone()),
                                Some(at),
                                Some(e.as_ref().clone()),
                            ));
                        }
                    }
                    // Missing required fields
                    use Type as T;
                    for (req_name, req_ty) in &schema {
                        let provided = seen_names.contains(req_name.as_str());
                        if !provided {
                            let optional = matches!(req_ty, T::Optional(_))
                                || matches!(req_ty, T::Union(v) if v.contains(&T::Nil));
                            if !optional {
                                return Err(Self::type_err(
                                    &format!("Missing required field '{}' for struct '{}'", req_name, name),
                                    None,
                                    None,
                                    None,
                                ));
                            }
                        }
                    }

                    return Ok(Type::Named(name.clone()));
                }
                // Type-check callee first
                let callee_type = self.check_expr(callee)?;

                // Type-check argument expressions and keep their types
                let mut pos_types: Vec<Type> = Vec::with_capacity(pos_args.len());
                for a in pos_args {
                    pos_types.push(self.check_expr(a)?);
                }
                let mut named_types: Vec<(String, Type)> = Vec::with_capacity(named_args.len());
                for (n, e) in named_args {
                    named_types.push((n.clone(), self.check_expr(e)?));
                }

                // If callee is a variable and we have a signature, enforce named rules
                if let Expr::Var(name) = callee.as_ref()
                    && let Some(sig) = self.get_function_sig(name).cloned()
                {
                    // Check positional arity
                    if sig.positional.len() != pos_types.len() {
                        return Err(Self::type_err(
                            &format!(
                                "Function '{}' expects {} positional args, got {}",
                                name,
                                sig.positional.len(),
                                pos_types.len()
                            ),
                            None,
                            None,
                            None,
                        ));
                    }
                    // Constrain positional types
                    for (pt, at) in sig.positional.iter().zip(pos_types.iter()) {
                        self.inference_engine.add_constraint(pt.clone(), at.clone());
                    }

                    // Duplicate/unknown
                    use std::collections::{HashMap as Map, HashSet};
                    let mut sig_lookup: Map<&str, &NamedParamSig> = Map::with_capacity(sig.named.len());
                    for decl in &sig.named {
                        sig_lookup.insert(decl.name.as_str(), decl);
                    }
                    let mut seen: HashSet<&str> = HashSet::with_capacity(named_types.len());
                    for (n, _) in &named_types {
                        let key = n.as_str();
                        if !seen.insert(key) {
                            return Err(Self::type_err(
                                &format!("Duplicate named argument: {}", n),
                                None,
                                None,
                                None,
                            ));
                        }
                        if !sig_lookup.contains_key(key) {
                            return Err(Self::type_err(
                                &format!("Unknown named argument: {}", n),
                                None,
                                None,
                                None,
                            ));
                        }
                    }
                    // Required named presence
                    for decl in &sig.named {
                        let is_optional = matches!(decl.ty, Type::Optional(_));
                        if !is_optional && !decl.has_default && !seen.contains(decl.name.as_str()) {
                            return Err(Self::type_err(
                                &format!("Missing required named argument: {}", decl.name),
                                None,
                                None,
                                None,
                            ));
                        }
                    }
                    // Type constraints for provided named
                    let mut name_to_ty: Map<&str, Type> = Map::with_capacity(sig.named.len());
                    for d in &sig.named {
                        name_to_ty.insert(d.name.as_str(), d.ty.clone());
                    }
                    for (n, at) in &named_types {
                        if let Some(decl_ty) = name_to_ty.get(n.as_str()) {
                            self.inference_engine.add_constraint(decl_ty.clone(), at.clone());
                        }
                    }
                }

                // Fall back to callee function type for return
                match callee_type {
                    Type::Function {
                        params,
                        named_params,
                        return_type,
                    } => {
                        // Basic positional arity check when no signature is available
                        if params.len() != pos_types.len() {
                            return Err(Self::type_err(
                                &format!("Function expects {} positional arguments", params.len()),
                                None,
                                None,
                                None,
                            ));
                        }
                        for (pt, at) in params.iter().zip(pos_types.iter()) {
                            self.inference_engine.add_constraint(pt.clone(), at.clone());
                        }
                        if !named_params.is_empty() || !named_types.is_empty() {
                            use std::collections::{HashMap as Map, HashSet};
                            let decl_map: Map<&str, &FunctionNamedParamType> =
                                named_params.iter().map(|np| (np.name.as_str(), np)).collect();
                            let mut provided: HashSet<&str> = HashSet::with_capacity(named_types.len());
                            for (n, ty) in &named_types {
                                let key = n.as_str();
                                if !decl_map.contains_key(key) {
                                    return Err(Self::type_err(
                                        &format!("Unknown named argument: {}", n),
                                        None,
                                        None,
                                        None,
                                    ));
                                }
                                provided.insert(key);
                                let decl_ty = &decl_map[key].ty;
                                self.inference_engine.add_constraint(decl_ty.clone(), ty.clone());
                            }
                            for decl in named_params {
                                let is_optional = matches!(decl.ty, Type::Optional(_)) || decl.has_default;
                                if !is_optional && !provided.contains(decl.name.as_str()) {
                                    return Err(Self::type_err(
                                        &format!("Missing required named argument: {}", decl.name),
                                        None,
                                        None,
                                        None,
                                    ));
                                }
                            }
                        }
                        Ok(*return_type)
                    }
                    Type::Any | Type::Variable(_) => Ok(Type::Any),
                    Type::Map(_, _) => Ok(Type::Any),
                    Type::Union(_) => Ok(Type::Any),
                    other => Err(Self::type_err("Cannot call non-function type", None, Some(other), None)),
                }
            }

            // Complex expressions
            Expr::Select {
                cases,
                default_case: default,
            } => self.check_select_expr(cases, default),
            Expr::TemplateString(parts) => self.check_template_string(parts),

            // Range expressions behave like synthetic Int lists
            Expr::Range {
                start,
                end,
                inclusive: _,
                step,
            } => {
                if let Some(start_expr) = start {
                    let start_ty = self.check_expr(start_expr)?;
                    self.enforce_int_type(start_expr.as_ref(), start_ty, "Range start")?;
                }
                if let Some(end_expr) = end {
                    let end_ty = self.check_expr(end_expr)?;
                    self.enforce_int_type(end_expr.as_ref(), end_ty, "Range end")?;
                }
                if let Some(step_expr) = step {
                    let step_ty = self.check_expr(step_expr)?;
                    self.enforce_int_type(step_expr.as_ref(), step_ty, "Range step")?;
                }
                Ok(Type::List(Box::new(Type::Int)))
            }
            Expr::Closure { params, body } => {
                // Infer closure as a function type with param type variables and an inferred return
                let mut param_types = Vec::with_capacity(params.len());
                for _ in params {
                    param_types.push(self.inference_engine.fresh_type_var());
                }
                // Body type is inferred by checking the body expression
                let ret_type = self.check_expr(body)?;
                Ok(Type::Function {
                    params: param_types,
                    named_params: Vec::new(),
                    return_type: Box::new(ret_type),
                })
            }
            Expr::Match { value, arms } => {
                // Check the matched value type
                let value_type = self.check_expr(value)?;

                if arms.is_empty() {
                    return Err(Self::type_err(
                        "Match expression must have at least one arm",
                        None,
                        None,
                        Some(expr.clone()),
                    ));
                }

                // Check all arms have compatible types
                let mut result_type: Option<Type> = None;
                for arm in arms {
                    // Ensure pattern is compatible with the matched value type
                    self.check_pattern_against_type(&arm.pattern, &value_type)?;

                    // Arm body is checked in a scope with pattern bindings available
                    let locals_snapshot = self.local_types.clone();
                    self.add_bindings_for_pattern(&arm.pattern, &value_type)?;
                    let arm_type = self.check_expr(&arm.body)?;
                    self.local_types = locals_snapshot;

                    if let Some(existing_type) = &result_type {
                        // Add constraint that all arms should return the same type
                        self.inference_engine
                            .add_constraint(existing_type.clone(), arm_type.clone());
                    } else {
                        result_type = Some(arm_type);
                    }
                }

                result_type
                    .ok_or_else(|| Self::type_err("Match expression has no arms", None, None, Some(expr.clone())))
            }
            Expr::Paren(expr) => self.check_expr(expr),
        }
    }

    /// Type check an expression and return a type with current constraints solved
    pub fn infer_resolved_type(&mut self, expr: &Expr) -> Result<Type> {
        let ty = self.check_expr(expr)?;
        // Attempt to solve constraints and substitute into the resulting type
        match self.inference_engine.solve_constraints() {
            Ok(subs) => Ok(ty.substitute(&subs)),
            Err(_) => Ok(ty), // On failure, return the unsolved type to avoid hard errors in tooling
        }
    }

    /// Generate a fresh type variable from the inference engine
    pub fn fresh_type_var(&mut self) -> Type {
        self.inference_engine.fresh_type_var()
    }

    /// Apply the given substitution map to a type
    pub fn apply_substitutions(&self, ty: Type, subs: &HashMap<String, Type>) -> Type {
        ty.substitute(subs)
    }

    /// Check identifier type
    fn check_identifier(&mut self, name: &str) -> Result<Type> {
        // Check local variables first
        if let Some(typ) = self.local_types.get(name) {
            return Ok(typ.clone());
        }

        // Check type registry for named types
        if let Some(typ) = self.registry.resolve_type(name) {
            return Ok(typ);
        }

        // Otherwise, assume it's a dynamic variable (type inference needed)
        let var_type = self.inference_engine.fresh_type_var();
        self.local_types.insert(name.to_string(), var_type.clone());
        Ok(var_type)
    }

    // Legacy '@' context access removed

    /// Check binary operation types
    fn check_binary_op_with_types(
        &mut self,
        left_expr: &Expr,
        left_type: Type,
        op: &BinOp,
        right_expr: &Expr,
        right_type: Type,
    ) -> Result<Type> {
        match op {
            BinOp::Add => {
                let left_resolved = self.resolve_aliases(&left_type);
                let right_resolved = self.resolve_aliases(&right_type);
                if matches!(left_resolved, Type::List(_)) || matches!(right_resolved, Type::List(_)) {
                    return self.check_list_addition(left_expr, &left_type, right_expr, &right_type);
                }
                if self.is_string_like(&left_type) || self.is_string_like(&right_type) {
                    self.check_string_addition(left_expr, &left_type, right_expr, &right_type)
                } else {
                    self.check_numeric_bin_op(left_expr, &left_type, right_expr, &right_type, op)
                }
            }
            BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                self.check_numeric_bin_op(left_expr, &left_type, right_expr, &right_type, op)
            }
            BinOp::Eq | BinOp::Ne => Ok(Type::Bool),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                self.ensure_numeric_operand(&left_type, left_expr, "左侧")?;
                self.ensure_numeric_operand(&right_type, right_expr, "右侧")?;
                Ok(Type::Bool)
            }
            BinOp::In => match self.resolve_aliases(&right_type) {
                Type::List(_) | Type::Map(_, _) => Ok(Type::Bool),
                other => Err(Self::type_err(
                    "'in' operator requires container type",
                    Some(Type::List(Box::new(Type::Any))),
                    Some(other),
                    Some(Expr::Bin(
                        Box::new(left_expr.clone()),
                        op.clone(),
                        Box::new(right_expr.clone()),
                    )),
                )),
            },
        }
    }

    fn check_binary_op_iter(&mut self, root: &Expr) -> Result<Type> {
        let mut chain: Vec<(&Expr, &Expr, &BinOp, &Expr)> = Vec::new();
        let mut current = root;
        while let Expr::Bin(left, op, right) = current {
            chain.push((current, left.as_ref(), op, right.as_ref()));
            current = left;
        }

        let mut acc_type = self.check_expr(current)?;
        for (node_expr, left_expr, op, right_expr) in chain.into_iter().rev() {
            let right_type = self.check_expr(right_expr)?;
            acc_type = self.check_binary_op_with_types(left_expr, acc_type, op, right_expr, right_type)?;
            self.record_expr_type(node_expr, &acc_type);
        }
        Ok(acc_type)
    }

    fn is_string_like(&self, ty: &Type) -> bool {
        matches!(self.resolve_aliases(ty), Type::String)
    }

    fn coerce_to_string(&mut self, ty: &Type) {
        if matches!(ty, Type::Variable(_)) {
            self.inference_engine.add_constraint(ty.clone(), Type::String);
        }
    }

    fn check_string_addition(
        &mut self,
        _left_expr: &Expr,
        left_ty: &Type,
        _right_expr: &Expr,
        right_ty: &Type,
    ) -> Result<Type> {
        self.coerce_to_string(left_ty);
        self.coerce_to_string(right_ty);
        Ok(Type::String)
    }

    fn check_list_addition(
        &mut self,
        left_expr: &Expr,
        left_ty: &Type,
        right_expr: &Expr,
        right_ty: &Type,
    ) -> Result<Type> {
        let left_resolved = self.resolve_aliases(left_ty);
        let right_resolved = self.resolve_aliases(right_ty);
        match (left_resolved, right_resolved) {
            (Type::List(left_inner), Type::List(right_inner)) => {
                let elem_ty = if self.is_assignable(left_inner.as_ref(), right_inner.as_ref()) {
                    (*left_inner).clone()
                } else if self.is_assignable(right_inner.as_ref(), left_inner.as_ref()) {
                    (*right_inner).clone()
                } else {
                    Type::Any
                };
                Ok(Type::List(Box::new(elem_ty)))
            }
            (Type::List(_), other) => Err(Self::type_err(
                "List concatenation requires both operands to be lists",
                Some(Type::List(Box::new(Type::Any))),
                Some(other),
                Some(Expr::Bin(
                    Box::new(left_expr.clone()),
                    BinOp::Add,
                    Box::new(right_expr.clone()),
                )),
            )),
            (other, Type::List(_)) => Err(Self::type_err(
                "List concatenation requires both operands to be lists",
                Some(Type::List(Box::new(Type::Any))),
                Some(other),
                Some(Expr::Bin(
                    Box::new(left_expr.clone()),
                    BinOp::Add,
                    Box::new(right_expr.clone()),
                )),
            )),
            _ => Err(Self::type_err(
                "List concatenation requires both operands to be lists",
                Some(Type::List(Box::new(Type::Any))),
                None,
                Some(Expr::Bin(
                    Box::new(left_expr.clone()),
                    BinOp::Add,
                    Box::new(right_expr.clone()),
                )),
            )),
        }
    }

    fn check_numeric_bin_op(
        &mut self,
        left_expr: &Expr,
        left_ty: &Type,
        right_expr: &Expr,
        right_ty: &Type,
        op: &BinOp,
    ) -> Result<Type> {
        let resolved_left = self.resolve_aliases(left_ty);
        let resolved_right = self.resolve_aliases(right_ty);

        let left_class = self.classify_numeric_operand(left_ty, &resolved_left, left_expr, "左侧")?;
        let right_class = self.classify_numeric_operand(right_ty, &resolved_right, right_expr, "右侧")?;

        let mut result_class = NumericHierarchy::result(left_class, right_class);
        if matches!(op, BinOp::Div) && result_class == NumericClass::Int {
            result_class = NumericClass::Float;
        }

        Ok(NumericHierarchy::to_type(result_class))
    }

    fn classify_numeric_operand(
        &mut self,
        original: &Type,
        resolved: &Type,
        expr: &Expr,
        label: &'static str,
    ) -> Result<NumericClass> {
        if let Some(class) = NumericHierarchy::classify(resolved) {
            return Ok(class);
        }
        if original.contains_variables() {
            self.inference_engine.add_constraint(original.clone(), Type::Int);
            return Ok(NumericClass::Int);
        }
        Err(Self::type_err(
            &format!("{label} must by numeric types"),
            Some(NumericHierarchy::expected_type()),
            Some(resolved.clone()),
            Some(expr.clone()),
        ))
    }

    fn ensure_numeric_operand(&mut self, ty: &Type, expr: &Expr, label: &'static str) -> Result<NumericClass> {
        let resolved = self.resolve_aliases(ty);
        self.classify_numeric_operand(ty, &resolved, expr, label)
    }

    /// Check logical operation types (&&, ||)
    fn check_logical_op(&mut self, left: &Expr, right: &Expr, result_type: Type) -> Result<Type> {
        let left_type = self.check_expr(left)?;
        let right_type = self.check_expr(right)?;

        // Both operands must be boolean
        if left_type != Type::Bool {
            return Err(Self::type_err(
                "Expected boolean type for logical operation",
                Some(Type::Bool),
                Some(left_type),
                None,
            ));
        }
        if right_type != Type::Bool {
            return Err(Self::type_err(
                "Expected boolean type for logical operation",
                Some(Type::Bool),
                Some(right_type),
                None,
            ));
        }

        Ok(result_type)
    }

    /// Check unary operation types
    fn check_unary_op(&mut self, op: &UnaryOp, expr: &Expr) -> Result<Type> {
        let expr_type = self.check_expr(expr)?;

        match op {
            UnaryOp::Not => {
                if expr_type != Type::Bool {
                    return Err(Self::type_err(
                        "Expected boolean type for '!' operator",
                        Some(Type::Bool),
                        Some(expr_type),
                        None,
                    ));
                }
                Ok(Type::Bool)
            }
        }
    }

    /// Check list literal type
    fn check_list(&mut self, items: &[Expr]) -> Result<Type> {
        if items.is_empty() {
            // Empty list, infer element type later
            let elem_type = self.inference_engine.fresh_type_var();
            return Ok(Type::List(Box::new(elem_type)));
        }

        // Collect element types and build a normalized union when heterogeneous
        let mut elems: Vec<Type> = Vec::with_capacity(items.len());
        for item in items {
            let t = self.check_expr(item)?;
            match t {
                Type::Union(ts) => elems.extend(ts.into_iter()),
                other => elems.push(other),
            }
        }

        // Deduplicate and produce a stable order by display string
        use std::collections::BTreeMap;
        let mut by_key: BTreeMap<String, Type> = BTreeMap::new();
        for ty in elems {
            by_key.entry(ty.display()).or_insert(ty);
        }
        let mut uniq: Vec<Type> = by_key.into_values().collect();
        let elem_type = if uniq.len() == 1 {
            uniq.remove(0)
        } else {
            Type::Union(uniq)
        };

        Ok(Type::List(Box::new(elem_type)))
    }

    /// Check map literal type
    fn check_map(&mut self, pairs: &[(Expr, Expr)]) -> Result<Type> {
        if pairs.is_empty() {
            // Empty map, infer key/value types later
            let key_type = self.inference_engine.fresh_type_var();
            let value_type = self.inference_engine.fresh_type_var();
            return Ok(Type::Map(Box::new(key_type), Box::new(value_type)));
        }

        // Collect key/value types and build normalized unions when heterogeneous
        let mut key_tys: Vec<Type> = Vec::with_capacity(pairs.len());
        let mut val_tys: Vec<Type> = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            let kt = self.check_expr(k)?;
            let vt = self.check_expr(v)?;
            match kt {
                Type::Union(ts) => key_tys.extend(ts.into_iter()),
                other => key_tys.push(other),
            }
            match vt {
                Type::Union(ts) => val_tys.extend(ts.into_iter()),
                other => val_tys.push(other),
            }
        }

        use std::collections::BTreeMap;
        let mut key_by_str: BTreeMap<String, Type> = BTreeMap::new();
        for t in key_tys {
            key_by_str.entry(t.display()).or_insert(t);
        }
        let mut val_by_str: BTreeMap<String, Type> = BTreeMap::new();
        for t in val_tys {
            val_by_str.entry(t.display()).or_insert(t);
        }

        let mut keys: Vec<Type> = key_by_str.into_values().collect();
        let mut vals: Vec<Type> = val_by_str.into_values().collect();

        let key_type = if keys.len() == 1 {
            keys.remove(0)
        } else {
            Type::Union(keys)
        };
        let value_type = if vals.len() == 1 {
            vals.remove(0)
        } else {
            Type::Union(vals)
        };

        Ok(Type::Map(Box::new(key_type), Box::new(value_type)))
    }

    fn struct_field_type(&self, struct_name: &str, field: &Expr) -> Result<Type> {
        let Some(def) = self.registry.get_struct(struct_name) else {
            return Err(Self::type_err(
                &format!("Unknown struct '{}'", struct_name),
                None,
                None,
                Some(field.clone()),
            ));
        };

        let field_name = match field {
            Expr::Val(Val::Str(name)) => name.as_ref().to_string(),
            Expr::Val(Val::Int(idx)) => idx.to_string(),
            _ => {
                return Err(Self::type_err(
                    "Struct field access requires a literal field name",
                    None,
                    None,
                    Some(field.clone()),
                ));
            }
        };

        if let Some(field_ty) = def.fields.get(&field_name) {
            return Ok(field_ty.clone());
        }

        if let Some(method_ty) = self.get_method_sig(&Type::Named(struct_name.to_string()), &field_name) {
            return Ok(method_ty);
        }

        Err(Self::type_err(
            &format!("Struct '{}' has no field '{}'", struct_name, field_name),
            None,
            None,
            Some(field.clone()),
        ))
    }

    /// Check access type (expr.field or expr[index])
    fn check_access(&mut self, expr: &Expr, field: &Expr) -> Result<Type> {
        let expr_type = self.check_expr(expr)?;
        let field_type = self.check_expr(field)?;
        let resolved_expr_type = self.resolve_aliases(&expr_type);

        match &resolved_expr_type {
            Type::List(elem_type) => {
                // Field must be integer index
                if field_type != Type::Int {
                    return Err(Self::type_err(
                        "List index must be integer",
                        Some(Type::Int),
                        Some(field_type),
                        None,
                    ));
                }
                Ok((**elem_type).clone())
            }
            Type::Tuple(elems) => {
                // Field must be integer index; if it's a literal index, pick that element
                if field_type != Type::Int {
                    return Err(Self::type_err(
                        "Tuple index must be integer",
                        Some(Type::Int),
                        Some(field_type),
                        None,
                    ));
                }
                // Try literal extraction
                if let Expr::Val(Val::Int(i)) = field {
                    let idx = *i as usize;
                    if idx < elems.len() {
                        return Ok(elems[idx].clone());
                    }
                }
                // Fallback: unknown index -> union of all element types
                let u = Type::Union(elems.to_vec());
                Ok(u)
            }
            Type::Map(key_type, value_type) => {
                // Field must match key type
                self.inference_engine.add_constraint((**key_type).clone(), field_type);
                Ok((**value_type).clone())
            }
            Type::Named(name) => self.struct_field_type(name, field),
            Type::Any | Type::Variable(_) => Ok(Type::Any),
            Type::Union(variants) => {
                let mut collected: Vec<Type> = Vec::new();
                for variant in variants {
                    match variant {
                        Type::Named(name) => collected.push(self.struct_field_type(name, field)?),
                        _ => return Ok(Type::Any),
                    }
                }
                match collected.len() {
                    0 => Ok(Type::Any),
                    1 => Ok(collected.remove(0)),
                    _ => Ok(Type::Union(collected)),
                }
            }
            _ => Err(Self::type_err(
                "Cannot access field on type",
                None,
                Some(expr_type),
                None,
            )),
        }
    }

    /// Check nullish coalescing type (expr ?? default)
    fn check_nullish_coalescing(&mut self, expr: &Expr, default: &Expr) -> Result<Type> {
        let expr_type = self.check_expr(expr)?;
        let default_type = self.check_expr(default)?;

        // Expression can be optional, default should be the base type
        match expr_type {
            Type::Optional(inner) => {
                self.inference_engine.add_constraint((*inner).clone(), default_type);
                Ok((*inner).clone())
            }
            Type::Nil => Ok(default_type),
            _ => {
                self.inference_engine.add_constraint(expr_type.clone(), default_type);
                Ok(expr_type)
            }
        }
    }

    /// Check optional chaining type (expr?.field)
    fn check_optional_chaining(&mut self, expr: &Expr, field: &Expr) -> Result<Type> {
        let expr_type = self.check_expr(expr)?;

        match expr_type.clone() {
            Type::Optional(inner) => {
                let resolved_inner = self.resolve_aliases(inner.as_ref());
                match resolved_inner {
                    Type::List(elem_type) => {
                        let field_ty = self.check_expr(field)?;
                        if field_ty != Type::Int {
                            return Err(Self::type_err(
                                "List index must be integer",
                                Some(Type::Int),
                                Some(field_ty),
                                None,
                            ));
                        }
                        Ok(Type::Optional(elem_type))
                    }
                    Type::Map(key_type, value_type) => {
                        let field_ty = self.check_expr(field)?;
                        self.inference_engine.add_constraint((*key_type).clone(), field_ty);
                        Ok(Type::Optional(value_type))
                    }
                    Type::Tuple(elems) => {
                        let field_ty = self.check_expr(field)?;
                        if field_ty != Type::Int {
                            return Err(Self::type_err(
                                "Tuple index must be integer",
                                Some(Type::Int),
                                Some(field_ty),
                                None,
                            ));
                        }
                        if let Expr::Val(Val::Int(i)) = field {
                            let idx = *i as usize;
                            if idx < elems.len() {
                                return Ok(Type::Optional(Box::new(elems[idx].clone())));
                            }
                        }
                        let u = Type::Union(elems);
                        Ok(Type::Optional(Box::new(u)))
                    }
                    Type::Named(name) => {
                        let field_ty = self.struct_field_type(&name, field)?;
                        Ok(Type::Optional(Box::new(field_ty)))
                    }
                    Type::Any | Type::Variable(_) => {
                        self.check_expr(field)?;
                        Ok(Type::Any)
                    }
                    other => Err(Self::type_err("Cannot access field on type", None, Some(other), None)),
                }
            }
            Type::Nil => Ok(Type::Nil),
            _ => self.check_access(expr, field),
        }
    }

    /// Check function call type
    fn check_function_call(&mut self, func: &Expr, args: &[Expr]) -> Result<Type> {
        if let Expr::Access(obj_expr, field_expr) = func {
            let receiver_ty = self.check_expr(obj_expr)?;
            if let Expr::Val(Val::Str(name)) = field_expr.as_ref() {
                if let Some(Type::Function {
                    params,
                    named_params,
                    return_type,
                }) = self.get_method_sig(&receiver_ty, name.as_ref())
                {
                    if params.is_empty() {
                        return Err(Self::type_err(
                            "Method signature missing receiver parameter",
                            None,
                            None,
                            Some(func.clone()),
                        ));
                    }
                    let mut params_iter = params.into_iter();
                    let self_param = params_iter.next().unwrap();
                    self.inference_engine.add_constraint(self_param, receiver_ty.clone());

                    let remaining_params: Vec<Type> = params_iter.collect();
                    if remaining_params.len() != args.len() {
                        return Err(Self::type_err(
                            &format!("Method expects {} arguments", remaining_params.len()),
                            None,
                            None,
                            None,
                        ));
                    }
                    for (param_type, arg) in remaining_params.iter().zip(args.iter()) {
                        let arg_type = self.check_expr(arg)?;
                        self.inference_engine.add_constraint(param_type.clone(), arg_type);
                    }
                    for decl in named_params {
                        let is_optional = matches!(decl.ty, Type::Optional(_)) || decl.has_default;
                        if !is_optional {
                            return Err(Self::type_err(
                                &format!("Missing required named argument: {}", decl.name),
                                None,
                                None,
                                None,
                            ));
                        }
                    }
                    return Ok(*return_type);
                } else {
                    // Dynamic method invocation without a registered signature: allow call but treat as `Any`.
                    for arg in args {
                        self.check_expr(arg)?;
                    }
                    return Ok(Type::Any);
                }
            }
        }

        if let Expr::Var(name) = func {
            match name.as_str() {
                "chan" => {
                    if args.is_empty() || args.len() > 2 {
                        return Err(Self::type_err("chan() expects 1 or 2 arguments", None, None, None));
                    }
                    let capacity_ty = self.check_expr(&args[0])?;
                    self.enforce_int_type(&args[0], capacity_ty, "chan capacity")?;
                    if args.len() == 2 {
                        let type_arg_ty = self.check_expr(&args[1])?;
                        if self.resolve_aliases(&type_arg_ty) != Type::String {
                            return Err(Self::type_err(
                                "chan() type hint must be String when provided",
                                Some(Type::String),
                                Some(type_arg_ty),
                                Some(args[1].clone()),
                            ));
                        }
                    }
                    return Ok(Type::Channel(Box::new(Type::Any)));
                }
                "send" => {
                    if args.len() != 2 {
                        return Err(Self::type_err("send() expects 2 arguments", None, None, None));
                    }
                    let channel_ty = self.check_expr(&args[0])?;
                    let value_ty = self.check_expr(&args[1])?;
                    match self.resolve_aliases(&channel_ty) {
                        Type::Channel(inner) => {
                            self.inference_engine.add_constraint((*inner).clone(), value_ty);
                            return Ok(Type::Nil);
                        }
                        other => {
                            return Err(Self::type_err(
                                "send() pattern requires a channel",
                                Some(Type::Channel(Box::new(Type::Any))),
                                Some(other),
                                Some(args[0].clone()),
                            ));
                        }
                    }
                }
                "recv" => {
                    if args.len() != 1 {
                        return Err(Self::type_err("recv() expects exactly 1 argument", None, None, None));
                    }
                    let channel_ty = self.check_expr(&args[0])?;
                    return match self.resolve_aliases(&channel_ty) {
                        Type::Channel(inner) => Ok((*inner).clone()),
                        other => Err(Self::type_err(
                            "recv() pattern requires a channel",
                            Some(Type::Channel(Box::new(Type::Any))),
                            Some(other),
                            Some(args[0].clone()),
                        )),
                    };
                }
                "spawn" => {
                    if args.len() != 1 {
                        return Err(Self::type_err("spawn() expects exactly 1 argument", None, None, None));
                    }
                    let callable_ty = self.check_expr(&args[0])?;
                    match self.resolve_aliases(&callable_ty) {
                        Type::Function { .. } => {}
                        Type::Any | Type::Variable(_) => {
                            let expected = Type::Function {
                                params: Vec::new(),
                                named_params: Vec::new(),
                                return_type: Box::new(Type::Any),
                            };
                            self.inference_engine.add_constraint(callable_ty, expected);
                        }
                        other => {
                            return Err(Self::type_err(
                                "spawn() expects a function or closure",
                                None,
                                Some(other),
                                Some(args[0].clone()),
                            ));
                        }
                    }
                    return Ok(Type::Task(Box::new(Type::Any)));
                }
                _ => {}
            }
        }

        let func_type = self.check_expr(func)?;
        let resolved = self.resolve_aliases(&func_type);

        if let Some((params, named_params, return_type)) = match resolved.clone() {
            Type::Function {
                params,
                named_params,
                return_type,
            } => Some((params, named_params, return_type)),
            Type::Optional(inner) => match *inner {
                Type::Function {
                    params,
                    named_params,
                    return_type,
                } => Some((params, named_params, return_type)),
                _ => None,
            },
            _ => None,
        } {
            if params.len() != args.len() {
                return Err(Self::type_err(
                    &format!("Function expects {} arguments", params.len()),
                    None,
                    None,
                    None,
                ));
            }

            for (param_type, arg) in params.iter().zip(args.iter()) {
                let arg_type = self.check_expr(arg)?;
                self.inference_engine.add_constraint(param_type.clone(), arg_type);
            }

            for decl in &named_params {
                let is_optional = matches!(decl.ty, Type::Optional(_)) || decl.has_default;
                if !is_optional {
                    return Err(Self::type_err(
                        &format!("Missing required named argument: {}", decl.name),
                        None,
                        None,
                        None,
                    ));
                }
            }

            return Ok(*return_type);
        }

        match resolved {
            Type::Any | Type::Variable(_) => {
                for arg in args {
                    self.check_expr(arg)?;
                }
                Ok(Type::Any)
            }
            Type::Union(variants) => {
                let mut saw_function = false;
                for variant in variants {
                    match variant {
                        Type::Function { .. } => {
                            saw_function = true;
                            break;
                        }
                        Type::Optional(inner) if matches!(*inner, Type::Function { .. }) => {
                            saw_function = true;
                            break;
                        }
                        _ => {}
                    }
                }
                if saw_function {
                    for arg in args {
                        self.check_expr(arg)?;
                    }
                    Ok(Type::Any)
                } else {
                    Err(Self::type_err(
                        "Cannot call non-function type",
                        None,
                        Some(func_type),
                        None,
                    ))
                }
            }
            _ => Err(Self::type_err(
                "Cannot call non-function type",
                None,
                Some(func_type),
                None,
            )),
        }
    }

    /// Check select expression type
    fn check_select_expr(&mut self, cases: &[SelectCase], default: &Option<Box<Expr>>) -> Result<Type> {
        // Accumulate unified result type across cases and default
        let mut unified: Option<Type> = None;

        for case in cases {
            // Guard must be Bool when present
            if let Some(guard) = &case.guard {
                let gty = self.check_expr(guard)?;
                if gty != Type::Bool {
                    return Err(Self::type_err(
                        "Select guard must be Bool",
                        Some(Type::Bool),
                        Some(gty),
                        Some(*guard.clone()),
                    ));
                }
            }

            // Pattern checks and per-case bindings
            let snapshot = self.local_types.clone();
            match &case.pattern {
                SelectPattern::Recv { binding, channel } => {
                    let ch_ty = self.check_expr(channel)?;
                    match ch_ty {
                        Type::Channel(inner) => {
                            if let Some(name) = binding {
                                // Bind to a tuple [Bool, T]
                                self.add_local_type(name.clone(), Type::Tuple(vec![Type::Bool, (*inner).clone()]));
                            }
                        }
                        other => {
                            return Err(Self::type_err(
                                "recv() pattern requires a channel",
                                Some(Type::Channel(Box::new(Type::Any))),
                                Some(other),
                                Some(*channel.clone()),
                            ));
                        }
                    }
                }
                SelectPattern::Send { channel, value } => {
                    let ch_ty = self.check_expr(channel)?;
                    let val_ty = self.check_expr(value)?;
                    match ch_ty {
                        Type::Channel(inner) => {
                            self.inference_engine.add_constraint(*inner, val_ty);
                        }
                        other => {
                            return Err(Self::type_err(
                                "send() pattern requires a channel",
                                Some(Type::Channel(Box::new(Type::Any))),
                                Some(other),
                                Some(*channel.clone()),
                            ));
                        }
                    }
                }
            }

            // Check body with any per-case bindings in scope, then restore
            let case_ty = self.check_expr(&case.body)?;
            self.local_types = snapshot;

            if let Some(prev) = &unified {
                self.inference_engine.add_constraint(prev.clone(), case_ty.clone());
            } else {
                unified = Some(case_ty);
            }
        }

        if let Some(default_expr) = default {
            let default_type = self.check_expr(default_expr)?;
            if let Some(prev) = &unified {
                self.inference_engine.add_constraint(prev.clone(), default_type.clone());
            } else {
                unified = Some(default_type);
            }
        }

        unified.ok_or_else(|| {
            Self::type_err(
                "Select expression must have at least one case or default",
                None,
                None,
                None,
            )
        })
    }

    /// Check template string type
    fn check_template_string(&mut self, parts: &[TemplateStringPart]) -> Result<Type> {
        // All parts must be string-coercible
        for part in parts {
            match part {
                TemplateStringPart::Literal(_) => {
                    // String literals are fine
                }
                TemplateStringPart::Expr(expr) => {
                    let expr_type = self.check_expr(expr)?;
                    // Allow implicit string coercion for template parts (adds constraint for vars)
                    self.coerce_to_string(&expr_type);
                }
            }
        }

        Ok(Type::String)
    }

    /// Check literal value type
    fn check_literal(&mut self, val: &Val) -> Result<Type> {
        match val {
            Val::Nil => Ok(Type::Nil),
            Val::Bool(_) => Ok(Type::Bool),
            Val::Int(_) => Ok(Type::Int),
            Val::Float(_) => Ok(Type::Float),
            Val::Str(_) => Ok(Type::String),
            Val::List(items) => {
                if items.is_empty() {
                    let elem_type = self.registry.fresh_type_var();
                    Ok(Type::List(Box::new(elem_type)))
                } else {
                    let first_type = self.infer_list_element_type(&items[0])?;
                    Ok(Type::List(Box::new(first_type)))
                }
            }
            Val::Map(map) => {
                if map.is_empty() {
                    let key_type = self.registry.fresh_type_var();
                    let value_type = self.registry.fresh_type_var();
                    Ok(Type::Map(Box::new(key_type), Box::new(value_type)))
                } else {
                    let (_first_key, first_value) = map.iter().next().unwrap();
                    let key_type = Type::String; // Map keys are always strings
                    let value_type = self.infer_val_type(first_value)?;
                    Ok(Type::Map(Box::new(key_type), Box::new(value_type)))
                }
            }
            // Other types return Any for now
            Val::Closure(_) => Ok(Type::Any),
            Val::RustFunction(_) | Val::RustFunctionNamed(_) => Ok(Type::Any),
            Val::Task(_) => Ok(Type::Any),
            Val::Channel(_) => Ok(Type::Any),
            Val::Stream(_) => Ok(Type::Any),
            Val::StreamCursor { .. } => Ok(Type::Any),
            Val::Iterator(_) => Ok(Type::Any),
            Val::MutationGuard(_) => Ok(Type::Any),
            Val::Object(_) => Ok(Type::Any),
        }
    }

    /// Infer type from a Val (for use in literal checking)
    pub(super) fn infer_val_type(&mut self, val: &Val) -> Result<Type> {
        match val {
            Val::Nil => Ok(Type::Nil),
            Val::Bool(_) => Ok(Type::Bool),
            Val::Int(_) => Ok(Type::Int),
            Val::Float(_) => Ok(Type::Float),
            Val::Str(_) => Ok(Type::String),
            Val::List(items) => {
                if items.is_empty() {
                    let elem_type = self.registry.fresh_type_var();
                    Ok(Type::List(Box::new(elem_type)))
                } else {
                    let elem_type = self.infer_list_element_type(&items[0])?;
                    Ok(Type::List(Box::new(elem_type)))
                }
            }
            Val::Map(map) => {
                if map.is_empty() {
                    let key_type = self.registry.fresh_type_var();
                    let value_type = self.registry.fresh_type_var();
                    Ok(Type::Map(Box::new(key_type), Box::new(value_type)))
                } else {
                    let (_first_key, first_value) = map.iter().next().unwrap();
                    let key_type = Type::String; // Map keys are always strings
                    let value_type = self.infer_val_type(first_value)?;
                    Ok(Type::Map(Box::new(key_type), Box::new(value_type)))
                }
            }
            Val::Closure(_) => Ok(Type::Any),
            Val::RustFunction(_) | Val::RustFunctionNamed(_) => Ok(Type::Any),
            Val::Task(_) => Ok(Type::Any),
            Val::Channel(_) => Ok(Type::Any),
            Val::Stream(_) => Ok(Type::Any),
            Val::StreamCursor { .. } => Ok(Type::Any),
            Val::Iterator(_) => Ok(Type::Any),
            Val::MutationGuard(_) => Ok(Type::Any),
            Val::Object(_) => Ok(Type::Any),
        }
    }

    /// Infer list element type from a Val
    fn infer_list_element_type(&mut self, item: &Val) -> Result<Type> {
        self.infer_val_type(item)
    }
}
