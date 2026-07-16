mod tests {
    use crate::*;
    use lk_core::vm::VmRuntimeMetrics;

    #[test]
    fn test_sanitize_path_allows_simple_relative() {
        let p = sanitize_path("foo/bar.lk").expect("relative path should be allowed");
        assert_eq!(p, PathBuf::from("foo/bar.lk"));
    }

    #[test]
    fn test_sanitize_path_rejects_parent_dir() {
        let err = sanitize_path("foo/../bar.lk").unwrap_err();
        assert!(err.to_string().contains("Parent directory components"));
    }

    #[cfg(unix)]
    #[test]
    fn test_sanitize_path_allows_absolute_unix() {
        let p = sanitize_path("/etc/passwd").expect("absolute path should be allowed");
        assert_eq!(p, PathBuf::from("/etc/passwd"));
    }

    #[cfg(windows)]
    #[test]
    fn test_sanitize_path_allows_absolute_windows() {
        let p = sanitize_path(r"C:\\Windows").expect("absolute path should be allowed");
        assert_eq!(p, PathBuf::from(r"C:\\Windows"));
    }

    #[test]
    fn test_cli_args_rejects_parent_dir_in_compile() {
        let args = CliArgs::try_parse_from(["lk", "compile", "foo/../bar.lk"]).expect("should parse");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            let err = split_compile_args(&positional).expect_err("should reject parent dirs");
            assert!(err.to_string().contains("Parent directory components"));
        } else {
            panic!("expected compile command");
        }
    }

    #[test]
    fn test_cli_args_accepts_simple_file() {
        let args = CliArgs::try_parse_from(["lk", "a.lk"]).expect("should parse");
        assert!(args.command.is_none());
        assert_eq!(args.file.as_deref(), Some(Path::new("a.lk")));
    }

    #[test]
    fn test_cli_args_accepts_coverage_file() {
        let args = CliArgs::try_parse_from(["lk", "coverage", "bench/workloads_business_algorithms.lk"])
            .expect("should parse coverage command");
        if let Some(Commands::Coverage {
            file,
            disassemble,
            runtime,
        }) = args.command
        {
            assert_eq!(file, PathBuf::from("bench/workloads_business_algorithms.lk"));
            assert!(!disassemble);
            assert!(!runtime);
        } else {
            panic!("expected coverage command");
        }
    }

    #[test]
    fn test_vm_profile_line_contains_benchmark_fields() {
        let line = vm_profile_line(VmRuntimeMetrics {
            opcode_steps: 11,
            call_ops: 2,
            branch_ops: 3,
            typed_branch_ops: 4,
            container_ops: 5,
            list_ops: 6,
            map_ops: 7,
            string_ops: 8,
            index_key_metrics: [12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1],
            register_write_sources: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
            copy_policy_heap_clones: 9,
            register_copy_heap_clones: 10,
            local_copy_heap_clones: 12,
            local_load_heap_clones: 13,
            local_store_heap_clones: 14,
            const_load_heap_clones: 15,
            call_arg_heap_clones: 16,
            container_copy_heap_clones: 17,
            ..VmRuntimeMetrics::default()
        });

        assert!(line.starts_with("VM profile: "));
        assert!(line.contains("opcode_steps=11"));
        assert!(line.contains("calls=2"));
        assert!(line.contains("branches=3"));
        assert!(line.contains("typed_branches=4"));
        assert!(line.contains("containers=5"));
        assert!(line.contains("write_sources=other:10,string:9,global:8,call_return:7,index:6,container:5"));
        assert!(line.contains(
            "index_keys=known_string_key:12,dynamic_register_key:11,dynamic_int_key:10,dynamic_short_string_key:9,dynamic_object_key:8,dynamic_other_key:7"
        ));
        assert!(line.contains("val_clones=9"));
        assert!(line.contains("heap_clones=9"));
        assert!(line.contains("copy_policy_heap_clones=9"));
        assert!(line.contains("register_copy_heap_clones=10"));
        assert!(line.contains("local_copy_heap_clones=12"));
        assert!(line.contains("local_load_heap_clones=13"));
        assert!(line.contains("local_store_heap_clones=14"));
        assert!(line.contains("const_load_heap_clones=15"));
        assert!(line.contains("call_arg_heap_clones=16"));
        assert!(line.contains("container_copy_heap_clones=17"));
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn test_cli_args_compile_positional_target() {
        let args =
            CliArgs::try_parse_from(["lk", "compile", "bytecode", "foo.lk"]).expect("should parse positional target");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            let (target, file) = split_compile_args(&positional).expect("should split compile args");
            assert_eq!(target, CompileMode::Bytecode);
            assert_eq!(file, PathBuf::from("foo.lk"));
        } else {
            panic!("expected compile command");
        }
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn direct_source_execution_defaults_to_vm_and_native_is_opt_in() {
        assert!(!native_run_enabled_from_flags(false, false, false, false));
        assert!(native_run_enabled_from_flags(false, false, false, true));
        assert!(!native_run_enabled_from_flags(true, false, false, false));
        assert!(!native_run_enabled_from_flags(false, true, false, false));
        assert!(!native_run_enabled_from_flags(false, false, true, false));
        assert!(!native_run_enabled_from_flags(true, false, false, true));
        assert!(!native_run_enabled_from_flags(false, true, false, true));
        assert!(!native_run_enabled_from_flags(false, false, true, true));
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn native_cache_proc_macro_dependency_metadata_stales_on_file_change() {
        let dir = tempfile::tempdir().expect("temp dir");
        let source = dir.path().join("main.lk");
        let output = dir.path().join("lk-native-test");
        std::fs::write(&source, "return generated!();\n").expect("write source");
        std::fs::write(dir.path().join("schema.txt"), "one").expect("write dependency");
        let dependencies = vec![ProcMacroDependency {
            path: "schema.txt".to_string(),
            digest: None,
        }];

        write_native_cache_proc_macro_dependencies(&source, &output, &dependencies).expect("write dependency metadata");
        assert!(native_cache_proc_macro_dependencies_fresh(&source, &output));

        std::fs::write(dir.path().join("schema.txt"), "two").expect("rewrite dependency");
        assert!(!native_cache_proc_macro_dependencies_fresh(&source, &output));
    }

    #[test]
    fn test_cli_args_compile_default_target_is_exe() {
        let args = CliArgs::try_parse_from(["lk", "compile", "foo.lk"]).expect("should parse default compile");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            let (target, file) = split_compile_args(&positional).expect("should split compile args");
            assert_eq!(target, CompileMode::Exe);
            assert_eq!(file, PathBuf::from("foo.lk"));
        } else {
            panic!("expected compile command");
        }
    }

    #[test]
    fn test_cli_args_compile_allows_omitted_file() {
        let args = CliArgs::try_parse_from(["lk", "compile"]).expect("should parse compile without file");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            assert!(positional.is_empty());
        } else {
            panic!("expected compile command");
        }
    }

    #[test]
    fn test_split_compile_args_defaults_to_cwd_main() {
        let temp = tempfile::tempdir().expect("temp dir");
        let main = temp.path().join("main.lk");
        std::fs::write(&main, "return 1;\n").expect("write main.lk");

        let (target, file) = split_compile_args_with_cwd(&[], temp.path()).expect("should find main.lk");

        assert_eq!(target, CompileMode::Exe);
        assert_eq!(file, main.canonicalize().expect("canonical main"));
    }

    #[test]
    fn test_split_compile_args_defaults_to_package_src_main() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            temp.path().join("Lk.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        )
        .expect("write manifest");
        let src = temp.path().join("src");
        std::fs::create_dir_all(&src).expect("create src");
        let main = src.join("main.lk");
        std::fs::write(&main, "return 1;\n").expect("write src/main.lk");

        let (target, file) = split_compile_args_with_cwd(&[], temp.path()).expect("should find src/main.lk");

        assert_eq!(target, CompileMode::Exe);
        assert_eq!(file, main.canonicalize().expect("canonical main"));
    }

    #[test]
    fn test_split_compile_args_accepts_target_with_omitted_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let main = temp.path().join("main.lk");
        std::fs::write(&main, "return 1;\n").expect("write main.lk");

        let args = vec!["bytecode".to_string()];
        let (target, file) = split_compile_args_with_cwd(&args, temp.path()).expect("should find main.lk");

        assert_eq!(target, CompileMode::Bytecode);
        assert_eq!(file, main.canonicalize().expect("canonical main"));
    }

    #[test]
    fn test_split_compile_args_rejects_removed_exe_target() {
        let args = vec!["exe".to_string(), "main.lk".to_string()];
        let err = split_compile_args(&args).expect_err("exe target was removed");
        assert!(err.to_string().contains("`lk compile exe` was removed"));
    }

    #[test]
    fn test_pkg_init_parses_package_name() {
        let args = CliArgs::try_parse_from(["lk", "pkg", "init", "demo"]).expect("should parse pkg init");
        if let Some(Commands::Pkg {
            command: PkgCommand::Init { name },
        }) = args.command
        {
            assert_eq!(name.as_deref(), Some("demo"));
        } else {
            panic!("expected pkg init command");
        }
    }

    #[test]
    fn test_pkg_check_parses() {
        let args = CliArgs::try_parse_from(["lk", "pkg", "check"]).expect("should parse pkg check");
        if let Some(Commands::Pkg {
            command: PkgCommand::Check,
        }) = args.command
        {
        } else {
            panic!("expected pkg check command");
        }
    }

    #[test]
    fn test_split_compile_args_defaults_to_single_workspace_app() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("Lk.toml"), "[workspace]\nmembers = [\"apps/*\"]\n").expect("write manifest");
        let app = temp.path().join("apps").join("demo");
        let src = app.join("src");
        std::fs::create_dir_all(&src).expect("create app src");
        std::fs::write(app.join("Lk.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n")
            .expect("write app manifest");
        let main = src.join("main.lk");
        std::fs::write(&main, "return 1;\n").expect("write app main");

        let (target, file) = split_compile_args_with_cwd(&[], temp.path()).expect("should find single workspace app");

        assert_eq!(target, CompileMode::Exe);
        assert_eq!(file, main.canonicalize().expect("canonical main"));
    }

    #[test]
    fn test_split_compile_args_rejects_workspace_manifest_without_entry() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("Lk.toml"), "[workspace]\nmembers = []\n").expect("write manifest");

        let err = split_compile_args_with_cwd(&[], temp.path()).expect_err("workspace root has no single entry");

        assert!(err.to_string().contains("no member src/main.lk was found"));
    }

    #[test]
    fn test_split_compile_args_rejects_multiple_workspace_apps() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("Lk.toml"), "[workspace]\nmembers = [\"apps/*\"]\n").expect("write manifest");
        for name in ["a", "b"] {
            let app = temp.path().join("apps").join(name);
            let src = app.join("src");
            std::fs::create_dir_all(&src).expect("create app src");
            std::fs::write(
                app.join("Lk.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
            )
            .expect("write app manifest");
            std::fs::write(src.join("main.lk"), "return 1;\n").expect("write app main");
        }

        let err = split_compile_args_with_cwd(&[], temp.path()).expect_err("workspace root has multiple entries");

        assert!(err.to_string().contains("multiple workspace app entries"));
    }

    #[cfg(not(feature = "llvm"))]
    #[test]
    fn compile_target_errors_when_llvm_disabled() {
        let args = CliArgs::try_parse_from(["lk", "compile", "llvm", "foo.lk"]).expect("should parse");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            let err = split_compile_args(&positional).expect_err("llvm target should be rejected without feature");
            assert!(err.to_string().contains("LLVM backend disabled"));
        } else {
            panic!("expected compile command");
        }
    }
}
