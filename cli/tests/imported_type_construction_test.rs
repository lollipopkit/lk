//! Constructing a type another module declares.
//!
//! Every `struct S` gets a generated top-level `fn S$new({…}) -> S` beside it
//! (`stmt::struct_ctors`), and `m.S { … }` is parse-time sugar for calling it.
//! The constructor runs in the declaring module, so the object it returns
//! carries that module's `TypeScope` and its methods dispatch.
//!
//! `use { S } from "m"` now binds that constructor under the name, so the bare
//! spellings — `S { … }` and `S(field: …)` — reach the same sugar. What stays
//! refused is a bare literal for a type this file only sees through a namespace
//! import: there the name is not bound to anything, and `NewObject` would stamp
//! the *constructing* module's scope, producing a same-named type with none of
//! the methods.

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

fn with_geo(dir: &std::path::Path) {
    std::fs::write(
        dir.join("geo.lk"),
        "struct P { x: Int }\n\
         impl P { fn norm(self) -> Int { return self.x * self.x; } }\n\
         trait Shape { fn area(self) -> Int; }\n\
         struct Sq { s: Int }\n\
         impl Shape for Sq { fn area(self) -> Int { return self.s * self.s; } }\n\
         type Pair = List<Int>;\n",
    )
    .expect("write geo");
}

/// Both bare spellings build the declaring module's type: the value renders and
/// answers `typeof` as `P`, and — the part a same-named local type would fail —
/// its methods resolve.
#[test]
fn a_type_imported_by_name_is_constructible_by_its_bare_name() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_geo(dir.path());
    let (stdout, stderr, ok) = run(
        dir.path(),
        "use { P } from \"geo\";\n\
         let a = P { x: 4 };\n\
         let b = P(x: 3);\n\
         println(a);\n\
         println(typeof(a));\n\
         println(a.norm());\n\
         println(b.norm());\n\
         println(a == P { x: 4 });\n",
    );
    assert!(ok, "stderr: {stderr}");
    assert_eq!(stdout, "P{x:4}\nP\n16\n9\ntrue\n");
}

/// An alias renames the binding, not the type: `use { P as Q }` makes `Q { … }`
/// build a `P` — same identity, same methods, and `typeof` still answers `P`.
#[test]
fn an_alias_renames_the_binding_not_the_type() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_geo(dir.path());
    let (stdout, stderr, ok) = run(
        dir.path(),
        "use { P as Q } from \"geo\";\n\
         let a = Q { x: 4 };\n\
         let b = Q(x: 4);\n\
         println(a);\n\
         println(typeof(a));\n\
         println(a.norm());\n\
         println(a == b);\n",
    );
    assert!(ok, "stderr: {stderr}");
    assert_eq!(stdout, "P{x:4}\nP\n16\ntrue\n");

    // And the schema it is checked against is the declaring module's, named
    // by the declaring module's spelling.
    let (_, stderr, ok) = run(dir.path(), "use { P as Q } from \"geo\";\nlet a = Q { z: 4 };\n");
    assert!(!ok, "an undeclared field is refused");
    assert!(stderr.contains("struct 'P'"), "{stderr}");
}

/// The same type reached through a namespace import: `m.P { … }` works, and the
/// value it builds is interchangeable with the one the bare spelling builds —
/// one identity, two spellings.
#[test]
fn the_namespace_spelling_builds_the_same_identity() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_geo(dir.path());
    let (stdout, stderr, ok) = run(
        dir.path(),
        "use \"geo\";\n\
         use { P } from \"geo\";\n\
         let a = geo.P { x: 4 };\n\
         let b = P { x: 4 };\n\
         println(a == b);\n\
         println(typeof(a) == typeof(b));\n\
         println(a.norm() + b.norm());\n",
    );
    assert!(ok, "stderr: {stderr}");
    assert_eq!(stdout, "true\ntrue\n32\n");
}

/// Seen only through a namespace, the bare literal is still refused, and the
/// message names both ways out.
#[test]
fn a_type_seen_only_through_a_namespace_is_not_constructible_by_its_bare_name() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_geo(dir.path());
    let (_, stderr, ok) = run(dir.path(), "use \"geo\";\nlet a = P { x: 4 };\nprintln(a);\n");
    assert!(!ok, "a bare literal for a namespace-visible type is refused");
    assert!(stderr.contains("only sees it through its namespace"), "{stderr}");
    assert!(stderr.contains("geo.P") || stderr.contains("m.P"), "{stderr}");
    assert!(stderr.contains("use { P } from"), "{stderr}");
}

/// A `trait` has no constructor to bind, so importing one by name is refused —
/// with the reason, not with "not an export".
#[test]
fn a_trait_cannot_be_imported_by_name() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_geo(dir.path());
    let (_, stderr, ok) = run(dir.path(), "use { Shape } from \"geo\";\nprintln(1);\n");
    assert!(!ok, "importing a trait by name is refused");
    assert!(stderr.contains("Shape"), "{stderr}");
    assert!(stderr.contains("no constructor to bind"), "{stderr}");
}

