# Fusion identity portfolio evaluation

Discovery seeks properties; evaluation seeks flaws in the set. This pass was
run at RP2.7.U1 by an evaluator that had not taken part in the discovery: it
was given `../../METHOD.md`, `catalog.md`, `existing-checks.md`,
`fault-map.md`, `crates/retrieval/src/fusion/`, and
`crates/retrieval/tests/identity.rs`, and did not open `evidence/`. It ran
`cargo test --locked -p retrieval --test identity` (11 tests, pass) and
`grep -rn 'fusion::' crates --include=*.rs` to check the reachability class.
The disposition is ours.

Four lenses were applied: harness fit, coverage balance, implementability, and
a wildcard pass that questioned the framing itself.

## Disposition summary

| Category | Count | Status |
| --- | --- | --- |
| refinement | 4 | applied to the catalog and existing-checks |
| gap | 1 | queued |
| bias | 3 | require human judgment, listed below |

## Refinements applied

1. **The preamble asserted reachability for every record at once.** The
   catalog's contract section stated that every record is `test-only` and no
   record carried its own evidence, which `METHOD.md` rule 4 forbids. Each
   record now names the functions under test, their sole caller, and the grep
   that found no other; the preamble describes only the observation point.
2. **`existing-checks.md` overstated the kernel prefix check.**
   `identity_matching_requires_the_complete_exact_prefix`
   (`crates/kernel/src/source_identity.rs:548-609`) flips bytes only in
   `0..prefix_len` (`:594-601`) and accepts every slice at or past the prefix
   (`:588-593`), so revision, representation, and span damage is outside it.
   The row now says so, and its limitation names the suffix.
3. **`fusion-occurrence-identity-never-collapses-payload` credited a refusal
   its Exercised list did not name.** The Check required an encoding-version
   refusal, which
   `a_lane_refuses_foreign_scores_non_finite_scores_and_other_encoding_versions`
   (`crates/retrieval/tests/identity.rs:185-212`) asserts together with the
   foreign-lane and non-finite score refusals; the test is now listed and the
   Check names all three refusals.
4. **The catalog ended without a relationship map.** One is added: the
   occurrence identifier is upstream of both digests and the parent key
   (`crates/retrieval/src/fusion/identity.rs:266-273`, `:176-190`), the
   preparation digest hashes the selection digest (`:277-299`), and the
   arithmetic records will sit downstream of the first record.

## Gaps queued

1. **`identities_add_no_authority_beyond_byte_equality_with_the_route_binding`
   (`crates/retrieval/tests/identity.rs:513-555`) has no record.** It asserts
   that no fusion identifier string names a project scope unless it is
   byte-equal to the bound project id. The subject is the route binding
   (`ProjectScope::names_project`), so the record belongs to the query-route
   part that RP2.7.U3 lands, with this test as its existing check. Queued
   there; not added here because this part owns identity derivation, not
   authorization.

## Biases for a human

1. **Every record is `always` over a pure function.** The semantics
   distribution is `always` 3, `always-or-unreached` 0, `sometimes` 0,
   `reachable` 0, `unreachable` 0. That is right for value types, but it means
   the part has no coverage check and no liveness record, and the Impact lines
   (a stale preparation applied, a retry applying an unselected edit) describe
   route behaviour no record here can observe. Whether those impacts get
   `sometimes` records in the U3 and U4 parts is a scoping decision.
2. **`fusion-parent-groups-are-not-voters` bundles an executable clause with a
   deferred one.** The identity clauses run today; the fusion clause has no
   subject until RP2.7.U2. Keeping one slug preserves traceability to the
   specification's acceptance row at the cost of a record that stays `partial`
   through one ticket. Splitting it would trade that for a second slug the
   specification does not name.
3. **One seeded generator behind the property tests.** `runner()`
   (`crates/retrieval/tests/identity.rs:23-31`) fixes a ChaCha seed and 512
   cases, so the permutation, duplication, and component-split checks are
   deterministic and reproducible but never explore beyond one fixed sample per
   revision of the test. Raising the case count or adding an unseeded run in a
   separate job is a cost decision.
