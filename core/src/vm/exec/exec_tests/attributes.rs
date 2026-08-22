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

/// A missing method names the struct the user wrote, not the heap kind.
///
/// `RuntimeVal::type_name_in` is the function the migration guard points every
/// message at so that none of them prints `Object` — and it printed `Object` for
/// every struct instance, because it returned `&'static str` and a struct's name
/// is not static. Inside `call_trait_method_runtime` the correct name was two
/// lines below the message, already computed for dispatch.
#[test]
fn a_missing_method_on_a_struct_names_the_struct() {
    let error = execute_source(
        r#"
        struct Point { x: Int }
        let p = Point { x: 1 };
        return p.nonexistent();
        "#,
    )
    .expect_err("a struct has no such method");

    let message = format!("{error:#}");
    assert!(message.contains("Point has no method 'nonexistent'"), "{message}");
}
