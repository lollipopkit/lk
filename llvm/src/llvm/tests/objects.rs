use crate::{
    llvm::{LlvmBackendOptions, compile_module_artifact_to_llvm, compile_program_to_llvm},
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{Compiler, ModuleArtifact},
};

#[test]
fn llvm_backend_lowers_source_static_object_get_index_without_artifact_shell() {
    let source = r#"
        struct User { name: String, score: Int }
        let user = User { name: "Ada", score: 42 };
        return user.score;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_source_static_object_optional_access_without_artifact_shell() {
    let source = r#"
        struct User { name: String, score: Int }
        let user = User { name: "Ada", score: 42 };
        return user?.score;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_source_nil_optional_access_without_artifact_shell() {
    let source = r#"
        let user = nil;
        return user?.score;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(!artifact.module.ir.contains("ptr @lk_nil_text"));
}

#[test]
fn llvm_backend_lowers_source_static_list_set_index_without_artifact_shell() {
    let source = r#"
        let values = [1, 2, 3];
        values[1] = 42;
        return values.1;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_text_backend_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_source_static_list_push_without_artifact_shell() {
    let source = r#"
        let values = [];
        values.push(40);
        values.push(2);
        return values[0] + values[1];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_text_backend_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("store i64 40"));
    assert!(artifact.module.ir.contains("store i64 2"));
}

#[test]
fn llvm_backend_lowers_source_static_map_set_index_without_artifact_shell() {
    let source = r#"
        let values = {"a": 1};
        values["b"] = 42;
        return values.b;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_text_backend_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_source_static_object_set_index_without_artifact_shell() {
    let source = r#"
        struct User { score: Int }
        let user = User { score: 1 };
        user.score = 42;
        return user.score;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_source_if_let_static_list_destructuring_without_artifact_shell() {
    let source = r#"
        if let [head, ..tail] = [40, 1, 2] {
            return head + tail.1;
        }
        return 0;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("add i64"));
}

#[test]
fn llvm_backend_lowers_source_if_let_static_map_destructuring_without_artifact_shell() {
    let source = r#"
        let data = {"a": 40, "b": 2};
        if let {"a": a, ..rest} = data {
            return a + rest.b;
        }
        return 0;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("add i64"));
}

#[test]
fn llvm_backend_lowers_static_new_object_return_without_artifact_shell() {
    let source = r#"
        struct User { name: String, score: Int }
        return User { name: "Ada", score: 42 };
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_new_object_"));
    assert!(artifact.module.ir.contains("c\"<User {name: Ada, score: 42}>\\00\""));
}

#[test]
fn llvm_backend_lowers_static_object_list_return_display_without_artifact_shell() {
    let source = r#"
        struct User { name: String, age: Int }
        return [User { name: "a", age: 1 }, User { name: "b", age: 2 }];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_static_arg_list_return"));
    assert!(
        artifact
            .module
            .ir
            .contains("c\"[<User {age: 1, name: a}>, <User {age: 2, name: b}>]\\00\"")
    );
}

#[test]
fn llvm_backend_lowers_static_callable_list_return_display_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return [|x| x + 1];").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_static_arg_list_return"));
    assert!(artifact.module.ir.contains("c\"[<fn #1(0 captures)>]\\00\""));
}

#[test]
fn llvm_backend_lowers_static_object_arg_list_len_without_artifact_shell() {
    let source = r#"
        struct User { name: String }
        let values = [User { name: "a" }];
        return values.len();
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_static_callable_arg_list_len_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("let values = [|x| x + 1]; return values.len();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_static_callable_arg_list_contains_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("let f = |x| x + 1; let values = [f]; return f in values;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
}

#[test]
fn llvm_backend_lowers_static_object_arg_list_contains_without_artifact_shell() {
    let source = r#"
        struct User { name: String }
        let user = User { name: "a" };
        let values = [user];
        return user in values;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
}

#[test]
fn llvm_backend_lowers_static_object_arg_list_first_without_artifact_shell() {
    let source = r#"
        struct User { name: String }
        let user = User { name: "a" };
        return [user].first();
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("c\"<User {name: a}>\\00\""));
}

#[test]
fn llvm_backend_lowers_static_callable_arg_list_first_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("let f = |x| x + 1; return [f].first();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("c\"<fn #1(0 captures)>\\00\""));
}

#[test]
fn llvm_backend_lowers_static_object_arg_list_get_without_artifact_shell() {
    let source = r#"
        struct User { name: String }
        let user = User { name: "a" };
        return [user].get(0);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("c\"<User {name: a}>\\00\""));
}

#[test]
fn llvm_backend_lowers_static_object_arg_list_concat_without_artifact_shell() {
    let source = r#"
        struct User { name: String }
        let a = User { name: "a" };
        let b = User { name: "b" };
        return [a].concat([b]);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(
        artifact
            .module
            .ir
            .contains("c\"[<User {name: a}>, <User {name: b}>]\\00\"")
    );
}

#[test]
fn llvm_backend_lowers_static_object_arg_list_take_skip_without_artifact_shell() {
    let source = r#"
        struct User { name: String }
        let a = User { name: "a" };
        let b = User { name: "b" };
        return [a, b].skip(1).take(1);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("c\"[<User {name: b}>]\\00\""));
}

