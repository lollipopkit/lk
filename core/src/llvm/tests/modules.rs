use crate::{
    llvm::{LlvmBackendOptions, compile_module32_artifact_to_llvm},
    stmt::import::collect_program_imports,
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{Compiler32, Module32Artifact},
};

#[test]
fn llvm_backend_lowers_static_module_return_display_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return iter;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["iter"]).expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_static_module_return"));
    assert!(artifact.module.ir.contains("chain: <native fn chain(2 args)>"));
    assert!(artifact.module.ir.contains("range: <native fn range(...)>"));
    assert!(artifact.module.ir.contains("zip: <native fn zip(2 args)>"));
}

#[test]
fn llvm_backend_lowers_static_parse_result_method_call_with_recovered_key_without_shell() {
    let source = r#"
        import toml;
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        let toml_cfg = "[ssl]\nenabled = true\ncert = \"/etc/ssl/cert.pem\"\n";
        let t = toml.parse(toml_cfg);
        fn is_feature_enabled(cfg, feature) {
            if (!cfg.ssl.has(feature)) { return false; }
            return cfg.ssl[feature] == true;
        }
        assert(is_feature_enabled(t, "enabled"));
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(
        &program,
        Vec::new(),
        ["panic", "__lk_call_method", "toml"],
    )
    .expect("module32");
    let module = Module32Artifact::new(collect_program_imports(&program), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
}

#[test]
fn llvm_backend_lowers_dynamic_bool_list_module_methods_without_artifact_shell() {
    let source = r#"
        import list;
        let xs = [];
        for n in [1, 2, 3] {
            xs = xs.push(n > 1);
        }
        return [list.contains(xs, true), list.index_of(xs, true), list.reverse(xs), list.sort(xs), list.pop(xs), xs.concat([false])];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "list"])
            .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_contains_i64_list"));
    assert!(artifact.module.ir.contains("@lk_index_of_i64_list"));
    assert!(artifact.module.ir.contains("call void @lk_reverse_i64_list"));
    assert!(artifact.module.ir.contains("call void @lk_sort_i64_list"));
    assert!(artifact.module.ir.contains("call void @lk_concat_i64_list"));
    assert!(artifact.module.ir.contains("@lk_pop_i64_list"));
    assert!(artifact.module.ir.contains("ret.arg.list."));
}

#[test]
fn llvm_backend_lowers_dynamic_bool_list_module_mutators_without_artifact_shell() {
    let source = r#"
        import list;
        let xs = [];
        for n in [1, 2, 3] {
            xs = xs.push(n > 1);
        }
        return [
            list.push(xs, false),
            list.slice(xs, 1, 3),
            list.insert(xs, 1, false),
            list.remove_at(xs, 1),
            list.set(xs, 1, false)
        ];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "list"])
            .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_push_i64_list"));
    assert!(artifact.module.ir.contains("@lk_slice_range_i64_list"));
    assert!(artifact.module.ir.contains("@lk_insert_i64_list"));
    assert!(artifact.module.ir.contains("@lk_remove_at_i64_list"));
    assert!(artifact.module.ir.contains("@lk_set_i64_list"));
}

#[test]
fn llvm_backend_lowers_dynamic_i64_string_map_has_delete_without_artifact_shell() {
    let source = r#"
        import map;
        let names = {};
        for n in [1, 2] {
            names = map.set(names, n, "v${n}");
        }
        let removed = map.delete(names, 1);
        let without = removed[0];
        return [map.has(names, 1), map.has(names, 3), removed[1], map.has(without, 1), map.values(without), without];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_set_i64_ptr_map"));
    assert!(artifact.module.ir.contains("lk.delete.i64.map."));
    assert!(artifact.module.ir.contains("lk.has.i64.map."));
    assert!(artifact.module.ir.contains("ret.arg.map."));
}

