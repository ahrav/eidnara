# policy-digest-pins-revision-one

## Discovery trigger

Plan R4 and the stop condition: "no change to `evaluate_admission` or its
tables; `POLICY_REVISION` stays 1". The plan's verification contract names
the pin test at `admission.rs:5820` as the done signal. Cataloged so the
guarantee the plan leans on is checkable rather than assumed.

## Evidence trail

- `crates/kernel/src/admission.rs:20` `pub const POLICY_REVISION: i64 = 1;`
- `:21-23` `#[cfg(test)] const REVISION_1_SOURCE_DIGEST` =
  `097ed31de1a3f3a49b83de0cedf83e7a1b9850f1cfce8d33aa4fa777d7ca0d17`.
- Marker pairs in the source: `evaluator` (`:263`, `:389`), `serving`
  (`:438`/`:661`, `:2798`/`:3038`, `:3376`/`:3960`), `authority` (`:663`,
  `:907`), `chain` (`:2594`, `:2700`), `tables` (`:4066`, `:4299`).
- `:5734-5821` `policy_revision_matches_policy_content`: reads
  `include_str!("admission.rs")`, normalizes line endings, joins the
  `ALL` vocabularies of `Maturity`, `SourceClass`, `TaintClass`,
  `Disposition`, `EventKind`, `VisibilityRow`, `Surface`, `Sensitivity`, and
  `Outcome`, appends the `evaluator`, `tables`, `authority`, and `chain`
  sections and every `serving` section, hashes with SHA-256, and asserts
  `POLICY_REVISION == 1` and the digest equality (`:5819-5820`).
- `:5688-5698` `digest_sections` collects every `serving` section;
  `:5703-5731` `the_served_fold_is_under_the_serving_digest` asserts the
  served-row fold sits inside a marked section.
- Every stored admission row carries `policy_revision` (`:1748`), and
  `approval_row_fields`, `non_restrictive_row_fields` require
  `policy_revision = {POLICY_REVISION}` (`:853`, `:835`), so a revision bump
  invalidates stored approvals as authority.
- CI runs `cargo +1.98 nextest run --profile ci --workspace --all-targets
  --all-features --locked` (plan verification contract;
  `.github/workflows/ci.yml` is the source of truth per `AGENTS.md`).

## Failure scenario

An edit inside a marked section (a new `automatic_ceiling` arm, a changed
`approval_row_fields` predicate, a new `source_allows_taint` pair) lands
without the pin failing, so stored rows written under the old policy are
reinterpreted under new rules while still labeled revision 1. Or the pin
fails and the implementer updates the digest constant without bumping
`POLICY_REVISION`, which has the same effect.

## Timing windows and dependencies

- Build-time only; the test is `#[cfg(test)]`.
- The pin covers marked sections only. `derive_classes` in the daemon and
  `names_project` in `eligibility.rs` are outside it. Policy-relevant code
  outside the markers (for example `prepare_with_facts`'s approval back-fill
  rule at `:1569-1580`) is not digested.

## What a test must construct

1. The existing test, run in CI: passes at HEAD.
2. Mutation evidence: edit one character inside each marked section in a
   scratch copy and confirm the test fails for each; this is situation
   coverage that the pin reaches every section.
3. A grep gate (optional): no `policy-digest:` marker appears outside
   `admission.rs`, and each `-start` has a matching `-end`.

## Investigation log

### Q: Is `prepare_with_facts` (trigger fall-through, approval back-fill) inside the digest?

- Sources examined: marker line numbers above; `prepare_with_facts`
  (`:1553-1668`); `validate_trigger` (`:2726-2795`).
- Findings: both sit between `:907` (`authority-end`) and `:2594`
  (`chain-start`), outside every marked section. The plan's R4 ("evaluator
  and tables unchanged") is pinned; the request-preparation rules that decide
  which event the evaluator sees are not.
- Missing evidence: none at HEAD.
- Conclusion: resolved with answer: the pin does not cover request
  preparation. Recorded here; whether to extend the markers is a design
  decision for the repository owner (needs human input).
