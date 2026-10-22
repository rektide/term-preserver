//! Real-server sampling tests. Every tmux command selects an explicit absolute
//! socket under /tmp/opencode, verified absent before the first mutation.

#![cfg(feature = "tmux")]

use std::io::Write;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use term_preserver::launch::TmuxMode;
use term_preserver::tmux::{sample_at, QueryBackend, TmuxEndpoint, TmuxSampler, TmuxState};

const TMUX: &str = "/usr/bin/tmux";

struct Server {
    directory: TempDir,
    endpoint: TmuxEndpoint,
    sessions: Vec<String>,
}

impl Server {
    fn new() -> Option<Self> {
        if !Command::new(TMUX)
            .arg("-V")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            eprintln!("skipping real-server test: {TMUX} unavailable");
            return None;
        }
        let directory = tempfile::Builder::new()
            .prefix("tp-sampler-")
            .tempdir_in("/tmp/opencode")
            .unwrap();
        let socket = directory.path().join("sock");
        assert!(socket.is_absolute());
        let socket = socket.to_str().unwrap().to_owned();
        let probe = Command::new(TMUX)
            .args(["-S", &socket, "ls"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&probe.stderr);
        assert!(!probe.status.success(), "unexpected server at {socket}");
        assert!(
            stderr.contains(&socket)
                && (stderr.contains("no server running")
                    || stderr.contains("No such file or directory")),
            "isolation not proven: {stderr}"
        );

        // Sampling must use this configured executable, not a hardcoded tmux.
        let executable = directory.path().join("selected-tmux");
        symlink(TMUX, &executable).unwrap();
        Some(Self {
            directory,
            endpoint: TmuxEndpoint {
                program: executable.to_str().unwrap().to_owned(),
                socket: Some(socket),
                backend: QueryBackend::Cli,
            },
            sessions: Vec::new(),
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(TMUX);
        command.args(["-S", self.endpoint.socket.as_deref().unwrap()]);
        command.env_remove("TMUX");
        command
    }

    fn run(&self, args: &[&str]) {
        let output = self.command().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "tmux {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn create(&mut self, name: &str) {
        // Register ownership before spawning so panic cleanup remains narrow.
        self.sessions.push(name.into());
        self.run(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            name,
            "-n",
            "first",
            "sleep",
            "300",
        ]);
    }

    fn backends(&self) -> Vec<TmuxEndpoint> {
        let endpoints = vec![self.endpoint.clone()];
        #[cfg(any(feature = "tta-lib", feature = "classic-tmux-tmax"))]
        let endpoints = {
            let mut endpoints = endpoints;
            endpoints.push(TmuxEndpoint {
                // Native sampling must work without invoking an executable.
                program: self
                    .directory
                    .path()
                    .join("absent-program")
                    .to_str()
                    .unwrap()
                    .into(),
                backend: QueryBackend::Tmax,
                ..self.endpoint.clone()
            });
            endpoints
        };
        endpoints
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        for name in &self.sessions {
            let result = self
                .command()
                .args(["kill-session", "-t", &format!("={name}")])
                .status();
            if !result.is_ok_and(|status| status.success()) {
                eprintln!("cleanup of own session {name} failed");
            }
        }
    }
}

struct ControlClient(Child);

impl Drop for ControlClient {
    fn drop(&mut self) {
        // Only the exact client process created here is terminated.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn endpoint_queries_exact_session_and_preserves_all_pane_fields() {
    let Some(mut server) = Server::new() else {
        return;
    };
    server.create("sample-long-name");
    server.run(&[
        "new-window",
        "-d",
        "-t",
        "=sample-long-name",
        "-n",
        "second",
        "sleep",
        "300",
    ]);

    let baseline = sample_at(&server.endpoint, u32::MAX, Some("sample-long-name")).unwrap();
    assert_eq!(baseline.session_name.as_deref(), Some("sample-long-name"));
    assert!(baseline.session_id.as_deref().unwrap().starts_with('$'));
    assert_eq!(baseline.panes.len(), 2);
    for pane in &baseline.panes {
        assert!(!pane.window_index.is_empty());
        assert!(pane.pane_id.starts_with('%'));
        assert!(pane.pane_pid.is_some());
        assert!(!pane.current_command.is_empty());
        assert!(pane.current_path.starts_with('/'));
    }
    assert_eq!(baseline.panes[0].window_name, "first");
    assert_eq!(baseline.panes[1].window_name, "second");

    for endpoint in server.backends() {
        assert_eq!(
            sample_at(&endpoint, u32::MAX, Some("sample-long-name")),
            Some(baseline.clone())
        );
        assert!(sample_at(&endpoint, u32::MAX, Some("sample-long")).is_none());
        assert!(sample_at(&endpoint, u32::MAX, Some("sample-*")).is_none());
        assert!(sample_at(&endpoint, u32::MAX, None).is_none());
    }
}

#[test]
fn client_pid_wins_over_exact_fallback_and_supports_pid_only_sampling() {
    let Some(mut server) = Server::new() else {
        return;
    };
    server.create("attached");
    server.create("fallback");
    let mut client = ControlClient(
        server
            .command()
            .args(["-C", "attach-session", "-t", "=attached"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while sample_at(&server.endpoint, client.0.id(), None).is_none() {
        assert!(Instant::now() < deadline, "control client never attached");
        assert!(
            client.0.try_wait().unwrap().is_none(),
            "control client exited"
        );
        thread::sleep(Duration::from_millis(20));
    }
    for endpoint in server.backends() {
        for fallback in [None, Some("fallback")] {
            let sample = sample_at(&endpoint, client.0.id(), fallback).unwrap();
            assert_eq!(sample.session_name.as_deref(), Some("attached"));
        }
        let state = TmuxState::new();
        let sampler = TmuxSampler::start_at(
            client.0.id(),
            None,
            Arc::new(AtomicBool::new(true)),
            state.clone(),
            endpoint,
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while state.snapshot().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        sampler.stop();
        assert_eq!(
            state.snapshot().unwrap().session_name.as_deref(),
            Some("attached")
        );
    }
    client
        .0
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"detach-client\n")
        .unwrap();
}

#[test]
fn failed_pane_query_keeps_last_complete_sample() {
    let Some(mut server) = Server::new() else {
        return;
    };
    server.create("cached");
    let previous = sample_at(&server.endpoint, u32::MAX, Some("cached")).unwrap();
    let shim = server.directory.path().join("fail-panes");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\n[ \"$1\" = -S ] || exit 91\n[ \"$2\" = '{}' ] || exit 92\n[ \"$3\" = list-panes ] && exit 93\nexec {TMUX} \"$@\"\n",
            server.endpoint.socket.as_deref().unwrap(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o700)).unwrap();
    let endpoint = TmuxEndpoint {
        program: shim.to_str().unwrap().into(),
        ..server.endpoint.clone()
    };
    assert!(sample_at(&endpoint, u32::MAX, Some("cached")).is_none());
    let state = TmuxState::new();
    state.set(previous.clone());
    let sampler = TmuxSampler::start_at(
        u32::MAX,
        Some(TmuxMode::CreateOrAttach("cached".into())),
        Arc::new(AtomicBool::new(true)),
        state.clone(),
        endpoint,
    );
    thread::sleep(Duration::from_millis(200));
    sampler.stop();
    assert_eq!(state.snapshot(), Some(previous));
}

#[cfg(not(any(feature = "tta-lib", feature = "classic-tmux-tmax")))]
#[test]
fn unavailable_native_backend_does_not_fall_back_to_cli() {
    let Some(mut server) = Server::new() else {
        return;
    };
    server.create("native-disabled");
    assert!(sample_at(&server.endpoint, u32::MAX, Some("native-disabled")).is_some());
    let endpoint = TmuxEndpoint {
        backend: QueryBackend::Tmax,
        ..server.endpoint.clone()
    };
    assert!(sample_at(&endpoint, u32::MAX, Some("native-disabled")).is_none());
}