#[test]
fn llvm_backend_lowers_dynamic_i64_map_has_delete_without_artifact_shell() {
    for source in [
        r#"
            import map;
            let counts = {};
            for n in [1, 2] {
                counts = map.set(counts, n, n * 10);
            }
            let removed = map.delete(counts, 1);
            let without = removed[0];
            let old = removed[1];
            return [map.has(counts, 1), map.has(without, 1), old, map.get(without, 1), map.values(without), without];
        "#,
        r#"
            import map;
            let weights = {};
            for n in [1, 2] {
                weights = map.set(weights, n, n + 0.5);
            }
            let removed = map.delete(weights, 1);
            let without = removed[0];
            let old = removed[1];
            return [map.has(weights, 1), map.has(without, 1), old, map.get(without, 1), map.values(without), without];
        "#,
        r#"
            import map;
            let flags = {};
            for n in [1, 2] {
                flags = map.set(flags, n, n == 2);
            }
            let removed = map.delete(flags, 1);
            let without = removed[0];
            let old = removed[1];
            return [map.has(flags, 1), map.has(without, 1), old, map.get(without, 1), map.values(without), without];
        "#,
    ] {
        let tokens = Tokenizer::tokenize(&source).expect("tokens");
        let program = StmtParser::new(&tokens).parse_program().expect("program");
        let module =
            Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
                .expect("compile module");
        let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

        let artifact =
            compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

        assert!(!artifact.module.ir.contains("@lk_module32_json"));
        assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
        assert!(artifact.module.ir.contains("lk.delete.i64.map."));
        assert!(artifact.module.ir.contains("lk.has.i64.map."));
        assert!(artifact.module.ir.contains("ret.arg.map."));
        assert!(artifact.module.ir.contains("nil"));
    }
}

#[test]
fn llvm_backend_lowers_dynamic_string_map_has_delete_without_artifact_shell() {
    for source in [
        r#"
            import map;
            let weights = {};
            for n in [1, 2] {
                weights = map.set(weights, "k${n}", n + 0.5);
            }
            let removed = map.delete(weights, "k1");
            let without = removed[0];
            return [map.has(weights, "k1"), map.has(weights, "k3"), removed[1], map.has(without, "k1"), map.values(without), without];
        "#,
        r#"
            import map;
            let flags = {};
            for n in [1, 2] {
                flags = map.set(flags, "k${n}", n > 1);
            }
            let removed = map.delete(flags, "k1");
            let without = removed[0];
            return [map.has(flags, "k1"), map.has(flags, "k3"), removed[1], map.has(without, "k1"), map.values(without), without];
        "#,
        r#"
            import map;
            let names = {};
            for n in [1, 2] {
                names = map.set(names, "k${n}", "v${n}");
            }
            let removed = map.delete(names, "k1");
            let without = removed[0];
            return [map.has(names, "k1"), map.has(names, "k3"), names["k2"], removed[1], map.has(without, "k1"), map.values(without), without];
        "#,
    ] {
        let tokens = Tokenizer::tokenize(&source).expect("tokens");
        let program = StmtParser::new(&tokens).parse_program().expect("program");
        let module =
            Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
                .expect("compile module");
        let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

        let artifact =
            compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

        assert!(!artifact.module.ir.contains("@lk_module32_json"));
        assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
        assert!(artifact.module.ir.contains("lk.delete.string.map."));
        assert!(artifact.module.ir.contains("lk.has.string.map."));
        assert!(artifact.module.ir.contains("ret.arg.map."));
    }
}

