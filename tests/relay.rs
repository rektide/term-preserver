use assert_cmd::cargo::CargoError;
use assert_cmd::Command;

fn bin() -> Result<assert_cmd::Command, CargoError> {
    Command::cargo_bin("title-proxy")
}

#[test]
fn pty_relay_passthrough() {
    let output = bin()
        .unwrap()
        .args(["--", "sh", "-c", "printf hello"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("hello"),
        "expected stdout to contain 'hello', got: {stdout:?}"
    );
}
