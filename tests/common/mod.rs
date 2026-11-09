//! Shared fixture scratch selection. `TTA_TEST_TMPDIR` overrides; otherwise
//! `/tmp/opencode` is used when the current user can create fixtures there,
//! falling back to the system temp directory. Every fixture still creates its
//! own uniquely named private directory below the chosen root, so isolation
//! never depends on which root was selected.
use std::path::PathBuf;

pub fn scratch_root() -> PathBuf {
    if let Some(path) = std::env::var_os("TTA_TEST_TMPDIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    if tempfile::Builder::new().prefix("tp-scratch-probe-").tempdir_in("/tmp/opencode").is_ok() {
        return PathBuf::from("/tmp/opencode");
    }
    std::env::temp_dir()
}

pub fn scratch_dir(prefix: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(scratch_root())
        .expect("fixture scratch root must be usable; set TTA_TEST_TMPDIR to an owned directory")
}
