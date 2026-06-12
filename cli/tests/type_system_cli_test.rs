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
        .stderr(predicate::str::contains("must by numeric types"));

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
