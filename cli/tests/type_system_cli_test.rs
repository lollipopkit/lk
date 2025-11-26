use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::error::Error;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn reports_numeric_operand_error() -> Result<(), Box<dyn Error>> {
    let dir = tempdir()?;
    let script_path = dir.path().join("bad_numeric.lkr");
    fs::write(
        &script_path,
        r#"
            let lhs = "foo";
            let rhs = 1;
            lhs - rhs;
        "#,
    )?;

    let mut cmd = Command::cargo_bin("lkr")?;
    cmd.args(["check", script_path.to_str().unwrap()]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("must by numeric types"));

    Ok(())
}
