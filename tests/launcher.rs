//! Launcher integration tests: backend availability policy, remote argv
//! shapes, local quoting, and end-to-end launches against a real tmux server.
//!
//! Every tmux command here uses an explicit absolute socket under /tmp/opencode
//! whose absence is proven read-only before the first mutation (AGENTS.md
//! rules): no default server, no `-L`, no TMUX_TMPDIR reliance. Cleanup is
//! `kill-session` of sessions on this test's own socket only; the server is
//! never killed and exits itself via `exit-empty`.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serial_test::serial;
use tempfile::TempDir;

use term_preserver::launch::LaunchConfig;
use term_preserver::launcher::{normalize_local, prepare, select_backend, SessionBackend};

const TMUX: &str = "/usr/bin/tmux";
/// Sibling tta CLI build, used through `--tta-program` (never installed).
/// Resolved relative to this manifest: the tta checkout sits beside this one.
const TTA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tta/target/debug/tta");
const DEADLINE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);

// --- pure helpers ---------------------------------------------------------

fn launch() -> LaunchConfig {
    LaunchConfig {
        shell: Some("/bin/sh".into()),
        login: false,
        interactive: false,
        ..LaunchConfig::default()
    }
}

fn trailing(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

/// The first backend compiled into this build, if any. Lets shape tests pin an
/// explicit backend so they exercise quoting/argv logic regardless of which
/// feature subset is enabled.
fn any_compiled_backend() -> Option<SessionBackend> {
    if cfg!(feature = "classic-tmux-cli") {
        Some(SessionBackend::ClassicTmuxCli)
    } else if cfg!(feature = "tta-cli") {
        Some(SessionBackend::TtaCli)
    } else if cfg!(feature = "tta-lib") {
        Some(SessionBackend::TtaLib)
    } else if cfg!(feature = "classic-tmux-tmax") {
        Some(SessionBackend::ClassicTmuxTmax)
    } else {
        None
    }
}

// --- backend availability policy (no tmux needed) -------------------------

#[test]
fn requesting_a_backend_this_binary_lacks_is_rejected() {
    let mut l = launch();
    l.tmux = Some("dev".into());
    // Pick a backend this binary genuinely lacks, whichever it is.
    let absent = [
        (cfg!(feature = "classic-tmux-tmax"), SessionBackend::ClassicTmuxTmax),
        (cfg!(feature = "tta-lib"), SessionBackend::TtaLib),
        (cfg!(feature = "classic-tmux-cli"), SessionBackend::ClassicTmuxCli),
        (cfg!(feature = "tta-cli"), SessionBackend::TtaCli),
    ]
    .iter()
    .find(|(compiled, _)| !compiled)
    .map(|(_, backend)| *backend);
    let Some(absent) = absent else {
        eprintln!("skipping: every backend is compiled in this build");
        return;
    };
    l.session_backend = absent;
    let error = prepare(&l, &[], None).expect_err("uncompiled backend must be rejected");
    assert!(format!("{error:#}").contains("was not compiled"), "{error:#}");
}

#[test]
fn family_mismatched_explicit_backends_are_rejected() {
    let mut create = launch();
    create.tmux_create = Some("dev".into());
    create.session_backend = SessionBackend::TtaCli;
    assert!(select_backend(&create, true).is_err(), "tta cannot serve --tmux-create");

    let mut tta = launch();
    tta.tta = Some("dev".into());
    tta.session_backend = SessionBackend::ClassicTmuxCli;
    assert!(select_backend(&tta, true).is_err(), "classic cannot serve --tta");
}

#[test]
fn auto_local_resolution_matches_compiled_backends() {
    let mut l = launch();
    l.tmux = Some("dev".into());
    let selected = select_backend(&l, true);
    if cfg!(any(feature = "tta-lib", feature = "tta-cli", feature = "classic-tmux-cli", feature = "classic-tmux-tmax")) {
        let selected = selected.unwrap();
        let expected = if cfg!(feature = "tta-lib") {
            SessionBackend::TtaLib
        } else if cfg!(feature = "tta-cli") {
            SessionBackend::TtaCli
        } else if cfg!(feature = "classic-tmux-tmax") {
            SessionBackend::ClassicTmuxTmax
        } else {
            SessionBackend::ClassicTmuxCli
        };
        assert_eq!(selected, expected, "auto must prefer tta when compiled and local");
    } else {
        assert!(selected.is_err(), "no backend compiled: auto must fail loudly");
    }
}

#[test]
fn auto_ignores_tta_cli_presence_without_the_program() {
    let mut l = launch();
    l.tmux = Some("dev".into());
    let selected = select_backend(&l, false);
    if cfg!(feature = "tta-lib") {
        // The embedded backend does not depend on an executable being present.
        assert_eq!(selected.unwrap(), SessionBackend::TtaLib);
        return;
    }
    if !cfg!(any(feature = "classic-tmux-cli", feature = "classic-tmux-tmax")) {
        assert!(selected.is_err(), "no classic fallback compiled and tta absent");
        return;
    }
    let selected = selected.unwrap();
    assert!(
        matches!(selected, SessionBackend::ClassicTmuxCli | SessionBackend::ClassicTmuxTmax),
        "absent tta program must fall through to classic, got {selected:?}"
    );
}

#[test]
fn auto_remote_never_selects_local_only_backends() {
    let mut l = launch();
    l.tmux = Some("dev".into());
    l.ssh = Some("host".into());
    let selected = select_backend(&l, true);
    if !cfg!(any(feature = "classic-tmux-cli", feature = "tta-cli")) {
        // Remote cannot use embedded backends; without a remote-capable CLI
        // backend auto fails loudly.
        assert!(selected.is_err(), "remote auto must fail without a remote-capable backend");
        return;
    }
    let selected = selected.unwrap();
    assert!(
        !matches!(selected, SessionBackend::TtaLib | SessionBackend::ClassicTmuxTmax),
        "embedded/native backends cannot reach a remote server, got {selected:?}"
    );
}

#[test]
fn auto_tta_target_never_falls_back_to_classic() {
    let mut l = launch();
    l.tta = Some("dev".into());
    if cfg!(feature = "tta-lib") {
        assert_eq!(select_backend(&l, false).unwrap(), SessionBackend::TtaLib);
        return;
    }
    if !cfg!(feature = "tta-cli") {
        assert!(select_backend(&l, false).is_err(), "--tta must fail without a tta backend");
        return;
    }
    assert_eq!(select_backend(&l, true).unwrap(), SessionBackend::TtaCli);
    assert!(
        select_backend(&l, false).is_err(),
        "an absent tta program must not silently launch classic tmux for --tta"
    );
}

// --- remote argv shapes (constructed, never executed) ---------------------

#[test]
fn remote_auto_builds_presence_conditional_with_fallback() {
    let mut l = launch();
    l.tmux = Some("dev".into());
    l.ssh = Some("host".into());
    l.tmux_socket = Some("/remote/sock".into());
    if any_compiled_backend().is_none()
        || !cfg!(any(feature = "tta-cli", feature = "classic-tmux-cli"))
    {
        // Remote auto needs a remote-capable backend; none is compiled.
        assert!(prepare(&l, &trailing(&["tail", "-f"]), None).is_err());
        return;
    }
    let spec = prepare(&l, &trailing(&["tail", "-f"]), None).unwrap();
    assert_eq!(spec.program, "ssh");
    assert_eq!(spec.args[0], "-t");
    assert_eq!(spec.args[1], "host");
    let line = spec.args[2].clone();
    if cfg!(feature = "tta-cli") && !cfg!(feature = "classic-tmux-cli") {
        // No classic fallback compiled: tta probe with a loud 127 fallback.
        assert!(line.contains("command -v tta"), "{line}");
        assert!(line.contains("exit 127"), "{line}");
    } else if cfg!(feature = "tta-cli") {
        assert!(line.contains("command -v tta"), "{line}");
        assert!(line.contains("--socket-path /remote/sock"), "{line}");
        assert!(line.contains("has-session -t =dev"), "{line}");
    } else if cfg!(feature = "classic-tmux-cli") {
        assert!(!line.contains("command -v"), "{line}");
        assert!(line.contains("has-session -t =dev"), "{line}");
        assert!(line.contains("/remote/sock"), "{line}");
    }
    // Trailing arguments travel inside the remote script, quoted.
    assert!(line.contains("tail"), "{line}");
}

#[cfg(feature = "tta-cli")]
#[test]
fn remote_explicit_tta_cli_skips_the_presence_probe() {
    let mut l = launch();
    l.tmux = Some("dev".into());
    l.ssh = Some("host".into());
    l.tmux_socket = Some("/remote/sock".into());
    l.session_backend = SessionBackend::TtaCli;
    let spec = prepare(&l, &trailing(&["vim"]), None).unwrap();
    let line = spec.args[2].clone();
    assert!(!line.contains("command -v"), "explicit backend trusts presence: {line}");
    assert!(line.contains("--socket-path /remote/sock"), "{line}");
    assert!(line.contains("vim"), "{line}");
}

#[test]
fn remote_embedded_backends_are_rejected() {
    let mut lib = launch();
    lib.tmux = Some("dev".into());
    lib.ssh = Some("host".into());
    lib.session_backend = SessionBackend::TtaLib;
    assert!(prepare(&lib, &[], None).is_err());

    let mut tmax = launch();
    tmax.tmux_attach = Some("dev".into());
    tmax.mosh = Some("host".into());
    tmax.session_backend = SessionBackend::ClassicTmuxTmax;
    assert!(prepare(&tmax, &[], None).is_err());
}

#[test]
fn remote_socket_paths_stay_remote() {
    let backend = if cfg!(feature = "tta-cli") {
        Some(SessionBackend::TtaCli)
    } else if cfg!(feature = "classic-tmux-cli") {
        Some(SessionBackend::ClassicTmuxCli)
    } else {
        None
    };
    let Some(backend) = backend else {
        eprintln!("skipping: no remote-capable backend compiled");
        return;
    };
    let mut l = launch();
    l.tmux = Some("dev".into());
    l.ssh = Some("host".into());
    l.session_backend = backend;
    l.tmux_socket = Some("rel/sock".into());
    let spec = prepare(&l, &[], None).unwrap();
    let line = spec.args[2].clone();
    assert!(line.contains("rel/sock"), "{line}");
    let cwd = std::env::current_dir().unwrap();
    assert!(!line.contains(cwd.to_str().unwrap()), "remote socket must not be localized: {line}");
}

#[cfg(feature = "classic-tmux-cli")]
#[test]
fn remote_classic_only_targets_skip_the_tta_probe() {
    let mut l = launch();
    l.tmux_attach = Some("dev".into());
    l.ssh = Some("host".into());
    let spec = prepare(&l, &[], None).unwrap();
    let line = spec.args[2].clone();
    assert!(!line.contains("command -v"), "{line}");
    assert!(line.contains("attach-session"), "{line}");
}

// --- local prepare: worker request, quoting, startup flags ----------------

#[test]
fn local_prepare_carries_an_absolute_socket_in_the_worker_request() {
    let Some(backend) = any_compiled_backend() else { eprintln!("skipping: no backend compiled"); return };
    let mut l = launch();
    l.tmux = Some("dev".into());
    l.session_backend = backend;
    l.tmux_socket = Some("rel/sock".into());
    let spec = prepare(&l, &trailing(&["echo", "a b;c"]), None).unwrap();
    // The outer carrier is the selected shell; the worker rides inside -c.
    assert_eq!(spec.program, "/bin/sh");
    assert_eq!(spec.args[0], "-c");
    let script = spec.args[1].clone();
    assert!(script.starts_with("exec "), "{script}");
    // The worker is this same binary, re-entered through --launch-worker.
    assert!(script.contains("term-preserver"), "{script}");
    assert!(script.contains("--launch-worker"), "{script}");
    // The relative socket was normalized against the caller's cwd inside the
    // frozen request, so the worker cannot re-resolve it differently. The JSON
    // rides inside a single-quoted token, so its characters appear literally.
    let cwd = std::env::current_dir().unwrap();
    let absolute = cwd.join("rel/sock").to_str().unwrap().to_string();
    assert!(script.contains(&format!("\"tmux_socket\":\"{absolute}\"")), "{script}");
    assert!(script.contains("\"tmux\":\"dev\""), "{script}");
    assert!(script.contains("\"trailing\":[\"echo\",\"a b;c\"]"), "{script}");
}

#[test]
fn default_startup_flags_are_login_and_interactive() {
    let mut l = launch();
    l.login = true;
    l.interactive = true;
    let spec = prepare(&l, &[], None).unwrap();
    assert_eq!(spec.program, "/bin/sh");
    assert_eq!(spec.args, vec!["-li"]);

    let bare = launch(); // helper disables both flags
    let spec = prepare(&bare, &[], None).unwrap();
    assert!(spec.args.is_empty(), "no flags, no -c: {:?}", spec.args);
}

#[test]
fn fish_family_avoids_posix_quote_escapes() {
    let Some(backend) = any_compiled_backend() else { eprintln!("skipping: no backend compiled"); return };
    let mut l = launch();
    l.shell = Some("/usr/bin/fish".into());
    l.tmux = Some("dev".into());
    l.session_backend = backend;
    let spec = prepare(&l, &trailing(&["echo", "it's"]), None).unwrap();
    let script = spec.args.last().unwrap().clone();
    assert!(script.starts_with("exec "), "{script}");
    // The POSIX '\'' escape is misparsed by fish; fish quoting is \' and \\.
    assert!(!script.contains("'\\''"), "posix escape leaked into fish: {script}");
    assert!(script.contains("it\\'s"), "fish quote escape missing: {script}");
    // Login/interactive flags are disabled in the helper; fish carries only -c.
    assert_eq!(&spec.args[..spec.args.len() - 1], &["-c".to_string()]);
}

#[test]
fn nushell_uses_json_external_invocation_without_login_flags() {
    let Some(backend) = any_compiled_backend() else { eprintln!("skipping: no backend compiled"); return };
    let mut l = launch();
    l.shell = Some("nu".into()); // login/interactive stay default-true
    l.tmux = Some("dev".into());
    l.session_backend = backend;
    let spec = prepare(&l, &trailing(&["echo", "$HOME"]), None).unwrap();
    assert_eq!(spec.program, "nu");
    // Nu has no -l/-i pairing here; the outer argv is exactly [nu, -c, script].
    assert_eq!(spec.args[0], "-c");
    let script = spec.args[1].clone();
    assert!(script.starts_with("^\""), "{script}");
    // Every token is a JSON string: dollars survive literally.
    assert!(script.contains("\"$HOME\"") || script.contains("$HOME"), "{script}");
    assert!(!script.contains(" -l "), "{script}");
}

// --- local socket normalization -------------------------------------------

#[test]
#[serial]
fn normalize_local_joins_relative_socket_to_cwd() {
    let mut l = launch();
    l.tmux = Some("dev".into());
    l.tmux_socket = Some("rel/sock".into());
    normalize_local(&mut l).unwrap();
    let expected = std::env::current_dir().unwrap().join("rel/sock");
    assert_eq!(l.tmux_socket.as_deref(), Some(expected.to_str().unwrap()));
}

#[test]
#[serial]
fn normalize_local_inherits_the_current_tmux_server_socket() {
    let saved = std::env::var("TMUX").ok();
    std::env::set_var("TMUX", "/tmp/opencode/some/server,12345,0");
    let mut l = launch();
    l.tmux = Some("dev".into());
    normalize_local(&mut l).unwrap();
    match saved {
        Some(v) => std::env::set_var("TMUX", v),
        None => std::env::remove_var("TMUX"),
    }
    // The absolute $TMUX socket wins over the cwd join entirely.
    assert_eq!(l.tmux_socket.as_deref(), Some("/tmp/opencode/some/server"));
}

#[test]
#[serial]
fn normalize_local_defaults_to_the_euid_socket_path() {
    let saved_tmux = std::env::var("TMUX").ok();
    let saved_tmpdir = std::env::var("TMUX_TMPDIR").ok();
    std::env::remove_var("TMUX");
    std::env::remove_var("TMUX_TMPDIR");
    let mut l = launch();
    l.tmux = Some("dev".into());
    normalize_local(&mut l).unwrap();
    let expected = format!("/tmp/tmux-{}/default", nix::unistd::geteuid());
    assert_eq!(l.tmux_socket.as_deref(), Some(expected.as_str()));
    std::env::set_var("TMUX_TMPDIR", "/tmp/opencode");
    let mut l = launch();
    l.tmux = Some("dev".into());
    normalize_local(&mut l).unwrap();
    let expected = format!("/tmp/opencode/tmux-{}/default", nix::unistd::geteuid());
    assert_eq!(l.tmux_socket.as_deref(), Some(expected.as_str()));
    match saved_tmux {
        Some(v) => std::env::set_var("TMUX", v),
        None => std::env::remove_var("TMUX"),
    }
    match saved_tmpdir {
        Some(v) => std::env::set_var("TMUX_TMPDIR", v),
        None => std::env::remove_var("TMUX_TMPDIR"),
    }
}

#[test]
fn normalize_local_skips_remote_and_rejects_empty() {
    let mut remote = launch();
    remote.tmux = Some("dev".into());
    remote.ssh = Some("host".into());
    remote.tmux_socket = Some("rel/sock".into());
    normalize_local(&mut remote).unwrap();
    assert_eq!(remote.tmux_socket.as_deref(), Some("rel/sock"));

    let mut empty = launch();
    empty.tmux = Some("dev".into());
    empty.tmux_socket = Some("".into());
    assert!(normalize_local(&mut empty).is_err());
}

// --- end-to-end harness against a real isolated tmux server ----------------

struct Server {
    socket: String,
    _dir: TempDir,
}

impl Server {
    /// Returns None (skip) when tmux is unavailable. Proves the socket is
    /// absent before any mutation, exactly per AGENTS.md.
    fn new() -> Option<Self> {
        if !Command::new(TMUX).arg("-V").output().is_ok_and(|o| o.status.success()) {
            eprintln!("skipping launcher e2e: {TMUX} unavailable");
            return None;
        }
        let dir = tempfile::Builder::new()
            .prefix("tp-launcher-")
            .tempdir_in("/tmp/opencode")
            .unwrap();
        let socket = dir.path().join("sock");
        assert!(socket.is_absolute());
        let socket = socket.to_str().unwrap().to_owned();
        let probe = Command::new(TMUX).args(["-S", &socket, "ls"]).output().unwrap();
        let stderr = String::from_utf8_lossy(&probe.stderr);
        assert!(!probe.status.success(), "unexpected server at {socket}");
        assert!(
            stderr.contains(&socket)
                && (stderr.contains("no server running") || stderr.contains("No such file or directory")),
            "isolation not proven: {stderr}"
        );
        // Satellites run a default-shell window when no startup command is
        // given; in minimal environments that shell can exit instantly and
        // kill the satellite before its keep-last option is observable. Start
        // the server with a config giving every default window a durable
        // command, plus a seed session that keeps the server alive.
        let config = dir.path().join("tmux.conf");
        std::fs::write(&config, "set-option -g default-command \"sleep 60\"\n").unwrap();
        let server = Self { socket, _dir: dir };
        server.run_ok(&[
            "-f",
            config.to_str().unwrap(),
            "new-session",
            "-d",
            "-s",
            "tp-seed",
            "sleep",
            "60",
        ]);
        assert_eq!(server.sessions(), vec!["tp-seed".to_string()]);
        Some(server)
    }

    fn tmux(&self, args: &[&str]) -> std::process::Output {
        Command::new(TMUX)
            .args(["-S", &self.socket])
            .args(args)
            .env_remove("TMUX")
            .output()
            .unwrap()
    }

    fn run_ok(&self, args: &[&str]) {
        let output = self.tmux(args);
        assert!(
            output.status.success(),
            "tmux {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn sessions(&self) -> Vec<String> {
        let output = self.tmux(&["list-sessions", "-F", "#{session_name}|#{session_group}"]);
        if !output.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|l| l.split('|').next().unwrap_or(l).to_owned())
            .collect()
    }

    fn groups(&self) -> Vec<(String, String)> {
        let output = self.tmux(&["list-sessions", "-F", "#{session_name}|#{session_group}"]);
        assert!(output.status.success(), "no sessions to inspect");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|l| l.split_once('|').map(|(a, b)| (a.to_owned(), b.to_owned())).unwrap_or_default())
            .collect()
    }

    fn option(&self, session: &str, option: &str) -> String {
        // Plain name: tmux 3.8's show-options rejects the `=` exact-prefix
        // target ("no such session: =name") even though has-session and
        // new-session accept it. Test names are unique, so this is exact.
        let output = self.tmux(&["show-options", "-v", "-t", session, option]);
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// Narrow cleanup: kill-session of every session on this test's own socket.
    fn kill_all(&self) {
        for name in self.sessions() {
            let output = self.tmux(&["kill-session", "-t", &format!("={name}")]);
            if !output.status.success() {
                eprintln!("cleanup of own session {name} failed");
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.kill_all();
    }
}

fn child_env(command: &mut Command) -> &mut Command {
    for name in ["TMUX", "TTA_BACKEND", "TTA_ROOT", "TTA_TMUX", "TTA_TMUX_CONFIG", "TTA_SOCKET_NAME", "TTA_SOCKET_PATH", "PROJECTS_DIR"] {
        command.env_remove(name);
    }
    command
}

fn spawn_proxy(server: &Server, extra: &[impl AsRef<str>], trailing: &[&str]) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_term-preserver"));
    command
        .args(["--config", "/dev/null", "--tmux-socket", &server.socket, "--tmux-program", TMUX])
        .args(extra.iter().map(AsRef::as_ref))
        .args(["--shell", "/bin/sh", "--no-login", "--no-interactive"])
        .arg("--")
        .args(trailing)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    child_env(&mut command).spawn().unwrap()
}

/// Drain a finished/nearly-finished child, killing it at the deadline (it is
/// this test's own process). Stdin is closed up front so the retry prompt
/// sees EOF and the proxy exits once the child exits.
fn drain(mut child: Child) -> (String, String) {
    drop(child.stdin.take());
    let deadline = Instant::now() + DEADLINE;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut all = Vec::new();
        while Instant::now() < deadline {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => all.extend_from_slice(&buf[..n]),
            }
        }
        all
    });
    let err = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut all = Vec::new();
        while Instant::now() < deadline {
            match stderr.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => all.extend_from_slice(&buf[..n]),
            }
        }
        all
    });
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(POLL);
    }
    if child.try_wait().unwrap().is_none() {
        eprintln!("launcher e2e: killing own over-deadline proxy child");
        let _ = child.kill();
        let _ = child.wait();
    }
    (
        String::from_utf8_lossy(&out.join().unwrap()).to_string(),
        String::from_utf8_lossy(&err.join().unwrap()).to_string(),
    )
}

