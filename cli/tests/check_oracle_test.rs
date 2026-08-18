//! What `lk check` must refuse, and what it must not.
//!
//! Every other differential gate compares the two *engines*; this one compares
//! the checker against the language. A mistake it lets through is a program
//! that fails later — at run time, or on one backend only — and a valid program
//! it refuses is a feature nobody can use. Both directions are here because the
//! two failures look nothing alike and only one of them is loud.
//!
//! Four defects came out of writing this table: a trait impl could carry a
//! method the trait never declared, a struct literal could write one field
//! twice, `use math as m;` made `m.nope()` uncheckable, and `P { ..5 }` was a
//! run-time error with a message naming the desugaring. The cases that pass
//! *by design* are listed too, with the reason — they are the ones a later
//! reader would otherwise "fix".

use std::process::Command;

fn check(label: &str, source: &str) -> (bool, String) {
    let dir = std::env::temp_dir().join(format!("lk_check_oracle_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    // One file per case: the two tests run in parallel and a shared path makes
    // them read each other's source.
    let slug: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let file = dir.join(format!("{slug}.lk"));
    std::fs::write(&file, source).expect("write case");
    let out = Command::new(env!("CARGO_BIN_EXE_lk"))
        .arg("check")
        .arg(&file)
        .output()
        .expect("run lk check");
    let message = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_file(&file);
    (out.status.success(), message)
}

/// Mistakes the checker has to name, with the shape that produces each.
const MUST_REFUSE: &[(&str, &str)] = &[
    (
        "too few arguments",
        "fn f(a: Int, b: Int) -> Int { return a + b; }\nprintln(f(1));\n",
    ),
    (
        "too many arguments",
        "fn f(a: Int) -> Int { return a; }\nprintln(f(1, 2));\n",
    ),
    (
        "argument type",
        "fn f(a: Int) -> Int { return a; }\nprintln(f(\"x\"));\n",
    ),
    ("return type", "fn f() -> Int { return \"x\"; }\nprintln(f());\n"),
    ("undefined name", "println(nope);\n"),
    ("unknown method", "println(\"a\".nope());\n"),
    (
        "unknown field",
        "struct P { x: Int }\nlet p = P { x: 1 };\nprintln(p.y);\n",
    ),
    (
        "missing field",
        "struct P { x: Int, y: Int }\nlet p = P { x: 1 };\nprintln(p.x);\n",
    ),
    (
        "extra field",
        "struct P { x: Int }\nlet p = P { x: 1, z: 2 };\nprintln(p.x);\n",
    ),
    (
        "repeated field",
        "struct P { x: Int, y: Int }\nlet p = P { x: 1, x: 2, y: 3 };\nprintln(p.x);\n",
    ),
    ("annotation mismatch", "let x: Int = \"s\";\nprintln(x);\n"),
    ("nil into non-nullable", "let x: Int = nil;\nprintln(x);\n"),
    (
        "unknown type name",
        "fn f(a: Nope) -> Int { return 1; }\nprintln(f(1));\n",
    ),
    (
        "impl of an unknown trait",
        "struct P { x: Int }\nimpl Nope for P { fn a(self) -> Int { return 1; } }\nprintln(1);\n",
    ),
    (
        "impl missing a method",
        "trait T { fn a(self) -> Int; }\nstruct P { x: Int }\nimpl T for P { }\nprintln(1);\n",
    ),
    (
        "impl signature",
        "trait T { fn a(self) -> Int; }\nstruct P { x: Int }\nimpl T for P { fn a(self) -> String { return \"s\"; } }\nprintln(1);\n",
    ),
    (
        "method the trait never declared",
        "trait T { fn a(self) -> Int; }\nstruct P { x: Int }\nimpl T for P { fn a(self) -> Int { return 1; } fn b(self) -> Int { return 2; } }\nprintln(1);\n",
    ),
    (
        "duplicate fn",
        "fn f() -> Int { return 1; }\nfn f() -> Int { return 2; }\nprintln(f());\n",
    ),
    (
        "duplicate struct",
        "struct P { x: Int }\nstruct P { y: Int }\nprintln(1);\n",
    ),
    (
        "duplicate parameter",
        "fn f(a: Int, a: Int) -> Int { return a; }\nprintln(f(1,2));\n",
    ),
    ("break outside a loop", "break;\n"),
    ("assignment to a const", "const C = 1;\nC = 2;\nprintln(C);\n"),
    ("call a scalar", "let n = 5;\nprintln(n(1));\n"),
    ("bit operand", "println(1 & true);\n"),
    ("closure arity", "let f = |a, b| a + b;\nprintln(f(1));\n"),
    (
        "function-type argument",
        "fn take(g: (Int) -> Int) -> Int { return g(1); }\nprintln(take(|a, b| a));\n",
    ),
    ("assignment changes type", "let x: Int = 1;\nx = \"s\";\nprintln(x);\n"),
    ("for over a scalar", "for x in 5 { println(x); }\n"),
    (
        "trait declared twice",
        "trait T { fn a(self) -> Int; }\ntrait T { fn b(self) -> Int; }\nprintln(1);\n",
    ),
    (
        "let over a declaration",
        "fn pick() -> Int { return 1; }\nlet pick = 2;\nprintln(pick);\n",
    ),
    ("import of a missing name", "use { nope } from math;\nprintln(1);\n"),
    ("member of an aliased module", "use math as m;\nprintln(m.nope(1));\n"),
    (
        "spread of a scalar",
        "struct P { x: Int }\nlet p = P { ..5 };\nprintln(p.x);\n",
    ),
    (
        "struct field type",
        "struct P { x: Int }\nlet p = P { x: \"s\" };\nprintln(p.x);\n",
    ),
    ("method on the wrong receiver", "println([1,2].upper());\n"),
];

/// Valid programs, including the ones a stricter reading would reject.
const MUST_ACCEPT: &[(&str, &str)] = &[
    ("empty list annotation", "let xs: List<Int> = [];\nprintln(xs);\n"),
    ("heterogeneous list", "let xs = [1, \"a\"];\nprintln(xs);\n"),
    (
        "nullable field",
        "struct P { x: Int? }\nlet p = P { x: nil };\nprintln(p.x);\n",
    ),
    (
        "Int where Float is declared",
        "fn f(x: Float) -> Float { return x; }\nprintln(f(1));\n",
    ),
    (
        "annotated closure",
        "let f: (Int) -> Int = |x| x + 1;\nprintln(f(1));\n",
    ),
    (
        "trait default",
        "trait G { fn hi(self) -> String { return \"h\"; } }\nstruct P { x: Int }\nimpl G for P {}\nprintln(P { x: 1 }.hi());\n",
    ),
    (
        "inherent impl beside a trait impl",
        "trait T { fn a(self) -> Int; }\nstruct P { x: Int }\nimpl T for P { fn a(self) -> Int { return 1; } }\nimpl P { fn b(self) -> Int { return 2; } }\nprintln(P{x:1}.b());\n",
    ),
    (
        "union parameter",
        "fn f(x: Int | String) -> String { return \"{}\".format(x); }\nprintln(f(1));\n",
    ),
    (
        "try as an expression",
        "let v = try { 1 } catch e { 2 };\nprintln(v);\n",
    ),
    ("member of an alias", "use math as m;\nprintln(m.abs(0 - 3));\n"),
    (
        "spread of a struct",
        "struct P { x: Int }\nlet b = P { x: 1 };\nlet q = P { ..b, x: 2 };\nprintln(q.x);\n",
    ),
    // Accepted *by design*, and each for a stated reason.
    ("truthiness", "if 5 { println(1); }\n"),
    ("repeated map key", "let m = {\"a\": 1, \"a\": 2};\nprintln(m);\n"),
    ("top-level return", "return 1;\n"),
    (
        "index is not nullable",
        "let m = {\"a\": 1};\nlet v: Int = m[\"a\"];\nprintln(v);\n",
    ),
];

#[test]
fn check_refuses_what_it_must() {
    let mut wrong = Vec::new();
    for (label, source) in MUST_REFUSE {
        let (accepted, _) = check(label, source);
        if accepted {
            wrong.push(*label);
        }
    }
    assert!(wrong.is_empty(), "`lk check` accepted these mistakes: {wrong:?}");
}

#[test]
fn check_accepts_what_it_must() {
    let mut wrong = Vec::new();
    for (label, source) in MUST_ACCEPT {
        let (accepted, message) = check(label, source);
        if !accepted {
            wrong.push(format!("{label}: {}", message.lines().next().unwrap_or("").trim()));
        }
    }
    assert!(
        wrong.is_empty(),
        "`lk check` refused these valid programs:\n  {}",
        wrong.join("\n  ")
    );
}
