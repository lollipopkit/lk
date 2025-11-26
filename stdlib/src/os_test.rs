#[cfg(test)]
mod tests {
    use anyhow::Result;
    use lkr_core::{module, stmt, stmt::stmt_parser::StmtParser, token::Tokenizer, val::Val, vm};

    fn run(source: &str) -> Result<Val> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = module::ModuleRegistry::new();
        crate::register_stdlib_modules(&mut registry)?;
        let resolver = std::sync::Arc::new(stmt::ModuleResolver::with_registry(registry));
        let mut env = vm::VmContext::new().with_resolver(resolver);
        let mut machine = vm::Vm::new();
        program.execute_with_vm(&mut machine, &mut env)
    }

    #[test]
    fn test_os_arch() -> Result<()> {
        let out = run("import os; return os.arch();")?;
        match out {
            Val::Str(s) => {
                // Should match the compile-time target arch
                assert_eq!(&*s, std::env::consts::ARCH);
            }
            other => panic!("expected Str, got {:?}", other),
        }
        Ok(())
    }

    #[test]
    fn test_os_os() -> Result<()> {
        let out = run("import os; return os.os();")?;
        match out {
            Val::Str(s) => {
                assert_eq!(&*s, std::env::consts::OS);
            }
            other => panic!("expected Str, got {:?}", other),
        }
        Ok(())
    }

    #[test]
    fn test_os_env_get_default_set_unset() -> Result<()> {
        // Use a very unlikely var name
        let var = "LKR_TEST_ENV_SHOULD_NOT_EXIST_42";
        // Get default when unset
        let src_default = format!("import os; let e = os.env; return e.get(\"{}\", \"dflt\");", var);
        let out = run(&src_default)?;
        assert_eq!(out, Val::Str("dflt".into()));

        // Set and then read
        let src_set_get = format!(
            "import os; let e = os.env; e.set(\"{}\", \"X\"); return e.get(\"{}\");",
            var, var
        );
        let out = run(&src_set_get)?;
        assert_eq!(out, Val::Str("X".into()));

        // Unset and confirm Nil
        let src_unset = format!(
            "import os; let e = os.env; e.unset(\"{}\"); return e.get(\"{}\");",
            var, var
        );
        let out = run(&src_unset)?;
        assert_eq!(out, Val::Nil);

        Ok(())
    }

    #[test]
    fn test_os_dir_temp_and_current_and_list() -> Result<()> {
        use std::fs::{File, create_dir_all};
        use std::io::Write;
        use std::path::PathBuf;

        // Create a temporary directory with a couple of files
        let mut td = std::env::temp_dir();
        td.push("lkr_os_test");
        td.push(format!("case_{}", std::process::id()));
        create_dir_all(&td)?;

        let mut f1 = td.clone();
        f1.push("a.txt");
        let mut f2 = td.clone();
        f2.push("b.txt");
        let mut w1 = File::create(&f1)?;
        let mut w2 = File::create(&f2)?;
        writeln!(w1, "hello")?;
        writeln!(w2, "world")?;

        // os.dir.temp() returns a string
        let out = run("import os; return os.dir.temp();")?;
        match out {
            Val::Str(_) | Val::Nil => (),
            other => panic!("expected Str or Nil, got {:?}", other),
        }

        // os.dir.current() returns a string
        let out = run("import os; return os.dir.current();")?;
        match out {
            Val::Str(_) | Val::Nil => (),
            other => panic!("expected Str or Nil, got {:?}", other),
        }

        // os.dir.list(tempdir) should include created file names
        let src = format!(
            "import os; let xs = os.dir.list(\"{}\"); return xs;",
            td.to_string_lossy()
        );
        let out = run(&src)?;
        match out {
            Val::List(list) => {
                let names: Vec<String> = list
                    .iter()
                    .filter_map(|v| match v {
                        Val::Str(s) => Some(s.to_string()),
                        _ => None,
                    })
                    .collect();
                assert!(names.contains(&"a.txt".to_string()));
                assert!(names.contains(&"b.txt".to_string()));
            }
            other => panic!("expected List, got {:?}", other),
        }

        // Cleanup best-effort
        let _ = std::fs::remove_file(f1);
        let _ = std::fs::remove_file(f2);
        let _ = std::fs::remove_dir_all(PathBuf::from(&td));
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_os_exec_capture_unix() -> Result<()> {
        let out = run("import os; return os.exec(\"/bin/echo\", [\"hello\"]);")?;
        match out {
            Val::Str(s) => {
                assert_eq!(s.trim_end(), "hello");
            }
            other => panic!("expected Str, got {:?}", other),
        }
        Ok(())
    }

    #[test]
    #[cfg(windows)]
    fn test_os_exec_capture_windows() -> Result<()> {
        let out = run("import os; return os.exec(\"cmd.exe\", [\"/C\", \"echo\", \"hello\"]);")?;
        match out {
            Val::Str(s) => {
                assert_eq!(s.trim_end(), "hello");
            }
            other => panic!("expected Str, got {:?}", other),
        }
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_os_exec_stream_unix() -> Result<()> {
        // Stream mode returns a Stream; collect one item with blocking
        let out =
            run("import os; let s = os.exec(\"/bin/echo\", [\"a\", \"b\"], true); return s.collect_block(1, 2000);")?;
        match out {
            Val::List(l) => {
                assert_eq!(l.len(), 1);
                assert_eq!(l[0], Val::Str("a b".into()));
            }
            other => panic!("expected List, got {:?}", other),
        }
        Ok(())
    }

    #[test]
    #[cfg(windows)]
    fn test_os_exec_stream_windows() -> Result<()> {
        // Stream mode returns a Stream; collect one item with blocking
        let out = run(
            "import os; let s = os.exec(\"cmd.exe\", [\"/C\", \"echo\", \"a b\"], true); return s.collect_block(1, 2000);",
        )?;
        match out {
            Val::List(l) => {
                assert_eq!(l.len(), 1);
                assert_eq!(l[0], Val::Str("a b".into()));
            }
            other => panic!("expected List, got {:?}", other),
        }
        Ok(())
    }
}
