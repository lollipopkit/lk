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

/// A user module's namespace answers the same way a standard library module's
/// does.
///
/// `use * as lib from "./lib.lk"; lib.nothere()` type-checked and died with "nil
/// is not a function" — the same hole as a stdlib member, one layer over, and
/// the checker had the namespace's exports the whole time (it already reports
/// the *arity* of a member that does exist).
#[test]
fn a_namespace_member_that_does_not_exist_is_refused_at_check_time() -> Result<(), Box<dyn Error>> {
    let dir = tempdir()?;
    fs::write(dir.path().join("lib.lk"), "fn hi() { return 1; }\n")?;
    let script_path = dir.path().join("main.lk");
    fs::write(
        &script_path,
        "use * as lib from \"./lib.lk\";\nlet r = lib.nothere();\n",
    )?;

    let mut cmd = Command::cargo_bin("lk")?;
    cmd.args(["check", script_path.to_str().unwrap()]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("`lib` has no member `nothere`"));
    Ok(())
}

/// A *local* named after the namespace takes the name back.
///
/// The predicate here is `has_local_binding`, deliberately not the
/// `lookup_binding` the standard-library check uses — that one counts a
/// namespace as a binding, which is exactly what disqualifies the library
/// reading of `math.f()` and exactly the opposite of what this check wants.
/// Using the wrong one made this check silently never fire.
#[test]
fn a_local_named_after_a_namespace_is_an_ordinary_value() -> Result<(), Box<dyn Error>> {
    let dir = tempdir()?;
    fs::write(dir.path().join("lib.lk"), "fn hi() { return 1; }\n")?;
    let script_path = dir.path().join("main.lk");
    fs::write(
        &script_path,
        "use * as lib from \"./lib.lk\";\n\
         fn main() {\n  let lib = {\"anything\": 1};\n  println(\"${lib.anything}\");\n}\n\
         main();\nprintln(\"${lib.hi()}\");\n",
    )?;

    let mut cmd = Command::cargo_bin("lk")?;
    cmd.args(["check", script_path.to_str().unwrap()]);
    cmd.assert().success();
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

/// `lk check` refuses named arguments to a builtin global, and lets a program's
/// own function of the same name keep its own rules.
///
/// The rule holds for every builtin — none declares a named parameter — so the
/// checker only needs to know which names the standard library registers, not
/// their signatures. Saying it here rather than at run time is the difference
/// between `lk check` passing a program that cannot run and catching it with a
/// span.
#[test]
fn named_arguments_to_a_builtin_are_a_check_time_error() {
    let dir = tempfile::tempdir().expect("temp dir");

    let refused = dir.path().join("refused.lk");
    std::fs::write(&refused, "assert(cond: true);\n").expect("write");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .arg("check")
        .arg(&refused)
        .output()
        .expect("run lk check");
    assert!(!output.status.success(), "the call cannot run, so it must not check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("assert() does not accept named arguments"),
        "the checker should use the same sentence the native does: {stderr}"
    );

    // A program that declares its own `assert` owns the name, and its named
    // parameters are its own business.
    let shadowed = dir.path().join("shadowed.lk");
    std::fs::write(
        &shadowed,
        "fn assert({cond: Bool}) -> Nil { return nil; }\nassert(cond: true);\n",
    )
    .expect("write");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .arg("check")
        .arg(&shadowed)
        .output()
        .expect("run lk check");
    assert!(
        output.status.success(),
        "a user function of the same name keeps its own rules: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `lk check` catches a builtin called with the wrong number of arguments —
/// including the ones whose range only its own body used to know.
///
/// The count is stated once, where the native enforces it, and handed to the
/// checker at registration. Before that it lived in three places (the
/// registry's single `arity`, the body's own check, a hand-written arm in the
/// checker) and only three globals had the third — so `assert(true, "a", "b")`
/// type-checked and then failed.
#[test]
fn builtin_arity_is_a_check_time_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    for (source, expected) in [
        ("assert(true, \"a\", \"b\");\n", "assert() expects 1 or 2 arguments"),
        (
            "assert_eq(1, 1, \"a\", \"b\");\n",
            "assert_eq() expects 2 or 3 arguments",
        ),
        ("spawn(|| 1, 2);\n", "spawn() expects exactly 1 argument"),
        ("recv();\n", "recv() expects exactly 1 argument"),
    ] {
        let path = dir.path().join("case.lk");
        std::fs::write(&path, source).expect("write");
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
            .arg("check")
            .arg(&path)
            .output()
            .expect("run lk check");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "`{source}` must not check");
        assert!(stderr.contains(expected), "`{source}` reported `{stderr}`");
    }

    // A genuinely variadic builtin takes what it is given.
    let ok = dir.path().join("ok.lk");
    std::fs::write(&ok, "println(1, 2, 3);\nprint();\n").expect("write");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .arg("check")
        .arg(&ok)
        .output()
        .expect("run lk check");
    assert!(
        output.status.success(),
        "println is variadic: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `lk check` answers the same question the executors answer.
///
/// It used to run a *stricter* checker than either backend: an unannotated
/// parameter or return type was `Function 'f' infers implicit Any …`, and the
/// same file ran fine and compiled to a native executable. Four of the
/// language's own examples were rejected by the command whose whole job is to
/// be run before running.
///
/// The strict pass is still there behind `--strict`, where it is what it
/// always was: a lint about under-specified signatures.
#[test]
fn check_accepts_what_the_executors_accept_and_strict_is_opt_in() -> Result<(), Box<dyn Error>> {
    let dir = tempdir()?;
    let script_path = dir.path().join("unannotated.lk");
    fs::write(
        &script_path,
        r#"
            fn process(xs) {
                return xs.map(|x| x * x).reduce(0, |a, b| a + b);
            }
            assert(process([1, 2, 3]) == 14);
        "#,
    )?;

    Command::cargo_bin("lk")?
        .args(["check", script_path.to_str().unwrap()])
        .assert()
        .success();

    // …and it really does run, which is what makes the old answer wrong rather
    // than merely strict.
    Command::cargo_bin("lk")?
        .arg(script_path.to_str().unwrap())
        .assert()
        .success();

    Command::cargo_bin("lk")?
        .args(["check", "--strict", script_path.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("infers implicit Any"));

    Ok(())
}
