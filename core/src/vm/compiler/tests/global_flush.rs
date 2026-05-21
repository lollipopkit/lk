use super::parse_compile_and_run;
use crate::{
    stmt::{Program, stmt_parser::StmtParser},
    token::Tokenizer,
    val::Val,
    vm::{Op, Vm, compile_program, context::VmContext},
};

fn define_global_names(function: &crate::vm::Function) -> Vec<String> {
    function
        .code
        .iter()
        .filter_map(|op| {
            let Op::DefineGlobal(name_idx, _) = op else {
                return None;
            };
            function
                .consts
                .get(*name_idx as usize)
                .and_then(Val::as_str)
                .map(str::to_string)
        })
        .collect()
}

fn parse_compile_and_run_with_ctx(
    source: &str,
    setup: impl FnOnce(&mut VmContext),
) -> (crate::vm::Function, VmContext, anyhow::Result<Val>) {
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    let program: Program = parser.parse_program().expect("parse program");
    let function = compile_program(&program);
    let mut ctx = VmContext::new();
    setup(&mut ctx);
    let mut vm = Vm::new();
    let result = vm.exec_with(&function, &mut ctx, None);
    (function, ctx, result)
}

#[test]
fn nested_range_loop_defers_global_flush_to_outer_loop_exit() {
    let source = r#"
        let total = 0;
        for r in 1..=3 {
            for i in 1..=4 {
                total += i;
            }
        }
        return total;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(30));
    let total_syncs = define_global_names(&function)
        .iter()
        .filter(|name| name.as_str() == "total")
        .count();
    assert_eq!(
        total_syncs, 2,
        "total should sync once for initialization and once after the outer loop, not after every nested loop: {:?}",
        function.code
    );
}

#[test]
fn pending_nested_loop_global_flush_is_visible_to_context_observing_call() {
    fn read_total(_args: &[Val], ctx: &mut VmContext) -> anyhow::Result<Val> {
        Ok(ctx.get("total").cloned().unwrap_or(Val::Nil))
    }

    let source = r#"
        let total = 0;
        for r in 1..=1 {
            for i in 1..=2 {
                total += i;
            }
            return read_total();
        }
        return -1;
    "#;
    let (_function, _ctx, result) = parse_compile_and_run_with_ctx(source, |ctx| {
        ctx.define("read_total", Val::RustFunction(read_total));
    });

    assert_eq!(result.expect("vm exec"), Val::Int(3));
}
