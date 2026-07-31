use super::{FormatOptions, format_source};
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

fn fmt(src: &str) -> String {
    format_source(src, FormatOptions::default()).expect("source should tokenize")
}

#[test]
fn indents_nested_blocks() {
    let src = "fn main() {\nlet x = 1;\nif x > 0 {\nprintln(x);\n}\n}\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    let x = 1;\n    if x > 0 {\n        println(x);\n    }\n}\n"
    );
}

#[test]
fn dedents_else_arm_without_losing_body_indent() {
    let src = "fn main() {\nif a {\nb();\n} else {\nc();\n}\n}\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    if a {\n        b();\n    } else {\n        c();\n    }\n}\n"
    );
}

#[test]
fn is_idempotent() {
    let src = "fn main() {\n  let m = { \"a\": [1, 2] };\n      if true {\n print(m);\n}\n}\n";
    let once = fmt(src);
    assert_eq!(fmt(&once), once);
}

#[test]
fn braces_inside_strings_do_not_move_the_indent() {
    let src = "fn main() {\nlet s = \"}}}\";\nlet t = '{{{';\nprintln(s);\n}\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    let s = \"}}}\";\n    let t = '{{{';\n    println(s);\n}\n"
    );
}

#[test]
fn braces_inside_comments_do_not_move_the_indent() {
    let src = "fn main() {\n// closes here }\nlet x = 1; /* { */\nprintln(x);\n}\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    // closes here }\n    let x = 1; /* { */\n    println(x);\n}\n"
    );
}

#[test]
fn interpolation_braces_are_string_content() {
    let src = "fn main() {\nlet s = \"${a} and ${b}\";\nprintln(s);\n}\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    let s = \"${a} and ${b}\";\n    println(s);\n}\n"
    );
}

#[test]
fn multiline_raw_string_content_is_verbatim() {
    let src = "fn main() {\nlet s = r#\"\n   keep   me\n      indented }\n\"#;\nprintln(s);\n}\n";
    let out = fmt(src);
    assert!(out.contains("\n   keep   me\n      indented }\n"), "{out}");
    assert_eq!(
        out,
        "fn main() {\n    let s = r#\"\n   keep   me\n      indented }\n\"#;\n    println(s);\n}\n"
    );
}

#[test]
fn block_comment_body_is_verbatim() {
    let src = "fn main() {\n/* box\n *  art\n */\nlet x = 1;\n}\n";
    assert_eq!(fmt(src), "fn main() {\n    /* box\n *  art\n */\n    let x = 1;\n}\n");
}

#[test]
fn comment_lines_between_statements_are_reindented() {
    let src = "fn main() {\nlet a = 1;\n// leading comment\nlet b = 2;\n}\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    let a = 1;\n    // leading comment\n    let b = 2;\n}\n"
    );
}

#[test]
fn indents_method_chain_continuations_one_level() {
    let src = "let r = [1, 2, 3]\n.filter(|x| x > 1)\n.map(|x| x * x);\n";
    let out = fmt(src);
    assert_eq!(out, "let r = [1, 2, 3]\n    .filter(|x| x > 1)\n    .map(|x| x * x);\n");
    assert_eq!(fmt(&out), out);
}

#[test]
fn continuation_indent_stacks_on_block_depth_not_on_itself() {
    let src = "fn main() {\nlet r = xs\n.map(f)\n.sum();\n}\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    let r = xs\n        .map(f)\n        .sum();\n}\n"
    );
}

#[test]
fn leading_negative_list_elements_are_not_continuations() {
    let src = "let xs = [\n1,\n-2,\n-x,\n];\n";
    assert_eq!(fmt(src), "let xs = [\n    1,\n    -2,\n    -x,\n];\n");
}

/// A line that opens two brackets still costs exactly one level: bracket depth
/// would say two, but nothing is ever written at the level in between.
#[test]
fn a_line_opening_two_brackets_indents_one_level() {
    // `docs/macros.md` — a macro invocation whose argument is a block.
    let src = "unless!(x == 9 {\nprintln(\"not nine\");\n});\n";
    let out = fmt(src);
    assert_eq!(out, "unless!(x == 9 {\n    println(\"not nine\");\n});\n");
    assert_eq!(fmt(&out), out);

    let call = "foo(bar(\nbaz,\n));\n";
    assert_eq!(fmt(call), "foo(bar(\n    baz,\n));\n");
}

/// The clamp must not lose the *real* depth: closing both brackets on separate
/// lines still walks back down one level at a time, ending at column 0.
#[test]
fn stepwise_nesting_keeps_every_level() {
    let src = "foo(\nbar(\na\n)\n);\nlet after = 1;\n";
    assert_eq!(fmt(src), "foo(\n    bar(\n        a\n    )\n);\nlet after = 1;\n");
}

#[test]
fn statement_after_a_multi_bracket_block_returns_to_column_zero() {
    let src = "unless!(x == 9 {\nprintln(1);\n});\nlet after = 1;\n";
    assert_eq!(fmt(src), "unless!(x == 9 {\n    println(1);\n});\nlet after = 1;\n");
}

