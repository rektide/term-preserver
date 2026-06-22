use std::process::Command as StdCommand;
use std::thread;
use std::time::Duration;

fn bin() -> StdCommand {
    StdCommand::new(env!("CARGO_BIN_EXE_title-preserver"))
}

#[test]
fn sigterm_forwarded_to_child() {
    let child = bin()
        .args([
            "--",
            "sh",
            "-c",
            "trap \"echo term; exit 42\" TERM; while true; do sleep 0.1; done",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let proxy_pid = child.id();
    thread::sleep(Duration::from_millis(500));

    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(proxy_pid as i32),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("term"),
        "expected stdout to contain 'term', got: {stdout:?}"
    );
}
