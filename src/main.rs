// pattern: Imperative Shell

//! Shared entry for the two binary names. `term-preserver` (src/main.rs) and
//! `tper` (src/bin/tper.rs) both delegate here; see `src/completion.rs` for
//! the hidden completion lane.

fn main() -> eyre::Result<()> {
    term_preserver::bin_entry()
}
