use assert_cmd::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_term-preserver"))
}

#[test]
fn pty_relay_passthrough() {
    let output = bin()
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

#[test]
fn reemit_restores_icon_and_palette() {
    // OSC 1 (icon) and OSC 4 (palette) should be re-emitted on exit alongside
    // the window title when --reemit-title is set.
    let output = bin()
        .args([
            "--reemit-title",
            "true",
            "--",
            "sh",
            "-c",
            "printf '\\033]1;myicon\\007\\033]4;0;rgb:aa/bb/cc\\007'",
        ])
        .write_stdin("q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\x1b]1;myicon\x07"),
        "icon re-emit missing: {stdout:?}"
    );
    assert!(
        stdout.contains("\x1b]4;0;rgb:aa/bb/cc\x07"),
        "palette re-emit missing: {stdout:?}"
    );
}

#[test]
fn print_gone_title_when_enabled() {
    let output = bin()
        .args([
            "--print-gone-title",
            "true",
            "--",
            "sh",
            "-c",
            "printf '\\033]2;user@host\\007'",
        ])
        .write_stdin("q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("gone: user@host"),
        "expected visible gone line, got: {stdout:?}"
    );
    assert!(
        stdout.contains("pid "),
        "expected pid in gone line, got: {stdout:?}"
    );
}

#[test]
fn reemit_restores_fg_bg_cursor() {
    let output = bin()
        .args([
            "--reemit-title",
            "true",
            "--",
            "sh",
            "-c",
            "printf '\\033]10;#112233\\007\\033]11;rgb:00/00/00\\007\\033]12;rgbi:1.0/0.0/0.0\\007'",
        ])
        .write_stdin("q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\x1b]10;#112233\x07"),
        "fg re-emit missing: {stdout:?}"
    );
    assert!(
        stdout.contains("\x1b]11;rgb:00/00/00\x07"),
        "bg re-emit missing: {stdout:?}"
    );
    assert!(
        stdout.contains("\x1b]12;rgbi:1.0/0.0/0.0\x07"),
        "cursor re-emit missing: {stdout:?}"
    );
}

#[test]
fn print_gone_title_silent_when_no_title_captured() {
    // With no window title emitted, the gone line should not appear even when
    // the flag is on.
    let output = bin()
        .args(["--print-gone-title", "true", "--", "sh", "-c", "printf hi"])
        .write_stdin("q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("gone:"),
        "no gone line expected without a title, got: {stdout:?}"
    );
}
