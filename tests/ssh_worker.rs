//! Local process carrier, NOT SSH evidence. Never contacts a host or tmux.
#![cfg(feature = "tta-lib")]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use term_preserver::launch::LaunchConfig;
use term_preserver::launcher::SessionBackend;

fn executable(path: &Path, source: &str) {
    std::fs::write(path, source).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn failed_prepare_reports_earlier_and_failed_evidence_without_handoff() {
    let dir = common::scratch_dir("tp-ssh-worker-");
    executable(
        &dir.path().join("ssh"),
        "#!/bin/sh\n[ \"$1\" = -T ] || { touch \"$FIXTURE/mutation\"; exit 92; }\nprintf 'probe\\n' >> \"$FIXTURE/probes\"\nif [ -f \"$FIXTURE/first\" ]; then\n printf 'failed stdout evidence\\n'; printf 'transport failure evidence\\n' >&2; exit 23\nfi\ntouch \"$FIXTURE/first\"\nexec /bin/sh -c \"$3\"\n",
    );
    executable(
        &dir.path().join("selected-shell"),
        "#!/bin/sh\n[ \"$1\" = -c ] || exit 91\nprintf 'startup evidence\\n'\nprintf 'startup stderr evidence\\n' >&2\n/bin/sh -c \"$2\"; rc=$?\nprintf 'trailing evidence\\n'\nexit \"$rc\"\n",
    );
    executable(
        &dir.path().join("id"),
        "#!/bin/sh\nprintf 'operation evidence\\n' >&2\nexec /usr/bin/id \"$@\"\n",
    );
    let launch = LaunchConfig {
        ssh: Some("fixture-only".into()),
        tmux: Some("never-created".into()),
        shell: Some(dir.path().join("selected-shell").to_str().unwrap().into()),
        session_backend: SessionBackend::TtaLib,
        ssh_master: false,
        login: false,
        interactive: false,
        ..Default::default()
    };
    let request = serde_json::json!({"launch": launch, "trailing": [], "ssh_options": []});
    let output = Command::new(env!("CARGO_BIN_EXE_tper"))
        .args(["--launch-worker", &request.to_string()])
        .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
        .env("FIXTURE", dir.path())
        .env("TTA_CONFIG", dir.path().join("absent/config.toml"))
        .env_remove("TTA_SOCKET_NAME")
        .env_remove("TTA_SOCKET_PATH")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "diagnostics must not contaminate stdout");
    let stderr = String::from_utf8_lossy(&output.stderr);
    for evidence in [
        "startup evidence",
        "startup stderr evidence",
        "trailing evidence",
        "operation evidence",
        "transport failure evidence",
        "failed stdout evidence",
        "SSH entry stopped at registry_read",
        "EffectKnowledge { finished: [], in_flight: [] }",
        "Some(23)",
        "capture=Complete",
        "complete=true",
        "stdout_eof=true",
        "stderr_eof=true",
        "omitted_stdout=0",
        "omitted_stderr=0",
        "cleanup=[]",
    ] {
        assert!(stderr.contains(evidence), "missing {evidence}: {stderr}");
    }
    assert_eq!(std::fs::read_to_string(dir.path().join("probes")).unwrap(), "probe\nprobe\n");
    assert!(!dir.path().join("mutation").exists(), "no terminal action/fallback on failed prepare");
}

#[test]
fn worker_probe_limits_survive_serialization_and_bound_actual_capture() {
    use std::time::{Duration, Instant};
    let dir = common::scratch_dir("tp-ssh-limits-");
    executable(&dir.path().join("ssh"), "#!/bin/sh\n[ \"$1\" = -T ] || { touch \"$FIXTURE/mutation\"; exit 92; }\ncase \"$MODE\" in\n slow) sleep 0.15; printf 'malformed reply';;\n flood) head -c 2048 /dev/zero;;\nesac\n");
    for (mode, timeout, retention, expected) in [
        ("slow", 20, 4096, "capture=Deadline"),
        ("slow", 1000, 4096, "capture=Complete"),
        ("flood", 1000, 64, "capture=OutputCap"),
        ("flood", 1000, 4096, "capture=Complete"),
    ] {
        let launch = LaunchConfig {
            ssh: Some("fixture-only".into()),
            tmux: Some("never-created".into()),
            session_backend: SessionBackend::TtaLib,
            ssh_master: false,
            ssh_probe_timeout: std::num::NonZeroU64::new(timeout).unwrap(),
            ssh_probe_retention: std::num::NonZeroUsize::new(retention).unwrap(),
            ..Default::default()
        };
        let request = serde_json::json!({"launch": launch, "trailing": [], "ssh_options": []});
        let start = Instant::now();
        let output = Command::new(env!("CARGO_BIN_EXE_tper"))
            .args(["--launch-worker", &request.to_string()])
            .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
            .env("FIXTURE", dir.path())
            .env("MODE", mode)
            .output()
            .unwrap();
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{mode}/{timeout}/{retention}: {stderr}");
        assert!(stderr.contains("cleanup="), "{stderr}");
    }
    assert!(!dir.path().join("mutation").exists());
}