#[test]
fn llvm_backend_lowers_dynamic_string_map_receiver_get_missing_without_artifact_shell() {
    for (source, maybe_nil_label) in [
        (
            r#"
                import map;
                let weights = {};
                for n in [1, 2] {
                    weights = map.set(weights, "k${n}", n + 0.5);
                }
                let removed = map.delete(weights, "k1");
                let without = removed[0];
                return [without.get("k1")];
            "#,
            "lk_block_return_maybe_f64_nil",
        ),
        (
            r#"
                import map;
                let flags = {};
                for n in [1, 2] {
                    flags = map.set(flags, "k${n}", n > 1);
                }
                let removed = map.delete(flags, "k1");
                let without = removed[0];
                return [without.get("k1")];
            "#,
            "lk_block_return_maybe_bool_nil",
        ),
    ] {
        let tokens = Tokenizer::tokenize(source).expect("tokens");
        let program = StmtParser::new(&tokens).parse_program().expect("program");
        let module =
            Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
                .expect("compile module");
        let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

        let artifact =
            compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

        assert!(!artifact.module.ir.contains("@lk_module32_json"));
        assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
        assert!(artifact.module.ir.contains(maybe_nil_label), "{}", artifact.module.ir);
    }
}

#[test]
fn llvm_backend_lowers_dynamic_string_string_map_missing_get_without_artifact_shell() {
    for expr in ["map.get(without, \"k1\")", "without[\"k1\"]", "without.get(\"k1\")"] {
        let source = dynamic_string_string_map_missing_get_source(expr);
        let tokens = Tokenizer::tokenize(&source).expect("tokens");
        let program = StmtParser::new(&tokens).parse_program().expect("program");
        let module =
            Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
                .expect("compile module");
        let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

        let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default())
            .unwrap_or_else(|err| panic!("{expr}: {err}"));

        assert!(!artifact.module.ir.contains("@lk_module32_json"));
        assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
        assert!(artifact.module.ir.contains("lk_block_return_maybe_str_nil"));
    }
}

fn dynamic_string_string_map_missing_get_source(expr: &str) -> String {
    format!(
        r#"
            import map;
            let names = {{}};
            for n in [1, 2] {{
                names = map.set(names, "k${{n}}", "v${{n}}");
            }}
            let removed = map.delete(names, "k1");
            let without = removed[0];
            return [{expr}];
        "#
    )
}

#[test]
fn llvm_backend_lowers_dynamic_i64_list_module_methods_without_artifact_shell() {
    let source = r#"
        import list;
        let xs = [];
        for n in [1, 2, 3] {
            xs = xs.push(n * 2);
        }
        return [list.contains(xs, 4), list.index_of(xs, 6), list.reverse(xs), list.pop(xs)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "list"])
            .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_contains_i64_list"));
    assert!(artifact.module.ir.contains("@lk_index_of_i64_list"));
    assert!(artifact.module.ir.contains("@lk_reverse_i64_list"));
    assert!(artifact.module.ir.contains("@lk_pop_i64_list"));
}

#[test]
fn llvm_backend_lowers_dynamic_i64_list_module_mutators_without_artifact_shell() {
    let source = r#"
        import list;
        let xs = [];
        for n in [1, 2, 3] {
            xs = xs.push(n * 2);
        }
        return [
            list.push(xs, 8),
            list.slice(xs, 1, 3),
            list.insert(xs, 1, 9),
            list.remove_at(xs, 1),
            list.set(xs, 1, 7)
        ];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "list"])
            .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_push_i64_list"));
    assert!(artifact.module.ir.contains("@lk_slice_range_i64_list"));
    assert!(artifact.module.ir.contains("@lk_insert_i64_list"));
    assert!(artifact.module.ir.contains("@lk_remove_at_i64_list"));
    assert!(artifact.module.ir.contains("@lk_set_i64_list"));
}

#[test]
fn llvm_backend_lowers_block_static_module_return_display_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("if 1 { return iter; }\nreturn nil;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["iter"]).expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_block_return_module_"));
    assert!(artifact.module.ir.contains("collect: <native fn collect(1 args)>"));
    assert!(artifact.module.ir.contains("reduce: <native fn reduce(3 args)>"));
}

