mod tests {
    use crate::*;

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

    #[cfg(feature = "llvm")]
    #[test]
    fn test_cli_args_compile_positional_target() {
        let args =
            CliArgs::try_parse_from(["lk", "compile", "llvm", "foo.lk"]).expect("should parse positional target");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            let (target, file) = split_compile_args(&positional).expect("should split compile args");
            assert_eq!(target, Some(CompileMode::Llvm));
            assert_eq!(file, PathBuf::from("foo.lk"));
        } else {
            panic!("expected compile command");
        }
    }

    #[test]
    fn test_cli_args_compile_default_target_is_none() {
        let args = CliArgs::try_parse_from(["lk", "compile", "foo.lk"]).expect("should parse default compile");
        if let Some(Commands::Compile { positional, .. }) = args.command {
            let (target, file) = split_compile_args(&positional).expect("should split compile args");
            assert_eq!(target, None);
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

        assert_eq!(target, None);
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

        assert_eq!(target, None);
        assert_eq!(file, main.canonicalize().expect("canonical main"));
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn test_split_compile_args_accepts_target_with_omitted_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let main = temp.path().join("main.lk");
        std::fs::write(&main, "return 1;\n").expect("write main.lk");

        let args = vec!["exe".to_string()];
        let (target, file) = split_compile_args_with_cwd(&args, temp.path()).expect("should find main.lk");

        assert_eq!(target, Some(CompileMode::Exe));
        assert_eq!(file, main.canonicalize().expect("canonical main"));
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn default_executable_path_uses_target_platform_extension() {
        let source = Path::new("apps/demo/src/main.lk");

        assert_eq!(
            default_executable_path(source, Some("aarch64-apple-darwin")),
            PathBuf::from("apps/demo/src/main")
        );
        assert_eq!(
            default_executable_path(source, Some("x86_64-unknown-linux-gnu")),
            PathBuf::from("apps/demo/src/main.elf")
        );
        assert_eq!(
            default_executable_path(source, Some("x86_64-pc-windows-msvc")),
            PathBuf::from("apps/demo/src/main.exe")
        );
        assert_eq!(
            default_executable_path(source, Some("wasm32-unknown-unknown")),
            PathBuf::from("apps/demo/src/main.out")
        );
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn default_executable_path_uses_host_platform_without_target_triple() {
        let source = Path::new("main.lk");
        let path = default_executable_path(source, None);

        if cfg!(windows) {
            assert_eq!(path, PathBuf::from("main.exe"));
        } else if cfg!(target_os = "macos") {
            assert_eq!(path, PathBuf::from("main"));
        } else if cfg!(unix) {
            assert_eq!(path, PathBuf::from("main.elf"));
        } else {
            assert_eq!(path, PathBuf::from("main.out"));
        }
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn compile_exe_defaults_to_release_runtime() {
        assert_eq!(default_runtime_profile_for_exe(), RuntimeProfile::Release);
        assert!(default_runtime_profile_for_exe().use_release());
        assert_eq!(default_runtime_profile_for_exe().label(), "release");
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn strip_is_limited_to_host_target() {
        assert!(should_strip_executable(None));
        assert!(!should_strip_executable(Some("aarch64-apple-darwin")));
        assert!(!should_strip_executable(Some("x86_64-unknown-linux-gnu")));
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

        assert_eq!(target, None);
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

    #[cfg(feature = "llvm")]
    #[test]
    fn runtime_init_plan_embeds_assets() {
        let module_ir = "define i64 @lk_entry() { ret i64 0 }\n";
        let search_paths = vec!["examples".to_string()];
        let imports_json = Some("[{\"File\":{\"path\":\"examples/fib.lk\"}}]".to_string());
        let modules = vec![EncodedBundledModule {
            path: "examples/fib.lk".to_string(),
            bytes: b"LKB".to_vec(),
        }];

        let plan = build_runtime_init_plan(
            module_ir,
            &search_paths,
            imports_json.as_deref(),
            None,
            &modules,
            &[],
            false,
        );

        assert!(
            plan.declarations
                .iter()
                .any(|decl| decl.contains("lk_rt_begin_session"))
        );
        assert!(plan.globals.iter().any(|g| g.contains("@.lk_path.0")));
        assert!(plan.globals.iter().any(|g| g.contains("@.lk_mod_blob.0")));
        assert!(plan.body_lines.first().unwrap().contains("lk_rt_begin_session"));
        assert!(
            plan.body_lines
                .iter()
                .any(|line| line.contains("lk_rt_register_bundled_module"))
        );
        assert!(plan.body_lines.last().unwrap().contains("lk_rt_apply_imports"));
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
