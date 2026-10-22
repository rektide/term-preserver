//! Exercise the short binary's self-worker handoff without a live tmux server.

#![cfg(feature = "tta-cli")]

use std::os::unix::fs::PermissionsExt;

#[test]
fn tper_hands_its_session_attempt_to_its_own_worker() {
    std::fs::create_dir_all("/tmp/opencode").unwrap();
    let dir = tempfile::Builder::new().prefix("tper-worker-").tempdir_in("/tmp/opencode").unwrap();
    let client = dir.path().join("tta stub");
    let config = dir.path().join("config.toml");
    let socket = dir.path().join("unused.sock");
    std::fs::write(&client, "#!/bin/sh\nprintf '%s\\n' 'tper-worker-ok' \"$@\"\n").unwrap();
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o700)).unwrap();
    // No sampler: this checks the executable handoff only, and never queries a
    // user's server. The client is a private argv-printing stub, not tmux/tta.
    std::fs::write(&config, "[tmux]\nsources = []\n").unwrap();
    let mut command = assert_cmd::Command::new(env!("CARGO_BIN_EXE_tper"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("TERM_PRESERVER_") {
            command.env_remove(name);
        }
    }
    let output = command
        .args(["--config", config.to_str().unwrap(), "--tmux", "tper-project", "--session-backend", "tta-cli"])
        .args(["--tta-program", client.to_str().unwrap(), "--tmux-socket", socket.to_str().unwrap()])
        .args(["--shell", "/bin/sh", "--no-login", "--no-interactive", "--", "literal ; argument"])
        .write_stdin("q")
        .timeout(std::time::Duration::from_secs(5))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("tper-worker-ok"), "{stdout}");
    assert!(stdout.contains("tper-project"), "{stdout}");
    assert!(stdout.contains("literal ; argument"), "{stdout}");
    assert!(!socket.exists());
}
