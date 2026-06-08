use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_title-proxy"))
}

#[test]
fn cli_command_overrides_default() {
    let output = bin()
        .args(["--", "sh", "-c", "printf ok"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ok"), "stdout: {stdout:?}");
}

#[test]
fn command_not_found_reports_error() {
    let output = bin()
        .args(["--", "nonexistent_command_xyz"])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Error") || !output.status.success(),
        "expected error for nonexistent command"
    );
}

#[test]
fn help_documents_options() {
    let output = bin().arg("--help").output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--config"));
    assert!(stdout.contains("--forward-signals"));
    assert!(stdout.contains("--reemit-title"));
}
