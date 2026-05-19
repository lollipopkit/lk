use crate::{
    stmt::Stmt,
    val::{FunctionNamedParamType, Type, Val},
    vm::{Op, compiler::FunctionBuilder},
};

impl FunctionBuilder {
    pub(crate) fn compile_trait_registration(&mut self, name: &str, methods: &[(String, Type)]) {
        let known_builtin = self.const_env.get("__lk_register_trait").cloned();
        let reg_fn = self.emit_known_or_global_callable("__lk_register_trait", known_builtin.as_ref());

        let arg_base = self.alloc();
        let name_idx = self.k(Val::from_str(name));
        self.emit(Op::LoadK(arg_base, name_idx));

        let method_entries: Vec<Val> = methods
            .iter()
            .map(|(method_name, ty)| {
                let type_str = ty.display();
                Val::List(vec![Val::from_str(method_name.as_str()), Val::from_str(type_str.as_str())].into())
            })
            .collect();
        let methods_list = Val::List(method_entries.into());
        let methods_idx = self.k(methods_list);
        let arg_methods = self.alloc();
        self.emit(Op::LoadK(arg_methods, methods_idx));

        self.emit_positional_call(reg_fn, arg_base, 2, 1, known_builtin.as_ref());
    }

    pub(crate) fn compile_trait_impl_registration(&mut self, trait_name: &str, target_type: &Type, methods: &[Stmt]) {
        let known_builtin = self.const_env.get("__lk_register_trait_impl").cloned();
        let reg_fn = self.emit_known_or_global_callable("__lk_register_trait_impl", known_builtin.as_ref());

        let arg_base = self.alloc();
        let arg_target = self.alloc();
        let arg_methods = self.alloc();

        let trait_name_idx = self.k(Val::from_str(trait_name));
        self.emit(Op::LoadK(arg_base, trait_name_idx));

        let target_type_str = target_type.display();
        let target_type_idx = self.k(Val::from_str(target_type_str.as_str()));
        self.emit(Op::LoadK(arg_target, target_type_idx));

        let mut entry_regs: Vec<u16> = Vec::with_capacity(methods.len());
        for method in methods {
            if let Stmt::Function {
                name,
                params,
                param_types,
                return_type,
                body,
                named_params,
            } = method
            {
                let closure_reg = self.emit_function_closure(
                    Some(name.as_str()),
                    params,
                    param_types,
                    named_params,
                    body.as_ref(),
                    false,
                );

                let entry_base = self.alloc();
                let name_idx = self.k(Val::from_str(name.as_str()));
                self.emit(Op::LoadK(entry_base, name_idx));

                let closure_slot = self.alloc();
                self.emit(Op::Move(closure_slot, closure_reg));

                let positional_types: Vec<Type> = params
                    .iter()
                    .enumerate()
                    .map(|(i, _)| param_types.get(i).cloned().flatten().unwrap_or(Type::Any))
                    .collect();
                let named_type_sigs: Vec<FunctionNamedParamType> = named_params
                    .iter()
                    .map(|np| FunctionNamedParamType {
                        name: np.name.clone(),
                        ty: np.type_annotation.clone().unwrap_or(Type::Any),
                        has_default: np.default.is_some(),
                    })
                    .collect();
                let return_ty = return_type.clone().unwrap_or(Type::Any);
                let signature = Type::Function {
                    params: positional_types,
                    named_params: named_type_sigs,
                    return_type: Box::new(return_ty),
                };
                let signature_str = signature.display();
                let signature_idx = self.k(Val::from_str(signature_str.as_str()));
                let signature_slot = self.alloc();
                self.emit(Op::LoadK(signature_slot, signature_idx));

                let entry_list = self.alloc();
                self.emit(Op::BuildList {
                    dst: entry_list,
                    base: entry_base,
                    len: 3,
                });
                entry_regs.push(entry_list);
            }
        }

        if entry_regs.is_empty() {
            let empty_list = Val::List(Vec::<Val>::new().into());
            let empty_idx = self.k(empty_list);
            self.emit(Op::LoadK(arg_methods, empty_idx));
        } else {
            let first_slot = self.alloc();
            self.emit(Op::Move(first_slot, entry_regs[0]));
            for entry_reg in entry_regs.iter().skip(1) {
                let slot = self.alloc();
                self.emit(Op::Move(slot, *entry_reg));
            }
            self.emit(Op::BuildList {
                dst: arg_methods,
                base: first_slot,
                len: entry_regs.len() as u16,
            });
        }

        self.emit_positional_call(reg_fn, arg_base, 3, 1, known_builtin.as_ref());
    }
}