#[test]
fn normalizes_trailing_whitespace_and_final_newline() {
    assert_eq!(fmt("let x = 1;   \n\n\n"), "let x = 1;\n");
    assert_eq!(fmt("let x = 1;"), "let x = 1;\n");
    assert_eq!(fmt(""), "");
    assert_eq!(fmt("\n  \n"), "");
}

#[test]
fn blank_lines_between_statements_survive_without_indent() {
    assert_eq!(
        fmt("fn main() {\nlet a = 1;\n\nlet b = 2;\n}\n"),
        "fn main() {\n    let a = 1;\n\n    let b = 2;\n}\n"
    );
}

#[test]
fn preserves_crlf_line_endings() {
    let out = fmt("fn main() {\r\nlet x = 1;\r\n}\r\n");
    assert_eq!(out, "fn main() {\r\n    let x = 1;\r\n}\r\n");
}

#[test]
fn honors_indent_width_and_tabs() {
    let src = "fn main() {\nlet x = 1;\n}\n";
    let two = format_source(
        src,
        FormatOptions {
            indent_width: 2,
            use_tabs: false,
        },
    )
    .unwrap();
    assert_eq!(two, "fn main() {\n  let x = 1;\n}\n");
    let tabs = format_source(
        src,
        FormatOptions {
            indent_width: 4,
            use_tabs: true,
        },
    )
    .unwrap();
    assert_eq!(tabs, "fn main() {\n\tlet x = 1;\n}\n");
}

#[test]
fn rejects_sources_that_do_not_tokenize() {
    assert!(format_source("let s = \"unterminated;\n", FormatOptions::default()).is_err());
    assert!(format_source("/* never closed\n", FormatOptions::default()).is_err());
}

#[test]
fn multibyte_source_keeps_byte_exact_content() {
    let src = "fn main() {\nlet s = \"中文字符串 }\";\n// 注释 {\nprintln(s);\n}\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    let s = \"中文字符串 }\";\n    // 注释 {\n    println(s);\n}\n"
    );
}

/// A CRLF file stays CRLF, and an LF file stays LF even when a string literal
/// contains the two bytes that spell CRLF.
///
/// Deciding by "does `\r\n` appear anywhere" reads content as if it were
/// structure: one escaped sequence inside a string would rewrite every line
/// ending in the file, which is the kind of diff nobody can explain.
#[test]
fn line_endings_come_from_the_lines_not_the_content() {
    let crlf = "fn main() {\r\nlet x = 1;\r\n}\r\n";
    assert_eq!(fmt(crlf), "fn main() {\r\n    let x = 1;\r\n}\r\n");

    let lf_with_crlf_inside_a_string = "let s = \"a\\r\\nb\";\nlet y = 2;\n";
    assert_eq!(fmt(lf_with_crlf_inside_a_string), "let s = \"a\\r\\nb\";\nlet y = 2;\n");
}

/// Two lines at the same bracket depth land in the same column.
///
/// The indent used to be `depth.min(prev_level + 1)` — a clamp meant to stop a
/// multi-bracket open from jumping two levels, which also made the column
/// depend on how many lines had already been emitted. A wrapped argument list
/// after a two-bracket open therefore climbed a staircase, one level per line,
/// which is the one thing a re-indenter cannot get wrong and still be one.
#[test]
fn lines_at_the_same_depth_get_the_same_column() {
    let src = "fn f() {\nwrite([110, 97,\n32, 116,\n102, 105]);\n}\n";
    assert_eq!(
        fmt(src),
        "fn f() {\n    write([110, 97,\n        32, 116,\n        102, 105]);\n}\n"
    );
}

/// A line led by an infix operator continues the previous one.
///
/// The rule is "a token that can never *start* an expression". Of the infix
/// operators only `-` can (`-x` is negation), so `+`, `*`, `/`, `%` and `&`
/// were being dedented to statement level for no reason. `|` is decided by
/// looking further: `|x|` is a lambda's parameter list, `| expr` is a bitwise
/// or.
#[test]
fn an_infix_lead_is_a_continuation_but_a_lambda_is_not() {
    let src = "fn f() {\nlet d = (a & 1)\n| (b << 16)\n| c;\nlet e = a\n+ b\n% 3;\n}\n";
    assert_eq!(
        fmt(src),
        "fn f() {\n    let d = (a & 1)\n        | (b << 16)\n        | c;\n    let e = a\n        + b\n        % 3;\n}\n"
    );

    // The `|y|` here opens a lambda, so the line is a list element at the
    // bracket's own level — not one column further in.
    let src = "fn f() {\nlet fs = [|x| x + 1,\n|y| y * 2];\n}\n";
    assert_eq!(fmt(src), "fn f() {\n    let fs = [|x| x + 1,\n        |y| y * 2];\n}\n");
}
