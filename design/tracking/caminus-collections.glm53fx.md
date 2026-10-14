---
type: Assessment
title: "Kopacetic EntityIdentity and kopacetic-caminus collections: multiprocess shared-root assessment"
description: "Source-verified assessment of kopacetic-core EntityIdentity and the kopacetic-caminus CaminusStore collection: different-identity vs same-identity multiprocess writes on a shared root, enumeration/discovery gaps, identity collision and path handling, existing tests; distinguishes implemented capability from missing concept/API and proposes narrowly scoped Kopacetic handoff work. Context: term-preserver write-side persistence of per-shell state (Caminus prime interest, 9p future, Turso useful)."
resource: /home/rektide/src/term-preserver/design/tracking/caminus-collections.glm53fx.md
status: draft
author: model:zai-coding-plan/glm-5.3-flash#max (subagent, petname nimble-panda)
date: 2026-10-14
sources:
  - resource: "opencode://ses_ec843ae4bffeC3iIfhQaSKCCM9"
    title: "FX assessment of Kopacetic EntityIdentity and kopacetic-caminus collections under shared-root multiprocess writes, with handoff scoping for term-preserver shell-state persistence"
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

**Different identities, shared root, multiple processes:** safe by
construction. Each identity owns a disjoint tree; writers of different
identities never touch the same files. The only shared mutation is the
parent-prefix removal in `delete`, and that path already treats concurrent
interference as benign (lib.rs:163-173). Isolation across identities under
one root is directly tested (store.rs:83-99, 128-136).

**Same identity, multiple processes:** per-*file* atomic, per-*entity* not.

- Each published file is staged then moved with a single `renameat`
  (facet-caminus mutation.rs:68-96 `replace_file`; create-once variant with
  `RENAME_NOREPLACE` at :105-133). No torn single file.
- But `FilesystemDeser::write` "reconcil[es] with whatever is already
  there" in place — the store "owns every non-dot entry" and prunes stale
  entries (facet-caminus lib.rs:106-125). There is no staging tree swap,
  generation directory, lock, or reader snapshot; kopacetic-caminus's own
  README says exactly this: "A value is decomposed into a plan and
  reconciled in place… no generation directory, fsync protocol, atomic root
  swap, reader snapshot, or recovery marker" (Limitations → "Publication is
  not atomic or crash-durable").
- Consequence: two processes writing the **same** identity interleave
  per-file; a concurrent reader can observe a mixed old/new entity. This is
  last-writer-per-*file*, not last-writer-per-entity. Contrast:
  kopacetic-turso publishes the whole entity in one transaction — upsert
  identity, `DELETE` + re-`INSERT` of nodes (transaction.rs:26-54) under
  `UNIQUE (api_version, kind, namespace, name)` (schema.rs:9). Turso's
  worry is multiprocess *connection* behavior; Caminus's is absent
  entity-level publication atomicity.
- Manifest cache caveat for out-of-band edits: `.caminus/manifest` skips
  hashing when size+mtime match; an edit preserving both is invisible
  (facet-caminus manifest.rs:3-15, "mtime-race caveat"). Nanosecond mtimes
  make the window small; documented upstream, worth restating for shared
  roots.

**Write-side cost per persist:** reconcile walks the entity (O(fields) reads)
per `put`; the per-entity manifest makes unchanged files O(1)-check
(facet-caminus lib.rs:63-71 doc). For per-shell-state persistence at exit
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

**Not covered anywhere:** concurrent/multiprocess behavior (no thread or
two-process test in the crate), enumeration (no API to test), `api_version`
distinctness, `EntityIdentity` has no unit tests in kopacetic-core (domain
mod.rs has none; lib.rs is re-exports only).

## 6. Implemented vs missing — one-line ledger

- **Implemented:** injective safe identity codec; put/get/delete with
  reconciliation and pruning; cross-identity isolation; per-file atomic
  publication; symlink-at-root rejection; race-benign prefix cleanup;
  solid roundtrip/isolation/hostile-path tests.
- **Missing concept/API:** entity enumeration/discovery; canonical core-
  owned identity key (single encoding, `Hash`/`Ord`/`Display`); entity-level
  publication boundary (lock/snapshot/staging) — or an explicit documented
  statement that per-file last-writer semantics are the contract;
  concurrency tests pinning whichever contract is chosen.

## 7. Proposed narrowly scoped handoff work for Kopacetic

Cam scope only; integrated design and backend-neutrality stay with the
parent, Turso multiprocess with G.

1. **Own the key in core.** One canonical, documented encoding of
   `EntityIdentity` in kopacetic-core (codec bytes + `Display` +
   `Hash`/`Ord` derives); derive the Caminus codec and Turso identity
   columns from it, closing the `None` vs `Some("")` divergence
   (§1). Small, test-only-visible, unblocks every map/collection use.
2. **Enumerate v1.** A minimal `CaminusStore::identities()` that walks
   `v1/` and decodes — promoted from "walking is not a supported API" to a
   versioned layout contract; a matching `decode_path` stays internal.
   This is the single missing collection primitive for shell-state
   listing ("which shells have state here?").
3. **State the same-identity contract.** Pick one, document it in the
   crate: (a) per-file last-writer reconciliation is the semantics, or
   (b) add a minimal entity-level guard (single advisory lock file at the
   entity prefix) before write. Either is small; today the behavior is
   real but unstated, and it is the one hazard for two processes touching
   one shell's state.
4. **Pin concurrency with tests.** Two-process (or two-thread) harness:
   different identities in parallel (expect clean interleave), same
   identity (assert the documented contract). Turns §2's analysis into a
   regression boundary.

Items 1-2 are the core of the "collections" gap; 3-4 make shared-root
multiprocess behavior explicit rather than accidental.
