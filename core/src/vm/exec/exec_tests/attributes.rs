use super::*;

#[test]
fn execute_source_treats_attributed_function_as_normal_item() {
    let result = execute_source(
        r#"
        #[test_attr]
        fn answer() {
            return 42;
        }

        return answer();
        "#,
    )
    .expect("execute attributed function");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute_source_treats_attributed_struct_as_normal_item() {
    let result = execute_source(
        r#"
        #[repr("lk")]
        struct User {
            id: Int,
        }

        let user = User { id: 7 };
        return user.id;
        "#,
    )
    .expect("execute attributed struct");

    assert_eq!(result.returns, vec![RuntimeVal::Int(7)]);
}