fn wait_until(deadline: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + deadline;
    while Instant::now() < end {
        if condition() {
            return true;
        }
        thread::sleep(POLL);
    }
    condition()
}

fn base_flags(backend: &str) -> Vec<String> {
    vec!["--session-backend".into(), backend.into(), "--tmux".into()]
}

// --- end-to-end: classic tmux CLI backend ----------------------------------

#[cfg(feature = "classic-tmux-cli")]
#[test]
fn classic_fresh_create_preserves_trailing_argv() {
    let Some(server) = Server::new() else { return };
    let dir = server._dir.path().join("work");
    std::fs::create_dir_all(&dir).unwrap();
    let mut extra = base_flags("classic-tmux-cli");
    extra.extend(["lp-fresh".to_string(), "--dir".into(), dir.to_str().unwrap().into()]);
    let child = spawn_proxy(&server, &extra, &["echo", "lp-quoted > done"]);
    let (out, err) = drain(child);
    assert!(out.contains("lp-quoted > done"), "stdout: {out:?} stderr: {err:?}");
    assert_eq!(server.sessions(), vec!["tp-seed".to_string()], "session must die with its command");
    assert!(!dir.join("done").exists(), "redirect leaked out of the quoted argv");
}

#[cfg(feature = "classic-tmux-cli")]
#[test]
fn classic_session_name_hash_survives_format_expansion() {
    let Some(server) = Server::new() else { return };
    let mut extra = base_flags("classic-tmux-cli");
    extra.push("lp#h".to_string());
    let child = spawn_proxy(&server, &extra, &["sleep", "4"]);
    let appeared = wait_until(Duration::from_secs(5), || {
        let mut names = server.sessions();
        names.sort();
        names == vec!["lp#h".to_string(), "tp-seed".to_string()]
    });
    server.kill_all();
    let (_out, err) = drain(child);
    assert!(appeared, "session lp#h never appeared exactly-named; stderr: {err:?}");
}

