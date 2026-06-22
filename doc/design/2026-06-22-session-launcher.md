# Session Launcher Design

## Summary

Adds composable CLI flags (`--shell`, `--ssh`, `--mosh`, `--tmux`, `--tmux-create`, `--tmux-attach`, `--dir`, `--no-login`, `--no-interactive`) that resolve into a single `CommandSpec` before session startup. The launcher builds the command vector inside-out: shell selection (innermost), optional tmux wrapping, optional ssh/mosh transport wrapping.

Follow-on to the [child-session orchestration design](/doc/design/2026-06-08-child-session-orchestration.md). Uses the existing `CommandSpec`, `RetryLoop`, and `ChildSession` types without modification.

## Goals

1. Composable flags that build remote/tmux/shell commands without manual quoting.
2. Retry reconnects and re-attaches (resolved `CommandSpec` carries the full stack).
3. Shell-aware invocation: POSIX (bash/zsh/sh), fish, and nushell families.
4. Pure functional resolution pipeline, testable without I/O.
5. Existing `-- command` path still works when no launch flags are present.

## Non-goals

1. No multi-hop SSH or jump host support.
2. No tmux session creation options beyond name and working directory.
3. No shell-specific configuration beyond login/interactive flags.
4. No persistence of launch preferences between runs beyond config file.

## Flags

| Flag | Takes value | Description |
|------|-------------|-------------|
| `--shell <name>` | yes | Shell override (full path or basename) |
| `--ssh <host>` | yes | SSH transport (mutually exclusive with `--mosh`) |
| `--mosh <host>` | yes | mosh transport (mutually exclusive with `--ssh`) |
| `--tmux <session>` | yes | Create or attach (tmux `new -As`) |
| `--tmux-create <session>` | yes | Create only (tmux `new -s`) |
| `--tmux-attach <session>` | yes | Attach only (tmux `a -t`) |
| `--dir <path>` | yes | Working directory for innermost layer |
| `--no-login` | no | Skip login shell mode (default: login) |
| `--no-interactive` | no | Skip interactive shell mode (default: interactive) |

Mutual exclusivity: `--ssh`/`--mosh` error if both set. `--tmux`/`--tmux-create`/`--tmux-attach` error if multiple set.

Trailing `-- <command>` still works. When combined with launch flags, the trailing command becomes the innermost layer (see Composition below).

## Composition model

Flags resolve the command vector inside-out. Each layer wraps the previous:

```
Layer 1 (shell):     determines shell binary and invocation mode
Layer 2 (inner cmd): trailing command > tmux command > cd+exec (dir) > bare shell
Layer 3 (transport): ssh/mosh wraps the shell invocation
```

### Examples

```
# Bare shell
title-proxy
→ [$SHELL]

# Shell override
title-proxy --shell bash
→ [bash]

# Local tmux
title-proxy --tmux dev
→ [zsh, -lic, "exec tmux new -As dev"]

# Remote tmux (the main use case)
title-proxy --mosh w --tmux compfuzor
→ [mosh, w, --, zsh, -lic, "exec tmux new -As compfuzor"]

# Remote tmux with nushell
title-proxy --mosh w --shell nu --tmux compfuzor
→ [mosh, w, --, nu, -c, "tmux new -As compfuzor"]

# Tmux with working directory
title-proxy --mosh w --tmux dev --dir ~/src/foo
→ [mosh, w, --, zsh, -lic, "exec tmux new -c ~/src/foo -As dev"]

# Tmux with trailing command (vim inside tmux)
title-proxy --mosh w --tmux dev -- vim
→ [mosh, w, --, zsh, -lic, "exec tmux new -As dev vim"]

# Dir without tmux (interactive shell in directory)
title-proxy --dir ~/src/foo
→ [zsh, -lic, "cd ~/src/foo && exec zsh -li"]

# Dir with trailing command, no tmux
title-proxy --dir ~/src/foo -- make test
→ [zsh, -lic, "cd ~/src/foo && exec make test"]

# SSH without tmux (gets remote login shell)
title-proxy --ssh w
→ [ssh, w]
```

