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
        .write_stdin("q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("hello"),
        "expected stdout to contain 'hello', got: {stdout:?}"
    );
}

#[test]
fn osc_title_passthrough() {
    let output = bin()
        .unwrap()
        .args(["--", "sh", "-c", "printf '\\033]2;my-title\\007visible'"])
        .write_stdin("q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\x1b]2;my-title\x07"),
        "expected stdout to contain OSC title sequence, got: {stdout:?}"
    );
    assert!(
        stdout.contains("visible"),
        "expected stdout to contain 'visible', got: {stdout:?}"
    );
}

#[test]
fn no_title_reemit_by_default() {
    let output = bin()
        .unwrap()
        .args(["--", "sh", "-c", "printf '\\033]2;captured\\007done'"])
        .write_stdin("q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.match_indices("\x1b]2;captured\x07").count();
    assert_eq!(
        first, 1,
        "expected exactly one OSC title sequence, got {first}: {stdout:?}"
    );
}

#[test]
fn reemit_title_when_enabled() {
    let output = bin()
        .unwrap()
        .args([
            "--reemit-title",
            "true",
            "--",
            "sh",
            "-c",
            "printf '\\033]2;captured\\007done'",
        ])
        .write_stdin("q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let count = stdout.match_indices("\x1b]2;captured\x07").count();
    assert!(
        count >= 2,
        "expected at least two OSC title sequences (original + re-emit), got {count}: {stdout:?}"
    );
}
