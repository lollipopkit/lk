//! Every complete program in the tutorial type-checks.
//!
//! Nothing checked the tutorial before this. Running its blocks by hand turned
//! up four defects at once: `let count := 0;` (both `let` and the short
//! declaration), `struct Point { x: Int, y: y: Int }`, a `select` whose cases
//! were separated by commas where the parser wants semicolons, and
//! `for entry in { "a": 1 } { … }` — where the `{` is the loop body, so a map
//! literal cannot be written in a `for` header at all.
//!
//! It also turned up a defect in the *language*: the expression table claimed
//! `[1, 2, 3] - [2]  // [1, 3]`, which both executors answered and the checker
//! refused. The tutorial was right and the implementation was not.
//!
//! A block that is a fragment — an expression table, a snippet naming something
//! an earlier block defined, an example whose point is the error it raises — is
//! fenced ```lk,fragment and skipped. That marker is the whole design: the
//! alternative is a gate that checks nothing because most blocks cannot stand
//! alone, or one that gets disabled the first time a fragment is added.

use std::io::Write;
use std::process::Command;

/// The tutorials this covers. Both translations, because they carry the same
/// code and a fix applied to one is the defect staying in the other.
const TUTORIALS: &[&str] = &["../website/src/learn/LEARN.md", "../website/src/learn/LEARN_zh.md"];

#[test]
fn every_complete_tutorial_example_type_checks() {
    let lk = env!("CARGO_BIN_EXE_lk");
    let dir = std::env::temp_dir().join(format!("lk-tutorial-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("example.lk");
    let mut checked = 0;

    for tutorial in TUTORIALS {
        let source = std::fs::read_to_string(tutorial).unwrap_or_else(|err| panic!("{tutorial}: {err}"));
        for (index, block) in complete_blocks(&source).into_iter().enumerate() {
            let mut handle = std::fs::File::create(&file).expect("write example");
            handle.write_all(block.as_bytes()).expect("write example");
            drop(handle);

            let output = Command::new(lk)
                .arg("check")
                .arg(&file)
                .output()
                .expect("run `lk check`");
            assert!(
                output.status.success(),
                "{tutorial}: complete example #{index} does not check. Fence it \
                 ```lk,fragment if it is deliberately incomplete.\n--- source ---\n{block}\n--- error ---\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            checked += 1;
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        checked >= 40,
        "expected the tutorials to carry examples, checked {checked}"
    );
}

/// The ```lk blocks, without the ```lk,fragment ones.
fn complete_blocks(source: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in source.lines() {
        match (&mut current, line.trim_end()) {
            (None, "```lk") => current = Some(String::new()),
            // Any other info string — `lk,fragment`, `rust`, `bash` — is not a
            // complete example, and is skipped fence and all.
            (None, _) => {}
            (Some(body), "```") => {
                blocks.push(std::mem::take(body));
                current = None;
            }
            (Some(body), _) => {
                body.push_str(line);
                body.push('\n');
            }
        }
    }
    blocks
}
