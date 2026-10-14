---
type: Brief
title: kopacetic-turso multi-process viability at turso 0.8.0-pre.4
description: Decisive source-verified answer on many term-preserver processes writing one shared Turso database, with caveats and a scoped handoff proposal.
resource: /design/tracking/turso-multiprocess.glm53m.md
status: draft
author: model:zai-coding-plan/glm-5.3#max
date: 2026-10-14
sources:
  - resource: "opencode://ses_ec8438a0cffekFPbiWgK70TqvU"
    title: "Commissioned assessment: pinned driver source in the cargo registry, archived upstream, crates.io metadata; two-OS-process probe incl. short-lived open-put-drop alternation and bounded overlap retry; verdict, mixed-mode finding, short-lived section, handoff"
  - resource: "file:///home/rektide/src/kopacetic/crates/kopacetic-turso/src/storage/connection.rs"
    title: "Kopacetic open path: turso::Builder::new_local(path).build() with default flags"
  - resource: "file:///home/rektide/src/kopacetic/doc/consumer/term-preserver.md"
    title: "Parent-prepared consumer contract: shared key/write law and qualification workload this verdict integrates with"
  - resource: "file:///home/rektide/src/term-preserver/.test-agent/turso-multiprocess-probe/README.md"
    title: "Gitignored scratch probe (two OS processes, three phases incl. mixed-mode control) confirming the pinned boundary empirically"
  - resource: "https://crates.io/api/v1/crates/turso/0.8.0-pre.4"
    title: "Publish dates for 0.8.0-pre.4 (2026-08-11), 0.8.0-pre.11 (2026-09-11), 0.8.1 (2026-09-29)"
  - resource: "https://github.com/tursodatabase/turso/blob/main/turso-docs/sql-reference/multiprocess-access.mdx"
    title: "Official Multi-Process Access doc (experimental multiprocess WAL), archived copy read locally"
extensions:
  related_work:
    - /design/tracking/README.md
    - /design/tracking/shell-tracking0.gpt6a.md
  scope: "Design research only; no implementation, no tickets."
  note: "Caminus collection mechanics are assessed by a parallel FX wave; shared entity/API shape by the parent. This brief stays on the Turso driver question."
---

# Can many term-preserver processes share one kopacetic-turso database?

Scenario: every shell runs under its own lightweight term-preserver login-shell
proxy; each process updates only its own per-shell entity in one shared
Kopacetic collection. Question: is `kopacetic-turso`, as pinned and written
today, safe when **multiple OS processes** open the **same local database
file**, each writing a different entity?

## Verdict

