//! Completion-candidate protocol and Zsh widget tests. Live-server tests use
//! explicit absolute private sockets under /tmp/opencode, verified absent
//! before the first mutation, and clean up the narrowest owned target (the
//! session they created) rather than the whole server.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
const TMUX: &str = "/usr/bin/tmux";

/// A private tmux server bound to an explicit absolute socket under
/// /tmp/opencode. Isolation is proven read-only before anything starts.
struct Server {
    /// Kept alive so the private socket directory outlives the server.
    _directory: tempfile::TempDir,
    socket: PathBuf,
}

impl Server {
    fn start(tag: &str) -> Option<Self> {
        if !Command::new(TMUX).arg("-V").output().is_ok_and(|output| output.status.success()) {
            eprintln!("skipping live-server completion test: {TMUX} unavailable");
            return None;
        }
        let directory = tempfile::Builder::new().prefix(tag).tempdir_in("/tmp/opencode").unwrap();
        let socket = directory.path().join("sock");
        assert!(socket.is_absolute());
        let probe = Command::new(TMUX).args(["-S", socket.to_str().unwrap(), "ls"]).output().unwrap();
        let stderr = String::from_utf8_lossy(&probe.stderr);
        assert!(!probe.status.success(), "unexpected server at {}", socket.display());
        assert!(
            stderr.contains(socket.to_str().unwrap()) && (stderr.contains("no server running") || stderr.contains("No such file or directory")),
            "isolation not proven: {stderr}"
        );
        let output = Command::new(TMUX)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .args(["-f", "/dev/null", "-S"])
            .arg(&socket)
            .args(["new-session", "-d", "-s", "live-work", "sleep", "60"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        Some(Self { _directory: directory, socket })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Narrowest cleanup: kill only the session created here. exit-empty
        // then removes the server; kill-server is never used.
        let _ = Command::new(TMUX)
            .arg("-S")
            .arg(&self.socket)
            .args(["kill-session", "-t", "live-work"])
            .output();
    }
}

fn tper() -> Command {
    let mut command = tper_base();
    // Never read the operator's real config file from a test.
    command.args(["--config", "/dev/null"]);
    command
}

/// A `tper` invocation with hermetic environment but no --config yet, for
/// tests that exercise an explicit config file of their own.
fn tper_base() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tper"));
    for (key, _) in std::env::vars_os() {
        let encoded = key.as_encoded_bytes();
        if encoded.starts_with(b"TERM_PRESERVER_") || encoded.starts_with(b"TTA_") || encoded == b"PROJECTS_DIR" {
            command.env_remove(&key);
        }
    }
    command.env_remove("TMUX").env_remove("TMUX_PANE");
    command
}

fn names(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

fn projects_fixture(root: &Path) {
    fs::create_dir_all(root.join("zebra/deep")).unwrap();
    for name in ["alpha", "space ' [x]", "line\nbreak"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    fs::write(root.join("file"), "not a project").unwrap();
    symlink("zebra", root.join("linked")).unwrap();
    symlink("missing", root.join("dangling")).unwrap();
    symlink("file", root.join("linked-file")).unwrap();
}

#[test]
fn project_inventory_is_sorted_literal_immediate_and_observation_only() {
    let temp = tempfile::Builder::new().prefix("tp-completion-").tempdir_in("/tmp/opencode").unwrap();
    projects_fixture(&temp.path().join("root"));
    let output = names(
        tper()
            .env_remove("HOME")
            .env("TTA_ROOT", "/unused-env-root")
            .arg("--session-root")
            .arg(temp.path().join("root"))
            .args(["--completion-candidates", "projects"]),
    );
    assert_eq!(output, "alpha\nlinked\nspace ' [x]\nzebra\n");
    // A session target conflicts with the completion lane: usage error, no output.
    let conflict = tper().args(["--completion-candidates=projects"]).arg("--tmux").arg("alpha").output().unwrap();
    assert_eq!(conflict.status.code(), Some(2));
    assert!(conflict.stdout.is_empty());
}

#[test]
fn completion_root_precedence_matches_launch_configuration() {
    let temp = tempfile::Builder::new().prefix("tp-completion-").tempdir_in("/tmp/opencode").unwrap();
    for root in ["src", "legacy", "env", "cli"] {
        fs::create_dir_all(temp.path().join(root).join(root)).unwrap();
    }
    for (env, legacy, cli, expected) in [
        ("", "", None, "src"),
        ("", "legacy", None, "legacy"),
        ("env", "legacy", None, "env"),
        ("env", "legacy", Some("cli"), "cli"),
    ] {
        let mut command = tper();
        command
            .current_dir(temp.path())
            .env("HOME", temp.path())
            .env("TTA_ROOT", env)
            .env("PROJECTS_DIR", legacy)
            .arg("--completion-candidates=projects");
        if let Some(cli) = cli {
            command.args(["--session-root", cli]);
        }
        assert_eq!(names(&mut command), format!("{expected}\n"));
    }
}

#[test]
fn candidate_errors_leave_stdout_empty_and_backends_match_build_features() {
    let temp = tempfile::Builder::new().prefix("tp-completion-").tempdir_in("/tmp/opencode").unwrap();
    let output = tper()
        .arg("--session-root")
        .arg(temp.path().join("missing"))
        .arg("--completion-candidates=projects")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot inspect project directory"));

    let backends = names(tper().arg("--completion-candidates=backends"));
    let offered: Vec<&str> = backends.lines().collect();
    assert!(offered.contains(&"auto"));
    assert_eq!(backends.contains("tta-lib"), cfg!(feature = "tta-lib"));
    assert_eq!(backends.contains("tta-cli"), cfg!(feature = "tta-cli"));
    assert_eq!(backends.contains("classic-tmux-cli"), cfg!(feature = "classic-tmux-cli"));
    assert_eq!(backends.contains("classic-tmux-tmax"), cfg!(feature = "classic-tmux-tmax"));
}

#[cfg(feature = "classic-tmux-cli")]
#[test]
fn live_session_inventory_uses_selected_endpoint_without_creating_a_server() {
    let temp = tempfile::Builder::new().prefix("tp-completion-").tempdir_in("/tmp/opencode").unwrap();
    let Some(server) = Server::start("tp-completion-live-") else {
        return;
    };
    assert_eq!(
        names(
            tper()
                .env_remove("HOME")
                .env("TMUX", "/some-other-server,12,0")
                .env("TTA_ROOT", "/no-such-root")
                .arg("--session-backend")
                .arg("classic-tmux-cli")
                .arg("--tmux-socket")
                .arg(&server.socket)
                .arg("--tmux-program")
                .arg(TMUX)
                .args(["--completion-candidates", "sessions"]),
        ),
        "live-work\n"
    );
    let cold = temp.path().join("cold-socket");
    assert_eq!(
        names(
            tper()
                .arg("--session-backend")
                .arg("classic-tmux-cli")
                .arg("--tmux-socket")
                .arg(&cold)
                .arg("--tmux-program")
                .arg(TMUX)
                .args(["--completion-candidates=sessions"]),
        ),
        ""
    );
    // An inventory query never starts a server.
    assert!(!cold.exists());
}

/// Auto availability without a usable tta program: any compiled local backend
/// answers from the same endpoint, so the inventory is feature-independent.
#[cfg(any(feature = "tta-lib", feature = "classic-tmux-tmax", feature = "classic-tmux-cli"))]
#[test]
fn auto_inventory_falls_back_to_a_compiled_local_backend() {
    let temp = tempfile::Builder::new().prefix("tp-completion-").tempdir_in("/tmp/opencode").unwrap();
    let Some(server) = Server::start("tp-completion-auto-") else {
        return;
    };
    assert_eq!(
        names(
            tper()
                .env_remove("HOME")
                .arg("--tta-program")
                .arg(temp.path().join("definitely-not-installed-tta"))
                .arg("--tmux-socket")
                .arg(&server.socket)
                .arg("--tmux-program")
                .arg(TMUX)
                .args(["--completion-candidates", "sessions"]),
        ),
        "live-work\n"
    );
}

#[cfg(feature = "tta-lib")]
#[test]
fn scaffold_inventory_lists_configured_generators_and_fails_loudly() {
    let temp = tempfile::Builder::new().prefix("tp-completion-").tempdir_in("/tmp/opencode").unwrap();
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        "[scaffold.generators.zeta]\nadapter = \"rekon\"\n[scaffold.generators.alpha]\nadapter = \"rekon\"\n",
    )
    .unwrap();
    let output = names(tper().env("TTA_CONFIG", &config).arg("--completion-candidates=scaffolds"));
    assert_eq!(output, "alpha\nzeta\n");
    // A broken registry fails the inventory with an error, not silence.
    fs::write(&config, "[scaffold.generators.x]\nadapter = \"nope\"\n").unwrap();
    let output = tper().env("TTA_CONFIG", &config).arg("--completion-candidates=scaffolds").output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("does not include"),
        "unexpected scaffold error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The subprocess scaffold path only runs when no embedded registry exists;
/// with tta-lib the registry loads in-process instead.
#[cfg(all(feature = "tta-cli", not(feature = "tta-lib")))]
#[test]
fn tta_cli_scaffold_inventory_forwards_to_the_tta_program() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::Builder::new().prefix("tp-completion-").tempdir_in("/tmp/opencode").unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let shim = bin.join("tta");
    fs::write(
        &shim,
        "#!/bin/sh\n[ \"$1\" = --completion-candidates ] && [ \"$2\" = scaffolds ] && printf 'zeta\\nalpha\\n' && exit 0\nexit 64\n",
    )
    .unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    let mut dirs = vec![bin];
    dirs.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let path = std::env::join_paths(dirs).unwrap();
    assert_eq!(
        names(
            tper()
                .env("PATH", &path)
                .arg("--session-backend")
                .arg("tta-cli")
                .arg("--tta-program")
                .arg("tta")
                .args(["--completion-candidates", "scaffolds"]),
        ),
        "alpha\nzeta\n"
    );
}

/// With both tta implementations compiled, an explicit `tta-cli` — selected
/// through a real config file, with the tta program itself named there — must
/// answer through that CLI even though the embedded registry could answer
/// in-process. The inventory then matches what that launch would consult.
#[cfg(all(feature = "tta-cli", feature = "tta-lib"))]
#[test]
fn scaffold_inventory_honors_explicit_tta_cli_and_configured_program_over_the_registry() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::Builder::new().prefix("tp-completion-").tempdir_in("/tmp/opencode").unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let shim = bin.join("my-tta");
    fs::write(
        &shim,
        "#!/bin/sh\n[ \"$1\" = --completion-candidates ] && [ \"$2\" = scaffolds ] && printf 'shim-zeta\\nshim-alpha\\n' && exit 0\nexit 64\n",
    )
    .unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    // A loadable registry exists and would answer in-process names...
    let registry = temp.path().join("tta-config.toml");
    fs::write(&registry, "[scaffold.generators.registry]\nadapter = \"rekon\"\n").unwrap();
    // ...but the config file selects the subprocess backend with its own tta.
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        format!("[launch]\nsession_backend = \"tta-cli\"\ntta_program = \"{}\"\n", shim.display()),
    )
    .unwrap();
    assert_eq!(
        names(
            tper_base()
                .env("TTA_CONFIG", &registry)
                .arg("--config")
                .arg(&config)
                .args(["--completion-candidates", "scaffolds"]),
        ),
        "shim-alpha\nshim-zeta\n"
    );
}