#[cfg(feature = "classic-tmux-cli")]
#[test]
fn classic_rerun_of_same_launch_reprobes_and_joins_group() {
    let Some(server) = Server::new() else { return };
    let mut extra = base_flags("classic-tmux-cli");
    extra.push("lp-grp".to_string());

    // Run 1 on an empty server: fresh primary; its command exits immediately.
    let (out, err) = drain(spawn_proxy(&server, &extra, &["true"]));
    assert!(out.is_empty() || !out.contains("error"), "run 1: {out:?} {err:?}");
    assert_eq!(server.sessions(), vec!["tp-seed".to_string()], "run 1 session must be gone");

    // An identical re-run now finds lp-grp existing and must join its group as
    // a satellite (independent window, destroy-unattached keep-last) instead
    // of re-creating or plain-attaching.
    server.run_ok(&["-f", "/dev/null", "new-session", "-d", "-s", "lp-grp", "sleep", "8"]);
    let child = spawn_proxy(&server, &extra, &["sleep", "30"]);
    // A satellite has joined once a second session shares lp-grp's group.
    let joined = wait_until(Duration::from_secs(5), || {
        let groups = server.groups();
        let Some(primary) = groups.iter().find(|(n, _)| n == "lp-grp") else {
            return false;
        };
        let primary_group = &primary.1;
        !primary_group.is_empty()
            && groups.iter().any(|(n, g)| n != "lp-grp" && g == primary_group)
    });
    let groups = server.groups();
    let primary_group = groups.iter().find(|(n, _)| n == "lp-grp").map(|(_, g)| g.clone()).unwrap_or_default();
    let satellite: Vec<_> = groups.iter().filter(|(n, g)| n != "lp-grp" && *g == primary_group).collect();
    let (satellite_name, _) = satellite[0].clone();
    // Query the satellite's option while it is still alive: an unset option
    // prints empty (tmux exits 1) rather than "off".
    let destroy = server.option(&satellite_name, "destroy-unattached");
    server.kill_all();
    let (_out, err) = drain(child);

    assert!(joined, "no satellite appeared: {groups:?} stderr: {err:?}");
    assert_eq!(satellite.len(), 1, "exactly one satellite expected: {groups:?}");
    assert_eq!(destroy, "keep-last", "satellite must set destroy-unattached keep-last");
}

