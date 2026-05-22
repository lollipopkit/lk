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

    #[test]
    fn test_cli_args_accepts_coverage_file() {
        let args = CliArgs::try_parse_from(["lk", "coverage", "bench/workloads_business_algorithms.lk"])
            .expect("should parse coverage command");
        if let Some(Commands::Coverage { file, runtime }) = args.command {
            assert_eq!(file, PathBuf::from("bench/workloads_business_algorithms.lk"));
            assert!(!runtime);
        } else {
            panic!("expected coverage command");
        }
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
