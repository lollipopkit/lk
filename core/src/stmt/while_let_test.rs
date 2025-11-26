#[cfg(test)]
mod tests {
    use crate::{
        stmt::{StmtParser, run_program},
        token::Tokenizer,
        val::{Val, methods},
        vm::VmContext,
    };
    use std::sync::{Arc, Once};

    // Simple list push function for testing
    fn list_push(args: &[Val], _ctx: &mut VmContext) -> anyhow::Result<Val> {
        if args.len() != 2 {
            return Err(anyhow::anyhow!("push() takes exactly 2 arguments"));
        }

        match &args[0] {
            Val::List(list) => {
                let mut new_list: Vec<Val> = (*list).to_vec();
                new_list.push(args[1].clone());
                Ok(Val::List(Arc::from(new_list)))
            }
            _ => Err(anyhow::anyhow!("push() first argument must be a list")),
        }
    }

    fn parse_and_execute_stmt(stmt_code: &str, seed: &Val) -> Result<Val, anyhow::Error> {
        // Register list methods for testing
        static REGISTER: Once = Once::new();
        REGISTER.call_once(|| {
            methods::register_method("List", "push", list_push);
        });

        let tokens = Tokenizer::tokenize(stmt_code)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        // Populate a VM context from the seed map before running the program
        let mut env = VmContext::new();
        if let Val::Map(m) = seed {
            for (k, v) in m.iter() {
                env.define(k.to_string(), v.clone());
            }
        }
        run_program(&program, &mut env)
    }

    #[test]
    fn test_while_let_list_destructuring() {
        let seed: Val = [(
            "data".to_string(),
            Val::List(Arc::from(vec![Val::Int(1), Val::Int(2), Val::Int(3)])),
        )]
        .into_iter()
        .collect::<std::collections::HashMap<String, Val>>()
        .into();

        let result = parse_and_execute_stmt(
            r#"
            let result = [];
            let x = data;
            while let [first, ..rest] = x {
                result.push(first);
                x = rest;
            }
            return result;
            "#,
            &seed,
        )
        .unwrap();

        assert_eq!(
            result,
            Val::List(Arc::from(vec![Val::Int(1), Val::Int(2), Val::Int(3)]))
        );
    }

    #[test]
    fn test_while_let_variable_binding() {
        let seed: Val = [("counter".to_string(), Val::Int(5))]
            .into_iter()
            .collect::<std::collections::HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt(
            r#"
            let count = 0;
            let x = counter;
            while let val = x {
                if val <= 0 { break; }
                count = count + 1;
                x = val - 1;
            }
            return count;
            "#,
            &seed,
        )
        .unwrap();

        assert_eq!(result, Val::Int(5));
    }

    #[test]
    fn test_while_let_map_destructuring() {
        let seed: Val = [(
            "items".to_string(),
            Val::List(Arc::from(vec![
                ([
                    ("name".to_string(), Val::Str("Alice".into())),
                    ("value".to_string(), Val::Int(10)),
                ]
                .into_iter()
                .collect::<std::collections::HashMap<String, Val>>()
                .into()),
                ([
                    ("name".to_string(), Val::Str("Bob".into())),
                    ("value".to_string(), Val::Int(20)),
                ]
                .into_iter()
                .collect::<std::collections::HashMap<String, Val>>()
                .into()),
            ])),
        )]
        .into_iter()
        .collect::<std::collections::HashMap<String, Val>>()
        .into();

        let result = parse_and_execute_stmt(
            r#"
            let names = [];
            let list = items;
            while let [{"name": name}] = list {
                names.push(name);
                if list.len() == 1 { break; }
                list = [list[1]];
            }
            return names;
            "#,
            &seed,
        )
        .unwrap();

        assert_eq!(
            result,
            Val::List(Arc::from(vec![Val::Str("Alice".into()), Val::Str("Bob".into())]))
        );
    }

    #[test]
    fn test_while_let_wildcard() {
        let seed: Val = [(
            "data".to_string(),
            Val::List(Arc::from(vec![Val::Int(1), Val::Int(2), Val::Int(3)])),
        )]
        .into_iter()
        .collect::<std::collections::HashMap<String, Val>>()
        .into();

        let result = parse_and_execute_stmt(
            r#"
            let count = 0;
            let x = data;
            while let [_] = x {
                count = count + 1;
                if x.len() == 1 { break; }
                x = [x[1]];
            }
            return count;
            "#,
            &seed,
        )
        .unwrap();

        assert_eq!(result, Val::Int(3));
    }

    #[test]
    fn test_while_let_no_match() {
        let seed: Val = [("data".to_string(), Val::Nil)]
            .into_iter()
            .collect::<std::collections::HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt(
            r#"
            let count = 0;
            while let x = data {
                count = count + 1;
                // This should never execute since Nil doesn't match variable pattern
                break;
            }
            return count;
            "#,
            &seed,
        )
        .unwrap();

        assert_eq!(result, Val::Int(0));
    }