## Key types

### No standalone LaunchSpec

The parsed launch flags live as a nested `LaunchConfig` inside `Config`, not as a separate top-level type. Clap parses the flags, `CliOverlay` carries them through figment2, `Config.launch` holds the result. The resolution logic (`resolve()`) is a pure method on `LaunchConfig`. No duplication between clap structs and a separate spec type.

### LaunchConfig (data, lives in config.rs or launch.rs)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LaunchConfig {
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub ssh: Option<String>,
    #[serde(default)]
    pub mosh: Option<String>,
    #[serde(default)]
    pub tmux: Option<String>,
    #[serde(default)]
    pub tmux_create: Option<String>,
    #[serde(default)]
    pub tmux_attach: Option<String>,
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default = "default_launch_login")]
    pub login: bool,
    #[serde(default = "default_launch_interactive")]
    pub interactive: bool,
}

fn default_launch_login() -> bool { true }
fn default_launch_interactive() -> bool { true }
```

Tmux flags are three separate `Option<String>` fields rather than an enum. Mutual exclusivity is validated in `resolve()`, not in the type system. This keeps clap/figment2/serde derives clean.

### ShellFamily (lives in launch.rs)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellFamily {
    Posix,    // bash, zsh, sh, dash
    Fish,     // fish
    Nushell,  // nu, nushell
}
```

Detection takes a shell name (or path basename) and maps it to a family. Unknown shells map to `Posix` (safe default -- most shells are POSIX-compatible).

### Config integration

```rust
pub struct Config {
    // ... existing fields ...
    #[serde(default)]
    pub launch: LaunchConfig,
}
```

### is_active guard

```rust
impl LaunchConfig {
    pub fn is_active(&self) -> bool {
        self.shell.is_some()
            || self.ssh.is_some()
            || self.mosh.is_some()
            || self.tmux.is_some()
            || self.tmux_create.is_some()
            || self.tmux_attach.is_some()
            || self.dir.is_some()
            || !self.login
            || !self.interactive
    }
}
```

In `proxy::run()`:

```rust
let command = if config.launch.is_active() {
    config.launch.resolve(&config.command)?
} else {
    CommandSpec::try_from(config.command.clone())?
};
```

The old path (`CommandSpec::try_from(config.command)`) is unchanged when no launch flags are present. Launch flags are additive.

## Resolution pipeline

`LaunchConfig::resolve()` runs four pure stages:

### Stage 1: resolve shell

```rust
fn resolve_shell(&self) -> ShellResolution
```

Returns a `ShellResolution` containing the shell name, family, and invocation mode (login, interactive).

Resolution order:
1. `--shell` flag value (basename for detection, pass through as-is)
2. `$SHELL` env var (basename)
3. Runtime detection snippet (see Shell fallback below)

### Shell fallback (no --shell, no $SHELL)

Emit a portable `sh -c` snippet that checks for zsh at runtime:

- Without inner command: `sh -c 'command -v zsh >/dev/null 2>&1 && exec zsh -li || exec sh -li'`
- With inner command (POSIX):
  ```
  sh -c 'if command -v zsh >/dev/null 2>&1; then exec zsh -lic "exec <inner>"; else exec sh -lc "exec <inner>"; fi'
  ```

This works locally and remotely without probing the target filesystem before connecting.

### Stage 2: resolve inner command

```rust
fn resolve_inner_command(&self, trailing: &[String]) -> eyre::Result<Option<String>>
```

Priority:
1. **Trailing command + tmux**: `tmux new -As <session> <trailing...>` (tmux handles the command natively)
2. **Trailing command, no tmux**: the trailing command itself (shell wraps with exec)
3. **Tmux, no trailing**: `tmux new -As <session>` (with `-c <dir>` if `--dir` present)
4. **Dir, no tmux, no trailing**: `cd <dir> && exec <shell> -li` (interactive shell in directory)
5. **Dir + trailing, no tmux**: `cd <dir> && exec <trailing>`
6. **Nothing**: `None` (bare shell)