#[cfg(feature = "classic-tmux-cli")]
#[test]
fn classic_exact_probe_rejects_prefix_sessions() {
    let Some(server) = Server::new() else { return };
    server.run_ok(&["-f", "/dev/null", "new-session", "-d", "-s", "lp-pre", "sleep", "8"]);
    let mut extra = base_flags("classic-tmux-cli");
    extra.push("lp-pre2".to_string());
    let child = spawn_proxy(&server, &extra, &["echo", "lp-pre2-marker"]);
    let (out, err) = drain(child);
    assert!(out.contains("lp-pre2-marker"), "primary branch must run for lp-pre2: {out:?} {err:?}");
    let mut remaining = server.sessions();
    remaining.sort();
    assert_eq!(remaining, vec!["lp-pre".to_string(), "tp-seed".to_string()], "prefix session must be untouched");
}

#[cfg(feature = "classic-tmux-cli")]
#[test]
fn classic_no_group_plain_attach_creates_no_satellite() {
    let Some(server) = Server::new() else { return };
    server.run_ok(&["-f", "/dev/null", "new-session", "-d", "-s", "lp-plain", "sleep", "8"]);
    let mut extra = base_flags("classic-tmux-cli");
    extra.extend(["lp-plain".to_string(), "--no-tmux-group".to_string()]);
    let child = spawn_proxy(&server, &extra, &["sleep", "30"]);
    let settled = wait_until(Duration::from_millis(1200), || true); // let any wrong branch act
    let count = server.sessions().len(); // seed + attached session; a satellite would make three
    let destroy = server.option("lp-plain", "destroy-unattached");
    server.kill_all();
    let (_out, err) = drain(child);
    assert!(settled && count == 2, "no satellite may be created under --no-tmux-group: {count} stderr: {err:?}");
    // An unset option prints empty (tmux exits 1); it must NOT be keep-last.
    assert!(
        destroy.is_empty() || destroy == "off",
        "plain attach must not set destroy-unattached, got {destroy:?}"
    );
}

