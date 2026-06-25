use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_term-preserver"))
}

#[test]
fn prompt_appears_after_child_exit() {
    let mut child = bin()
        .args(["--", "sh", "-c", "printf marker; exit 7"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdout = child.stdout.take().unwrap();
    let mut stdin = child.stdin.take().unwrap();

    let mut buf = vec![0u8; 4096];
    let mut all_output = Vec::new();

    loop {
        thread::sleep(Duration::from_millis(100));
        match stdout.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                all_output.extend_from_slice(&buf[..n]);
                let s = String::from_utf8_lossy(&all_output);
                if s.contains("space=retry") {
                    break;
                }
            }
        }
    }

    let _ = stdin.write_all(b"q");
    drop(stdin);

    let result = child.wait_with_output().unwrap();
    let remaining = result.stdout;
    all_output.extend(remaining);
    let stdout = String::from_utf8_lossy(&all_output);
    let stderr_out = String::from_utf8_lossy(&result.stderr);
    assert!(
        stdout.contains("marker"),
        "stdout: {stdout:?}, stderr: {stderr_out:?}"
    );
    assert!(
        stdout.contains("space=retry q=quit"),
        "stdout: {stdout:?}, stderr: {stderr_out:?}"
    );
    assert!(
        stdout.contains("code 7"),
        "stdout: {stdout:?}, stderr: {stderr_out:?}"
    );
}

#[test]
fn space_retries_child() {
    let mut child = bin()
        .args(["--", "sh", "-c", "printf run; exit 0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdout = child.stdout.take().unwrap();
    let mut stdin = child.stdin.take().unwrap();

    let mut buf = vec![0u8; 4096];
    let mut all_output = Vec::new();

    loop {
        thread::sleep(Duration::from_millis(100));
        match stdout.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                all_output.extend_from_slice(&buf[..n]);
                let s = String::from_utf8_lossy(&all_output);
                if s.contains("space=retry") {
                    break;
                }
            }
        }
    }

    let _ = stdin.write_all(b" ");
    thread::sleep(Duration::from_millis(500));

    loop {
        thread::sleep(Duration::from_millis(100));
        match stdout.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                all_output.extend_from_slice(&buf[..n]);
                let s = String::from_utf8_lossy(&all_output);
                if s.matches("run").count() >= 2 {
                    break;
                }
            }
        }
    }

    let _ = stdin.write_all(b"q");
    drop(stdin);

    let mut remaining = Vec::new();
    let _ = stdout.read_to_end(&mut remaining);
    all_output.extend(remaining);

    let _ = child.wait();
    let stdout = String::from_utf8_lossy(&all_output);
    let count = stdout.matches("run").count();
    assert!(
        count >= 2,
        "expected 'run' at least twice, got {count}: {stdout:?}"
    );
}