/// Inventories are local-only: a remote transport answers nothing, and the
/// refusal must be observable as zero process contact — no ssh, no mosh, no
/// tmux query against a local server a remote launch would never use.
#[test]
fn remote_transport_offers_no_host_inventory_and_never_contacts_a_host() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::Builder::new().prefix("tp-completion-").tempdir_in("/tmp/opencode").unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    for name in ["ssh", "mosh", "tmux"] {
        let shim = bin.join(name);
        fs::write(&shim, format!("#!/bin/sh\nprintf 'contacted {name}\\n' >> \"$CONTACT_LOG\"\nexit 99\n")).unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let log = temp.path().join("contact.log");
    let mut dirs = vec![bin];
    dirs.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let path = std::env::join_paths(dirs).unwrap();
    for transport in ["--ssh", "--mosh"] {
        for what in ["sessions", "projects", "scaffolds"] {
            let output = tper()
                .env("PATH", &path)
                .env("CONTACT_LOG", &log)
                .arg(transport)
                .arg("elsewhere")
                .arg("--tmux-program")
                .arg("tmux")
                .args(["--completion-candidates", what])
                .output()
                .unwrap();
            assert!(output.status.success(), "{} {}: {}", transport, what, String::from_utf8_lossy(&output.stderr));
            assert!(output.stdout.is_empty(), "{transport} {what}: {}", String::from_utf8_lossy(&output.stdout));
        }
    }
    // The compiled backend enum is host-independent and still answers.
    let backends = tper()
        .env("PATH", &path)
        .env("CONTACT_LOG", &log)
        .arg("--ssh")
        .arg("elsewhere")
        .arg("--completion-candidates=backends")
        .output()
        .unwrap();
    assert!(backends.status.success());
    assert!(String::from_utf8(backends.stdout).unwrap().contains("auto"));
    // Nothing ever reached a transport or tmux.
    assert!(!log.exists(), "a shim was contacted");
}

#[cfg(feature = "classic-tmux-cli")]
#[test]
fn zsh_completes_targets_options_and_quoted_names() {
    if !Command::new("zsh").arg("-c").arg("exit 0").output().is_ok_and(|output| output.status.success()) {
        eprintln!("skipping zsh widget test: zsh unavailable");
        return;
    }
    let Some(server) = Server::start("tp-completion-zsh-") else {
        return;
    };
    let fixture = tempfile::Builder::new().prefix("tp-completion-zshfix-").tempdir_in("/tmp/opencode").unwrap();
    let shell = Path::new(env!("CARGO_MANIFEST_DIR")).join("shell/zsh");
    let output = Command::new("zsh")
        .arg("-f")
        .arg(shell.join("tests/completion.zsh"))
        .arg(env!("CARGO_BIN_EXE_tper"))
        .arg(shell.join("functions"))
        .arg(fixture.path())
        .arg(&server.socket)
        .output()
        .expect("zsh widget test requires zsh");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