#[test]
fn llvm_backend_lowers_math_module_return_display_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return math;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["math"]).expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(
        artifact
            .module
            .ir
            .contains("epsilon: 0.0000000000000002220446049250313")
    );
    assert!(artifact.module.ir.contains("inf: inf"));
    assert!(artifact.module.ir.contains("max_int: 9223372036854775807"));
    assert!(artifact.module.ir.contains("nan: NaN"));
}

#[test]
fn llvm_backend_lowers_math_epsilon_member_display_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("import math; return math.epsilon;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["math"]).expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("0.0000000000000002220446049250313"));
    assert!(!artifact.module.ir.contains("2.220446049250313e-16"));
}

#[test]
fn llvm_backend_lowers_static_math_module_more_helpers_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"import math;
return [
  math.tan(0),
  math.log(1),
  math.log10(100),
  math.log2(8),
  math.atan2(0, 1),
  math.hypot(3, 4),
  math.cbrt(8),
  math.sinh(0),
  math.cosh(0),
  math.tanh(0),
  math.trunc(3.9),
  math.fract(3.25),
  math.sign(-3),
  math.clamp(120),
  math.clamp(-5, 0, 10),
  math.to_int(3.9),
  math.to_float(true),
  math.is_nan(math.nan),
  math.is_inf(math.inf)
];"#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["math"]).expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(
        artifact
            .module
            .ir
            .contains("[0, 0, 2, 3, 0, 5, 2, 0, 1, 0, 3, 0.25, -1, 100, 0, 3, 1, true, true]")
    );
}

#[test]
fn llvm_backend_lowers_os_module_return_display_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return os;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["os"]).expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("arch: <native fn os::<native>(0 args)>"));
    assert!(artifact.module.ir.contains("env_get: <native fn os::<native>(...)>"));
    assert!(artifact.module.ir.contains("path_join: <native fn os::<native>(...)>"));
}

#[test]
fn llvm_backend_lowers_os_env_get_member_display_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("import os; return os.env.get;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["os"]).expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("c\"<native fn os::<native>(...)>\\00\""));
}

#[test]
fn llvm_backend_lowers_os_module_builtin_return_display_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return os.hostname;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["os"]).expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("c\"<native fn os::<native>(0 args)>\\00\""));
}

#[test]
fn llvm_backend_lowers_list_module_get_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"import list; return list.get(["a", "b"], 1);"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["list", "__lk_call_method"])
            .expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("c\"b\\00\""));
}

#[test]
fn llvm_backend_lowers_static_list_module_helpers_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"import list;
let first = list.first(["a", "b"]);
let last = list.last(["a", "b"]);
let contains = list.contains(["a", "b"], "b");
let empty = list.is_empty([]);
let joined = list.join(["a", "b"], "-");
let concat = list.concat(["a"], ["b"]);
return [first, last, contains, empty, joined, concat];"#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["list", "__lk_call_method"])
            .expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(!artifact.module.ir.contains("@lk_list_join_"));
    assert!(artifact.module.ir.contains("a-b"));
    assert!(artifact.module.ir.contains("[a, b, true, true, a-b, [a, b]]"));
}

#[test]
fn llvm_backend_lowers_static_list_module_more_helpers_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"import list;
return [
  list.push([1, 2], 3),
  list.reverse([1, 2, 3]),
  list.pop([1, 2, 3]),
  list.index_of(["a", "b"], "b"),
  list.slice([1, 2, 3, 4], 1, 3),
  list.slice([1, 2, 3, 4], 2),
  list.sort([3, 1, 2])
];"#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["list", "__lk_call_method"])
            .expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(
        artifact
            .module
            .ir
            .contains("[[1, 2, 3], [3, 2, 1], 3, 1, [2, 3], [3, 4], [1, 2, 3]]")
    );
}

#[test]
fn llvm_backend_lowers_static_list_module_mutators_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"import list;
return [
  list.set([1, 2, 3], 1, 9),
  list.insert([1, 3], 1, 2),
  list.remove_at([1, 2, 3], 1)
];"#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["list", "__lk_call_method"])
            .expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("[[[1, 9, 3], 2], [1, 2, 3], [[1, 3], 2]]"));
}

