// pattern: Imperative Shell

//! The `tper` binary: the everyday short name for term-preserver. Same
//! entry point, same behavior; only the install name differs.

fn main() -> eyre::Result<()> {
    term_preserver::bin_entry()
}
