use super::*;
use crate::stmt::stmt_parser::StmtParser;
use crate::token::Tokenizer;
use crate::vm::compile_program;

#[test]
fn tiny_call_plan_handles_add_then_mod_return() {
    let source = r#"
        fn mix(a, b) {
            return (a + b) % 1000000007;
        }
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");
    let function = compile_program(&program);
    let proto_function = function.protos[0].func.as_ref().expect("compiled proto");
    let plan = TinyCallPlan::analyze(proto_function).expect("tiny plan");

    let result = plan
        .try_eval(&[Val::Int(10), Val::Int(5)], Some(&ClosureCapture::empty()))
        .expect("tiny eval");

    assert_eq!(result, Val::Int(15));
}

#[test]
fn tiny_call_plan_handles_generic_arithmetic_return() {
    let source = r#"
        fn mix(a, b, c) {
            return (((a * 3) + (b % 11)) + (c * 5)) % 1000000007;
        }
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");
    let function = compile_program(&program);
    let proto_function = function.protos[0].func.as_ref().expect("compiled proto");
    let plan = TinyCallPlan::analyze(proto_function).expect("tiny plan");

    let result = plan
        .try_eval(
            &[Val::Int(7), Val::Int(23), Val::Int(5)],
            Some(&ClosureCapture::empty()),
        )
        .expect("tiny eval");

    assert_eq!(result, Val::Int(((7 * 3) + (23 % 11)) + (5 * 5)));
}

#[test]
fn tiny_call_plan_handles_euclid_gcd_loop() {
    let source = r#"
        fn gcd(a0, b0) {
            let a = a0;
            let b = b0;
            while (b != 0) {
                let t = a % b;
                a = b;
                b = t;
            }
            return a;
        }
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");
    let function = compile_program(&program);
    let proto_function = function.protos[0].func.as_ref().expect("compiled proto");
    let plan = TinyCallPlan::analyze(proto_function).expect("tiny plan");

    assert!(matches!(plan, TinyCallPlan::EuclidGcd { lhs: 0, rhs: 1 }));
    assert_eq!(
        plan.try_eval(&[Val::Int(312), Val::Int(210)], Some(&ClosureCapture::empty())),
        Some(Val::Int(6))
    );
    assert_eq!(
        plan.try_eval(&[Val::Int(312), Val::Int(0)], Some(&ClosureCapture::empty())),
        Some(Val::Int(312))
    );
    assert_eq!(
        plan.try_eval(&[Val::Float(312.0), Val::Int(210)], Some(&ClosureCapture::empty())),
        None
    );
}

#[test]
fn tiny_call_plan_handles_implicit_binary_search_loop() {
    let source = r#"
        fn binary_search_implicit(target, n) {
            let lo = 0;
            let hi = n - 1;
            while (lo <= hi) {
                let mid = math.floor((lo + hi) / 2);
                let value = mid * 2;
                if value == target {
                    return mid;
                }
                if value < target {
                    lo = mid + 1;
                } else {
                    hi = mid - 1;
                }
            }
            return -1;
        }
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");
    let function = compile_program(&program);
    let proto_function = function.protos[0].func.as_ref().expect("compiled proto");
    let plan = TinyCallPlan::analyze(proto_function).expect("tiny plan");

    assert!(matches!(
        plan,
        TinyCallPlan::BinarySearchImplicit {
            target: 0,
            len: 1,
            scale: 2
        }
    ));
    assert_eq!(
        plan.try_eval(&[Val::Int(120), Val::Int(200)], Some(&ClosureCapture::empty())),
        Some(Val::Int(60))
    );
    assert_eq!(
        plan.try_eval(&[Val::Int(121), Val::Int(200)], Some(&ClosureCapture::empty())),
        Some(Val::Int(-1))
    );
    assert_eq!(
        plan.try_eval(&[Val::Int(0), Val::Int(0)], Some(&ClosureCapture::empty())),
        Some(Val::Int(-1))
    );
    assert_eq!(
        plan.try_eval(&[Val::Str("120".into()), Val::Int(200)], Some(&ClosureCapture::empty())),
        None
    );
}

#[test]
fn tiny_call_plan_handles_is_prime_trial_division() {
    let source = r#"
        fn is_prime(n) {
            if n < 2 { return false; }
            if n == 2 { return true; }
            if ((n % 2) == 0) { return false; }
            let d = 3;
            while ((d * d) <= n) {
                if ((n % d) == 0) { return false; }
                d += 2;
            }
            return true;
        }
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");
    let function = compile_program(&program);
    let proto_function = function.protos[0].func.as_ref().expect("compiled proto");
    let plan = TinyCallPlan::analyze(proto_function).expect("tiny plan");

    assert!(matches!(plan, TinyCallPlan::IsPrimeTrialDivision { input: 0 }));
    assert_eq!(
        plan.try_eval(&[Val::Int(1)], Some(&ClosureCapture::empty())),
        Some(Val::Bool(false))
    );
    assert_eq!(
        plan.try_eval(&[Val::Int(2)], Some(&ClosureCapture::empty())),
        Some(Val::Bool(true))
    );
    assert_eq!(
        plan.try_eval(&[Val::Int(99)], Some(&ClosureCapture::empty())),
        Some(Val::Bool(false))
    );
    assert_eq!(
        plan.try_eval(&[Val::Int(101)], Some(&ClosureCapture::empty())),
        Some(Val::Bool(true))
    );
    assert_eq!(
        plan.try_eval(&[Val::Str("101".into())], Some(&ClosureCapture::empty())),
        None
    );
}

fn native_noop(_: &[Val], _: &mut crate::vm::VmContext) -> anyhow::Result<Val> {
    Ok(Val::Nil)
}

fn native_named_noop(
    _: crate::val::NativeArgs<'_>,
    _: &[(String, Val)],
    _: &mut crate::vm::VmContext,
) -> anyhow::Result<Val> {
    Ok(Val::Nil)
}

#[test]
fn call_ic_native_entry_carries_return_layout() {
    let layout = CallReturnLayout::new(3, 2);
    let entry = CallIc::Rust(native_noop, 1, layout);
    let cloned = entry.clone();

    let CallIc::Rust(_, argc, ret) = cloned else {
        panic!("expected rust call ic");
    };
    assert_eq!(argc, 1);
    assert!(ret.matches(3, 2));
    assert!(!ret.matches(3, 1));
    assert!(!ret.matches(4, 2));

    let fast_named = CallIc::RustFastNamed(native_named_noop, 2, layout).clone();
    let CallIc::RustFastNamed(_, argc, ret) = fast_named else {
        panic!("expected fast named call ic");
    };
    assert_eq!(argc, 2);
    assert!(ret.matches(3, 2));
}
