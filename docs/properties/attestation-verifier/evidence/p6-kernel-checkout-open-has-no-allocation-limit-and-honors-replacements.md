# p6-kernel-checkout-open-has-no-allocation-limit-and-honors-replacements

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface S4. Existing behavior at the branch tip; its existing checks are `unaudited`.

## Evidence trail

`open_isolated` (`crates/kernel/src/applicability/checkout.rs:639`) opens with `gix::open::Options::isolated()` and strips filter, diff, and merge command sections; it sets no allocation limit and leaves replacement refs to the repository's configuration; under gix 0.87.1 `refs/replace` loads only when the repository sets `core.useReplaceRefs = false`, so a replacement entry alone substitutes nothing. `isolated_options` (`crates/daemon/src/git_sources.rs:131`) overrides `gitoxide.objects.allocLimit` after the repository's own configuration, and `open_with_slots` (`:148`) sets `objects.ignore_replacements`. The verifier opens with `git_sources::open_with_slots` (`:148`) and, after a store lookup exhausts the index slots, with `git_sources::open` (`:138`).

## Failure scenario

A verifier routed through the Kernel opener materializes a delta chain above the budget.

## Timing windows and dependencies

None.

## What a test must construct

A delta-packed blob over an oversized base, read through each opener.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the branch that adds this part.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Which opener does the U1 driver use?

- Sources examined: `lookup` (`crates/daemon/src/attestation_verifier.rs:532`) and `replacement_refs_are_ignored` (`crates/daemon/tests/attestation_verifier.rs:505`).
- Findings: `git_sources::open_with_slots` with `MAX_BLOB_BYTES` as the limit and 32 index slots, reopening through `git_sources::open` when a store lookup needs more slots; its own-pack decodes and loose reads apply the same limit.
- Missing evidence: none.
- Conclusion: resolved with answer; this resolves the open question the portfolio evaluation tied to G3.
