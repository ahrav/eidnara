# typed-mutation-is-visible-and-copy-isolated

## Discovery trigger

KTD1 removes mark_modified and retained originals but keeps content/kind
accessors. Typed fields become the only serialization and equality authority.
The replay and concurrency lenses identify stale-output and alias failures.

System: `/local/home/ahrav/scratch/eidnara`, 2026-09-13.
HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
This record extends B1; identity/hash equivalence stays with the identity agent.

## Evidence trail

- `crates/memory-store/src/lib.rs:99-111` derives equality over typed fields
  and original. `:232-240` does the same for WireBlock.
- `crates/memory-store/src/lib.rs:145-160,266-278` makes original override
  typed fields at serialization time when present.
- `crates/memory-store/src/lib.rs:203-228` clears message original through
  content_mut or explicit marking, not through edits to public shell fields.
- `crates/memory-store/src/lib.rs:306-323` gives kind_mut the corresponding
  block side effect; public provider_extras edits need explicit marking.
- `crates/daemon/src/transform.rs:13722-13734` deliberately expects a
  latent meta.synthetic edit to serialize exactly like its old original.
- `crates/memory-store/src/lib.rs:16096-16112` expects a changed first
  block and preserved unknown sibling field under the old contract.
- `crates/daemon/src/wire.rs:1785-1792` exercises Arc::make_mut isolation
  and surviving projected block ownership.

Mutation APIs and shared projection owners are ordinary production surfaces.
The stale-edit behavior is source-confirmed and test-asserted, not rerun.
KTD1 accepts changing that behavior; it is not a new bug report against HEAD.

## Failure scenario

After removing marking calls, a hidden replay cache still emits pre-edit
content. Alternatively, a mutable accessor with no edit changes equality by
clearing retained history. A third failure mutates a shared cached shell while
the editing caller appears to hold only a copy.

An independently constructed expected typed object distinguishes these from
changes caused by default omission or canonical key order.

## Timing windows and dependencies

Hold at least two owners before Arc::make_mut. Edit role, origin, meta,
message extras, block extras, content structure, and a block kind separately.
The public-field cases must not call a removed invalidation method.
Read-only and no-op mutable accessor calls must not change state.

Compare unchanged siblings against their canonical typed pre-edit value,
not their pre-normalization unknown-field bytes. R3 accepts dropping those
fields. The identity agent owns all numeric equality versus hash caveats.

## What a test must construct

Decode a message with two distinct blocks, nonempty extras and origin, and
non-default meta. Build a separate typed expected object for each edit.
Assert immediate serialization of that edit and unchanged other owners.
Also compare decoded and constructor-built blocks with equal kind/extras,
and vary one typed field at a time as negative equality controls.

For messages, compare the full typed tuple including role, content, origin,
extras, and meta. For blocks, compare exactly kind and provider_extras.
Do not claim equal numeric Values imply identical serialized byte strings.
No tests run; contradictory golden assertions remain marked unaudited.

## Investigation log

### Q: Is stale replay merely a hypothetical public-field hazard?

- Sources examined: memory-store serialize paths and transform.rs:13714-13755.
- Findings: source chooses original, and the golden explicitly expects stale
  serialization for latent meta mutation.
- Missing evidence: none for the source behavior; no runtime reproduction runs.
- Conclusion: resolved with answer: this is the implemented old contract,
  intentionally superseded by accepted typed serialization.

### Q: Does existing sibling coverage span the whole public mutation surface?

- Sources examined: memory-store lib.rs:16096-16112, wire.rs:1785-1792,
  and the canonical-shell golden.
- Findings: they cover a block text edit, content clear, and a stale meta edit.
- Missing evidence: final independent matrix for all public fields, extras,
  no-op access, and decode-history-independent equality.
- Conclusion: unresolved, needs the focused mutation matrix.

## Named handoff

`/testing:test-strategy` owns the matrix and observation seam.
`/testing:invariant-test-review` owns the old stale-edit and sibling oracles.
The identity agent owns downstream equality/digest compatibility.

## Typed-wire U1 execution, 2026-09-13

Branch `perf/typed-wire-u1-owned-decode`, `cargo test -p daemon --locked
--features test-support` (1,489 tests pass; `lifecycle_cli` is platform-unsupported
on the aarch64 host). Edits through `content_mut`/`kind_mut` or
public fields serialize immediately; `mark_modified` is gone. Equality and the
identity digest cover `(kind, provider_extras)`. Witnesses:
`a_block_edit_leaves_its_sibling_unchanged_and_envelope_unknowns_are_discarded`,
`overlay_canonicalizes_only_the_mutated_block`, `one_edited_block_message`
(siblings equal after an edit), and the receipt-reuse test in `transform.rs`
where blocks differing only in discarded envelope fields are equal with equal
digests and the first equal candidate (index 1) is selected.