Tmux command construction by mode:

| Flag | Command |
|------|---------|
| `--tmux <s>` | `tmux new -As <s>` |
| `--tmux-create <s>` | `tmux new -s <s>` |
| `--tmux-attach <s>` | `tmux a -t <s>` |

With `--dir <path>`, tmux commands get `-c <path>`:
- `--tmux dev --dir ~/foo`: `tmux new -c ~/foo -As dev`
- `--tmux-attach dev --dir ~/foo`: `tmux attach-session -c ~/foo -t dev`

### Stage 3: build shell invocation

```rust
fn build_shell_invocation(
    shell: &str,
    family: ShellFamily,
    login: bool,
    interactive: bool,
    inner_cmd: Option<&str>,
) -> Vec<String>
```

| Family | Inner cmd | Result |
|--------|-----------|--------|
| POSIX | `Some(cmd)` | `[shell, (-l?), (-i?), -c, "exec {cmd}"]` |
| POSIX | `None` | `[shell, (-l?), (-i?)]` |
| Fish | `Some(cmd)` | `[fish, (-l?), (-i?), -c, "exec {cmd}"]` |
| Fish | `None` | `[fish, (-l?), (-i?)]` |
| Nushell | `Some(cmd)` | `[nu, -c, "{cmd}"]` |
| Nushell | `None` | `[nu]` |

Nushell has no `-l` flag and no `exec` builtin. `nu -c` exits after running the command, so `exec` is unnecessary.

Login and interactive flags are independent. Default: both true (`-li` for POSIX/fish). `--no-login` drops `-l`. `--no-interactive` drops `-i`.

### Stage 4: wrap transport

```rust
fn wrap_transport(shell_cmd: Vec<String>, ssh: Option<&str>, mosh: Option<&str>) -> Vec<String>
```

- `--ssh host`: prepend `[ssh, host, --]`
- `--mosh host`: prepend `[mosh, host, --]`
- Neither: unchanged

### Full pipeline

```rust
impl LaunchConfig {
    pub fn resolve(&self, trailing: &[String]) -> eyre::Result<CommandSpec> {
        self.validate()?; // mutual exclusivity checks
        let shell = self.resolve_shell();
        let inner_cmd = self.resolve_inner_command(trailing)?;
        let shell_cmd = build_shell_invocation(&shell.name, shell.family, self.login, self.interactive, inner_cmd.as_deref());
        let full_cmd = wrap_transport(shell_cmd, self.ssh.as_deref(), self.mosh.as_deref());
        Ok(CommandSpec::try_from(full_cmd)?)
    }
}
```

## CLI and config integration

### CliArgs (clap derive)

New fields on the existing `CliArgs` struct:

```rust
#[arg(long)]
pub shell: Option<String>,

#[arg(long)]
pub ssh: Option<String>,

#[arg(long)]
pub mosh: Option<String>,

#[arg(long)]
pub tmux: Option<String>,

#[arg(long)]
pub tmux_create: Option<String>,

#[arg(long)]
pub tmux_attach: Option<String>,

#[arg(long)]
pub dir: Option<String>,

#[arg(long)]
pub no_login: bool,

#[arg(long)]
pub no_interactive: bool,
```

The existing `trailing_var_arg` + `allow_hyphen_values` on `command` handles trailing args after flags. Clap stops parsing flags at the first positional or after `--`.

### CliOverlay

New fields in `CliOverlay` mapping clap output to figment2:

```rust
#[serde(skip_serializing_if = "Option::is_none")]
launch_shell: Option<String>,
#[serde(skip_serializing_if = "Option::is_none")]
launch_ssh: Option<String>,
// ... etc ...

// Login/interactive are inverted: --no-login → Some(false)
#[serde(skip_serializing_if = "Option::is_none")]
launch_login: Option<bool>,
#[serde(skip_serializing_if = "Option::is_none")]
launch_interactive: Option<bool>,
```