    #[test]
    fn test_while_let_break_statement() {
        let seed: Val = [(
            "data".to_string(),
            Val::List(Arc::from(vec![Val::Int(1), Val::Int(2), Val::Int(3)])),
        )]
        .into_iter()
        .collect::<std::collections::HashMap<String, Val>>()
        .into();

        let result = parse_and_execute_stmt(
            r#"
            let result = [];
            let x = data;
            while let [first, ..rest] = x {
                result.push(first);
                if first == 2 { break; }
                x = rest;
            }
            return result;
            "#,
            &seed,
        )
        .unwrap();

        assert_eq!(result, Val::List(Arc::from(vec![Val::Int(1), Val::Int(2)])));
    }

    #[test]
    fn test_while_let_continue_statement() {
        let seed: Val = [(
            "data".to_string(),
            Val::List(Arc::from(vec![Val::Int(1), Val::Int(2), Val::Int(3), Val::Int(4)])),
        )]
        .into_iter()
        .collect::<std::collections::HashMap<String, Val>>()
        .into();

        let result = parse_and_execute_stmt(
            r#"
            let result = [];
            let x = data;
            while let [first, ..rest] = x {
                if first == 2 {
                    x = rest;
                    continue;
                }
                result.push(first);
                x = rest;
            }
            return result;
            "#,
            &seed,
        )
        .unwrap();

        assert_eq!(
            result,
            Val::List(Arc::from(vec![Val::Int(1), Val::Int(3), Val::Int(4)]))
        );
    }

    #[test]
    fn test_while_let_nested_patterns() {
        let seed: Val = [(
            "data".to_string(),
            Val::List(Arc::from(vec![
                ([
                    ("first".to_string(), Val::Int(1)),
                    ("second".to_string(), Val::Str("test".into())),
                ]
                .into_iter()
                .collect::<std::collections::HashMap<String, Val>>()
                .into()),
            ])),
        )]
        .into_iter()
        .collect::<std::collections::HashMap<String, Val>>()
        .into();

        let result = parse_and_execute_stmt(
            r#"
            let result = [];
            let x = data;
            while let [{"first": first}] = x {
                result.push(first);
                x = [];
            }
            return result;
            "#,
            &seed,
        )
        .unwrap();

        assert_eq!(result, Val::List(Arc::from(vec![Val::Int(1)])));
    }

    #[test]
    fn test_while_let_range_pattern() {
        let seed: Val = [(
            "values".to_string(),
            Val::List(Arc::from(vec![Val::Int(5), Val::Int(15), Val::Int(25)])),
        )]
        .into_iter()
        .collect::<std::collections::HashMap<String, Val>>()
        .into();

        let result = parse_and_execute_stmt(
            r#"
            let result = [];
            let x = values;
            while let val if val >= 10 = x[0] {
                result.push(val);
                if x.len() == 1 { break; }
                x = [x[1]];
            }
            return result;
            "#,
            &seed,
        )
        .unwrap();

        assert_eq!(result, Val::List(Arc::from(vec![Val::Int(15), Val::Int(25)])));
    }

    #[test]
    fn test_while_let_variable_scoping() {
        let seed: Val = [("data".to_string(), Val::List(Arc::from(vec![Val::Int(1), Val::Int(2)])))]
            .into_iter()
            .collect::<std::collections::HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt(
            r#"
            let outer_var = "outer";
            let result = [];
            let x = data;
            while let [first, ..rest] = x {
                result.push(first);
                result.push(outer_var);
                x = rest;
            }
            return result;
            "#,
            &seed,
        )
        .unwrap();

        assert_eq!(
            result,
            Val::List(Arc::from(vec![
                Val::Int(1),
                Val::Str("outer".into()),
                Val::Int(2),
                Val::Str("outer".into())
            ]))
        );
    }

    #[test]
    fn test_while_let_empty_list_termination() {
        let ctx: Val = [(
            "data".to_string(),
            Val::List(Arc::from(vec![Val::Int(1), Val::Int(2), Val::Int(3)])),
        )]
        .into_iter()
        .collect::<std::collections::HashMap<String, Val>>()
        .into();

        let result = parse_and_execute_stmt(
            r#"
            let count = 0;
            let x = data;
            while let [first, ..rest] = x {
                count = count + 1;
                x = rest;
            }
            return count;
            "#,
            &ctx,
        )
        .unwrap();

        assert_eq!(result, Val::Int(3));
    }

    #[test]
    fn test_while_let_single_element_destructuring() {
        let seed: Val = [("data".to_string(), Val::List(Arc::from(vec![Val::Int(42)])))]
            .into_iter()
            .collect::<std::collections::HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt(
            r#"
            let result = [];
            let x = data;
            while let [first] = x {
                result.push(first);
                x = [];
            }
            return result;
            "#,
            &seed,
        )
        .unwrap();

        assert_eq!(result, Val::List(Arc::from(vec![Val::Int(42)])));
    }
}
