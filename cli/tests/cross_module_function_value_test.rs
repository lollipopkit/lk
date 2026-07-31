//! Passing a function to a function in another file.
//!
//! `apply(double, 5)` is the shape: a higher-order helper in one module, the
//! function it works on in another. It used to fail at run time — a bare
//! closure is a `function_index` into *its own* module's table, so the copy into
//! the callee's heap refused it outright.
//!
//! It now promotes instead: the value crossing the boundary becomes a callable
//! that carries the module it came from, so the index still means what it meant.
//! The promoted callable runs against a **fresh** state, which is what the
//! refusals below are about — a function that needs its module's globals cannot
//! be handed to another module, and says which global and why.

use std::process::Command;

fn lk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lk"))
}

fn run(dir: &std::path::Path, main: &str) -> (String, String, bool) {
    let source = dir.join("main.lk");
    std::fs::write(&source, main).expect("write main");
    let output = lk().arg(&source).output().expect("run lk");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.success(),
    )
}

fn with_helper(dir: &std::path::Path) {
    std::fs::write(
        dir.join("helper.lk"),
        "fn apply(f: (Int) -> Int, x: Int) -> Int { return f(x); }\n\
         fn apply_str(f: (String) -> String, s: String) -> String { return f(s); }\n\
         fn twice(f: (Int) -> Int, x: Int) -> Int { return f(f(x)); }\n",
    )
    .expect("write helper");
}

/// The shapes that cross: a named function, one that calls another function of
/// its own module, a lambda, a lambda with a capture, and a function applied
/// more than once.
#[test]
fn a_function_can_be_passed_to_a_function_in_another_module() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_helper(dir.path());
    let (stdout, stderr, ok) = run(
        dir.path(),
        "use { apply, apply_str, twice } from \"helper\";\n\
         fn double(x: Int) -> Int { return x * 2; }\n\
         fn plus_one(x: Int) -> Int { return x + 1; }\n\
         fn chained(x: Int) -> Int { return plus_one(x) * 2; }\n\
         let n = 7;\n\
         println(apply(double, 5));\n\
         println(apply(chained, 5));\n\
         println(apply(|x| x + n, 1));\n\
         println(apply_str(|s| s + \"!\", \"hi\"));\n\
         println(twice(double, 3));\n",
    );
    assert!(ok, "the program should run: {stderr}");
    assert_eq!(stdout, "10\n12\n8\nhi!\n12\n", "stderr: {stderr}");
}

/// A function that reads one of its module's globals is refused, by the name of
/// the global.
///
/// The promoted callable runs against a fresh state, so that global is nil
/// there. Answering with nil would be a wrong answer that looks like a working
/// program — `x * factor` silently becoming `x * nil`'s error, or worse, zero.
#[test]
fn a_function_that_reads_a_module_global_is_refused_by_name() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_helper(dir.path());
    let (_, stderr, ok) = run(
        dir.path(),
        "use { apply } from \"helper\";\n\
         let factor = 10;\n\
         fn scaled(x: Int) -> Int { return x * factor; }\n\
         println(apply(scaled, 5));\n",
    );
    assert!(!ok, "reading a module global across the boundary must not run");
    assert!(
        stderr.contains("reads the module global `factor`"),
        "the refusal should name the global: {stderr}"
    );
}

/// A function whose body makes a call this check cannot follow — `println` is
/// the everyday one — is refused separately, and told apart from a write.
///
/// It is not known to be wrong, only unproven: a builtin arrives through a
/// register, and nothing says what it reaches. Saying "it writes a global"
/// would have been a guess dressed as a fact.
#[test]
fn a_function_whose_calls_cannot_be_followed_says_so() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_helper(dir.path());
    let (_, stderr, ok) = run(
        dir.path(),
        "use { apply } from \"helper\";\n\
         fn noisy(x: Int) -> Int { println(\"called\"); return x; }\n\
         println(apply(noisy, 5));\n",
    );
    assert!(!ok, "an unprovable body must not cross");
    assert!(
        stderr.contains("makes a call this check cannot follow"),
        "the refusal should name the real reason: {stderr}"
    );
    assert!(
        !stderr.contains("it writes a module global"),
        "an unfollowable call is not a write, and saying so would be a guess: {stderr}"
    );
}

/// A function that writes a module global gets the write's own refusal.
#[test]
fn a_function_that_writes_a_module_global_is_refused_as_a_write() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_helper(dir.path());
    let (_, stderr, ok) = run(
        dir.path(),
        "use { apply } from \"helper\";\n\
         let seen = 0;\n\
         fn record(x: Int) -> Int { seen = x; return x; }\n\
         println(apply(record, 5));\n",
    );
    assert!(!ok, "a write across the boundary must not run");
    assert!(
        stderr.contains("it writes a module global"),
        "the refusal should name the write: {stderr}"
    );
}
