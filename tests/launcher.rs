//! Launcher integration tests: backend availability policy, remote argv
//! shapes, local quoting, scaffold-on-miss through a fake `rekon` generator,
//! and end-to-end launches against a real tmux server.
//!
//! Every tmux command here uses an explicit absolute socket under /tmp/opencode
//! whose absence is proven read-only before the first mutation (AGENTS.md
//! rules): no default server, no `-L`, no TMUX_TMPDIR reliance. Cleanup is
//! `kill-session` of sessions on this test's own socket only; the server is
//! never killed and exits itself via `exit-empty`.
//!
//! Scaffold tests never touch the user's real generators: every child sees a
//! controlled empty `TTA_CONFIG` registry (scaffold-by-default therefore has
//! nothing to run), and scaffold cases run a fake `rekon` shim on a PATH
//! restricted to the shim directory plus the two system directories proven
//! rekon-free at setup.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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

/// The classic tmux backends compiled into this build. The classic e2e tests
/// exercise each in turn: `classic-tmux-cli` probes sessions by spawning the
/// tmux CLI, `classic-tmux-tmax` by its real in-process native lookup — both
/// against the same real per-test tmux server.
fn classic_backends() -> Vec<&'static str> {
    let mut backends = Vec::new();
    if cfg!(feature = "classic-tmux-cli") {
        backends.push("classic-tmux-cli");
    }
    if cfg!(feature = "classic-tmux-tmax") {
        backends.push("classic-tmux-tmax");
    }
    if backends.is_empty() {
        eprintln!("skipping classic e2e: no classic backend compiled");
    }
    backends
}

