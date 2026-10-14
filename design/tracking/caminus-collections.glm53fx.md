---
type: Assessment
title: "Kopacetic EntityIdentity and kopacetic-caminus collections: multiprocess shared-root assessment"
description: "Source-verified assessment of kopacetic-core EntityIdentity and the kopacetic-caminus CaminusStore collection: the LIVE write call chain (std::fs::write legacy layout, rooted v1 mutation unwired), different-identity vs same-identity multiprocess qualification on a shared root, enumeration/discovery gaps, identity collision and path handling, existing tests; distinguishes implemented capability from missing concept/API and proposes narrowly scoped Kopacetic handoff work. Context: term-preserver write-side persistence of per-shell state (Caminus prime interest, 9p future, Turso useful). Rev 3: live-chain correction, multiprocess qualification, handoff rescoping."
resource: /home/rektide/src/term-preserver/design/tracking/caminus-collections.glm53fx.md
status: draft
author: model:zai-coding-plan/glm-5.3-flash#max (subagent, petname nimble-panda)
date: 2026-10-14
sources:
  - resource: "opencode://ses_ec843ae4bffeC3iIfhQaSKCCM9"
    title: "FX assessment of Kopacetic EntityIdentity and kopacetic-caminus collections under shared-root multiprocess writes; rev 3: traced live write chain (legacy std::fs layout, rooted v1 unwired), qualified multiprocess independence, rescoped handoff (equality law, write-first, process-pair tests)"
  - resource: "/home/rektide/src/kopacetic (5090da3, 2026-10-12)"
    title: "kopacetic-core domain/mod.rs; kopacetic-caminus src/lib.rs, tests/store.rs, README; kopacetic-turso storage/{schema,connection,transaction}.rs"
  - resource: "/home/rektide/src/facet-caminus (9671917, 2026-09-22)"
    title: "facet-caminus src/lib.rs (FilesystemDeser::write), serializer.rs, path/rooted/linux/mutation.rs, manifest.rs"
---

# Kopacetic Caminus collections — assessment

