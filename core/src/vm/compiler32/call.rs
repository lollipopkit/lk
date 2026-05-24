use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};

use crate::{expr::Expr, val::LiteralVal, vm::analysis::PerfCallFact};

use super::{
    Compiler32, Instr32, Opcode32,
    support::{FunctionSignature32, checked_u8},
};

impl Compiler32 {
    pub(super) fn lower_named_call(&mut self, name: &str, args: &[Box<Expr>]) -> Result<u16> {
        if let Some(signature) = self.function_signatures.get(name).cloned()
            && !signature.named_params.is_empty()
            && self.function_names.contains_key(name)
        {
            return self.lower_signature_positional_call(name, &signature, args);
        }

        let callee = if let Some(local) = self.locals.get(name).copied() {
            local
        } else {
            self.load_callable_by_name(name)?
        };
        self.lower_call_window_boxes(callee, args)
    }

    pub(super) fn lower_call_expr(&mut self, callee: &Expr, args: &[Box<Expr>]) -> Result<u16> {
        if let Expr::Var(name) = callee {
            return self.lower_named_call(name, args);
        }
        if let Expr::Access(target, method) = callee
            && let Some(method) = method_name(method)
            && !self.is_external_global_access_target(target)
        {
            return self.lower_builtin_method_call(target, method, args);
        }
        let callee = self.lower_expr(callee)?;
        self.lower_call_window_boxes(callee, args)
    }

    fn is_external_global_access_target(&self, target: &Expr) -> bool {
        let Expr::Var(name) = target else {
            return false;
        };
        self.global_names.contains_key(name)
            && !self.locals.contains_key(name)
            && !self.function_names.contains_key(name)
            && !self.native_names.contains_key(name)
    }

    fn lower_builtin_method_call(&mut self, target: &Expr, method: &str, args: &[Box<Expr>]) -> Result<u16> {
        match method {
            "len" => {
                if !args.is_empty() {
                    bail!("Compiler32 method len expects 0 args, got {}", args.len());
                }
                let target = self.lower_expr(target)?;
                let dst = self.alloc_reg();
                self.emit(Instr32::abc(
                    Opcode32::Len,
                    checked_u8("method len dst", dst)?,
                    checked_u8("method len target", target)?,
                    0,
                ));
                Ok(dst)
            }
            "push" => {
                if args.len() != 1 {
                    bail!("Compiler32 method push expects 1 arg, got {}", args.len());
                }
                let target_reg = self.lower_expr(target)?;
                let arg = self.lower_expr(&args[0])?;
                let one = self.materialize_list(vec![arg])?;
                let dst = self.alloc_reg();
                self.emit_bin_op_to_register(dst, &crate::operator::BinOp::Add, target_reg, one)?;
                if let Expr::Var(name) = target {
                    if let Some(local) = self.locals.get(name).copied() {
                        self.emit_move(local, dst, "method push writeback")?;
                        return Ok(local);
                    }
                    if self.top_level
                        && let Some(slot) = self.global_names.get(name).copied()
                    {
                        self.emit_set_global(dst, slot)?;
                    }
                }
                Ok(dst)
            }
            _ => self.lower_dynamic_method_call(target, method, args),
        }
    }

    fn lower_dynamic_method_call(&mut self, target: &Expr, method: &str, args: &[Box<Expr>]) -> Result<u16> {
        let helper = self.load_callable_by_name("__lk_call_method")?;
        let receiver = self.lower_expr(target)?;
        let method = self.lower_val(&LiteralVal::from_str(method))?;
        let mut arg_regs = Vec::with_capacity(args.len());
        for arg in args {
            arg_regs.push(self.lower_expr(arg)?);
        }
        let args_list = self.materialize_list(arg_regs)?;
        self.lower_call_window_regs(helper, &[receiver, method, args_list])
    }