/// Run `body` for every tta backend this build can exercise end-to-end: the
/// embedded library whenever compiled, the CLI whenever compiled and the
/// sibling binary exists.
fn for_each_tta_backend(mut body: impl FnMut(&str)) {
    let mut ran = false;
    if cfg!(feature = "tta-lib") {
        ran = true;
        body("tta-lib");
    }
    if cfg!(feature = "tta-cli") && tta_available() {
        ran = true;
        body("tta-cli");
    }
    if !ran {
        eprintln!("skipping tta e2e: no tta backend available in this build");
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

// --- scaffold choice plumbing (constructed, never executed) ----------------

#[test]
fn explicit_positive_scaffold_refuses_classic_but_off_allows_it() {
    // A create-or-attach target has no tta surface: a positive scaffold choice
    // must be refused rather than silently scaffolded through classic tmux.
    let mut create = launch();
    create.tmux_create = Some("dev".into());
    create.scaffold = Some("rk".into());
    let error = select_backend(&create, true).expect_err("positive scaffold needs a tta target");
    assert!(
        format!("{error:#}").contains("requires a tta create-or-match target"),
        "{error:#}"
    );

    let mut classic = launch();
    classic.tmux = Some("dev".into());
    classic.session_backend = SessionBackend::ClassicTmuxCli;
    classic.scaffold = Some("rk".into());
    let error = select_backend(&classic, true).expect_err("positive scaffold cannot ride classic");
    assert!(format!("{error:#}").contains("requires a tta backend"), "{error:#}");

    // The whole off vocabulary opts back into classic semantics.
    for off in ["false", "0", "off", "none"] {
        classic.scaffold = Some(off.into());
        assert!(
            select_backend(&classic, true).is_ok(),
            "--scaffold={off} must still allow classic"
        );
    }
}

#[test]
fn scaffold_choices_travel_verbatim_into_worker_requests_and_tta_argv() {
    // Positive scaffold choices require a tta backend, so the local shape uses
    // one; the frozen worker request carries the raw choice either way.
    let tta_backend = if cfg!(feature = "tta-lib") {
        Some(SessionBackend::TtaLib)
    } else if cfg!(feature = "tta-cli") {
        Some(SessionBackend::TtaCli)
    } else {
        None
    };
    for choice in ["rk", "false"] {
        let mut l = launch();
        l.tmux = Some("dev".into());
        l.session_backend = if choice == "false" {
            any_compiled_backend().expect("some backend compiled for shape tests")
        } else {
            tta_backend.expect("a tta backend must be compiled for scaffold shape tests")
        };
        l.scaffold = Some(choice.into());
        let spec = prepare(&l, &[], None).unwrap();
        let script = spec.args.last().unwrap();
        assert!(script.contains(&format!("\"scaffold\":\"{choice}\"")), "{script}");
    }
    if !cfg!(feature = "tta-cli") {
        eprintln!("skipping tta argv scaffold shape: tta-cli not compiled");
        return;
    }
    let mut l = launch();
    l.tmux = Some("dev".into());
    l.ssh = Some("host".into());
    l.session_backend = SessionBackend::TtaCli;
    for choice in ["rk", "false"] {
        l.scaffold = Some(choice.into());
        let spec = prepare(&l, &[], None).unwrap();
        let line = &spec.args[2];
        assert!(line.contains(&format!("--scaffold={choice}")), "{line}");
        assert!(!line.contains("TTA_SCAFFOLD"), "explicit choice needs no env indirection: {line}");
    }
}

#[test]
fn remote_tta_cli_carrier_supplies_the_default_from_the_execution_environment() {
    if !cfg!(feature = "tta-cli") {
        eprintln!("skipping remote scaffold default shape: tta-cli not compiled");
        return;
    }
    let mut l = launch();
    l.tmux = Some("dev".into());
    l.ssh = Some("host".into());
    l.session_backend = SessionBackend::TtaCli;
    let spec = prepare(&l, &[], None).unwrap();
    // An unset choice resolves on the remote host, independently of this CLI's
    // scaffold-by-default compile posture.
    let line = &spec.args[2];
    assert!(line.contains("--scaffold=\"${TTA_SCAFFOLD:-default}\""), "{line}");
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
    /// Controlled empty tta scaffold registry every child sees: an empty file
    /// loads as an empty registry, so scaffold-by-default can never reach a
    /// user generator. Registry tests override TTA_CONFIG per spawn.
    tta_registry: PathBuf,
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
        let tta_registry = dir.path().join("tta-registry.toml");
        std::fs::write(&tta_registry, b"").unwrap();
        let server = Self {
            socket,
            tta_registry,
            _dir: dir,
        };
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

    /// Session names currently holding an attached client.
    fn attached_sessions(&self) -> Vec<String> {
        let output = self.tmux(&["list-clients", "-F", "#{session_name}"]);
        if !output.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&output.stdout).lines().map(str::to_owned).collect()
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

/// Baseline child isolation. Every channel through which the invoking user's
/// real tta or term-preserver configuration could leak into a launch is
/// stripped or pointed at this server's own empty tta scaffold registry, so
/// scaffold-on-by-default can never execute a user generator. Scaffold tests
/// narrow PATH further through [`Scaffold::apply`].
fn child_env(command: &mut Command, server: &Server) {
    for name in [
        "TMUX",
        "TTA_BACKEND",
        "TTA_ROOT",
        "TTA_TMUX",
        "TTA_TMUX_CONFIG",
        "TTA_SOCKET_NAME",
        "TTA_SOCKET_PATH",
        "TTA_SCAFFOLD",
        "TTA_CONFIG",
        "XDG_CONFIG_HOME",
        "PROJECTS_DIR",
    ] {
        command.env_remove(name);
    }
    // figment overlays TERM_PRESERVER_* onto the config file; strip the launch
    // surface so leftover user settings cannot fight the flags every spawn
    // passes explicitly.
    for field in [
        "TTA",
        "TTA_PROGRAM",
        "SESSION_BACKEND",
        "SESSION_ROOT",
        "SCAFFOLD",
        "TMUX",
        "TMUX_CREATE",
        "TMUX_ATTACH",
        "TMUX_SOCKET",
        "TMUX_PROGRAM",
        "SHELL",
        "SSH",
        "MOSH",
        "DIR",
        "LOGIN",
        "INTERACTIVE",
        "TMUX_GROUP",
    ] {
        command.env_remove(format!("TERM_PRESERVER_LAUNCH__{field}"));
    }
    command.env_remove("TERM_PRESERVER_COMMAND");
    command.env("TTA_CONFIG", &server.tta_registry);
}

fn spawn_proxy(server: &Server, extra: &[impl AsRef<str>], trailing: &[&str]) -> Child {
    spawn_proxy_isolated(server, extra, trailing, |_| {})
}

/// Spawn the proxy with baseline isolation plus one test-specific adjustment
/// (a scaffold registry, a restricted PATH, an env choice).
fn spawn_proxy_isolated(
    server: &Server,
    extra: &[impl AsRef<str>],
    trailing: &[impl AsRef<str>],
    isolate: impl FnOnce(&mut Command),
) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_term-preserver"));
    command
        .args(["--config", "/dev/null", "--tmux-socket", &server.socket, "--tmux-program", TMUX])
        .args(extra.iter().map(AsRef::as_ref))
        .args(["--shell", "/bin/sh", "--no-login", "--no-interactive"])
        .arg("--")
        .args(trailing.iter().map(AsRef::as_ref))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    child_env(&mut command, server);
    isolate(&mut command);
    command.spawn().unwrap()
}

/// Drain a finished/nearly-finished child, killing it at the deadline (it is
/// this test's own process). Stdin is closed up front so the retry prompt
/// sees EOF and the proxy exits once the child exits. The third element is
/// the propagated exit code (None when the child died to a signal or kill).
fn drain(mut child: Child) -> (String, String, Option<i32>) {
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
    let code = child.wait().unwrap().code();
    (
        String::from_utf8_lossy(&out.join().unwrap()).to_string(),
        String::from_utf8_lossy(&err.join().unwrap()).to_string(),
        code,
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

// --- end-to-end: classic tmux backends (CLI and native) ---------------------

#[test]
fn classic_fresh_create_preserves_trailing_argv() {
    for backend in classic_backends() {
        let Some(server) = Server::new() else { return };
        let dir = server._dir.path().join("work");
        std::fs::create_dir_all(&dir).unwrap();
        let mut extra = base_flags(backend);
        extra.extend(["lp-fresh".to_string(), "--dir".into(), dir.to_str().unwrap().into()]);
        let child = spawn_proxy(&server, &extra, &["echo", "lp-quoted > done"]);
        let (out, err, _code) = drain(child);
        assert!(out.contains("lp-quoted > done"), "stdout: {out:?} stderr: {err:?}");
        assert_eq!(server.sessions(), vec!["tp-seed".to_string()], "session must die with its command");
        assert!(!dir.join("done").exists(), "redirect leaked out of the quoted argv");
    }
}

#[test]
fn classic_session_name_hash_survives_format_expansion() {
    for backend in classic_backends() {
        let Some(server) = Server::new() else { return };
        let mut extra = base_flags(backend);
        extra.push("lp#h".to_string());
        let child = spawn_proxy(&server, &extra, &["sleep", "4"]);
        let appeared = wait_until(Duration::from_secs(5), || {
            let mut names = server.sessions();
            names.sort();
            names == vec!["lp#h".to_string(), "tp-seed".to_string()]
        });
        server.kill_all();
        let (_out, err, _code) = drain(child);
        assert!(appeared, "session lp#h never appeared exactly-named; stderr: {err:?}");
    }
}

#[test]
fn classic_rerun_of_same_launch_reprobes_and_joins_group() {
    for backend in classic_backends() {
        let Some(server) = Server::new() else { return };
        let mut extra = base_flags(backend);
        extra.push("lp-grp".to_string());

        // Run 1 on an empty server: fresh primary; its command exits immediately.
        let (out, err, _code) = drain(spawn_proxy(&server, &extra, &["true"]));
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
        let (_out, err, _code) = drain(child);

        assert!(joined, "no satellite appeared: {groups:?} stderr: {err:?}");
        assert_eq!(satellite.len(), 1, "exactly one satellite expected: {groups:?}");
        assert_eq!(destroy, "keep-last", "satellite must set destroy-unattached keep-last");
    }
}

#[test]
fn classic_exact_probe_rejects_prefix_sessions() {
    for backend in classic_backends() {
        let Some(server) = Server::new() else { return };
        server.run_ok(&["-f", "/dev/null", "new-session", "-d", "-s", "lp-pre", "sleep", "8"]);
        let mut extra = base_flags(backend);
        extra.push("lp-pre2".to_string());
        let child = spawn_proxy(&server, &extra, &["echo", "lp-pre2-marker"]);
        let (out, err, _code) = drain(child);
        assert!(out.contains("lp-pre2-marker"), "primary branch must run for lp-pre2: {out:?} {err:?}");
        let mut remaining = server.sessions();
        remaining.sort();
        assert_eq!(remaining, vec!["lp-pre".to_string(), "tp-seed".to_string()], "prefix session must be untouched");
    }
}

#[test]
fn classic_no_group_plain_attach_creates_no_satellite() {
    for backend in classic_backends() {
        let Some(server) = Server::new() else { return };
        server.run_ok(&["-f", "/dev/null", "new-session", "-d", "-s", "lp-plain", "sleep", "8"]);
        let mut extra = base_flags(backend);
        extra.extend(["lp-plain".to_string(), "--no-tmux-group".to_string()]);
        let child = spawn_proxy(&server, &extra, &["sleep", "30"]);
        // Wait for the real attach before inspecting: a wrong grouped branch
        // parks the client on a satellite session instead, so only a client
        // sitting directly on lp-plain proves the plain-attach path ran.
        let attached =
            wait_until(Duration::from_secs(5), || server.attached_sessions().contains(&"lp-plain".to_owned()));
        let count = server.sessions().len(); // seed + attached session; a satellite would make three
        let destroy = server.option("lp-plain", "destroy-unattached");
        server.kill_all();
        let (_out, err, _code) = drain(child);
        assert!(attached, "no client attached directly to lp-plain; stderr: {err:?}");
        assert_eq!(count, 2, "no satellite may be created under --no-tmux-group: {count} sessions, stderr: {err:?}");
        // An unset option prints empty (tmux exits 1); it must NOT be keep-last.
        assert!(
            destroy.is_empty() || destroy == "off",
            "plain attach must not set destroy-unattached, got {destroy:?}"
        );
    }
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
    let (out, err, _code) = drain(child);
    assert!(
        out.contains("lp-tta > ran"),
        "startup argv must survive tta quoting untouched: stdout {out:?} stderr {err:?}"
    );
    assert_eq!(server.sessions(), vec!["tp-seed".to_string()], "session must die with its startup command");
}

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

// --- end-to-end: scaffold-on-miss through a fake rekon generator ------------

/// A fake `rekon` scaffold generator plus the tta registry naming it. The shim
/// is the only `rekon` the child can reach: its PATH is the shim directory plus
/// the two system directories, and construction refuses to run if either holds
/// a real rekon — the user's own script must never be invoked. The shim logs
/// its exact invocation, creates `<root>/$1`, and marks it; the `failing`
/// variant logs and exits 7 without creating anything.
struct Scaffold {
    registry: PathBuf,
    shim_dir: PathBuf,
    log: PathBuf,
    root: PathBuf,
}

impl Scaffold {
    fn new(server: &Server, default: Option<&str>, extra_generator: bool, failing: bool) -> Self {
        for dir in ["/usr/bin", "/bin"] {
            assert!(
                !Path::new(dir).join("rekon").exists(),
                "a real rekon in {dir} would be reachable on the restricted PATH; refusing to run"
            );
        }
        let root = server._dir.path().join("scaffold-root");
        std::fs::create_dir_all(&root).unwrap();
        let log = server._dir.path().join("rekon.log");
        let shim_dir = server._dir.path().join("shim");
        std::fs::create_dir_all(&shim_dir).unwrap();
        let guard = if failing { "\nexit 7" } else { "" };
        let shim = format!(
            "#!/bin/sh\necho \"rekon $*\" >> {}{}\nmkdir -p \"{}/$1\" || exit 3\n: > \"{}/$1/.shim-created\"\n",
            log.display(),
            guard,
            root.display(),
            root.display()
        );
        let shim_path = shim_dir.join("rekon");
        std::fs::write(&shim_path, shim).unwrap();
        std::fs::set_permissions(&shim_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut registry = String::new();
        if let Some(default) = default {
            registry.push_str(&format!("[scaffold]\ndefault = \"{default}\"\n"));
        }
        registry.push_str("[scaffold.generators.rk]\nadapter = \"rekon\"\n");
        if extra_generator {
            registry.push_str("[scaffold.generators.alt]\nadapter = \"rekon\"\n");
        }
        let registry_path = server._dir.path().join("scaffold-registry.toml");
        std::fs::write(&registry_path, registry).unwrap();
        Self {
            registry: registry_path,
            shim_dir,
            log,
            root,
        }
    }

    fn project(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Controlled TTA_CONFIG plus a PATH where `rekon` resolves only to the
    /// shim; every other lookup falls back to the two proven-clean system dirs.
    fn apply(&self, command: &mut Command) {
        command
            .env("TTA_CONFIG", &self.registry)
            .env("PATH", format!("{}:/usr/bin:/bin", self.shim_dir.display()));
    }

    fn invocations(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn scaffold_extra(scaffold: &Scaffold, backend: &str, name: &str) -> Vec<String> {
    vec![
        "--session-backend".into(),
        backend.into(),
        "--tta-program".into(),
        TTA.into(),
        "--session-root".into(),
        scaffold.root.to_str().unwrap().into(),
        "--tmux".into(),
        name.into(),
    ]
}

/// The startup command only writes its marker when the file the shim creates
/// already exists — startup runs inside the scaffolded directory, so a marker
/// proves the generator ran (and created the exact target) before the session.
/// The marker lands in a file, not on the PTY: pane rendering races the
/// session's death, files do not. Multi-word startup rides the plan as a raw
/// argv value (no shell interpretation), so the ordering test carries its own
/// explicit shell.
fn scaffold_startup(server: &Server, scaffold: &Scaffold, name: &str) -> (Vec<String>, PathBuf) {
    let created = scaffold.project(name).join(".shim-created");
    let marker = server._dir.path().join(format!("{name}.started"));
    let trailing = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!("test -f '{}' && echo scaf-startup-ok > '{}'", created.display(), marker.display()),
    ];
    (trailing, marker)
}

#[test]
fn tta_scaffold_create_runs_the_generator_before_the_session() {
    for_each_tta_backend(|backend| {
        let Some(server) = Server::new() else { return };
        let scaffold = Scaffold::new(&server, Some("rk"), false, false);
        let name = "scaf-fresh";
        let extra = scaffold_extra(&scaffold, backend, name);
        let (trailing, marker) = scaffold_startup(&server, &scaffold, name);
        let child = spawn_proxy_isolated(&server, &extra, &trailing, |cmd| scaffold.apply(cmd));
        let (out, err, _code) = drain(child);
        let started = std::fs::read_to_string(&marker).unwrap_or_default();
        assert!(
            started.contains("scaf-startup-ok"),
            "startup must run inside the scaffolded directory: stdout {out:?} stderr {err:?}"
        );
        // The adapter passes exactly the project name — nothing else, no shell.
        assert_eq!(scaffold.invocations(), vec![format!("rekon {name}")], "shim invocation log");
        assert!(scaffold.project(name).exists(), "shim must create the exact configured target");
        assert_eq!(server.sessions(), vec!["tp-seed".to_string()], "session dies with its startup command");
    });
}

#[test]
fn tta_scaffold_off_by_flag_or_environment_keeps_the_clean_miss() {
    for_each_tta_backend(|backend| {
        let Some(server) = Server::new() else { return };
        let scaffold = Scaffold::new(&server, Some("rk"), false, false);

        // Explicit --scaffold=false: the miss stays a miss, the shim never runs.
        let name = "scaf-off-flag";
        let mut extra = scaffold_extra(&scaffold, backend, name);
        extra.push("--scaffold=false".into());
        let child = spawn_proxy_isolated(&server, &extra, &["echo", "never-marker"], |cmd| scaffold.apply(cmd));
        let (out, err, _code) = drain(child);
        let combined = format!("{out}{err}");
        assert!(
            combined.contains("no session or immediate project directory matches"),
            "scaffold=false must keep the clean miss: {combined:?}"
        );
        assert!(!out.contains("never-marker"), "no session may start: {out:?}");
        assert!(scaffold.invocations().is_empty(), "no generator may run: {:?}", scaffold.invocations());
        assert!(!scaffold.project(name).exists());
        assert_eq!(server.sessions(), vec!["tp-seed".to_string()]);

        // The same through the environment: TTA_SCAFFOLD=false with a registry
        // whose default would scaffold must stay off.
        let name = "scaf-off-env";
        let extra = scaffold_extra(&scaffold, backend, name);
        let child = spawn_proxy_isolated(&server, &extra, &["echo", "never-marker"], |cmd| {
            scaffold.apply(cmd);
            cmd.env("TTA_SCAFFOLD", "false");
        });
        let (out, err, _code) = drain(child);
        let combined = format!("{out}{err}");
        assert!(
            combined.contains("no session or immediate project directory matches"),
            "TTA_SCAFFOLD=false must keep the clean miss: {combined:?}"
        );
        assert!(scaffold.invocations().is_empty(), "no generator may run: {:?}", scaffold.invocations());
        assert!(!scaffold.project(name).exists());
        assert_eq!(server.sessions(), vec!["tp-seed".to_string()]);
    });
}

#[test]
fn tta_scaffold_generator_failure_blocks_the_session_and_startup() {
    for_each_tta_backend(|backend| {
        let Some(server) = Server::new() else { return };
        let scaffold = Scaffold::new(&server, Some("rk"), false, true);
        let name = "scaf-fail";
        let extra = scaffold_extra(&scaffold, backend, name);
        let child = spawn_proxy_isolated(&server, &extra, &["echo", "never-marker"], |cmd| scaffold.apply(cmd));
        let (out, err, code) = drain(child);
        // The invocation attempt is logged, but nothing else may happen.
        assert_eq!(scaffold.invocations(), vec![format!("rekon {name}")], "shim was invoked");
        assert!(!scaffold.project(name).exists(), "a failed generator must not create the target");
        assert_eq!(server.sessions(), vec!["tp-seed".to_string()], "no session may exist after a failed generator");
        let (_trailing, marker) = scaffold_startup(&server, &scaffold, name);
        assert!(!marker.exists(), "startup must not run: {out:?}");
        assert!(code.is_some_and(|c| c != 0), "the launch must fail loudly, exit {code:?}: {out:?} {err:?}");
    });
}

#[test]
fn tta_scaffold_named_selection_is_honored_and_strict() {
    for_each_tta_backend(|backend| {
        let Some(server) = Server::new() else { return };
        let scaffold = Scaffold::new(&server, Some("rk"), true, false);

        // A configured name wins over the configured default.
        let name = "scaf-alt";
        let mut extra = scaffold_extra(&scaffold, backend, name);
        extra.push("--scaffold=alt".into());
        let (trailing, marker) = scaffold_startup(&server, &scaffold, name);
        let child = spawn_proxy_isolated(&server, &extra, &trailing, |cmd| scaffold.apply(cmd));
        let (out, err, _code) = drain(child);
        let started = std::fs::read_to_string(&marker).unwrap_or_default();
        assert!(
            started.contains("scaf-startup-ok"),
            "the named generator must scaffold: stdout {out:?} stderr {err:?}"
        );
        assert!(scaffold.project(name).exists());
        assert_eq!(scaffold.invocations(), vec![format!("rekon {name}")], "shim invocation log");

        // An unknown name is a loud lookup failure, never a silent default.
        let name = "scaf-ghost";
        let mut extra = scaffold_extra(&scaffold, backend, name);
        extra.push("--scaffold=ghost".into());
        let child = spawn_proxy_isolated(&server, &extra, &["echo", "never-marker"], |cmd| scaffold.apply(cmd));
        let (out, err, _code) = drain(child);
        let combined = format!("{out}{err}");
        assert!(combined.contains("unknown scaffold generator"), "{combined:?}");
        assert!(!out.contains("never-marker"), "no session may start: {out:?}");
        assert_eq!(scaffold.invocations().len(), 1, "ghost must not invoke the shim: {:?}", scaffold.invocations());
        assert!(!scaffold.project(name).exists());
        assert_eq!(server.sessions(), vec!["tp-seed".to_string()]);
    });
}

#[test]
fn explicit_positive_scaffold_refuses_classic_while_false_launches_classic() {
    if !cfg!(feature = "classic-tmux-cli") {
        eprintln!("skipping: classic-tmux-cli not compiled");
        return;
    }
    let Some(server) = Server::new() else { return };
    // Positive scaffold on an explicitly classic backend is refused before
    // anything runs — no silent classic creation.
    let mut extra = base_flags("classic-tmux-cli");
    extra.extend(["scaf-classic".to_string(), "--scaffold=rk".to_string()]);
    let child = spawn_proxy(&server, &extra, &["echo", "never-marker"]);
    let (out, err, code) = drain(child);
    let combined = format!("{out}{err}");
    assert!(combined.contains("requires a tta backend"), "{combined:?}");
    assert!(!out.contains("never-marker"), "no session may start: {out:?}");
    assert!(code.is_some_and(|c| c != 0), "refusal must exit non-zero, got {code:?}");
    assert_eq!(server.sessions(), vec!["tp-seed".to_string()]);

    // --scaffold=false opts back into classic creation.
    let dir = server._dir.path().join("work");
    std::fs::create_dir_all(&dir).unwrap();
    let mut extra = base_flags("classic-tmux-cli");
    extra.extend(["scaf-classic".to_string(), "--scaffold=false".to_string()]);
    let child = spawn_proxy(&server, &extra, &["echo", "scaf-off-classic-marker"]);
    let (out, err, _code) = drain(child);
    assert!(out.contains("scaf-off-classic-marker"), "classic must launch under scaffold=false: {out:?} {err:?}");
    assert_eq!(server.sessions(), vec!["tp-seed".to_string()], "session dies with its command");
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
    let (out, err, _code) = drain(child);
    assert!(out.contains("auto-classic-marker"), "stdout {out:?} stderr {err:?}");
    assert_eq!(server.sessions(), vec!["tp-seed".to_string()]);
}

#[cfg(feature = "tta-lib")]
#[test]
fn auto_prefers_embedded_tta_over_classic() {
    // --tmux names a fragment with no matching session or project: classic tmux
    // would have created the session anyway; embedded tta must instead fail
    // resolution, proving auto selected tta-lib over the classic fallback.
    // The child sees a controlled empty scaffold registry, so a clean miss is
    // reported either as the raw miss or as the empty-registry scaffold
    // guidance — both prove tta resolution failed without creating anything.
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
    let (out, err, _code) = drain(child);
    assert!(!out.contains("never-marker"), "classic fallback ran instead of tta: {out:?}");
    // Worker errors print inside the proxy's PTY, so they surface on stdout.
    let combined = format!("{out}{err}");
    assert!(
        combined.contains("no session or immediate project directory matches")
            || combined.contains("no generators are configured"),
        "expected tta resolve failure, stdout: {out:?} stderr: {err:?}"
    );
    assert_eq!(server.sessions(), vec!["tp-seed".to_string()]);
}