// --- end-to-end: tta backends share one surface ----------------------------

fn tta_available() -> bool {
    if Path::new(TTA).exists() {
        true
    } else {
        eprintln!("skipping tta e2e: sibling build {TTA} absent");
        false
    }
}

fn tta_extra(server: &Server, backend: &str) -> Vec<String> {
    let root = server._dir.path().join("projects");
    std::fs::create_dir_all(root.join("lp-tta-proj")).unwrap();
    vec![
        "--session-backend".into(),
        backend.into(),
        "--tta-program".into(),
        TTA.into(),
        "--session-root".into(),
        root.to_str().unwrap().into(),
        "--tmux".into(),
        "lp-tta-proj".into(),
    ]
}

fn assert_tta_surface(server: &Server, extra: &[String]) {
    let child = spawn_proxy(server, extra, &["echo", "lp-tta > ran"]);
    let (out, err) = drain(child);
    assert!(
        out.contains("lp-tta > ran"),
        "startup argv must survive tta quoting untouched: stdout {out:?} stderr {err:?}"
    );
    assert_eq!(server.sessions(), vec!["tp-seed".to_string()], "session must die with its startup command");
}

#[cfg(feature = "tta-cli")]
#[test]
fn tta_cli_launch_matches_the_documented_surface() {
    if !tta_available() {
        return;
    }
    let Some(server) = Server::new() else { return };
    let extra = tta_extra(&server, "tta-cli");
    assert_tta_surface(&server, &extra);
}

