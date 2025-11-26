mod tests {
    use crate::*;

    #[test]
    fn test_sanitize_path_allows_simple_relative() {
        let p = sanitize_path("foo/bar.lkr").expect("relative path should be allowed");
        assert_eq!(p, PathBuf::from("foo/bar.lkr"));
    }

    #[test]
    fn test_sanitize_path_rejects_parent_dir() {
        let err = sanitize_path("foo/../bar.lkr").unwrap_err();
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
        let args = CliArgs::try_parse_from(["lkr", "compile", "foo/../bar.lkr"]).expect("should parse");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            let err = split_compile_args(&positional).expect_err("should reject parent dirs");
            assert!(err.to_string().contains("Parent directory components"));
        } else {
            panic!("expected compile command");
        }
    }

    #[test]
    fn test_cli_args_accepts_simple_file() {
        let args = CliArgs::try_parse_from(["lkr", "a.lkr"]).expect("should parse");
        assert!(args.command.is_none());
        assert_eq!(args.file.as_deref(), Some(Path::new("a.lkr")));
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn test_cli_args_compile_positional_target() {
        let args =
            CliArgs::try_parse_from(["lkr", "compile", "llvm", "foo.lkr"]).expect("should parse positional target");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            let (target, file) = split_compile_args(&positional).expect("should split compile args");
            assert_eq!(target, Some(CompileMode::Llvm));
            assert_eq!(file, PathBuf::from("foo.lkr"));
        } else {
            panic!("expected compile command");
        }
    }

    #[test]
    fn test_cli_args_compile_default_target_is_none() {
        let args = CliArgs::try_parse_from(["lkr", "compile", "foo.lkr"]).expect("should parse default compile");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            let (target, file) = split_compile_args(&positional).expect("should split compile args");
            assert_eq!(target, None);
            assert_eq!(file, PathBuf::from("foo.lkr"));
        } else {
            panic!("expected compile command");
        }
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn runtime_init_plan_embeds_assets() {
        let module_ir = "define i64 @lkr_entry() { ret i64 0 }\n";
        let search_paths = vec!["examples".to_string()];
        let imports_json = Some("[{\"File\":{\"path\":\"examples/fib.lkr\"}}]".to_string());
        let modules = vec![EncodedBundledModule {
            path: "examples/fib.lkr".to_string(),
            bytes: b"LKRB".to_vec(),
        }];

        let plan = build_runtime_init_plan(module_ir, &search_paths, imports_json.as_deref(), &modules);

        assert!(
            plan.declarations
                .iter()
                .any(|decl| decl.contains("lkr_rt_begin_session"))
        );
        assert!(plan.globals.iter().any(|g| g.contains("@.lkr_path.0")));
        assert!(plan.globals.iter().any(|g| g.contains("@.lkr_mod_blob.0")));
        assert!(plan.body_lines.first().unwrap().contains("lkr_rt_begin_session"));
        assert!(
            plan.body_lines
                .iter()
                .any(|line| line.contains("lkr_rt_register_bundled_module"))
        );
        assert!(plan.body_lines.last().unwrap().contains("lkr_rt_apply_imports"));
    }

    #[cfg(not(feature = "llvm"))]
    #[test]
    fn compile_target_errors_when_llvm_disabled() {
        let args = CliArgs::try_parse_from(["lkr", "compile", "llvm", "foo.lkr"]).expect("should parse");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            let err = split_compile_args(&positional).expect_err("llvm target should be rejected without feature");
            assert!(err.to_string().contains("LLVM backend disabled"));
        } else {
            panic!("expected compile command");
        }
    }
}
