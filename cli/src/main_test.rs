mod tests {
    use crate::*;
    use lk_core::vm::VmRuntimeMetrics;

    /// A CLI path argument is taken as written, `..` included.
    ///
    /// There used to be a `sanitize_path` refusing any `..`, and four tests
    /// pinning it — including one asserting that `/etc/passwd` **is** allowed.
    /// The two halves say the guard stopped nothing: anything `..` reaches, an
    /// absolute path reaches too, and every caller is an argument the person
    /// running the command typed. What it did stop was `lk ../script.lk` from a
    /// subdirectory.
    #[test]
    fn a_path_argument_is_taken_as_written() {
        for raw in ["foo/bar.lk", "../bar.lk", "foo/../bar.lk", "/etc/passwd"] {
            assert_eq!(parse_path_arg(raw), Ok(PathBuf::from(raw)), "{raw}");
        }
    }

    /// `lk compile ../bar.lk` compiles `../bar.lk`.
    ///
    /// This asserted the opposite until the `..` guard came out — see
    /// `a_path_argument_is_taken_as_written`.
    #[test]
    fn test_cli_args_accepts_parent_dir_in_compile() {
        let args = CliArgs::try_parse_from(["lk", "compile", "../bar.lk"]).expect("should parse");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            let (_, file, _) = split_compile_args(&positional).expect("a path is a path");
            assert_eq!(file, PathBuf::from("../bar.lk"));
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
            call_ops: 9,
            native_call_ops: 2,
            exact_call_ops: 3,
            method_call_ops: 1,
            branch_ops: 3,
            typed_branch_ops: 4,
            container_ops: 5,
            list_ops: 6,
            map_ops: 7,
            string_ops: 8,
            index_key_metrics: [12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1],
            register_write_sources: [1, 2, 3, 4, 5, 6, 7, 8, 9],
            register_writes: 45,
            ..VmRuntimeMetrics::default()
        });

        assert!(line.starts_with("VM profile: "));
        assert!(line.contains("opcode_steps=11"));
        assert!(line.contains("calls=9"));
        // The breakdown, and the remainder that makes the parts add up: 2 + 3
        // + 1 classified out of 9, so 3 calls the executor did not classify.
        // Without the remainder a reader cannot tell "none of these kinds" from
        // "this build does not measure it".
        assert!(line.contains("call_kinds=native:2,exact:3,method:1,other:3"), "{line}");
        assert!(line.contains("branches=3"));
        assert!(line.contains("typed_branches=4"));
        assert!(line.contains("containers=5"));
        // No `other:` any more: every dispatch arm that writes a register
        // classifies it, so a catch-all bucket could only ever print zero.
        assert!(line.contains("write_sources=string:9,global:8,call_return:7,index:6,container:5,compare:4"));
        assert!(line.contains(
            "index_keys=known_string_key:12,dynamic_register_key:11,dynamic_int_key:10,dynamic_short_string_key:9,dynamic_object_key:8,dynamic_other_key:7"
        ));
        // The ten `*_heap_clones` fields this used to pin are gone. They were
        // written only by `record_copy_policy_clone`, which had no caller — and
        // because this test builds the struct by hand, it happily printed 9/10/12
        // while every real run printed ten zeros in a row. A formatter test cannot
        // tell you a counter is dead; only a caller scan can.
        assert!(line.contains("register_writes=45"));
    }

    #[test]
    fn the_profile_report_says_so_when_it_cannot_profile() {
        // `LK_VM_PROFILE=1` used to be answered by a well-formed profile of zeros
        // on a binary with no counters compiled in — `opcode_steps=0` right after
        // running four thousand of them. The report has to agree with the build it
        // is part of, so this test is a `cfg` pair rather than a value check: the
        // one that can measure must print numbers, the one that can't must say it
        // can't.
        let report = vm_profile_report();
        if vm_runtime_metrics_enabled() {
            assert!(report.starts_with("VM profile: "), "{report}");
            assert!(!report.contains("unavailable"), "{report}");
        } else {
            assert!(report.contains("unavailable"), "{report}");
            assert!(report.contains("--features vm-profile"), "{report}");
        }
    }

    #[test]
    fn test_cli_args_compile_positional_target() {
        // `bytecode` positional parsing is feature-independent (no LLVM/Cranelift
        // needed), so this runs in every build configuration.
        let args =
            CliArgs::try_parse_from(["lk", "compile", "bytecode", "foo.lk"]).expect("should parse positional target");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            let (target, file, _out) = split_compile_args(&positional).expect("should split compile args");
            assert_eq!(target, CompileMode::Bytecode);
            assert_eq!(file, PathBuf::from("foo.lk"));
        } else {
            panic!("expected compile command");
        }
    }

    #[cfg(feature = "aot")]
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

    #[cfg(feature = "aot")]
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
            let (target, file, _out) = split_compile_args(&positional).expect("should split compile args");
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

        let (target, file, output) = split_compile_args_with_cwd(&[], temp.path()).expect("should find main.lk");
        // A loose `./main.lk` keeps the old rule: `main.lk` -> `main` beside it
        // is what naming the file would have done anyway.
        assert_eq!(output, None);

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

        let (target, file, output) = split_compile_args_with_cwd(&[], temp.path()).expect("should find src/main.lk");

        assert_eq!(target, CompileMode::Exe);
        assert_eq!(file, main.canonicalize().expect("canonical main"));
        // A build output does not belong in `src/`. The entry is
        // `<pkg>/src/main.lk` and the output used to be that path without its
        // extension — a 20 MB executable dropped next to the source it was
        // built from, where the next `git add .` picks it up. It goes to the
        // package root, named after the package directory.
        let package_root = main.parent().and_then(std::path::Path::parent).expect("package root");
        assert_eq!(
            output.expect("a package build has an implicit output"),
            package_root.join(package_root.file_name().expect("package directory name"))
        );
    }

    #[test]
    fn test_split_compile_args_accepts_target_with_omitted_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let main = temp.path().join("main.lk");
        std::fs::write(&main, "return 1;\n").expect("write main.lk");

        let args = vec!["bytecode".to_string()];
        let (target, file, _out) = split_compile_args_with_cwd(&args, temp.path()).expect("should find main.lk");

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

        let (target, file, _out) =
            split_compile_args_with_cwd(&[], temp.path()).expect("should find single workspace app");

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

    #[test]
    fn compile_llvm_target_is_rejected_as_removed() {
        // `lk compile llvm` was removed with the LLVM-text backend (Cranelift is
        // the sole native codegen); the target is rejected regardless of feature.
        let args = CliArgs::try_parse_from(["lk", "compile", "llvm", "foo.lk"]).expect("should parse");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            let err = split_compile_args(&positional).expect_err("llvm target should be rejected");
            assert!(err.to_string().contains("was removed"));
        } else {
            panic!("expected compile command");
        }
    }
}
