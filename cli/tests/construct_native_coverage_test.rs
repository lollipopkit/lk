//! Every construct the language has, lowered natively.
//!
//! The AOT coverage gate walks `examples/`, so it measures the *programs* the
//! repository happens to contain. That is not the same as measuring the
//! language: `?.` had no native lowering at all — the nil test it compiles to
//! had no case for a container receiver, so every field form of the operator
//! dropped its whole program to the interpreter — and no gate saw it, because
//! no example used `?.` on a field.
//!
//! This walks the constructs instead. One minimal program per form, compiled
//! with fallback forbidden, so "compiles" means "lowered fully native".
//!
//! A construct that legitimately cannot lower belongs in `EXPECTED_FALLBACK`
//! with a reason, never quietly removed from the table.

use std::process::Command;

/// Constructs that do not lower yet, each with why. Empty: every form in the
/// table below is native.
const EXPECTED_FALLBACK: &[(&str, &str)] = &[];

/// `(name, source)` — one minimal program per language form.
const CONSTRUCTS: &[(&str, &str)] = &[
    (
        "add",
        "let a = 1;\n\
                 let b = 2;\n\
                 println(a + b);\n",
    ),
    (
        "and",
        "let a = true;\n\
                 println(a && false);\n",
    ),
    (
        "bitand",
        "let a = 6;\n\
                 println(a & 3);\n",
    ),
    (
        "bitnot",
        "let a: u8 = 1;\n\
                 println(~a);\n",
    ),
    (
        "bitor",
        "let a = 6;\n\
                 println(a | 3);\n",
    ),
    (
        "bitxor",
        "let a = 6;\n\
                 println(a ^ 3);\n",
    ),
    (
        "break",
        "let i = 0;\n\
                 while true { i = i + 1;\n\
                 if (i > 2) { break;\n\
                 } } println(i);\n",
    ),
    (
        "cast",
        "let a = 300;\n\
                 println(a as u8);\n",
    ),
    (
        "closure",
        "let f = |x| x + 1;\n\
                 println(f(1));\n",
    ),
    (
        "closure_capture",
        "let n = 1;\n\
                 let f = || n + 1;\n\
                 println(f());\n",
    ),
    (
        "coalesce",
        "let m = {\"k\": 1};\n\
                 println(m.get(\"z\") ?? -1);\n",
    ),
    (
        "compound_assign",
        "let a = 1;\n\
                 a += 2;\n\
                 println(a);\n",
    ),
    (
        "continue",
        "let t = 0;\n\
                 for i in 1..=4 { if ((i % 2) == 0) { continue;\n\
                 } t = t + i;\n\
                 } println(t);\n",
    ),
    (
        "defer",
        "fn f() -> Int { defer println(\"d\");\n\
                 return 1;\n\
                 } println(f());\n",
    ),
    (
        "destructure_list",
        "let [a, b] = [1, 2];\n\
                 println(a + b);\n",
    ),
    (
        "destructure_map",
        "let { k: v } = {\"k\": 1};\n\
                 println(v);\n",
    ),
    (
        "destructure_rest",
        "let m = {\"a\": 1, \"b\": 2};\n\
                 let { a: x, ..rest } = m;\n\
                 println(rest.len());\n",
    ),
    (
        "div",
        "let a = 3;\n\
                 println(a / 2);\n",
    ),
    (
        "eq",
        "let a = 1;\n\
                 println(a == 2);\n",
    ),
    (
        "field",
        "struct P { p: Int } let x = P { p: 1 };\n\
                 println(x.p);\n",
    ),
    (
        "field_assign",
        "struct P { p: Int } let x = P { p: 1 };\n\
                 x.p = 5;\n\
                 println(x.p);\n",
    ),
    (
        "for_list",
        "let t = 0;\n\
                 for x in [1, 2] { t = t + x;\n\
                 } println(t);\n",
    ),
    (
        "for_map",
        "let t = 0;\n\
                 for p in ({\"a\": 1}) { t = t + 1; }\n\
                 println(t);\n",
    ),
    (
        "for_range",
        "let t = 0;\n\
                 for i in 1..=3 { t = t + i;\n\
                 } println(t);\n",
    ),
    (
        "for_set",
        "let t = 0;\n\
                 for x in Set([1, 2]) { t = t + 1;\n\
                 } println(t);\n",
    ),
    (
        "for_str",
        "let t = 0;\n\
                 for c in \"ab\" { t = t + 1;\n\
                 } println(t);\n",
    ),
    (
        "if_else",
        "let a = 1;\n\
                 if (a > 0) { println(\"y\");\n\
                 } else { println(\"n\");\n\
                 }\n",
    ),
    (
        "impl_method",
        "struct P { p: Int } impl P { fn twice(self) -> Int { return self.p * 2;\n\
                 } } println(P { p: 2 }.twice());\n",
    ),
    (
        "in_list",
        "let xs = [1, 2];\n\
                 println(1 in xs);\n",
    ),
    (
        "in_map",
        "let m = {\"k\": 1};\n\
                 println(\"k\" in m);\n",
    ),
    (
        "in_str",
        "let s = \"abc\";\n\
                 println(\"b\" in s);\n",
    ),
    (
        "index_assign",
        "let xs = [1];\n\
                 xs[0] = 5;\n\
                 println(xs[0]);\n",
    ),
    (
        "index_list",
        "let xs = [1];\n\
                 println(xs[0]);\n",
    ),
    (
        "index_map",
        "let m = {\"k\": 1};\n\
                 println(m[\"k\"]);\n",
    ),
    (
        "le",
        "let a = 1;\n\
                 println(a <= 2);\n",
    ),
    (
        "list_concat",
        "let a = [1];\n\
                 println(a + [2]);\n",
    ),
    ("list_lit", "println([1, 2, 3]);\n"),
    (
        "lt",
        "let a = 1;\n\
                 println(a < 2);\n",
    ),
    ("map_lit", "println({\"a\": 1});\n"),
    (
        "map_merge",
        "let a = {\"x\": 1};\n\
                 println(a + {\"y\": 2});\n",
    ),
    (
        "method",
        "let xs = [1];\n\
                 println(xs.len());\n",
    ),
    (
        "mod",
        "let a = 3;\n\
                 println(a % 2);\n",
    ),
    (
        "mul",
        "let a = 3;\n\
                 println(a * 2);\n",
    ),
    (
        "ne",
        "let a = 1;\n\
                 println(a != 2);\n",
    ),
    (
        "neg",
        "let a = 1;\n\
                 println(0 - a);\n",
    ),
    (
        "nested_assign",
        "struct P { m: Map<String, Int> } let x = P { m: {\"a\": 1} };\n\
                 x.m[\"a\"] = 5;\n\
                 println(x.m[\"a\"]);\n",
    ),
    (
        "not",
        "let a = true;\n\
                 println(!a);\n",
    ),
    (
        "optional_field",
        "let m = {\"k\": 1};\n\
                 println(m?.k ?? -1);\n",
    ),
    (
        "or",
        "let a = true;\n\
                 println(a || false);\n",
    ),
    (
        "range_lit",
        "let r = 0..3;\n\
                 println(r.len());\n",
    ),
    (
        "recursion",
        "fn f(n: Int) -> Int { if (n <= 0) { return 0;\n\
                 } return n + f(n - 1);\n\
                 } println(f(5));\n",
    ),
    (
        "set_ops",
        "let a = Set([1]);\n\
                 println(a.union(Set([2])).len());\n",
    ),
    (
        "shl",
        "let a = 1;\n\
                 println(a << 3);\n",
    ),
    (
        "shr",
        "let a = 8;\n\
                 println(a >> 3);\n",
    ),
    (
        "spread",
        "struct P { p: Int, q: Int } let a = P { p: 1, q: 2 };\n\
                 println(P { ..a, q: 3 }.q);\n",
    ),
    (
        "string_concat",
        "let a = \"x\";\n\
                 println(a + \"y\");\n",
    ),
    ("struct_lit", "struct P { p: Int } println(P { p: 1 }.p);\n"),
    (
        "sub",
        "let a = 3;\n\
                 println(a - 1);\n",
    ),
    (
        "template",
        "let n = 1;\n\
                 println(\"v=${n}\");\n",
    ),
    (
        "ternary",
        "let a = 1;\n\
                 println(a > 0 ? \"y\" : \"n\");\n",
    ),
    (
        "trait_dispatch",
        "trait S { fn s(self) -> Int; }\n\
                 struct P { p: Int }\n\
                 impl S for P { fn s(self) -> Int { return self.p; } }\n\
                 fn render(v: Any) -> Int { return v.s(); }\n\
                 println(render(P { p: 7 }));\n",
    ),
    ("try_catch", "println(try { 1 % 0 } catch e { -1 });\n"),
    (
        "unsafe_block",
        "let a = 1;\n\
                 println(a);\n",
    ),
    (
        "unwrap",
        "let xs = [1];\n\
                 println(xs[0]!);\n",
    ),
    (
        "while",
        "let i = 0;\n\
                 while i < 3 { i = i + 1;\n\
                 } println(i);\n",
    ),
];