    pub(super) fn lower_named_arg_call(
        &mut self,
        callee: &Expr,
        positional: &[Box<Expr>],
        named: &[(String, Box<Expr>)],
    ) -> Result<u16> {
        if named.is_empty() {
            return self.lower_call_expr(callee, positional);
        }

        let Expr::Var(function_name) = callee else {
            return self.lower_dynamic_named_arg_call(callee, positional, named);
        };
        if self.locals.contains_key(function_name) && !self.function_names.contains_key(function_name) {
            bail!("Compiler32 named call `{function_name}` is shadowed by a local binding");
        }

        let signature = self
            .function_signatures
            .get(function_name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Compiler32 missing named-call signature for `{function_name}`"))?;
        if positional.len() != signature.positional_count {
            bail!(
                "Compiler32 named call `{function_name}` expects {} positional args, got {}",
                signature.positional_count,
                positional.len()
            );
        }

        let mut provided = HashMap::with_capacity(named.len());
        for (name, expr) in named {
            if provided.insert(name.as_str(), expr.as_ref()).is_some() {
                bail!("Compiler32 duplicate named argument `{name}` in call to `{function_name}`");
            }
        }

        let mut allowed = HashSet::with_capacity(signature.named_params.len());
        for param in &signature.named_params {
            allowed.insert(param.name.as_str());
        }
        for name in provided.keys() {
            if !allowed.contains(name) {
                bail!("Compiler32 unknown named argument `{name}` in call to `{function_name}`");
            }
        }

        self.lower_signature_named_call(function_name, &signature, positional, &provided)
    }

    fn lower_dynamic_named_arg_call(
        &mut self,
        callee: &Expr,
        positional: &[Box<Expr>],
        named: &[(String, Box<Expr>)],
    ) -> Result<u16> {
        if positional.len() > i8::MAX as usize || named.len() > i8::MAX as usize {
            bail!(
                "Compiler32 dynamic named call has {} positional and {} named args, max {} each",
                positional.len(),
                named.len(),
                i8::MAX
            );
        }

        let callee = self.lower_expr(callee)?;
        let mut positional_regs = Vec::with_capacity(positional.len());
        for arg in positional {
            positional_regs.push(self.lower_expr(arg)?);
        }
        let mut named_regs = Vec::with_capacity(named.len());
        for (name, value) in named {
            let name_reg = self.lower_val(&LiteralVal::from_str(name))?;
            let value_reg = self.lower_expr(value)?;
            named_regs.push((name_reg, value_reg));
        }

        let call_base = self.alloc_regs(1 + positional_regs.len() + named_regs.len() * 2)?;
        self.emit_move(call_base, callee, "named call callee")?;
        for (offset, arg) in positional_regs.iter().copied().enumerate() {
            self.emit_move(call_base + 1 + offset as u16, arg, "named call positional arg")?;
        }
        let mut offset = 1 + positional_regs.len() as u16;
        for (name, value) in named_regs {
            self.emit_move(call_base + offset, name, "named call arg name")?;
            self.emit_move(call_base + offset + 1, value, "named call arg value")?;
            offset += 2;
        }

        let payload = (u16::try_from(named.len())? << 7) | u16::try_from(positional.len())?;
        let pc = self.function.code.len();
        self.emit(Instr32::abx(
            Opcode32::CallNamed,
            checked_u8("named call return base", call_base)?,
            payload,
        ));
        let target_kind = self.function.performance.callable_kind(call_base);
        self.function.performance.clear_register(call_base);
        self.function.performance.set_call_fact(
            pc,
            PerfCallFact {
                call_base,
                positional_count: positional.len() as u16,
                named_count: named.len() as u16,
                target_kind,
            },
        );
        Ok(call_base)
    }

    fn lower_signature_positional_call(
        &mut self,
        function_name: &str,
        signature: &FunctionSignature32,
        args: &[Box<Expr>],
    ) -> Result<u16> {
        let total_count = signature.positional_count + signature.named_params.len();
        if args.len() < signature.positional_count || args.len() > total_count {
            bail!(
                "Compiler32 call `{function_name}` expects {}..{} args, got {}",
                signature.positional_count,
                total_count,
                args.len()
            );
        }

        let mut previous = Vec::with_capacity(total_count);
        let mut arg_regs = Vec::with_capacity(total_count);

        for (param_name, arg) in signature.positional_params.iter().zip(args.iter()) {
            let reg = self.lower_expr(arg)?;
            self.bind_call_param(param_name, reg, &mut previous);
            arg_regs.push(reg);
        }

        let supplied_named = args.len() - signature.positional_count;
        for (index, param) in signature.named_params.iter().enumerate() {
            let reg = if index < supplied_named {
                self.lower_expr(&args[signature.positional_count + index])?
            } else if let Some(default) = param.default.as_ref() {
                self.lower_expr(default)?
            } else {
                self.restore_call_params(previous);
                bail!(
                    "Compiler32 missing required named argument `{}` in call to `{function_name}`",
                    param.name
                );
            };
            self.bind_call_param(&param.name, reg, &mut previous);
            arg_regs.push(reg);
        }

        self.restore_call_params(previous);
        let callee = self.load_callable_by_name(function_name)?;
        self.lower_call_window_regs(callee, &arg_regs)
    }

    fn lower_signature_named_call(
        &mut self,
        function_name: &str,
        signature: &FunctionSignature32,
        positional: &[Box<Expr>],
        provided: &HashMap<&str, &Expr>,
    ) -> Result<u16> {
        let total_count = signature.positional_count + signature.named_params.len();
        let mut previous = Vec::with_capacity(total_count);
        let mut arg_regs = Vec::with_capacity(total_count);

        for (param_name, arg) in signature.positional_params.iter().zip(positional.iter()) {
            let reg = self.lower_expr(arg)?;
            self.bind_call_param(param_name, reg, &mut previous);
            arg_regs.push(reg);
        }

        for param in &signature.named_params {
            let reg = if let Some(expr) = provided.get(param.name.as_str()) {
                self.lower_expr(expr)?
            } else if let Some(default) = param.default.as_ref() {
                self.lower_expr(default)?
            } else {
                self.restore_call_params(previous);
                bail!(
                    "Compiler32 missing required named argument `{}` in call to `{function_name}`",
                    param.name
                );
            };
            self.bind_call_param(&param.name, reg, &mut previous);
            arg_regs.push(reg);
        }

        self.restore_call_params(previous);
        let callee = self.load_callable_by_name(function_name)?;
        self.lower_call_window_regs(callee, &arg_regs)
    }

    fn bind_call_param(&mut self, name: &str, reg: u16, previous: &mut Vec<(String, Option<u16>)>) {
        previous.push((name.to_string(), self.insert_local(name.to_string(), reg)));
    }

    fn restore_call_params(&mut self, previous: Vec<(String, Option<u16>)>) {
        for (name, old) in previous.into_iter().rev() {
            if let Some(old) = old {
                self.insert_local(name, old);
            } else {
                self.locals.remove(&name);
            }
        }
    }

    fn lower_call_window_boxes(&mut self, callee: u16, args: &[Box<Expr>]) -> Result<u16> {
        let mut refs = Vec::with_capacity(args.len());
        for arg in args {
            refs.push(arg.as_ref());
        }
        self.lower_call_window_exprs(callee, &refs)
    }

    fn lower_call_window_exprs(&mut self, callee: u16, args: &[&Expr]) -> Result<u16> {
        if args.len() > i8::MAX as usize {
            bail!("Compiler32 call has {} args, max {}", args.len(), i8::MAX);
        }
        let mut arg_regs = Vec::with_capacity(args.len());
        for arg in args {
            arg_regs.push(self.lower_expr(arg)?);
        }
        self.lower_call_window_regs(callee, &arg_regs)
    }

    fn lower_call_window_regs(&mut self, callee: u16, arg_regs: &[u16]) -> Result<u16> {
        if arg_regs.len() > i8::MAX as usize {
            bail!("Compiler32 call has {} args, max {}", arg_regs.len(), i8::MAX);
        }
        let call_base = self.alloc_regs(arg_regs.len() + 1)?;
        self.emit_move(call_base, callee, "call callee")?;
        for (offset, arg) in arg_regs.iter().copied().enumerate() {
            self.emit_move(call_base + 1 + offset as u16, arg, "call arg")?;
        }

        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::Call,
            checked_u8("call return base", call_base)?,
            checked_u8("call base", call_base)?,
            checked_u8("call argc", arg_regs.len() as u16)?,
        ));
        let target_kind = self.function.performance.callable_kind(call_base);
        self.function.performance.clear_register(call_base);
        self.function.performance.set_call_fact(
            pc,
            PerfCallFact {
                call_base,
                positional_count: arg_regs.len() as u16,
                named_count: 0,
                target_kind,
            },
        );
        Ok(call_base)
    }
}

fn method_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Var(name) => Some(name.as_str()),
        Expr::Literal(value) => value.as_str(),
        _ => None,
    }
}
