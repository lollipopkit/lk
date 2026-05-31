use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};

use crate::{
    expr::Expr,
    val::LiteralVal,
    vm::analysis::{PerfCallFact, PerfCallTargetKind, PerfContainerMoveFact, PerfIndexTargetKind, PerfValueKind},
};

use super::{
    Compiler32, Instr32, Opcode32,
    facts::index_fact_from_target,
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

        if self.can_direct_call_module_function(name) {
            return self.lower_direct_function_call(name, args);
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
        {
            if self.is_external_global_access_target(target) {
                if self.is_stdlib_module_method(target, "map", "get", method) {
                    return self.lower_map_get_function_call(args);
                }
                if self.is_stdlib_module_method(target, "math", "floor", method) {
                    return self.lower_math_floor_function_call(args);
                }
                let callee = self.lower_readonly_operand(callee)?;
                return self.lower_call_window_boxes(callee, args);
            }
            return self.lower_builtin_method_call(target, method, args);
        }
        let callee = self.lower_readonly_operand(callee)?;
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

    fn is_stdlib_module_method(&self, target: &Expr, module: &str, method: &str, actual_method: &str) -> bool {
        actual_method == method && matches!(target, Expr::Var(name) if name == module)
    }

    fn lower_map_get_function_call(&mut self, args: &[Box<Expr>]) -> Result<u16> {
        if args.len() != 2 {
            bail!("Compiler32 map.get expects 2 args, got {}", args.len());
        }
        let target = self.lower_readonly_index_operand(&args[0])?;
        let index_fact = index_fact_from_target(&self.function.performance, target);
        let (key, key_fact) = self.lower_readonly_index_key(&args[1])?;
        let dst = self.alloc_reg();
        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::GetIndex,
            checked_u8("map.get dst", dst)?,
            checked_u8("map.get target", target)?,
            checked_u8("map.get key", key)?,
        ));
        self.function.performance.clear_register(dst);
        if let Some(fact) = key_fact {
            self.function.performance.set_key_fact(pc, fact);
        }
        if let Some(fact) = index_fact {
            self.function.performance.set_index_fact(pc, fact);
        }
        Ok(dst)
    }

    fn lower_math_floor_function_call(&mut self, args: &[Box<Expr>]) -> Result<u16> {
        if args.len() != 1 {
            bail!("Compiler32 math.floor expects 1 arg, got {}", args.len());
        }
        let arg = self.lower_readonly_operand(&args[0])?;
        if self.function.performance.value_kind(arg) == PerfValueKind::Int {
            return Ok(arg);
        }
        let callee = self.lower_external_module_member("math", "floor")?;
        self.lower_call_window_regs(callee, &[arg])
    }

    fn lower_external_module_member(&mut self, module: &str, member: &str) -> Result<u16> {
        let slot = self
            .global_names
            .get(module)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("Compiler32 undefined external module `{module}`"))?;
        let target = self.emit_get_global(slot)?;
        let key_const = self.push_string(member)?;
        let key = self.alloc_reg();
        self.emit(Instr32::abx(
            Opcode32::LoadString,
            checked_u8("module member key", key)?,
            key_const,
        ));
        self.set_register_kind(key, PerfValueKind::String);
        let dst = self.alloc_reg();
        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::GetIndex,
            checked_u8("module member dst", dst)?,
            checked_u8("module member target", target)?,
            checked_u8("module member key", key)?,
        ));
        self.function.performance.clear_register(dst);
        self.function.performance.set_key_fact(
            pc,
            crate::vm::analysis::PerfKeyFact {
                const_key: Some(key_const),
                string_int: None,
            },
        );
        Ok(dst)
    }

    fn lower_readonly_index_operand(&mut self, expr: &Expr) -> Result<u16> {
        if let Expr::Var(name) = expr
            && let Some(local) = self.locals.get(name).copied()
            && !self.cell_locals.contains(name)
        {
            return Ok(local);
        }
        self.lower_expr(expr)
    }

    fn lower_readonly_index_key(&mut self, expr: &Expr) -> Result<(u16, Option<crate::vm::analysis::PerfKeyFact>)> {
        if matches!(expr, Expr::Var(_)) {
            return Ok((self.lower_readonly_index_operand(expr)?, None));
        }
        self.lower_index_key(expr)
    }

    fn lower_builtin_method_call(&mut self, target: &Expr, method: &str, args: &[Box<Expr>]) -> Result<u16> {
        match method {
            "len" => {
                if !args.is_empty() {
                    bail!("Compiler32 method len expects 0 args, got {}", args.len());
                }
                let target = self.lower_readonly_operand(target)?;
                let dst = self.alloc_reg();
                self.emit(Instr32::abc(
                    Opcode32::Len,
                    checked_u8("method len dst", dst)?,
                    checked_u8("method len target", target)?,
                    0,
                ));
                self.set_register_kind(dst, PerfValueKind::Int);
                Ok(dst)
            }
            "push" => {
                if args.len() != 1 {
                    bail!("Compiler32 method push expects 1 arg, got {}", args.len());
                }
                self.lower_push_method_call(target, &args[0])
            }
            "set" => {
                if args.len() != 2 {
                    bail!("Compiler32 method set expects 2 args, got {}", args.len());
                }
                self.lower_set_method_call(target, &args[0], &args[1])
            }
            "starts_with" => {
                if args.len() != 1 {
                    bail!("Compiler32 method starts_with expects 1 arg, got {}", args.len());
                }
                self.lower_starts_with_method_call(target, &args[0])
            }
            "split" => {
                if args.len() != 1 {
                    bail!("Compiler32 method split expects 1 arg, got {}", args.len());
                }
                self.lower_string_split_method_call(target, &args[0])
            }
            "join" => {
                if args.len() != 1 {
                    bail!("Compiler32 method join expects 1 arg, got {}", args.len());
                }
                self.lower_list_join_method_call(target, &args[0])
            }
            _ => self.lower_dynamic_method_call(target, method, args),
        }
    }

    fn lower_set_method_call(&mut self, target: &Expr, key: &Expr, value: &Expr) -> Result<u16> {
        let target_reg = self.lower_mutable_method_receiver(target)?;
        let index_fact = index_fact_from_target(&self.function.performance, target_reg)
            .filter(|fact| fact.target_kind != PerfIndexTargetKind::String);
        let (key_reg, key_fact) = self.lower_index_key(key)?;
        let value_reg = self.lower_readonly_operand(value)?;
        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::SetIndex,
            checked_u8("method set target", target_reg)?,
            checked_u8("method set key", key_reg)?,
            checked_u8("method set value", value_reg)?,
        ));
        self.function.performance.set_container_move_fact(
            pc,
            PerfContainerMoveFact {
                move_key: set_method_key_move_preferred(key),
                move_value: true,
            },
        );
        if let Some(fact) = index_fact {
            self.function.performance.set_index_fact(pc, fact);
        }
        if let Some(fact) = key_fact {
            self.function.performance.set_key_fact(pc, fact);
        }
        let dst = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::LoadNil,
            checked_u8("method set result", dst)?,
            0,
            0,
        ));
        self.set_register_kind(dst, PerfValueKind::Nil);
        Ok(dst)
    }

    fn lower_push_method_call(&mut self, target: &Expr, value: &Expr) -> Result<u16> {
        let target_reg = self.lower_mutable_method_receiver(target)?;
        let value_reg = self.lower_readonly_operand(value)?;
        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::ListPush,
            checked_u8("method push target", target_reg)?,
            checked_u8("method push value", value_reg)?,
            0,
        ));
        self.function.performance.set_container_move_fact(
            pc,
            PerfContainerMoveFact {
                move_key: false,
                move_value: true,
            },
        );
        Ok(target_reg)
    }

    fn lower_starts_with_method_call(&mut self, target: &Expr, prefix: &Expr) -> Result<u16> {
        let target_reg = self.lower_readonly_access_target(target)?;
        let prefix_reg = self.lower_readonly_operand(prefix)?;
        let dst = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::StringStartsWith,
            checked_u8("method starts_with dst", dst)?,
            checked_u8("method starts_with target", target_reg)?,
            checked_u8("method starts_with prefix", prefix_reg)?,
        ));
        self.set_register_kind(dst, PerfValueKind::Bool);
        Ok(dst)
    }

    fn lower_string_split_method_call(&mut self, target: &Expr, delimiter: &Expr) -> Result<u16> {
        let target_reg = self.lower_readonly_access_target(target)?;
        let delimiter_reg = self.lower_readonly_operand(delimiter)?;
        let dst = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::StringSplit,
            checked_u8("method split dst", dst)?,
            checked_u8("method split target", target_reg)?,
            checked_u8("method split delimiter", delimiter_reg)?,
        ));
        self.set_register_kind(dst, PerfValueKind::List);
        self.set_register_list_fact(
            dst,
            crate::vm::analysis::PerfContainerFact {
                value_kind: PerfValueKind::String,
                known_len: None,
                adoptable: false,
            },
        );
        Ok(dst)
    }

    fn lower_list_join_method_call(&mut self, target: &Expr, separator: &Expr) -> Result<u16> {
        let target_reg = self.lower_readonly_access_target(target)?;
        let separator_reg = self.lower_readonly_operand(separator)?;
        let dst = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::ListJoin,
            checked_u8("method join dst", dst)?,
            checked_u8("method join target", target_reg)?,
            checked_u8("method join separator", separator_reg)?,
        ));
        self.set_register_kind(dst, PerfValueKind::String);
        Ok(dst)
    }

    fn lower_mutable_method_receiver(&mut self, target: &Expr) -> Result<u16> {
        if let Expr::Var(name) = target
            && let Some(local) = self.locals.get(name).copied()
            && !self.cell_locals.contains(name)
        {
            return Ok(local);
        }
        self.lower_expr(target)
    }

    fn lower_dynamic_method_call(&mut self, target: &Expr, method: &str, args: &[Box<Expr>]) -> Result<u16> {
        let helper = self.load_callable_by_name("__lk_call_method")?;
        let receiver = self.lower_readonly_operand(target)?;
        let method = self.lower_val(&LiteralVal::from_str(method))?;
        let mut arg_regs = Vec::with_capacity(args.len());
        for arg in args {
            arg_regs.push(self.lower_readonly_operand(arg)?);
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

        let callee = self.lower_readonly_operand(callee)?;
        let call_base = self.alloc_regs(1 + positional.len() + named.len() * 2)?;
        self.emit_call_window_move(call_base, callee, "named call callee")?;
        for (offset, arg) in positional.iter().enumerate() {
            self.lower_expr_to_register(call_base + 1 + offset as u16, arg, "named call positional arg")?;
        }
        let mut offset = 1 + positional.len() as u16;
        for (name, value) in named {
            self.emit_literal_to_register(call_base + offset, &LiteralVal::from_str(name))?;
            self.lower_expr_to_register(call_base + offset + 1, value, "named call arg value")?;
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
        if let Some(function_index) = self.direct_function_index_u8(function_name)? {
            return self.lower_signature_positional_direct_call(
                function_name,
                function_index,
                signature,
                args,
                total_count,
            );
        }

        let mut previous = Vec::with_capacity(total_count);
        let mut arg_regs = Vec::with_capacity(total_count);

        for (param_name, arg) in signature.positional_params.iter().zip(args.iter()) {
            let reg = self.lower_readonly_operand(arg)?;
            self.bind_call_param(param_name, reg, &mut previous);
            arg_regs.push(reg);
        }

        let supplied_named = args.len() - signature.positional_count;
        for (index, param) in signature.named_params.iter().enumerate() {
            let reg = if index < supplied_named {
                self.lower_readonly_operand(&args[signature.positional_count + index])?
            } else if let Some(default) = param.default.as_ref() {
                self.lower_readonly_operand(default)?
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
        if self.can_direct_call_module_function(function_name) {
            self.lower_direct_function_call_regs(function_name, &arg_regs)
        } else {
            let callee = self.load_callable_by_name(function_name)?;
            self.lower_call_window_regs(callee, &arg_regs)
        }
    }

    fn lower_signature_named_call(
        &mut self,
        function_name: &str,
        signature: &FunctionSignature32,
        positional: &[Box<Expr>],
        provided: &HashMap<&str, &Expr>,
    ) -> Result<u16> {
        let total_count = signature.positional_count + signature.named_params.len();
        if let Some(function_index) = self.direct_function_index_u8(function_name)? {
            return self.lower_signature_named_direct_call(
                function_name,
                function_index,
                signature,
                positional,
                provided,
                total_count,
            );
        }

        let mut previous = Vec::with_capacity(total_count);
        let mut arg_regs = Vec::with_capacity(total_count);

        for (param_name, arg) in signature.positional_params.iter().zip(positional.iter()) {
            let reg = self.lower_readonly_operand(arg)?;
            self.bind_call_param(param_name, reg, &mut previous);
            arg_regs.push(reg);
        }

        for param in &signature.named_params {
            let reg = if let Some(expr) = provided.get(param.name.as_str()) {
                self.lower_readonly_operand(expr)?
            } else if let Some(default) = param.default.as_ref() {
                self.lower_readonly_operand(default)?
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
        if self.can_direct_call_module_function(function_name) {
            self.lower_direct_function_call_regs(function_name, &arg_regs)
        } else {
            let callee = self.load_callable_by_name(function_name)?;
            self.lower_call_window_regs(callee, &arg_regs)
        }
    }

    fn lower_signature_positional_direct_call(
        &mut self,
        function_name: &str,
        function_index: u8,
        signature: &FunctionSignature32,
        args: &[Box<Expr>],
        total_count: usize,
    ) -> Result<u16> {
        let call_base = self.alloc_regs(total_count + 1)?;
        let mut previous = Vec::with_capacity(total_count);

        let result = (|| {
            for (index, (param_name, arg)) in signature.positional_params.iter().zip(args.iter()).enumerate() {
                let dst = call_base + 1 + index as u16;
                self.lower_expr_to_register(dst, arg, "direct signature positional arg")?;
                self.bind_call_param(param_name, dst, &mut previous);
            }

            let supplied_named = args.len() - signature.positional_count;
            for (index, param) in signature.named_params.iter().enumerate() {
                let dst = call_base + 1 + signature.positional_count as u16 + index as u16;
                if index < supplied_named {
                    self.lower_expr_to_register(
                        dst,
                        &args[signature.positional_count + index],
                        "direct signature positional named arg",
                    )?;
                } else if let Some(default) = param.default.as_ref() {
                    self.lower_expr_to_register(dst, default, "direct signature default arg")?;
                } else {
                    bail!(
                        "Compiler32 missing required named argument `{}` in call to `{function_name}`",
                        param.name
                    );
                }
                self.bind_call_param(&param.name, dst, &mut previous);
            }
            Ok(())
        })();
        self.restore_call_params(previous);
        result?;

        self.emit_direct_call_at_window(function_index, call_base, total_count)
    }

    fn lower_signature_named_direct_call(
        &mut self,
        function_name: &str,
        function_index: u8,
        signature: &FunctionSignature32,
        positional: &[Box<Expr>],
        provided: &HashMap<&str, &Expr>,
        total_count: usize,
    ) -> Result<u16> {
        let call_base = self.alloc_regs(total_count + 1)?;
        let mut previous = Vec::with_capacity(total_count);

        let result = (|| {
            for (index, (param_name, arg)) in signature.positional_params.iter().zip(positional.iter()).enumerate() {
                let dst = call_base + 1 + index as u16;
                self.lower_expr_to_register(dst, arg, "direct signature named positional arg")?;
                self.bind_call_param(param_name, dst, &mut previous);
            }

            for (index, param) in signature.named_params.iter().enumerate() {
                let dst = call_base + 1 + signature.positional_count as u16 + index as u16;
                if let Some(expr) = provided.get(param.name.as_str()) {
                    self.lower_expr_to_register(dst, expr, "direct signature named arg")?;
                } else if let Some(default) = param.default.as_ref() {
                    self.lower_expr_to_register(dst, default, "direct signature named default arg")?;
                } else {
                    bail!(
                        "Compiler32 missing required named argument `{}` in call to `{function_name}`",
                        param.name
                    );
                }
                self.bind_call_param(&param.name, dst, &mut previous);
            }
            Ok(())
        })();
        self.restore_call_params(previous);
        result?;

        self.emit_direct_call_at_window(function_index, call_base, total_count)
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
        let call_base = self.alloc_regs(args.len() + 1)?;
        self.emit_call_window_move(call_base, callee, "call callee")?;
        for (offset, arg) in args.iter().copied().enumerate() {
            self.lower_expr_to_register(call_base + 1 + offset as u16, arg, "call arg")?;
        }

        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::Call,
            checked_u8("call return base", call_base)?,
            checked_u8("call base", call_base)?,
            checked_u8("call argc", args.len() as u16)?,
        ));
        let target_kind = self.function.performance.callable_kind(call_base);
        self.function.performance.clear_register(call_base);
        self.function.performance.set_call_fact(
            pc,
            PerfCallFact {
                call_base,
                positional_count: args.len() as u16,
                named_count: 0,
                target_kind,
            },
        );
        Ok(call_base)
    }

    pub(super) fn lower_call_window_regs(&mut self, callee: u16, arg_regs: &[u16]) -> Result<u16> {
        if arg_regs.len() > i8::MAX as usize {
            bail!("Compiler32 call has {} args, max {}", arg_regs.len(), i8::MAX);
        }
        let call_base = self.alloc_regs(arg_regs.len() + 1)?;
        self.emit_call_window_move(call_base, callee, "call callee")?;
        for (offset, arg) in arg_regs.iter().copied().enumerate() {
            self.emit_call_window_move(call_base + 1 + offset as u16, arg, "call arg")?;
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

    fn can_direct_call_module_function(&self, name: &str) -> bool {
        self.function_names.contains_key(name) && !self.locals.contains_key(name)
    }

    fn direct_function_index_u8(&self, name: &str) -> Result<Option<u8>> {
        if !self.can_direct_call_module_function(name) {
            return Ok(None);
        }
        let function_index = *self
            .function_names
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Compiler32 undefined function `{name}`"))?;
        if function_index > u8::MAX as u32 {
            return Ok(None);
        }
        Ok(Some(function_index as u8))
    }

    fn emit_call_window_move(&mut self, dst: u16, src: u16, context: &str) -> Result<()> {
        let move_source = !self.is_current_local_slot(src);
        self.emit_move_with_policy(dst, src, context, move_source)
    }

    fn emit_direct_call_at_window(&mut self, function_index: u8, call_base: u16, argc: usize) -> Result<u16> {
        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::CallDirect,
            checked_u8("direct call return base", call_base)?,
            checked_u8("direct call function index", function_index as u16)?,
            checked_u8("direct call argc", argc as u16)?,
        ));
        self.function.performance.clear_register(call_base);
        self.function.performance.set_call_fact(
            pc,
            PerfCallFact {
                call_base,
                positional_count: argc as u16,
                named_count: 0,
                target_kind: PerfCallTargetKind::Closure,
            },
        );
        Ok(call_base)
    }

    fn lower_direct_function_call(&mut self, function_name: &str, args: &[Box<Expr>]) -> Result<u16> {
        if let Some(inlined) = self.try_inline_direct_function_call(function_name, args)? {
            return Ok(inlined);
        }
        if args.len() > i8::MAX as usize {
            bail!("Compiler32 call has {} args, max {}", args.len(), i8::MAX);
        }
        let function_index = *self
            .function_names
            .get(function_name)
            .ok_or_else(|| anyhow::anyhow!("Compiler32 undefined function `{function_name}`"))?;
        if function_index > u8::MAX as u32 {
            let callee = self.load_callable_by_name(function_name)?;
            let mut refs = Vec::with_capacity(args.len());
            for arg in args {
                refs.push(arg.as_ref());
            }
            return self.lower_call_window_exprs(callee, &refs);
        }

        let call_base = self.alloc_regs(args.len() + 1)?;
        for (offset, arg) in args.iter().enumerate() {
            self.lower_expr_to_register(call_base + 1 + offset as u16, arg, "direct call arg")?;
        }

        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::CallDirect,
            checked_u8("direct call return base", call_base)?,
            checked_u8("direct call function index", function_index as u16)?,
            checked_u8("direct call argc", args.len() as u16)?,
        ));
        self.function.performance.clear_register(call_base);
        self.function.performance.set_call_fact(
            pc,
            PerfCallFact {
                call_base,
                positional_count: args.len() as u16,
                named_count: 0,
                target_kind: PerfCallTargetKind::Closure,
            },
        );
        Ok(call_base)
    }

    fn lower_direct_function_call_regs(&mut self, function_name: &str, arg_regs: &[u16]) -> Result<u16> {
        if arg_regs.len() > i8::MAX as usize {
            bail!("Compiler32 call has {} args, max {}", arg_regs.len(), i8::MAX);
        }
        let function_index = *self
            .function_names
            .get(function_name)
            .ok_or_else(|| anyhow::anyhow!("Compiler32 undefined function `{function_name}`"))?;
        if function_index > u8::MAX as u32 {
            let callee = self.load_callable_by_name(function_name)?;
            return self.lower_call_window_regs(callee, arg_regs);
        }

        let call_base = self.alloc_regs(arg_regs.len() + 1)?;
        for (offset, arg) in arg_regs.iter().copied().enumerate() {
            self.emit_call_window_move(call_base + 1 + offset as u16, arg, "direct call arg")?;
        }

        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::CallDirect,
            checked_u8("direct call return base", call_base)?,
            checked_u8("direct call function index", function_index as u16)?,
            checked_u8("direct call argc", arg_regs.len() as u16)?,
        ));
        self.function.performance.clear_register(call_base);
        self.function.performance.set_call_fact(
            pc,
            PerfCallFact {
                call_base,
                positional_count: arg_regs.len() as u16,
                named_count: 0,
                target_kind: PerfCallTargetKind::Closure,
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

fn set_method_key_move_preferred(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) => set_method_key_move_preferred(inner),
        Expr::Var(_) => false,
        _ => true,
    }
}
