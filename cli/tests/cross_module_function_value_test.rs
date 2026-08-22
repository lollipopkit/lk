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

/// The way back: a function *returned* by an imported function.
///
/// `make_adder(5)` builds a closure inside the other module and hands it over.
/// This direction needs no help from the executor — the module is the callee's
/// own — but it was refused for exactly as long as the argument direction was,
/// and fixing one without the other would have made `apply(make_adder(5), 1)`
/// half-work.
#[test]
fn a_function_can_be_returned_from_another_module() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("mk.lk"),
        "fn make_adder(n: Int) -> (Int) -> Int { return |x| x + n; }\n\
         fn apply(f: (Int) -> Int, x: Int) -> Int { return f(x); }\n\
         fn pass_through(f: (Int) -> Int, x: Int) -> Int { return apply(f, x); }\n",
    )
    .expect("write module");
    let (stdout, stderr, ok) = run(
        dir.path(),
        "use { make_adder, apply, pass_through } from \"mk\";\n\
         fn double(x: Int) -> Int { return x * 2; }\n\
         let add5 = make_adder(5);\n\
         println(add5(1));\n\
         println(apply(add5, 1));\n\
         println(pass_through(double, 4));\n\
         let fs = [double, double];\n\
         println(apply(fs[0], 3));\n\
         struct Box { f: (Int) -> Int }\n\
         let b = Box { f: double };\n\
         println(apply(b.f, 6));\n",
    );
    assert!(ok, "the program should run: {stderr}");
    assert_eq!(stdout, "6\n6\n8\n6\n12\n", "stderr: {stderr}");
}

/// Named arguments cross too — including a function passed by name.
///
/// Two separate things had to be true. The compiler collects named-call
/// signatures from *this* program's declarations, so an imported function had
/// none and the call failed to compile with a sentence about the compiler's
/// bookkeeping (`Compiler missing named-call signature`) — while the identical
/// call to a local function worked. And the named argument path had its own
/// copy of the argument copying, which still refused a function.
#[test]
fn named_arguments_cross_a_module_boundary() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("named.lk"),
        "fn scale({value: Int, by: Int}) -> Int { return value * by; }\n\
         fn apply({f: (Int) -> Int, x: Int}) -> Int { return f(x); }\n",
    )
    .expect("write module");
    let (stdout, stderr, ok) = run(
        dir.path(),
        "use { scale, apply } from \"named\";\n\
         fn double(x: Int) -> Int { return x * 2; }\n\
         println(scale(value: 3, by: 4));\n\
         println(apply(f: double, x: 5));\n",
    );
    assert!(ok, "the program should run: {stderr}");
    assert_eq!(stdout, "12\n10\n", "stderr: {stderr}");
}

/// A crossing that cannot name its source module still refuses — and says which
/// crossing it was.
///
/// A channel payload is copied by a function that is handed two heaps and
/// nothing else, so there is no module to promote against. The message says so
/// rather than describing the argument case it no longer applies to.
#[test]
fn a_channel_payload_still_refuses_a_function_and_says_why() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (_, stderr, ok) = run(
        dir.path(),
        "use chan;\n\
         fn double(x: Int) -> Int { return x * 2; }\n\
         let c = chan.new(1);\n\
         chan.send(c, double);\n",
    );
    assert!(!ok, "a function through a channel must not run");
    assert!(
        stderr.contains("a channel payload"),
        "the refusal should name the crossing it is about: {stderr}"
    );
}
