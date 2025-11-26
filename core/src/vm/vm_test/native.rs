use super::*;

#[test]
#[ignore = "Test depends on vm_blocked which has been removed"]
fn test_vm_native_context_global_sync_across_closure_calls() {
    fn set_flag(args: &[Val], _ctx: &mut VmContext) -> anyhow::Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("set_flag expects exactly 1 argument"));
        }
        let value = args[0].clone();
        if with_current_vm_ctx(|ctx| {
            ctx.set("flag", value.clone());
        })
        .is_some()
        {
            Ok(value)
        } else {
            Err(anyhow::anyhow!("set_flag requires VM context"))
        }
    }

    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "flag".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::Assign {
                name: "flag".into(),
                value: Box::new(Expr::Call("set_flag".into(), vec![Box::new(Expr::Val(Val::Int(1)))])),
                span: None,
            }),
            Box::new(Stmt::Assign {
                name: "flag".into(),
                value: Box::new(Expr::Call("set_flag".into(), vec![Box::new(Expr::Val(Val::Int(41)))])),
                span: None,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("flag".into()))),
            }),
        ],
    };

    let fun = Compiler::new().compile_stmt(&program);
    let mut vm = Vm::new();
    let mut env = VmContext::new();
    env.define("set_flag", Val::RustFunction(set_flag));

    let out = vm.exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::Int(41));
    assert_eq!(env.get("flag").cloned(), Some(Val::Int(41)));
}

#[test]
fn vm_trait_impl_executes() {
    let source = r#"
        trait TestTrait {
            fn test_method(self) -> Int;
        }

        struct TestStruct { value: Int }

        impl TestTrait for TestStruct {
            fn test_method(self) -> Int {
                return self.value * 2;
            }
        }

        let obj = TestStruct { value: 21 };
        return obj.test_method();
    "#;

    let tokens = Tokenizer::tokenize(source).expect("tokenize source");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");

    let compiled = vm::compile_program(&program);

    let mut env = VmContext::new().with_type_checker(Some(TypeChecker::new_strict()));

    let mut vm = Vm::new();
    let result = vm.exec_with(&compiled, &mut env, None).expect("vm execution");

    assert_eq!(result, Val::Int(42));
}