Verified revisions: **kopacetic `5090da3`** (2026-10-12, "Connect the handoff to
its exploration and consumer portfolio"), **facet-caminus `9671917`**
(2026-09-22). Line numbers are from those checkouts.

Use-case lens: term-preserver persisting each shell's state — many small
write-side objects under one shared root, one writer per shell, later
possibly on 9p; Turso useful as the indexed alternative.

## 1. EntityIdentity (kopacetic-core/src/domain/mod.rs)

```rust
pub struct EntityIdentity {          // mod.rs:5-10
    pub api_version: Option<String>,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
}
```

**Implemented:** `Clone, Debug, Eq, Facet, PartialEq` (mod.rs:4); standalone
constructor (mod.rs:13-21); Kubernetes adaptation fills identity + keeps
uid/resource_version in `KubernetesProvenance` only (mod.rs:62-88). Identity
is a plain 4-field value struct.

**Missing concept/API (the "pervading primary key" worry, realized):**

- No `Hash`, no `Ord` — the identity cannot key a `HashMap`/`BTreeMap` or be
  sorted without a wrapper (mod.rs:4 derives).
- No canonical string form (`Display`/`to_key()`), no validation.
- Nothing in core *owns an encoding*. Two encodings already exist and
  **diverge on `None` handling**: the Caminus codec preserves `None` vs
  `Some("")` (kopacetic-caminus lib.rs:200-208 option tag), while Turso
  collapses both to `""` (kopacetic-turso connection.rs:22-24
  `unwrap_or("")`). Same logical identity tuple, different cross-backend key
  semantics — backend-neutral identity is the parent's integrated-design
  concern; this is the concrete evidence it rests on.

## 2. CaminusStore: implemented collection behavior

**Deterministic injective identity → path** (kopacetic-caminus lib.rs:82-89,
191-224): length-prefixed fields + option tags, chunked at 60 bytes →
lowercase-hex components (≤ 120 chars) under `root/v1/…/data`. Injective by
construction (tagged length-prefixing then lossless hex); hostile strings
(`../`, `/`, leading `.`) never become path components; `None` vs
`Some("")` preserved. Verified by test `identities_are_isolated_and_encoded_
as_safe_bounded_components` (tests/store.rs:69-137, incl. none-vs-empty
assert at :100-111 and path-component assertions at :86-98).

**Operations:** `put`/`put_value` (reconcile-or-create; fresh paths work —
serializer creates directories on demand, facet-caminus serializer.rs:136,140;
test store.rs:147), `get`/`get_value` returning `None` for absent trees
(lib.rs:126-143), `delete` with empty-prefix pruning whose error handling is
race-benign by design — `DirectoryNotEmpty`/`NotFound` stop the walk
(lib.rs:163-173). A symlink (or any non-directory) at the entity `data` path
is rejected before put/get/delete (lib.rs:230-237; test store.rs:153-170,
asserting the symlink target survives).

**Different identities, shared root, multiple processes:** disjoint
addresses, source-inferred independence — **no multiprocess test exists**, so
this is inference from the codec, not a demonstrated guarantee. Each identity
owns a disjoint `data` tree; writers of different identities never write the
same file. Residual shared couplings to qualify:

- *Shared prefix directories:* identities whose encodings agree on leading
  60-byte chunks share `v1/<chunk…>` parents. Concurrent `create_dir_all`
  during put (serializer.rs:141–146) versus prefix pruning during another
  identity's delete (kopacetic-caminus lib.rs:157–177) can interleave; the
  prune itself stops benignly on `DirectoryNotEmpty`/`NotFound`, but a
  racing rmdir between a writer's directory creation and file write surfaces
  as a spurious `ENOENT` on that write.
- *VCS roots:* default `put`/`get` never commit (CaminusStore sets no
  CommitMode, lib.rs:46–48, 179–188). Commits are reachable only via the
  `entity()` escape hatch (lib.rs:96–105); `vcs::resolve` walks **up** from
  the entity tree preferring `.jj` then `.git` (facet-caminus vcs.rs:22,
  34–38), so commits can land in an enclosing shared repo and interleave
  with other entities' commits.
- *Manifests are per-entity* (`<entity data>/.caminus/manifest`,
  facet-caminus manifest.rs:103–152), not shared across identities; they
  carry the size+mtime caveat for out-of-band edits (manifest.rs:3–15).
- *Optional features:* the compio worker is in-process; notify/watchman
  watchers observe the shared tree read-only. Neither adds cross-process
  exclusion.

**Same identity, multiple processes: no atomicity at any granularity on the
live path.** The traced call chain (kopacetic-caminus lib.rs:116–123 →
facet-caminus lib.rs:106–137 → serializer.rs):

```text
CaminusStore::put_value            kopacetic-caminus lib.rs:116-123
  └─ FilesystemDeser::write        facet-caminus lib.rs:106-137
       └─ FilesystemSerializer     serializer.rs:68-74 (ambient PathBuf plan)
            └─ serialize_root      → FsInstruction queue (instruction.rs)
            └─ finalize            serializer.rs:111-167
                 ├─ diff_against_disk (ambient reads; manifest optional)
                 ├─ sync:   std::fs::create_dir_all + std::fs::write
                 │          serializer.rs:141-147
                 ├─ compio: compio_fs::write   compio_worker.rs:90-105
                 └─ remove: std::fs::remove_file/_dir, errors ignored
                            serializer.rs:150-158
```

- `std::fs::write` opens with truncate and writes in place: **not even a
  single file publishes atomically**. A concurrent reader can observe an
  empty or partial file mid-write; two writers of the same identity
  interleave per instruction, and a reader mid-reconcile can parse a mix or
  fail outright.
- The rooted temp+`renameat` machinery (path/rooted/linux/mutation.rs:68–96
  `replace_file`, :105–133 create-once `RENAME_NOREPLACE`) **exists but is
  not in this chain**. facet-caminus README:274–280: public `FilesystemDeser`
  APIs "currently use the legacy physical layout"; format-v1 is "not wired
  into serialization, parsing, watching, replay, or format selection", with
  "separate rooted-filesystem work required before v1 can be enabled safely".
  Matches the Kopacetic-side gate at design/path/README.md:59. My rev-2
  claim that per-file writes atomic-rename was wrong for the live path —
  the citation described an unwired helper.
- kopacetic-caminus's own README agrees in substance ("A value is
  decomposed into a plan and reconciled in place… no generation directory,
  fsync protocol, atomic root swap, reader snapshot, or recovery marker").
- Contrast: kopacetic-turso publishes the whole entity in one transaction —
  upsert identity, `DELETE` + re-`INSERT` of nodes (transaction.rs:26–54)
  under `UNIQUE (api_version, kind, namespace, name)` (schema.rs:9).
- Manifest cache caveat for out-of-band edits: unchanged-detection skips
  hashing when size+mtime match (manifest.rs:3–15). Single-writer-oriented;
  another process's same-size/same-mtime rewrite is invisible to the cache.

**Write-side cost per persist:** reconcile walks the entity (O(fields) reads)
per `put`; the per-entity manifest makes unchanged files O(1)-check
(facet-caminus lib.rs:63–71 doc). For per-shell-state persistence at exit
this is fine; frequent small updates want `with_manifest()`.

## 3. Enumeration / discovery: absent by explicit decision

**Implemented:** nothing. No `list`, no `decode_path`, no scan by
kind/namespace; a caller must already hold an `EntityIdentity`
(kopacetic-caminus README, Limitations → "Collection discovery is absent":
"Walking hexadecimal directories directly would couple application code to
layout v1 and is not a supported discovery API").

Missing **concept**, not just code: today `CaminusStore` is a per-identity
isolated-store facade, not a collection object — there is no way to answer
"what entities exist?". Structural notes for any future enumeration:
variable path depth (chunks of 60 encoded bytes → 1..N components) under
`v1/`, so discovery is a recursive walk; the encoding is reversible in
principle (README: "Identity addressing is reversible"), so `decode_path`
is feasible, but `delete`'s prefix pruning and the `v1` coupling mean the
layout must become a stated, versioned API first.

## 4. Identity collisions and path handling — summary

- Identity→path: collision-free (injective codec above); path-hostile
  identity strings tested (store.rs:72-99).
- No dedup/uniqueness story beyond the codec: nothing stops two
  *conceptions* of one entity (e.g. same k8s resource re-registered under a
  different `api_version` — those are distinct identities by design; the
  k8s `uid` deliberately lives in provenance, not identity, mod.rs:78-85).
- Payload-internal keys are a separate, documented gap: reflected map keys
  map directly to path components (case, `/`, `.`, length issues) — README
  "Payload keys are not encoded yet". Not a collection-identity problem,
  but it bounds what payloads are safe to persist.

## 5. Existing tests (kopacetic-caminus/tests/store.rs)

| Test | Proves |
| --- | --- |
| `entity_round_trips_and_replacement_prunes_stale_fields` (:30-66) | put/get roundtrip incl. provenance; replacement prunes stale field paths |
| `identities_are_isolated_and_encoded_as_safe_bounded_components` (:69-137) | hostile identities isolated; component bounds/hex; `None` vs `Some("")` distinct; delete of one preserves the other |
| `absent_and_deleted_values_return_none` (:139-151) | get/delete semantics on absent trees; fresh-path put works |
| `entity_root_symlink_is_rejected` (:153-170, unix) | non-directory at entity path rejected for put/get/delete; no following |

**Not covered anywhere:** concurrent behavior (no thread or two-process test
in the crate; note the crate's tests are single-process — a thread test
would not establish cross-process semantics either), enumeration (no API to
test), `api_version` distinctness, `EntityIdentity` has no unit tests in
kopacetic-core (domain mod.rs has none; lib.rs is re-exports only).

## 6. Implemented vs missing — one-line ledger

- **Implemented:** injective safe identity codec; put/get/delete with
  reconciliation and pruning; disjoint per-identity address derivation
  (independence source-inferred, untested under concurrency); legacy-layout
  write execution via `std::fs::write`/`compio_fs::write`; symlink-at-root
  rejection; prune stops benignly on contention; solid roundtrip/isolation/
  hostile-path tests (single-process only).
- **Missing concept/API:** atomic publication at any granularity on the
  live path (rooted v1 mutation exists but is unwired — README:274–280);
  entity enumeration/discovery; a core-owned identity **equality law** with
  a portable codec where address derivation must agree across backends; a
  stated same-identity concurrency contract; process-pair tests pinning it.

## 7. Proposed narrowly scoped handoff work for Kopacetic

Cam scope only; integrated design and backend-neutrality stay with the
parent, Turso multiprocess with G. The requirements landing spot is the
native consumer handoff at
`/home/rektide/src/kopacetic/doc/consumer/term-preserver.md`
(proposed-not-implemented); the items below are grounded purely in the
source evidence of §§1–6.

The write-first requirement is **independent different-key writers**: many
processes, each owning distinct identities, persisting under one shared
root without coordination. That works today at the address level (§2) and
needs qualification tests more than new machinery.

1. **Define the identity equality law in core.** Specify what makes two
   `EntityIdentity` values the same entity — including whether `None` and
   `Some("")` are equal — plus `Hash`/`Ord` derives so collections can key
   on it. Provide a portable codec **only where backends must derive
   addresses or keys compatibly**; identical physical encodings and a
   `Display` form are *not* required, and no migration of existing Caminus
   trees or Turso rows is demanded. The current `None`-collapsing Turso
   mapping (connection.rs:22–24) versus None-preserving Caminus codec
   (§1) is precisely the kind of question the law must answer.
2. **State the same-identity contract for the live path.** Document what a
   concurrent reader/writer of one identity may observe under
   `std::fs::write` reconcile (torn files and mixed-field states are
   currently observable). If a guard is added, note that an advisory
   writer lock alone only serializes writers — readers still see
   mid-reconcile state unless they take the same lock or publication
   becomes snapshot-atomic; atomicity for readers is a separate, larger
   piece (rooted v1 wiring) and is not demanded here.
3. **Qualify different-key independence with process-pair tests.** Spawned
   *processes* (not threads — a thread test exercises one process's
   memory, not cross-process semantics): different identities writing in
   parallel under one root (expect clean interleave, no spurious failures
   beyond the §2 ENOENT window), and the shared-prefix create/prune race,
   to pin current behavior as a regression boundary.
4. **Enumeration — later.** Per write-first steering, `identities()` over a
   versioned layout is deferred until the write side lands; recorded in
   the README's discovery-absent decision until then.

Priority for the shell-state use case: 1 and 3 unblock safe multi-shell
persistence now; 2 makes same-key behavior honest; 4 follows.