#[test]
fn every_language_construct_lowers_natively() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut refused = Vec::new();
    let mut stale = Vec::new();

    for (name, source) in CONSTRUCTS {
        let path = dir.path().join(format!("{name}.lk"));
        std::fs::write(&path, source).expect("write construct");

        // A malformed probe would look like a lowering gap, so the type check
        // is asserted separately and loudly.
        let checked = Command::new(env!("CARGO_BIN_EXE_lk"))
            .args(["check", path.to_str().expect("utf-8 path")])
            .output()
            .expect("run lk check");
        assert!(
            checked.status.success(),
            "the `{name}` probe does not type-check, so it measures nothing: {}",
            String::from_utf8_lossy(&checked.stderr)
        );

        let compiled = Command::new(env!("CARGO_BIN_EXE_lk"))
            .args(["compile", path.to_str().expect("utf-8 path")])
            .env("LK_AOT_NO_FALLBACK", "1")
            .env("LK_AOT_HYBRID", "0")
            .output()
            .expect("run lk compile");
        let expected = EXPECTED_FALLBACK.iter().find(|(listed, _)| listed == name);
        match (compiled.status.success(), expected) {
            (true, None) | (false, Some(_)) => {}
            (false, None) => {
                let message = String::from_utf8_lossy(&compiled.stderr).trim().to_string();
                // A linker or disk failure is not a lowering refusal, and
                // reading it as one has wasted a whole investigation before.
                assert!(
                    message.contains("native AOT does not support this program yet"),
                    "`{name}` failed to compile for a reason that is not a lowering refusal: {message}"
                );
                refused.push(format!("{name}: {message}"));
            }
            (true, Some((_, reason))) => stale.push(format!("{name} (listed as: {reason})")),
        }
    }

    assert!(
        refused.is_empty(),
        "constructs that stopped lowering natively:\n{}",
        refused.join("\n")
    );
    assert!(
        stale.is_empty(),
        "listed as unable to lower, but they do — drop them from EXPECTED_FALLBACK:\n{}",
        stale.join("\n")
    );
}
