//! Tests for concurrency features

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::expr::Expr;
    use crate::module::ModuleRegistry;
    use crate::rt::{self, with_runtime};
    use crate::stmt::import::ModuleResolver;
    use crate::val::{ChannelValue, TaskValue, Type, Val};
    use crate::vm::VmContext;
    use anyhow::{Result, anyhow};
    use once_cell::sync::Lazy;
    use std::sync::Mutex;

    static RUNTIME_TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    fn test_env() -> VmContext {
        fn spawn_fn(args: &[Val], ctx: &mut VmContext) -> anyhow::Result<Val> {
            if args.len() != 1 {
                return Err(anyhow!("spawn() expects exactly 1 argument (closure/function)"));
            }

            let fut: core::pin::Pin<Box<dyn core::future::Future<Output = anyhow::Result<Val>> + Send>> = match &args[0]
            {
                Val::Closure(_) => {
                    let func = args[0].clone();
                    let mut ctx_snapshot = ctx.clone();
                    Box::pin(async move { func.call(&[], &mut ctx_snapshot) })
                }
                Val::RustFunction(fptr) => {
                    let func = *fptr;
                    let mut ctx_snapshot = ctx.clone();
                    Box::pin(async move { func(&[], &mut ctx_snapshot) })
                }
                other => {
                    return Err(anyhow!(
                        "spawn() expects a function or closure, got {}",
                        other.type_name()
                    ));
                }
            };

            match with_runtime(|runtime| runtime.spawn(fut)) {
                Ok(task_id) => Ok(Val::Task(Arc::new(TaskValue {
                    id: task_id,
                    value: None,
                }))),
                Err(e) => Err(anyhow!("Failed to spawn task: {}", e)),
            }
        }

        fn chan_fn(args: &[Val], _ctx: &mut VmContext) -> anyhow::Result<Val> {
            if args.is_empty() || args.len() > 2 {
                return Err(anyhow!("chan() expects 1 or 2 arguments: capacity[, type_str]"));
            }
            let capacity = match &args[0] {
                Val::Int(n) => *n,
                Val::Float(f) => *f as i64,
                other => return Err(anyhow!("chan() capacity must be numeric, got {}", other.type_name())),
            };
            let inner_type = if args.len() >= 2 {
                match &args[1] {
                    Val::Str(s) => Type::parse(s.as_ref()).unwrap_or(Type::Nil),
                    Val::Nil => Type::Nil,
                    other => {
                        return Err(anyhow!(
                            "chan() type must be a string when provided, got {}",
                            other.type_name()
                        ));
                    }
                }
            } else {
                Type::Nil
            };

            let cap_opt = if capacity <= 0 { None } else { Some(capacity as usize) };
            let stored_capacity = if capacity <= 0 { None } else { Some(capacity) };
            match with_runtime(|runtime| runtime.create_channel(cap_opt)) {
                Ok(ch_id) => Ok(Val::Channel(Arc::new(ChannelValue {
                    id: ch_id,
                    capacity: stored_capacity,
                    inner_type,
                }))),
                Err(e) => Err(anyhow!("Failed to create channel: {}", e)),
            }
        }

        fn send_fn(args: &[Val], _ctx: &mut VmContext) -> anyhow::Result<Val> {
            if args.len() != 2 {
                return Err(anyhow!("send() expects exactly 2 arguments"));
            }
            let channel_id = match &args[0] {
                Val::Channel(ch) => ch.id,
                other => {
                    return Err(anyhow!(
                        "send() expects Channel as first argument, got {}",
                        other.type_name()
                    ));
                }
            };
            match with_runtime(|runtime| runtime.block_on(runtime.send_async(channel_id, args[1].clone()))) {
                Ok(sent) => Ok(Val::Bool(sent)),
                Err(e) => Err(anyhow!("Send operation failed: {}", e)),
            }
        }

        fn recv_fn(args: &[Val], _ctx: &mut VmContext) -> anyhow::Result<Val> {
            if args.len() != 1 {
                return Err(anyhow!("recv() expects exactly 1 argument"));
            }
            let channel_id = match &args[0] {
                Val::Channel(ch) => ch.id,
                other => {
                    return Err(anyhow!(
                        "recv() expects Channel as first argument, got {}",
                        other.type_name()
                    ));
                }
            };
            match with_runtime(|runtime| runtime.block_on(runtime.recv_async(channel_id))) {
                Ok((ok, value)) => Ok(Val::List(vec![Val::Bool(ok), value].into())),
                Err(e) => Err(anyhow!("Receive operation failed: {}", e)),
            }
        }

        let mut registry = ModuleRegistry::new();
        registry.register_builtin("spawn", Val::RustFunction(spawn_fn));
        registry.register_builtin("chan", Val::RustFunction(chan_fn));
        registry.register_builtin("send", Val::RustFunction(send_fn));
        registry.register_builtin("recv", Val::RustFunction(recv_fn));

        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        VmContext::new().with_resolver(resolver)
    }

    #[tokio::test]
    async fn test_spawn_expression_parsing() -> Result<()> {
        let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
        let expr = Expr::parse_cached_arc("spawn(|| 42)")?;

        crate::rt::init_runtime()?;
        let mut env = test_env();

        let result = expr.eval_with_ctx(&mut env)?;

        assert!(matches!(result, Val::Task(_)));

        rt::shutdown_runtime();

        Ok(())
    }

    #[tokio::test]
    async fn test_channel_creation() -> Result<()> {
        let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
        let expr = Expr::parse_cached_arc("chan(10)")?;

        crate::rt::init_runtime()?;
        let mut env = test_env();

        let result = expr.eval_with_ctx(&mut env)?;

        assert!(matches!(result, Val::Channel(channel) if channel.capacity == Some(10)));

        rt::shutdown_runtime();

        Ok(())
    }

    #[test]
    fn test_concurrency_ast_parsing() -> Result<()> {
        let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
        let spawn_expr = Expr::parse_cached_arc("spawn(|| 42)")?;
        if let Expr::CallExpr(callee, _) = &*spawn_expr {
            if let Expr::Var(name) = callee.as_ref() {
                assert_eq!(name, "spawn");
            } else {
                panic!("expected spawn to be a function call on identifier");
            }
        } else {
            panic!("expected spawn expression to parse as a call expression");
        }

        let chan_expr = Expr::parse_cached_arc("chan(5)")?;
        if let Expr::CallExpr(callee, _) = &*chan_expr {
            if let Expr::Var(name) = callee.as_ref() {
                assert_eq!(name, "chan");
            } else {
                panic!("expected chan to be a function call on identifier");
            }
        } else {
            panic!("expected chan expression to parse as a call expression");
        }

        Ok(())
    }

    #[test]
    fn test_select_parsing() -> Result<()> {
        let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
        let select_code = r#"select {
            case value <= recv(ch1) => value;
            case _ <= send(ch2, 42) => "sent";
            default => "timeout";
        }"#;

        let expr = Expr::parse_cached_arc(select_code)?;
        if let Expr::Select { cases, default_case } = &*expr {
            assert_eq!(cases.len(), 2);
            assert!(default_case.is_some());
        } else {
            panic!("Expected Select expression");
        }

        Ok(())
    }

    #[test]
    fn test_runtime_send_recv_select_and_cancel() -> Result<()> {
        use crate::rt::{self, SelectOperation};

        let _guard = RUNTIME_TEST_LOCK.lock().unwrap();

        rt::shutdown_runtime();
        rt::init_runtime()?;

        let channel_id = rt::with_runtime(|runtime| runtime.create_channel(Some(1)))?;

        assert!(rt::with_runtime(|runtime| runtime.try_send(channel_id, Val::Int(7)))?);
        let recv = rt::with_runtime(|runtime| runtime.try_recv(channel_id))?.expect("value available");
        assert_eq!(recv.1, Val::Int(7));
        assert!(recv.0);

        let send_ok = rt::with_runtime(|runtime| runtime.block_on(runtime.send_async(channel_id, Val::Int(42))))?;
        assert!(send_ok);

        let mut sel = SelectOperation::new();
        sel.add_recv(0, channel_id);
        let result = rt::with_runtime(|runtime| runtime.block_on(sel.execute(runtime, false)))?;
        assert_eq!(result.case_index, Some(0));
        let Some((ok, val)) = result.recv_payload else {
            panic!("expected payload")
        };
        assert!(ok);
        assert_eq!(val, Val::Int(42));

        let task_id = rt::with_runtime(|runtime| runtime.spawn(async { Ok(Val::Int(99)) }))?;
        rt::with_runtime(|runtime| runtime.cancel_task(task_id))?;

        rt::shutdown_runtime();

        Ok(())
    }
}
