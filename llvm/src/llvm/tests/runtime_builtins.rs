use crate::{
    llvm::{LlvmBackendOptions, compile_module_artifact_to_llvm},
    stmt::{ModuleResolver, import::collect_program_imports, stmt_parser::StmtParser},
    token::Tokenizer,
    vm::{Compiler, ModuleArtifact, VmContext, compile_program_module_with_ctx},
};
use lk_core::module::ModuleRegistry;
use std::sync::Arc;

#[test]
fn llvm_backend_lowers_assert_runtime_globals_without_artifact_shell() {
    let source = r#"
        assert(1);
        assert_eq(40 + 2, 42);
        assert_ne("left", "right");
        return 7;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["assert", "assert_eq", "assert_ne"])
            .expect("compile module");
    let artifact = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("lk_assert_fail"));
    assert!(artifact.module.ir.contains("i64 7"));
}

#[test]
fn llvm_backend_lowers_tcp_and_bytes_runtime_builtins_to_lkrt() {
    let source = r#"
        use bytes;
        use { socket, tcp } from net;

        let addr = socket.addr("127.0.0.1", 9);
        let conn = tcp.connect(addr);
        let sent = tcp.write(conn, "ping");
        let raw = tcp.read(conn, 4);
        let text = bytes.to_string_utf8(raw);
        tcp.close(conn);
        return sent;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let mut registry = ModuleRegistry::new();
    lk_stdlib::register_stdlib_modules(&mut registry).expect("stdlib registration");
    let resolver = Arc::new(ModuleResolver::with_registry(registry));
    let mut ctx = VmContext::new().with_resolver(resolver);
    let module = compile_program_module_with_ctx(&program, &mut ctx).expect("compile module");
    let artifact = ModuleArtifact::new(collect_program_imports(&program), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_tcp_connect"));
    assert!(artifact.module.ir.contains("@lkrt_tcp_write_str"));
    assert!(artifact.module.ir.contains("@lkrt_tcp_read"));
    assert!(artifact.module.ir.contains("@lkrt_tcp_close"));
    assert!(artifact.module.ir.contains("@lkrt_bytes_to_string_utf8"));
    assert!(artifact.module.ir.contains("call i64 @lkrt_tcp_write_str"));
    assert!(artifact.module.ir.contains("call ptr @lkrt_bytes_to_string_utf8"));
}

#[test]
fn llvm_backend_lowers_host_fs_and_env_runtime_builtins_to_lkrt() {
    let source = r#"
        use bytes;
        use env;
        use { get } from env;
        use fs;

        let path = "/tmp/lk-native-runtime-builtins.txt";
        fs.write(path, "hello");
        let raw = fs.read(path);
        let text = bytes.to_string_utf8(raw);
        let fallback = env.get_or("LK_TEST_ENV_SHOULD_NOT_EXIST_42", "dflt");
        let maybe_path = get("PATH");
        let has_path = env.has("PATH");
        let cwd_tmp = fs.temp_dir();
        let canonical = fs.canonicalize(path);
        return has_path;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let mut registry = ModuleRegistry::new();
    lk_stdlib::register_stdlib_modules(&mut registry).expect("stdlib registration");
    let resolver = Arc::new(ModuleResolver::with_registry(registry));
    let mut ctx = VmContext::new().with_resolver(resolver);
    let module = compile_program_module_with_ctx(&program, &mut ctx).expect("compile module");
    let artifact = ModuleArtifact::new(collect_program_imports(&program), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(artifact.module.ir.contains("declare void @lkrt_cleanup()"));
    assert!(artifact.module.ir.contains("call void @lkrt_cleanup()"));
    assert!(artifact.module.ir.contains("call i64 @lkrt_fs_write_str"));
    assert!(artifact.module.ir.contains("call i64 @lkrt_fs_read"));
    assert!(artifact.module.ir.contains("call ptr @lkrt_bytes_to_string_utf8"));
    assert!(artifact.module.ir.contains("call ptr @lkrt_env_get_or"));
    assert!(artifact.module.ir.contains("call i64 @lkrt_env_get"));
    assert!(artifact.module.ir.contains("call i64 @lkrt_env_has"));
    assert!(artifact.module.ir.contains("call ptr @lkrt_fs_temp_dir"));
    assert!(artifact.module.ir.contains("call ptr @lkrt_fs_canonicalize"));
}

#[test]
fn llvm_backend_static_socket_addr_matches_lkrt_ipv6_format() {
    let source = r#"
        use { socket } from net;
        return socket.addr("::1", 8080);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let mut registry = ModuleRegistry::new();
    lk_stdlib::register_stdlib_modules(&mut registry).expect("stdlib registration");
    let resolver = Arc::new(ModuleResolver::with_registry(registry));
    let mut ctx = VmContext::new().with_resolver(resolver);
    let module = compile_program_module_with_ctx(&program, &mut ctx).expect("compile module");
    let artifact = ModuleArtifact::new(collect_program_imports(&program), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(artifact.module.ir.contains("[::1]:8080"));
    assert!(!artifact.module.ir.contains("::1:8080"));
}