**No — not with the current open path.** `kopacetic-turso` opens with
`turso::Builder::new_local(path).build()` and default flags
([connection.rs, `storage::connection::open`](file:///home/rektide/src/kopacetic/crates/kopacetic-turso/src/storage/connection.rs)).
In the pinned driver (`turso = "0.8.0-pre.4"`, locked in the workspace
`Cargo.lock`; published 2026-08-11) the default mode is **single-OS-process by
design**: the first process takes an exclusive file lock at open and every
later process fails fast.

- The failure is explicit, not corruption: the second `build()`/open returns a
  locking error ("Failed locking file … File is locked by another process",
  `LimboError::LockingError`). Confirmed against Kopacetic's own `TursoStore`
  by the [two-process probe](#two-os-process-probe) below.
- There *is* an opt-in multi-process mode (`experimental_multiprocess_wal`),
  but Kopacetic does not enable it, and on this exact pin it contains a known
  unfixed reader race plus a mixed-mode guard that the bindings path never
  enforces (both below). It is not a configuration flip away.

For the [consumer contract](file:///home/rektide/src/kopacetic/doc/consumer/term-preserver.md)'s
original question — *can direct independent processes write different
entities in one database file?* — the answer for **long-lived per-proxy
handles** is: **rejected at open by default** (before entity semantics
matter); **only mechanically possible via the experimental flag**, which this
pin does not make safe. The topology correction below adds the shape the
human actually wants: **short-lived writers** (open → put → drop per
publication), which the pin *does* support — see
[Short-lived writers](#short-lived-writers-open--put--drop-across-processes).

## Two-OS-process probe

Minimal empirical confirmation lives in gitignored scratch:
[`.test-agent/turso-multiprocess-probe/`](file:///home/rektide/src/term-preserver/.test-agent/turso-multiprocess-probe/README.md)
(single binary re-execing itself as the child; three short scenarios, no
benchmarks). Results on the pinned driver, 2026-10-14:

1. **Kopacetic exact path** — parent `TursoStore::open` + write of entity
   `alpha`; second OS process `TursoStore::open` on the same file →
   `Locking error: Failed locking file '…p1-default.db'. File is locked by
   another process`. Parent's value unaffected. The boundary the contract's
   qualification workload asks about ("the second process's open attempt")
   behaves exactly as the source says.
2. **Mixed modes** — with the parent's lock *proven held* (a control child's
   default open is rejected), a child with
   `experimental_multiprocess_wal(true)` **opens successfully**. The
   documented mixed-mode rejection does not fire through the bindings path.
3. **Both multiprocess** — two processes, disjoint sequential writes, both
   rows persist and are read back. Mechanical capability only; establishes no
   safety on a pin carrying the reader race.
4. **Short-lived alternation** — two *persistent* OS processes, six
   open→put→drop publications each into their own entities, strictly
   alternating; all 12 values intact, both processes alive throughout.
   Drop — not process exit — releases the lock.
5. **Bounded overlap retry** — with a child-held store, the parent's open is
   rejected (control); with a parent-held store, the child's bounded retry
   observed 15 `LockingError`s over 323 ms, then acquired immediately after
   the parent dropped, and its value persisted. Final reads went through
   their own short-lived handles, dropped before reporting.

## Evidence chain (pinned sources actually read)

1. **Kopacetic open path** — `storage/connection.rs:12-18`:
   `Builder::new_local(path).build()`; no `experimental_multiprocess_wal`,
   no custom IO, no flags. Builder defaults in `turso-0.8.0-pre.4/src/lib.rs`
   set `enable_multiprocess_wal: false`.
2. **Lock acquisition** — `turso_core-0.8.0-pre.4/io/unix.rs:50-73`
   (`UnixIO::open_file`): unless `ReadOnly`/`NoLock` flags or the
   `LIMBO_DISABLE_FILE_LOCK` env var is set, every file open calls
   `lock_file(true)` — **exclusive, non-blocking `fcntl(F_SETLK)`** held for
   the fd's lifetime (`io/unix.rs:276-313`). Process-scoped: released by
   close or process death.
3. **It is tested, deliberately** — `turso_core-0.8.0-pre.4/multiprocess_tests.rs`:
   `database_open_without_experimental_multiprocess_wal_rejects_second_process`
   spawns a real child process; the child's default open must fail with
   `LockingError` ("default non-multiprocess open should stay DB-file locked
   across processes").
4. **Official doc** — `turso-docs/sql-reference/multiprocess-access.mdx`:
   "By default, a Turso database file is opened by a single OS process.
   Multiple threads and connections within that process can safely share the
   database, but opening the same file from a second process is rejected with
   a locking error."
5. **Design intent for the default WAL** — `turso_core-0.8.0-pre.4/storage/wal.rs`
   (`WalSharedRuntime` doc comment): "we will never support multi process"
   for the default WAL index — its frame cache and read-locks are
   process-local by construction, with no SQLite-style `-shm` index.

## The four distinctions that matter

| Shape | Works on 0.8.0-pre.4 default? | Why |
|---|---|---|
| Different DB files, one per process | Yes | No sharing to lose — but then there is no shared collection, defeating the goal |
| Multiple connections in one process | Yes | `Database::open` consults a process-wide registry (keyed by dev/ino); `connect()` yields independent connections over one shared Database/WAL (MVCC machinery present; MVCC↔multiprocess are mutually exclusive, see below) |
| Cloned stores in one process | Qualified | `TursoStore` is `Clone` wrapping `turso::Connection`, whose `Clone` shares one `Arc` connection handle — clones are **not** independently safe for concurrent `put` transactions on that shared connection; serialize use (e.g., a single writer task) or open a separately qualified connection. The workspace README already scopes this as "cloned-store concurrency retain the pinned driver's documented limits" |
| **Separate processes, same file (concurrent)** | **No — open rejected** | Exclusive `fcntl` lock held from first open to close/exit (evidence above) |
| Separate processes, same file (**short-lived**, non-overlapping) | **Yes** | Drop of all handles releases the lock while the process lives; see [Short-lived writers](#short-lived-writers-open--put--drop-across-processes) — probe phases 4–5 |

In-process double-`open` of the same path additionally cannot collide: the
registry returns the same `Arc<Database>`, and POSIX `fcntl` locks do not
block within one process anyway. The wall is specifically cross-process.

## The escape hatch, and why not on this pin

`Builder::experimental_multiprocess_wal(true)` (present in pre.4) switches to
coordination via a memory-mapped `.db-tshm` sidecar: one **single-writer slot
across all processes** (writers serialize), a single checkpointer, bounded
reader slots pinning WAL frames, and a shared frame index. Per the official
doc (current upstream docs, newer than the pin — noted where they diverge):

- **Experimental**: "The on-disk coordination format and the public API may
  change between releases. Do not rely on the format for long-term storage
  across Turso versions."
- **Known race on this pin**: commit `50f3a217f` (2026-09-14, in 0.8.0 final
  2026-09-28, *not* in 0.8.0-pre.4 of 2026-08-11 nor 0.8.0-pre.11 of
  2026-09-11): in multiprocess mode, a reader on a fully-checkpointed WAL did
  not register in the shared reader table, so another process's checkpoint
  could copy newer frames under it — the reader observed **mixed old/new
  pages** (surfaced as FTS index corruption in the concurrent simulator).
  Any use of multiprocess WAL on the pinned version inherits this.
- **Mixed-mode guard exists in core but not on the bindings path**: the
  both-directions rejection ("already open with/without experimental
  multiprocess WAL in another process") lives in
  `resolve_default_storage`, which runs only when the caller supplies no
  pre-opened storage. The `turso` crate's `Builder` (and sdk-kit beneath it)
  opens the file itself and passes storage in, skipping those probes —
  probe-confirmed on this pin, and the same call shape remains in 0.8.1
  source. Concretely: a legacy single-process opener and an experimental
  multiprocess opener of the same file are *not* prevented from coexisting
  through the Rust bindings, whatever the docs say.
- **MVCC exclusion**: `turso_core` rejects opening an MVCC database with
  multiprocess WAL ("MVCC does not support multiprocess access",
  `database.rs:1936`; landed 2026-07-13, so it *is* in pre.4).
- **Platform/filesystem gates**: 64-bit Unix only (Windows/WASM/32-bit accept
  the flag as a no-op); the filesystem reject list includes NFS, CIFS,
  CephFS, GFS2, Lustre, OCFS2, AFS, CODA, NCP, and 9P (v9fs) — meaning a
  database file living on such a filesystem is rejected *for multiprocess-WAL
  coordination in the inspected mode*. It says nothing about other possible
  9P integration shapes; the Kopacetic 9P direction (client, server, or OS
  mount) is still an open selection. `LIMBO_DISABLE_FILE_LOCK` in the
  environment silently removes locking entirely (a footgun to name in any
  deployment doc).
- **Mixed SQLite+Turso processes are unsupported** (`COMPAT.md`).
- **Cross-process writes serialize on the writer slot regardless** — many
  per-shell processes publishing on every state change would queue on one
  lock; correct but a contention point the design shouldn't need.

Version reality check: current stable is 0.8.3; the reader-race fix ships
from 0.8.0 final onward, while the bindings-path mixed-mode gap remains by
source shape at least through 0.8.1. Moving the pin would remove the race but
keep the "experimental format" caveat, the MVCC exclusion, and the
single-writer serialization.

## Short-lived writers: open → put → drop across processes

Topology correction from the human: writers should be **short-lived** — each
publication opens the store, puts its entity, and drops it — not a long-held
per-proxy handle. Re-assessed against source and probe (phases 4–5):

**Verdict: supported on this pin, in default mode, without the experimental
flag.** The exclusive lock becomes the serialization mechanism: each
publication is one complete store lifetime, overlapping publications fail
fast with `LockingError`, and a bounded retry acquires as soon as the current
owner drops. Distinct entities never share a transaction because no two
stores coexist.

Source chain (all pinned 0.8.0-pre.4):

- **Drop releases the lock while the process lives.** The ownership chain is
  `TursoStore` → `turso::Connection` (`Arc<TursoConnection>`) → core
  `Connection` (holds `db: Arc<Database>`) → `db_file` → the sole
  `UnixFile` fd, whose close releases the process's `fcntl` record lock.
  Nothing else retains it: `DATABASE_MANAGER` stores `Ready(Weak<Database>)`
  only (expired weaks are treated as absent and the slot reused), Kopacetic
  drops the `turso::Database` wrapper right after `connect()`, and the
  default-features build uses `PlatformIO` (plain fds; no io_uring
  registrations). Registry keying uses stat-based `file_id` on Unix — no
  incidental second fd.
- **POSIX footguns, named.** fcntl locks are per-process (an in-process
  second open cannot fail — and the registry dedups anyway), and closing
  *any* fd on a file releases *all* of the process's locks on it. The
  inspected path holds exactly one fd per `Database`, so the footgun is not
  live; code that ever opens the DB file a second time in-process would
  silently strip its own lock on close.
- **Drop does not checkpoint.** `Drop for Connection` rolls back active
  transactions, releases WAL read/write locks, and decrements the connection
  counter — but `checkpoint_shutdown` runs only via explicit
  `Connection::close()` on the last connection. Committed frames persist in
  the on-disk `-wal` and every short-lived opener reads them back
  (cross-process visibility held across all 13 phase-4/5 publications).
  WAL growth between checkpoints is a monitoring concern, not a correctness
  one; note Kopacetic's `TursoStore` has no `close()` today, so drop is the
  only path actually exercised.
- **Failure behavior is immediate, not busy-waiting.** `open_file` uses
  non-blocking `F_SETLK`; the `LockingError` arrives instantly. Retry policy
  (backoff, deadline) is the producer's choice — the probe used 20 ms
  polling and saw 15 rejections in a 323 ms hold, then immediate
  acquisition on release.

Caveats: this is strict mutual exclusion at publication granularity — right
for per-shell latest-state puts, not a throughput path; same-key concurrent
writers serialize the same way but need their own last-writer-wins policy;
the `Opening` registry sentinel could stick if an open panicked mid-flight
(theoretical, not observed across the probe's opens). The qualification
workload for this topology is tracked in Kopacetic as
`kop-collections-turso-writers`.

## Scoped handoff (proportionate to write-first / Caminus-first)

The [consumer contract](file:///home/rektide/src/kopacetic/doc/consumer/term-preserver.md)
already states the right shape: many independent writers, one entity each,
no writer ever prunes another's identity. Proportionate recommendations:

1. **Primary: Caminus for the shared collection.** The Turso verdict above is
   exactly the backend admitting the wrong kind of contention for that shape:
   per-shell processes must not open one shared SQL file. Kopacetic-caminus
   is identity-addressed, field-per-file trees — structurally, each
   term-preserver process owns *its own entity's files*, so "many processes,
   disjoint writes, one collection" is the native shape with no coordination
   lock at all. One correction from the companion trace: **the live write
   chain is not atomic today** — `FilesystemDeser::write` →
   `serializer.finalize` → `std::fs::write` or `compio_fs::write`
   truncates in place, and the rooted atomic helpers are present but
   unwired ([caminus-collections](caminus-collections.glm53fx.md)). So the
   disjoint-ownership argument stands while *atomic publication at any
   granularity remains an open capability*, which is exactly what that
   wave's handoff covers. (Shared entity/API shape stays with the parent.)
2. **Turso stays good for owned single-process use — and now also for
   short-lived multi-process publication.** One long-lived owner keeps the
   preferred typed-path backend unchanged; and per the section above, the
   corrected open→put→drop topology is supported on this pin without any
   new service, with the exclusive lock as the serializer and bounded
   `LockingError` retry as the producer-side admission policy. No re-pin is
   required for either use; if the pin ever moves for other reasons, prefer
   ≥0.8.0 final for the reader-race fix alone.
3. **Do not adopt `experimental_multiprocess_wal` for the shell collection**,
   on this pin or merely after a re-pin: it is experimental-format territory,
   serializes writers cross-process anyway, excludes MVCC, and its
   mixed-mode guard does not actually fire through the Rust bindings — one
   stray default-mode opener (a debugging CLI, an older binary) would
   silently coexist with multiprocess openers rather than fail fast.
4. **Shared Turso later is a deliberate design, not a forbidden one.** The
   chosen Caminus-first path needs no new service, and short-lived writers
   already give multi-process Turso publication without one. If a
   *long-lived shared* Turso store is wanted later, the only Turso-faithful
   shape on this driver is single-writer ownership with the other processes
   handing writes to the owner — an owner/write-forwarding arrangement that
   should be designed deliberately in Kopacetic (the human welcomes that
   handoff; the capability ticket `kop-collections-turso-writers` tracks it)
   rather than emerging implicitly from whoever happens to hold the fcntl
   lock.
5. **For the contract's qualification workload**, run it against Caminus when
   that wave lands; against Turso it can only reproduce phase 1 of the probe
   (second open rejected) — which the scratch probe already demonstrates in
   miniature.

## Provenance

- Pinned driver source read from the local cargo registry:
  `turso-0.8.0-pre.4` (`src/lib.rs` Builder), `turso_core-0.8.0-pre.4`
  (`database.rs`, `io/unix.rs`, `io/common.rs`, `storage/wal.rs`, `build.rs`,
  `multiprocess_tests.rs`), and `turso_sdk_kit-0.8.0-pre.4` (`src/rsapi.rs`
  open path); `turso_core`/`turso_sdk_kit` 0.8.1 consulted for the
  mixed-mode-probe call-shape comparison.
- Two-OS-process probe run against the pin 2026-10-14 (phases 1–3) and
  re-run after the short-lived topology correction (phases 4–5: two
  long-lived processes driving short-lived stores over a stdin/stdout line
  protocol); code and README in gitignored scratch at
  `.test-agent/turso-multiprocess-probe/` (this repository); no Kopacetic
  code modified, only consumed as path dependencies.
- Archived upstream: `~/archive/tursodatabase/turso` @ `852e22cf8`
  (2026-10-06): CHANGELOG entries mapped to release dates via git history
  (`50f3a217f` 2026-09-14; MVCC exclusion `7b86d505b` 2026-07-13); COMPAT.md;
  README `.tshm` description. Archived `tursodatabase/turso-docs`:
  `sql-reference/multiprocess-access.mdx` (read in full; current-main docs,
  newer than the pin — divergences noted inline).
- Kopacetic source read at `~/src/kopacetic` (Cargo.lock pin confirmed;
  `kopacetic-turso` open path; `TursoStore` clone semantics) and the parent's
  consumer contract `doc/consumer/term-preserver.md` (read for alignment;
  its shared key/write contract owns the collection-level law).
- crates.io API for publish dates: 0.8.0-pre.4 2026-08-11, 0.8.0-pre.11
  2026-09-11, 0.8.1 2026-09-29, max stable 0.8.3.
- Every load-bearing claim above was read in source or produced by the
  probe; the only forward-looking statements are the explicitly marked
  upstream-doc descriptions newer than the pin.