#[test]
fn llvm_backend_lowers_static_string_module_basic_helpers_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"import string;
return [
  string.upper("ab"),
  string.lower("AB"),
  string.trim("  x  "),
  string.starts_with("abc", "a"),
  string.ends_with("abc", "c"),
  string.contains("abc", "b"),
  string.replace("a-b-a", "a", "x"),
  string.substring("abcdef", 1, 3),
  string.reverse("abc"),
  string.repeat("ab", 2),
  string.char("abc", 1),
  string.byte("ABC", 1),
  string.chars("ab"),
  string.find("abcabc", "bc"),
  string.is_empty("")
];"#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["string", "__lk_call_method"])
            .expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(
        artifact
            .module
            .ir
            .contains("[AB, ab, x, true, true, true, x-b-x, bcd, cba, abab, b, 66, [a, b], 1, true]")
    );
}

#[test]
fn llvm_backend_lowers_static_string_module_more_helpers_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"import string;
return [
  string.split("a,b", ","),
  string.join(["a", "b"], ":"),
  string.strip("xab", "x"),
  string.strip_prefix("foobar", "foo"),
  string.strip_suffix("foobar", "bar"),
  string.count("banana", "an"),
  string.pad_left("x", 3, "0"),
  string.pad_right("x", 3, "0"),
  string.to_int(true),
  string.to_float(2),
  string.title("hello world"),
  string.capitalize("hELLO"),
  string.format("{} + {}", 1, 2)
];"#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["string", "__lk_call_method"])
            .expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(
        artifact
            .module
            .ir
            .contains("[[a, b], a:b, ab, bar, foo, 2, 00x, x00, 1, 2, Hello World, Hello, 1 + 2]")
    );
}

#[test]
fn llvm_backend_lowers_static_map_module_basic_helpers_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"import map;
return [
  map.len({"a": 1, "b": 2}),
  map.keys({"a": 1, "b": 2}),
  map.values({"a": 1, "b": 2}),
  map.has({"a": 1}, "a"),
  map.get({"a": 1}, "a"),
  map.get({"a": 1}, "z")
];"#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["map", "__lk_call_method"])
        .expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("[2, [a, b], [1, 2], true, 1, nil]"));
}

#[test]
fn llvm_backend_lowers_static_iter_next_collect_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"import iter;
return [
  iter.next([1, 2]),
  iter.next([]),
  iter.collect(["a", "b"]),
  iter.collect(iter.range(3))
];"#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["iter", "__lk_call_method"])
            .expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");
    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("[1, nil, [a, b], [0, 1, 2]]"));
}

#[test]
fn llvm_backend_lowers_static_iter_list_helpers_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"import iter;
return [
  iter.enumerate(["a", "b"]),
  iter.take([1, 2, 3], 2),
  iter.skip([1, 2, 3], 1),
  iter.chain([1], [2, 3]),
  iter.flatten([[1, 2], [3]]),
  iter.unique([1, 2, 1]),
  iter.chunk([1, 2, 3], 2),
  iter.zip([1, 2], ["a", "b"])
];"#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["iter", "__lk_call_method"])
            .expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(
        artifact.module.ir.contains(
            "[[[0, a], [1, b]], [1, 2], [2, 3], [1, 2, 3], [1, 2, 3], [1, 2], [[1, 2], [3]], [[1, a], [2, b]]]"
        )
    );
}

#[test]
fn llvm_backend_lowers_static_iter_map_filter_reduce_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"import iter;
return [
  iter.map([1, 2, 3], |x| x + 1),
  iter.filter([1, 2, 3], |x| x > 1),
  iter.reduce([1, 2, 3], 0, |acc, x| acc + x)
];"#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["iter", "__lk_call_method"])
            .expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("[[2, 3, 4], [2, 3], 6]"));
}
