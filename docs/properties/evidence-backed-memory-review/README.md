# Evidence-backed memory review

Property catalog for the evidence-backed memory review specification
([#709](https://github.com/ahrav/eidnara/issues/709)): production canonical
resolution, private-result expiry and recovery, observer-neutral review routes,
exact review-response integers, the packaged review commands, and cumulative
provider response budgets.

## Source

The specification's parent decision record
([#579](https://github.com/ahrav/eidnara/issues/579)) names an original
property portfolio under `docs/properties/shared-memory_reviewer-investigation/`
that was never published to the repository, and no recovered inventory exists.
The repository owner accepted the specification's supplemental slugs as the
replacement obligations for the seven residual implementation tickets
(#725 through #731). The records here are those slugs, reconstructed from the
specification's constraints and acceptance criteria and from the code and tests
that implement them. Each record names the code it was verified against at
authoring time. Nothing here recovers the original portfolio's identities or
claims to reproduce its text.

## Layout

| File | Contents |
| --- | --- |
| `catalog.md` | The records that have an implementation surface in the tree, one `### <slug>` block each, plus the index of every slug the specification assigns |
| `evidence/<slug>.md` | One evidence file per authored record |
| `existing-checks.md` | Claim-bearing checks in the tree, all `unaudited` |
| `fault-map.md` | Fault classes, required faults per property, and coverage checks to add |
| `portfolio-evaluation.md` | The independent pre-merge reviews that stood in for a fresh-context evaluation, and how each finding was dispositioned |
| `spec-integration.md` | Slug to specification section to implementation ticket to code crosswalk |

Records enter this catalog with the implementation ticket that gives them a
code surface. Slugs assigned to a later ticket appear in the index with no
record until that ticket lands. Landed so far: #725 (canonical resolution), #726 (private result expiry),
#727 (broker-free recovery), #728 (observational routes), #729 (exact
integers at the client seam), #730 (the review command), and #731 (cumulative
response budgets).

## Landed by

| Ticket | Slugs |
| --- | --- |
| #725 | `production-classes-reach-policy-eligible-proposal`, `canonical-resolution-refuses-changed-owner-and-target` |
| #726 | `private-result-transfer-preserves-queue-expiry` |
| #727 | `receipt-selection-fences-private-generation-results`, `durable-private-result-recovers-without-model-refire`, `unknown-dispatch-does-not-authorize-resend`, `uncited-owner-lineage-remains-read-authority` |
| #728 | `observer-route-does-not-change-background-rosters` |
| #729 | `status-sanitizer-preserves-inclusive-integer-domain` |
| #730 | `completed-outcome-pages-have-live-keyset-semantics`, `shared-path-fixture-reaches-selected-readable-proposal`, `reference-only-cli-outcomes-preserve-meaning`, `review-cli-owns-one-replay-free-connection`, `review-cli-validates-byte-exact-inert-payloads`, `review-cli-preserves-shared-kernel-refusal-shapes`, `status-freshness-never-defaults-unknown-to-zero`, `status-counts-preserve-overlapping-ledger-populations` |
| #731 | `provider-response-budget-is-cumulative-per-job`, `attempt-ledger-preserves-cross-generation-ceilings`, `guarded-request-handoff-precedes-network-polling` |

Slugs the specification assigns to other units (history lineage, lifecycle and
retention, gate and status truth beyond the two status records above, semantic
evidence, evaluation evidence, and later application) are indexed in
`catalog.md` without a record. They belong to the owners the specification
names and are not obligations of these seven tickets.
