use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::error::Error;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn reports_numeric_operand_error() -> Result<(), Box<dyn Error>> {
    let dir = tempdir()?;
    let script_path = dir.path().join("bad_numeric.lk");
    fs::write(
        &script_path,
        r#"
            let lhs = "foo";
            let rhs = 1;
            lhs - rhs;
        "#,
    )?;

    let mut cmd = Command::cargo_bin("lk")?;
    cmd.args(["check", script_path.to_str().unwrap()]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("must be numeric types"));

    Ok(())
}

#[test]
fn reports_macro_origin_for_macro_generated_type_error() -> Result<(), Box<dyn Error>> {
    let dir = tempdir()?;
    let script_path = dir.path().join("bad_macro_numeric.lk");
    fs::write(
        &script_path,
        r#"
            macro_rules! bad_numeric {
                () => { "foo" - 1; };
            }
            bad_numeric!();
        "#,
    )?;

    let mut cmd = Command::cargo_bin("lk")?;
    cmd.args(["check", script_path.to_str().unwrap()]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Macro origin stack:"))
        .stderr(predicate::str::contains("bad_numeric"));

    Ok(())
}

/// A stdlib member that does not exist is a check-time error.
///
/// `math.nonexistent(1)` used to type-check and die at run time with "nil is not
/// a function" — a sentence naming neither the module nor the member. It is how
/// `os.name()`, `hash.md5(s)` and `datetime.year(t)` end up looking like
/// *missing native lowerings* rather than what they are: names the standard
/// library does not have.
///
/// This needs the real stdlib linked, so it lives here rather than in `lk-core`,
/// whose own tests run with an empty signature registry — and that emptiness is
/// exactly what `has_stdlib_signatures` exists to tell apart from "unknown
/// member".
#[test]
fn a_stdlib_member_that_does_not_exist_is_refused_at_check_time() -> Result<(), Box<dyn Error>> {
    let dir = tempdir()?;
    for (source, module, member) in [
        ("use math;\nlet r = math.nonexistent(1);\n", "math", "nonexistent"),
        ("use string;\nlet r = string.bogus(\"a\");\n", "string", "bogus"),
        ("use os;\nlet r = os.name();\n", "os", "name"),
        ("use hash;\nlet r = hash.md5(\"a\");\n", "hash", "md5"),
    ] {
        let script_path = dir.path().join(format!("{module}_{member}.lk"));
        fs::write(&script_path, source)?;
        let mut cmd = Command::cargo_bin("lk")?;
        cmd.args(["check", script_path.to_str().unwrap()]);
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains(format!("`{module}` has no member `{member}`")));
    }
    Ok(())
}

/// …and everything that is not that keeps working.
///
/// A dotted call is `a.b.c()` for *any* `a`, so the check has to know when it is
/// not looking at the standard library at all: a struct field chain, a map
/// member, a local that happens to be named after a module, a namespace bound by
/// `use * as`, and — the case that would be silently wrong — a variadic export
/// like `path.join`, which has no single-arity signature and would look
/// undeclared to anything that asked `stdlib_signature` instead of asking
/// whether the member exists.
#[test]
fn the_member_check_leaves_everything_else_alone() -> Result<(), Box<dyn Error>> {
    let dir = tempdir()?;
    let script_path = dir.path().join("not_the_stdlib.lk");
    fs::write(
        &script_path,
        r#"
            use math;
            use path;
            struct P { v: Int }
            struct Q { p: P }
            fn main() {
                let q = Q { p: P { v: 1 } };
                println("${q.p.v}");
                let m = {"k": 1};
                println("${m.k}");
                let math = {"foo": 2};
                println("${math.foo}");
                println("${path.join("a", "b")}");
            }
            main();
        "#,
    )?;

    let mut cmd = Command::cargo_bin("lk")?;
    cmd.args(["check", script_path.to_str().unwrap()]);
    cmd.assert().success();
    Ok(())
}