#[cfg(feature = "tta-lib")]
#[test]
fn tta_lib_embedded_launch_matches_the_documented_surface() {
    let Some(server) = Server::new() else { return };
    let extra = tta_extra(&server, "tta-lib");
    assert_tta_surface(&server, &extra);
}

// --- end-to-end: automatic backend selection -------------------------------

#[cfg(all(feature = "classic-tmux-cli", not(feature = "tta-lib")))]
#[test]
fn auto_without_a_tta_program_falls_back_to_classic() {
    let Some(server) = Server::new() else { return };
    let extra = vec![
        "--tta-program".to_string(),
        "/nonexistent/tta".to_string(),
        "--tmux".to_string(),
        "lp-auto-classic".to_string(),
    ];
    let child = spawn_proxy(&server, &extra, &["echo", "auto-classic-marker"]);
    let (out, err) = drain(child);
    assert!(out.contains("auto-classic-marker"), "stdout {out:?} stderr {err:?}");
    assert_eq!(server.sessions(), vec!["tp-seed".to_string()]);
}

#[cfg(feature = "tta-lib")]
#[test]
fn auto_prefers_embedded_tta_over_classic() {
    // --tmux names a fragment with no matching session or project: classic tmux
    // would have created the session anyway; embedded tta must instead fail
    // resolution, proving auto selected tta-lib over the classic fallback.
    let Some(server) = Server::new() else { return };
    let root = server._dir.path().join("projects");
    std::fs::create_dir_all(&root).unwrap();
    let extra = vec![
        "--session-root".to_string(),
        root.to_str().unwrap().to_string(),
        "--tmux".to_string(),
        "lp-auto-missing".to_string(),
    ];
    let child = spawn_proxy(&server, &extra, &["echo", "never-marker"]);
    let (out, err) = drain(child);
    assert!(!out.contains("never-marker"), "classic fallback ran instead of tta: {out:?}");
    // Worker errors print inside the proxy's PTY, so they surface on stdout.
    let combined = format!("{out}{err}");
    assert!(
        combined.contains("no session or immediate project directory matches"),
        "expected tta resolve failure, stdout: {out:?} stderr: {err:?}"
    );
    assert_eq!(server.sessions(), vec!["tp-seed".to_string()]);
}
