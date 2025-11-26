use crate::{
    val::{RustFunction, Val},
    vm::VmContext,
};
use anyhow::Result;

#[test]
fn test_rust_function_call() -> Result<()> {
    // Create a simple Rust function that adds two numbers
    let add_func: RustFunction = |args, _ctx| {
        if args.len() != 2 {
            return Err(anyhow::anyhow!("add() takes exactly 2 arguments"));
        }

        match (&args[0], &args[1]) {
            (Val::Int(a), Val::Int(b)) => Ok(Val::Int(a + b)),
            (Val::Int(a), Val::Float(b)) => Ok(Val::Float(*a as f64 + b)),
            (Val::Float(a), Val::Int(b)) => Ok(Val::Float(a + *b as f64)),
            (Val::Float(a), Val::Float(b)) => Ok(Val::Float(a + b)),
            _ => Err(anyhow::anyhow!("add() requires numeric arguments")),
        }
    };

    // Create a Rust function value
    let rust_func = Val::RustFunction(add_func);

    // Create environment and seed variables
    let mut env = VmContext::new();

    // Test calling the function
    let result = rust_func.call(&[Val::Int(5), Val::Int(3)], &mut env)?;
    assert_eq!(result, Val::Int(8));

    // Test with wrong number of arguments
    let result = rust_func.call(&[Val::Int(5)], &mut env);
    assert!(result.is_err());

    // Test with wrong argument types
    let result = rust_func.call(&[Val::Str("hello".into()), Val::Int(3)], &mut env);
    assert!(result.is_err());

    Ok(())
}

#[test]
fn test_call_non_function() -> Result<()> {
    let mut env = VmContext::new();

    // Try to call a non-function value
    let result = Val::Int(42).call(&[], &mut env);
    assert!(result.is_err());

    Ok(())
}
