# u5-rejection-and-unknown-accounting-is-lossless

## Discovery trigger

Specification sections 'Echo provenance and use authority' and acceptance U4
and U5; the delivery implementation ticket assigns this slug.

## Evidence trail

`crates/daemon/tests/claim_sources.rs` - `final_use_is_judged_per_surface_from_current_canonical_policy`, `a_purged_representation_splits_row_verdicts_and_both_accounting_sets_keep_the_object`.

## Failure scenario

Merged counts hide how many rejections were also Unknown, or hide that a
permitted object also had a denied representation.

## Timing windows and dependencies

An artifact purge landing between selection and handoff: each representation
of a claim is its own artifact, so the purge denies one row while the other
stays permitted.

## What a test must construct

A rejected claim with Unknown lineage and a permitted claim with known lineage
in one batch; a claim whose rationale artifact is purged after its rows were
selected, validated again at the later snapshot.

## Investigation log

### Q: Can an object appear in both the permitted and the rejected set?

- Sources examined: `retrieval::claims::validate_for_surface`, which fills the
  sets per row; `kernel::cas::read::egress_facts_tx`, which denies a
  purge-tombstoned digest at any destination; `daemon::claim_sources`, which
  publishes each representation as a separate artifact.
- Findings: yes. The verdict is per row and the artifact gate is per digest, so
  a purge of one representation's artifact splits the object's rows. The
  daemon test constructs the purge and asserts both sets hold the object.
- Missing evidence: none.
- Conclusion: the overlap is the documented contract; presentation authority is
  each row's `UseVerdict`, never a set.