/// A `type` alias cannot be imported by name either, and the refusal does not
/// claim the module declares no such thing.
///
/// The message used to end "and this module declares neither" — a fact it had
/// not checked. `type Pair = List<Int>;` *is* declared, and got told it was
/// not.
#[test]
fn a_type_alias_is_refused_without_denying_it_exists() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_geo(dir.path());
    let (_, stderr, ok) = run(dir.path(), "use { Pair } from \"geo\";\nprintln(1);\n");
    assert!(!ok, "importing a type alias by name is refused");
    assert!(stderr.contains("Pair"), "{stderr}");
    assert!(
        stderr.contains("compile-time only"),
        "the refusal should say why a `type` cannot be a binding: {stderr}"
    );
    assert!(
        !stderr.contains("declares neither"),
        "the refusal must not claim the module declares no such thing: {stderr}"
    );
}

/// The two backends agree on a struct built in another file — through either
/// spelling.
///
/// The bundler merged an imported module's `impl` blocks but not its `struct`
/// declarations, and a declaration is what earns a type its runtime id: without
/// one `NewObject` skipped `obj_mark`, so native display rendered the carrier
/// (`{"x":4}`) where the VM prints `P{x:4}`. Method dispatch was unaffected —
/// it reads the *static* provenance — so the wrong answer showed up only in
/// output, and only for a type declared one file away.
#[cfg(feature = "aot")]
#[test]
fn the_two_backends_agree_on_a_struct_declared_in_another_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_geo(dir.path());
    for (name, main) in [
        (
            "item",
            "use { P } from \"geo\";\n\
             fn main() -> Int { let a = P { x: 4 }; println(a); println(typeof(a)); println(a.norm()); return 0; }\n\
             main();\n",
        ),
        (
            "trait_impl",
            "use \"geo\";\n\
             fn main() -> Int { let a = geo.Sq { s: 3 }; println(a); println(a.area()); return 0; }\n\
             main();\n",
        ),
        (
            "namespace",
            "use \"geo\";\n\
             fn main() -> Int { let a = geo.P { x: 4 }; println(a); println(typeof(a)); println(a.norm()); return 0; }\n\
             main();\n",
        ),
    ] {
        let source = dir.path().join(format!("{name}.lk"));
        std::fs::write(&source, main).expect("write main");
        let vm = lk().arg(&source).output().expect("run vm");
        assert!(vm.status.success(), "[{name}] {}", String::from_utf8_lossy(&vm.stderr));

        let compiled = lk()
            .current_dir(dir.path())
            .args(["compile", &format!("{name}.lk")])
            .env("LK_AOT_HYBRID", "0")
            .env("LK_AOT_NO_FALLBACK", "1")
            .output()
            .expect("compile natively");
        assert!(
            compiled.status.success(),
            "[{name}] native compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let native = std::process::Command::new(dir.path().join(name))
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .expect("run native");
        assert_eq!(
            String::from_utf8_lossy(&vm.stdout),
            String::from_utf8_lossy(&native.stdout),
            "[{name}] stdout diverged"
        );
    }
}

/// A local declaration of the same name wins over the *aliased* import too,
/// and the checker has to agree with the compiler about which one it is.
///
/// The compiler picks by the local `Q$new`'s existence, so it always built the
/// local type; the registry cleared its "declared elsewhere" mark on a local
/// declaration but not its "imported by name" one, and that one is keyed by the
/// bound name — so `Q { z: 4 }` was checked against `P`'s schema and refused
/// for a field the local `Q` declares.
#[test]
fn a_local_declaration_wins_over_an_aliased_import() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_geo(dir.path());
    let (stdout, stderr, ok) = run(
        dir.path(),
        "use { P as Q } from \"geo\";\n\
         struct Q { z: Int }\n\
         let q = Q { z: 4 };\n\
         println(q);\n\
         println(typeof(q));\n",
    );
    assert!(ok, "stderr: {stderr}");
    assert_eq!(stdout, "Q{z:4}\nQ\n");
}

/// A local declaration of the same name wins: the bare literal builds the local
/// type, and the import does not shadow it.
#[test]
fn a_local_declaration_of_the_same_name_wins() {
    let dir = tempfile::tempdir().expect("temp dir");
    with_geo(dir.path());
    let (stdout, stderr, ok) = run(
        dir.path(),
        "use \"geo\";\n\
         struct P { x: Int }\n\
         impl P { fn norm(self) -> Int { return self.x + 1; } }\n\
         let a = P { x: 4 };\n\
         println(a.norm());\n\
         println(geo.P { x: 4 }.norm());\n",
    );
    assert!(ok, "stderr: {stderr}");
    assert_eq!(stdout, "5\n16\n");
}
