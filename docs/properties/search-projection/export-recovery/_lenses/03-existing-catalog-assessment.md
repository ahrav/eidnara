# Existing-catalog coverage assessment

Date: 2026-09-10. Repository: `/local/home/ahrav/scratch/eidnara`.
Verified HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
Evidence scope is supplied by the user; see [catalog.md](../catalog.md).

## Assessment status

The original fresh-context request returned `Subagent depth limit reached (1)`;
this file preserves the same-context pre-addition assessment from that stage.
Independent central review is now complete under
`ses_f7623dcccffe3Y09nW2wVoABif`, with all four lenses run. Its supplied findings
and this author's verified dispositions are in
[portfolio-evaluation.md](../portfolio-evaluation.md). This historical trace is
not relabeled independent, and disposition is not independent re-review.

## Harness fit

- **Refinement:** Reuse
  [canonical-read-staleness-is-distinguishable-from-emptiness](../../../daemon/transform/catalog.md#canonical-read-staleness-is-distinguishable-from-emptiness)
  for the exact existing withheld/empty distinction. Its real-kernel fixture
  registers and advances a consumer at
  `crates/daemon/tests/transform_canonical_memory.rs:230-302`. The RP2.1 delta
  is lifecycle orchestration, not another formatter property.
- **Refinement:** Reuse
  [current-profile-never-names-an-unvalidatable-generation](../../../host-runtime/catalog.md#current-profile-never-names-an-unvalidatable-generation)
  for generic filesystem selection validity. The search-specific delta must
  compare complete coverage and compatibility, which a valid payload digest
  does not establish. Its interruption fixture constructs stale temporary
  files (`crates/host-runtime/src/generation.rs:1899-1926`); it does not kill a
  process or simulate lost writes.

## Coverage balance

- **Gap:** No existing catalog contains a live fixed-S bounded kernel export
  property or complete consumer replay from published retained history.
  Registration and pruning tests provide narrower evidence, not these claims.
- **Gap:** No existing catalog owns RP2.1 deletion-after-pruning convergence.
  The invalidated
  [mirror-reset-cycle-requires-a-rebuild-grant](../../../memory-store/catalog.md#mirror-reset-cycle-requires-a-rebuild-grant)
  concerns a removed implementation. Keep it as historical context only.
- **Bias:** Existing catalogs are rich in safety and weak in bounded recovery
  evidence for this new surface. Do not fill the gap with a fabricated timeout;
  the corrected catalog retains RP2.9-blocked recovery and catch-up envelopes.

## Implementability

- **Refinement:** The host-runtime catalog's `test-only` rationale for the
  generic generation property is stale. The CLI calls `stage_and_promote` at
  `crates/daemon/src/bin/eidnara-host.rs:988` under a supplied-payload path.
  It is an established explicit-input path, not an RP2.1 search implementation.
  Do not copy that old reachability classification into new records.
- **Refinement:** The facade catalog's invalidated claim-effects entries still
  say the store-side mirror exists. At this HEAD,
  `crates/memory-store/src/claim_mirror.rs` is absent; the memory-store catalog
  explicitly records its removal. Do not harden or resurrect it.
- **Gap:** Search export/selector/supervisor and two-database timing hooks are
  absent. All proposed-only records need individual `test-only` rationales
  and `Exercised: not yet`, even where lower-level fixtures exist.

## Wildcard, last

- **Refinement:**
  [validation-and-enumeration-address-one-directory-object](../../../host-runtime/catalog.md#validation-and-enumeration-address-one-directory-object)
  already owns descriptor/path identity. Link it when sharing publication
  machinery; do not make a second low-level filesystem property here.
- **Gap:** The settled disable wording says deregister, but current kernel
  rejects that transition when pending. Removing the last consumer also
  forbids pruning rather than giving a free horizon. Preserve both facts as
  open integration decisions, with the exact no-consumer surface distinction.
- **Bias:** Names such as `search` in kernel fixtures are not production
  registration evidence. The catalog must not promote fixture names into
  implemented daemons or APIs.