#[test]
fn llvm_backend_lowers_static_object_list_module_mutators_without_artifact_shell() {
    let source = r#"
        use list;
        struct User { name: String }
        let a = User { name: "a" };
        let b = User { name: "b" };
        return [
            list.reverse([a, b]),
            list.pop([a, b]),
            list.push([a], b),
            list.slice([a, b], 0, 1),
            list.insert([a], 0, b),
            list.remove_at([a, b], 0),
            list.set([a, b], 1, a),
            list.index_of([a, b], b)
        ];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "list"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(
        artifact.module.ir.contains(
            "[[<User {name: b}>, <User {name: a}>], <User {name: b}>, [<User {name: a}>, <User {name: b}>], [<User {name: a}>], [<User {name: b}>, <User {name: a}>], [[<User {name: b}>], <User {name: a}>], [[<User {name: a}>, <User {name: a}>], <User {name: b}>], -1]"
        )
    );
}

#[test]
fn llvm_backend_lowers_static_object_map_module_values_without_artifact_shell() {
    let source = r#"
        use map;
        struct User { name: String }
        let a = User { name: "a" };
        let b = User { name: "b" };
        let m = map.set({}, "a", a);
        let m2 = map.set(m, "b", b);
        return [map.has(m2, "a"), map.get(m2, "a"), map.values(m2), map.delete(m2, "a")];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains(
        "[true, <User {name: a}>, [<User {name: a}>, <User {name: b}>], [{b: <User {name: b}>}, <User {name: a}>]]"
    ));
}

#[test]
fn llvm_backend_lowers_static_callable_map_module_values_without_artifact_shell() {
    let source = r#"
        use map;
        let f = |x| x + 1;
        let m = map.set({}, "f", f);
        return [map.has(m, "f"), map.get(m, "f"), map.values(m), map.delete(m, "f")];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(
        artifact
            .module
            .ir
            .contains("[true, <fn #1(0 captures)>, [<fn #1(0 captures)>], [{}, <fn #1(0 captures)>]]")
    );
}

#[test]
fn llvm_backend_lowers_static_display_map_more_operations_without_artifact_shell() {
    let source = r#"
        use map;
        struct User { name: String }
        let a = User { name: "a" };
        let b = User { name: "b" };
        let f = |x| x + 1;
        let users = map.set(map.set({}, "a", a), "b", b);
        let funcs = map.set({}, "f", f);
        return [
            map.len(users),
            map.keys(users),
            users["a"],
            "a" in users,
            users,
            funcs,
            map.keys(funcs),
            funcs["f"],
        ];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains(
        "[2, [a, b], <User {name: a}>, true, {a: <User {name: a}>, b: <User {name: b}>}, {f: <fn #1(0 captures)>}, [f], <fn #1(0 captures)>]"
    ));
}

#[test]
fn llvm_backend_lowers_static_display_map_rest_without_artifact_shell() {
    let source = r#"
        use map;
        struct User { name: String }
        let a = User { name: "a" };
        let b = User { name: "b" };
        let users = map.set(map.set({}, "a", a), "b", b);
        if let {"a": first, ..rest} = users {
            return [first, rest];
        }
        return nil;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("[<User {name: a}>, {b: <User {name: b}>}]"));
}

#[test]
fn llvm_backend_lowers_static_display_map_values_methods_without_artifact_shell() {
    let source = r#"
        use map;
        struct User { name: String }
        let a = User { name: "a" };
        let b = User { name: "b" };
        let users = map.set(map.set({}, "a", a), "b", b);
        let values = map.values(users);
        return [values.len(), values.first(), values.last(), values.get(1)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(
        artifact
            .module
            .ir
            .contains("[2, <User {name: a}>, <User {name: b}>, <User {name: b}>]")
    );
}

#[test]
fn llvm_backend_lowers_static_display_map_equality_without_artifact_shell() {
    let source = r#"
        use map;
        struct User { name: String }
        let a = User { name: "a" };
        let lhs = map.set({}, "a", a);
        let rhs = map.set({}, "a", a);
        return [lhs == rhs, lhs != rhs];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("[true, false]"));
}

#[test]
fn llvm_backend_lowers_static_display_map_iter_without_artifact_shell() {
    let source = r#"
        use map;
        struct User { name: String }
        let a = User { name: "a" };
        let b = User { name: "b" };
        let users = map.set(map.set({}, "a", a), "b", b);
        let out = [];
        for pair in users {
            out.push(pair);
        }
        return out;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(
        artifact
            .module
            .ir
            .contains("[[a, <User {name: a}>], [b, <User {name: b}>]]")
    );
}

#[test]
fn llvm_backend_lowers_same_static_object_equality_without_artifact_shell() {
    let source = r#"
        struct User { score: Int }
        let user = User { score: 42 };
        let alias = user;
        return user == alias;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_distinct_static_object_inequality_without_artifact_shell() {
    let source = r#"
        struct User { score: Int }
        return User { score: 42 } != User { score: 42 };
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}
