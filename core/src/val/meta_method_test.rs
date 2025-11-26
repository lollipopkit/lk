#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use anyhow::Result;
    use crate::{
        stmt::stmt_parser::StmtParser,
        token::Tokenizer,
        val::{methods, Val},
    };
    use crate::vm::Vm;
    use std::sync::Arc;

    fn custom_run(args: &[Val], _env: &crate::vm::VmContext) -> Result<Val> {
        // Expect receiver only
        assert!(args.len() >= 1);
        // Return a constant to assert dispatch
        Ok(Val::Int(123))
    }

    #[test]
    fn test_custom_object_method_dispatch() -> Result<()> {
        // Register a method for type "Custom"
        methods::register_method("Custom", "run", custom_run);

        // Program that calls c.run();
        let source = "return c.run();";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        // No pre-bound variables
        let ctx = HashMap::<String, Val>::new().into();

        // Prepare environment with variable c bound to a custom object
        let mut env = crate::vm::VmContext::default();
        let obj = Val::object("Custom", HashMap::new());
        env.define("c".to_string(), obj);

        let mut vm = Vm::new();
        let result = program.execute_with_vm(&mut vm, &mut env)?;
        assert_eq!(result, Val::Int(123));
        Ok(())
    }
}
