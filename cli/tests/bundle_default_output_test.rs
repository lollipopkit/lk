//! `lk bundle FILE` names the executable itself, the way `lk compile` does.
//!
//! Both commands turn one source file into an executable, and one of them used
//! to work the name out (`path.with_extension("")`) while the other made
//! `--output` mandatory:
//!
//! ```text
//! $ lk bundle app.lk
//! Usage: lk bundle --output <OUT> <FILE>
//! ```
//!
//! That spelling — `lk bundle FILE`, no flag — is the one `CLAUDE.md` and the
//! README document, so the CLI surface and its description disagreed about a
//! required argument.

use assert_cmd::prelude::*;
use std::error::Error;
use std::process::Command;

#[test]
fn bundle_defaults_its_output_to_the_source_without_its_extension() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("greeter.lk");
    std::fs::write(&source, "println(\"bundled\");\n")?;

    Command::cargo_bin("lk")?
        .current_dir(dir.path())
        .args(["bundle", "greeter.lk"])
        .assert()
        .success();

    let produced = dir.path().join("greeter");
    assert!(produced.exists(), "`lk bundle greeter.lk` should write ./greeter");
    let output = Command::new(&produced).output()?;
    assert_eq!(String::from_utf8(output.stdout)?, "bundled\n");

    // …and naming it explicitly still works, to the name given.
    Command::cargo_bin("lk")?
        .current_dir(dir.path())
        .args(["bundle", "--output", "named", "greeter.lk"])
        .assert()
        .success();
    assert!(dir.path().join("named").exists());

    Ok(())
}
