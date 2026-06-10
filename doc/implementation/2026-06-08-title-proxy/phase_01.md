# title-proxy Implementation Plan

**Goal:** Create a Rust binary with CLI parsing and layered configuration for the PTY proxy.

**Architecture:** This phase creates the greenfield Rust package and the configuration boundary. Runtime behavior is split into an imperative entry point in `src/main.rs`, a pure config normalization core in `src/config.rs`, and placeholder module declarations for later phases only when they are needed.

**Tech Stack:** Rust 2021, `clap` v4 derive, `figment2`, `serde`, `bon`, `eyre`.

**Scope:** 6 phases from original design; this file implements phase 1.

**Codebase verified:** 2026-06-08. Current repo is greenfield: no `Cargo.toml`, no `src/`, empty `README.md`, no project-local `AGENTS.md` or `CLAUDE.md`.

---

## Acceptance Criteria Coverage

This phase implements project infrastructure only.

**Verifies:** None. Phase 1 is infrastructure; it is verified operationally by `cargo build`, `cargo clippy`, and CLI help output.

---

<!-- START_TASK_1 -->
### Task 1: Create Rust package manifest

**Files:**
- Create: `Cargo.toml`

**Step 1: Create the manifest**

Create `Cargo.toml` with:

```toml
[package]
name = "title-proxy"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"

[dependencies]
bon = "3.8"
clap = { version = "4.5", features = ["derive"] }
eyre = "0.6"
figment2 = { version = "0.11.5", features = ["toml"] }
nix = { version = "0.31", features = ["ioctl", "poll", "process", "signal", "term"] }
pty-process = "0.5.3"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
signal-hook = "0.3"

[dev-dependencies]
assert_cmd = "2.0"
expectrl = "0.9"
predicates = "3.1"
serial_test = "3.2"
tempfile = "3.20"
```

**Step 2: Verify dependency resolution**

Run: `cargo metadata --format-version 1`

Expected: Cargo resolves the dependency graph without errors and writes `Cargo.lock`.

**Step 3: Commit**

Run: `jj commit -m "Initialize Rust package" Cargo.toml Cargo.lock`
<!-- END_TASK_1 -->

<!-- START_SUBCOMPONENT_A (tasks 2-4) -->
<!-- START_TASK_2 -->
### Task 2: Add configuration model and loader

**Verifies:** None

**Files:**
- Create: `src/config.rs`

**Implementation:**

Create `src/config.rs` as a functional core module for config defaults plus an imperative shell function for loading external config. Add the required FCIS pattern comment at the top.

The module must define:
- `Config`, deriving `Debug`, `Clone`, `Serialize`, `Deserialize`, and `Builder`.
- `CliArgs`, deriving `clap::Parser`, with `command` as `#[arg(trailing_var_arg = true, allow_hyphen_values = true)]`.
- `CliOverlay` with `Option` fields so clap's default values do not overwrite environment or config file values.
- `load_config()` that merges precedence as defaults, config file, env, CLI overlay.
- `default_command()` returning `$SHELL` or `/bin/sh` if `$SHELL` is absent.
- `default_forward_signals()` returning `SIGTERM` and `SIGHUP`.

Use this merge order:

```rust
Figment::from(Serialized::defaults(Config::default()))
    .merge(Toml::file(config_path))
    .merge(Env::prefixed("TITLE_PROXY_").split("__").ignore_empty(true))
    .merge(Serialized::defaults(CliOverlay::from(cli_args)))
    .extract::<Config>()
```

Do not give clap defaults to `CliArgs` fields that should be overridable by env or config files.

**Testing:**

No tests in this task. Tests for config precedence are added in Task 4.

**Verification:**

Run: `rustfmt --edition 2021 src/config.rs --check`

Expected: `src/config.rs` is formatted.

**Commit:** `jj commit -m "Add configuration loader" src/config.rs`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Add CLI entry point

**Verifies:** None

**Files:**
- Create: `src/main.rs`

**Implementation:**

Create `src/main.rs` as an imperative shell with the FCIS pattern comment. It must:
- Import `clap::Parser`.
- Parse `CliArgs`.
- Call `config::load_config(cli_args)`.
- Print the loaded config in debug form for this phase only.
- Return `eyre::Result<()>` from `main`.

This temporary debug print is acceptable in phase 1 because no proxy runtime exists yet. Phase 2 replaces it with `proxy::run(config)`.

**Verification:**

Run: `cargo build`

Expected: Build succeeds.

Run: `cargo run -- --help`

Expected: Help text includes `title-proxy`, `--config`, `--forward-signals`, `--reemit-title`, and trailing command usage.

**Commit:** `jj commit -m "Add CLI entry point" src/main.rs`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Add configuration tests

**Verifies:** None. These tests protect the configuration scaffolding; AC6 coverage is completed and tracked in Phase 6.

**Files:**
- Modify: `src/config.rs`

**Testing:**

Add unit tests inside `src/config.rs` that verify behavior, not figment internals:
- Default command uses the provided shell fallback behavior.
- Default forwarded signals are exactly `SIGTERM` and `SIGHUP`.
- A CLI overlay with a command replaces the default command.
- Config-file values are overridden by env values, and env values are overridden by explicit CLI values.

Use `tempfile` for temporary config files. For environment tests, use `serial_test::serial` on every test that mutates `TITLE_PROXY_` variables, save previous values, and restore them before the test returns. Environment variables are process-global, so unique test data alone is not sufficient under parallel `cargo test`.

**Verification:**

Run: `cargo test`

Expected: All config tests pass.

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: Clippy passes with no warnings.

**Commit:** `jj commit -m "Test configuration precedence" src/config.rs`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_5 -->
### Task 5: Verify phase 1 operationally

**Files:**
- No file changes

**Verification:**

Run: `cargo build`

Expected: Build succeeds.

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: Clippy passes.

Run: `cargo run -- --help`

Expected: Binary prints help text.

**Commit:** No commit if no files changed.
<!-- END_TASK_5 -->