The `From<CliArgs> for CliOverlay` impl maps `no_login: true` to `launch_login: Some(false)`, and `no_login: false` to `launch_login: None` (don't override figment2 default).

### Config file

```toml
[launch]
shell = "zsh"
ssh = "w"
tmux = "compfuzor"
dir = "~/src/foo"
login = true
interactive = true
```

### Environment variables

Figment2 env support with `__` nesting:

```
TITLE_PROXY_LAUNCH__SSH=w
TITLE_PROXY_LAUNCH__TMUX=compfuzor
TITLE_PROXY_LAUNCH__LOGIN=false
```

## Module structure

Single `launch.rs` module. All pure functional core. Logical sections within the file:

```
// pattern: Functional Core

// --- types ---
LaunchConfig (or re-export from config.rs)
ShellFamily
ShellResolution

// --- stage 1: shell ---
fn detect_shell_family(name: &str) -> ShellFamily
fn resolve_shell(launch: &LaunchConfig) -> ShellResolution
fn build_fallback_snippet(login: bool, interactive: bool, inner: Option<&str>) -> Vec<String>

// --- stage 2: inner command ---
fn build_tmux_command(mode, session, dir, trailing) -> String
fn resolve_inner_command(launch: &LaunchConfig, trailing: &[String]) -> Option<String>

// --- stage 3: shell invocation ---
fn build_shell_invocation(shell, family, login, interactive, inner) -> Vec<String>

// --- stage 4: transport ---
fn wrap_transport(cmd, ssh, mosh) -> Vec<String>

// --- pipeline ---
impl LaunchConfig {
    fn validate(&self) -> eyre::Result<()>;
    fn is_active(&self) -> bool;
    fn resolve(&self, trailing: &[String]) -> eyre::Result<CommandSpec>;
}

// --- tests ---
```

If this grows large, it can split to `launch/` with `shell.rs`, `tmux.rs`, `transport.rs`, `mod.rs`. For now one file keeps the pipeline readable end-to-end.

## Retry behavior

`RetryLoop` already re-runs the resolved `CommandSpec`. Since `CommandSpec` includes the full transport + shell + tmux stack, pressing space at the retry prompt reconnects via ssh/mosh and re-attaches tmux. No changes to `RetryLoop` needed.

## Testing strategy

All resolution logic is pure (no I/O). Tests in `launch.rs`:

**Shell family detection:**
- `/bin/zsh` → Posix
- `bash` → Posix
- `fish` → Fish
- `nu` → Nushell
- `nushell` → Nushell
- `unknown-shell` → Posix (safe fallback)

**Shell invocation builder:**
- POSIX with inner command, login+interactive
- POSIX bare, login only
- POSIX bare, no login, no interactive
- Nushell with inner command
- Nushell bare
- Fish with inner command

**Tmux command construction:**
- `--tmux dev` → `tmux new -As dev`
- `--tmux-create dev` → `tmux new -s dev`
- `--tmux-attach dev` → `tmux a -t dev`
- `--tmux dev --dir ~/foo` → `tmux new -c ~/foo -As dev`
- `--tmux dev -- vim` → `tmux new -As dev vim`

**Transport wrapping:**
- `--ssh w` prepends `[ssh, w, --]`
- `--mosh w` prepends `[mosh, w, --]`

**Full resolution pipeline:**
- Each example from the Composition section above
- Mutual exclusivity errors (--ssh + --mosh, --tmux + --tmux-create)
- Fallback snippet emitted when no shell info available

**CLI parsing:**
- Clap parses new flags correctly
- Trailing args still work with and without launch flags
- `--no-login` and `--no-interactive` flags parse correctly

## Open questions

1. Should `--shell` validate that the shell exists locally before using it? Or just pass through and let the remote/host error if missing?
2. Should the fallback snippet prefer `$SHELL` on the remote over zsh? (e.g., source `$SHELL` env on the remote side). Currently we skip `$SHELL` when it's not set locally, but it might be set remotely.
3. tmux `attach-session -c <dir>` requires tmux 3.0+. Should we warn or fall back to cd-wrapping for older tmux?
