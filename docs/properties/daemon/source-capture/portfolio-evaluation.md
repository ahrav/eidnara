# Source capture: portfolio evaluation

A fresh-context evaluator that had not seen the authoring read `METHOD.md`,
this directory, the specification, and the ticket, then evaluated the
portfolio against the code and tests through the four METHOD lenses:
harness fit, coverage balance, implementability, and wildcard. It read 16
cited test bodies and found each asserts what its record claims. This file
records every finding and its disposition. The specification's own
evaluation of the baseline catalog is in
[#1066](https://github.com/ahrav/eidnara/issues/1066), comment 2.

## Mechanical checks

Records, index rows, and evidence files are the same 43 slugs. Every record
carries the 12 schema fields in order, every evidence file carries the six
required headings, and every relative link resolves. Every snake_case test
or function name the catalog files cite exists in `crates/`.

## Findings and dispositions

| Id | Class | Record or file | Finding | Disposition |
| --- | --- | --- | --- | --- |
| E1 | gap | coverage witnesses | The eight witnesses were names in comments and could not fire. | Applied: each is a constant marker in `crates/eval-core/src/markers.rs`; its test asserts the preconditions, records the marker, and checks `Coverage::complete` for its own path. `tool_output_contains_sentinel` now asserts the sentinel's placement before recording. |
| E2 | gap | R1 production reach | No record asserts that every text block, tool output, and tool error of a consented window reaches `publish_screened`. | Queued: a `reachability` record with a `sometimes` marker per class, representation, and harness at the transform-route seam. |
| E3 | gap | R14 budget, C3, C4 | The 5 s budget, the release of slot and charges on physical completion, and the shutdown drain are exercised by side tests that no record's check names. | Queued: one lifecycle record over `a_spent_budget_before_the_writer_commits_nothing` and `shutdown_drains_an_in_flight_batch_and_a_panicking_job_releases_its_slot`. |
| E4 | gap | stop condition "a second durable store" | `spec-integration.md` maps it to a record whose check never names durable stores. | Queued: a record with an executable check that the capture modules open no file or store write path. |
| E5 | refinement | `sc-sensitive-denied-remote-hidden-from-auto-surfaces` | The capture-path AutoInject read cannot tell `Sensitive` hiding from decisionless hiding. | Applied to `Exercised`. |
| E6 | refinement | `sc-capture-creates-no-decision` | The check gave a rationale and no assertable condition. | Applied: the check names the decision count, the admission row fields, and the evidence provenance and sensitivity. |
| E7 | refinement | `sc-tool-call-line-carries-no-outcome` | Labelled `default-production` while its sibling digest records are `explicit-config-only` over the same formatter. | Applied: `explicit-config-only`, with the `eidnara_folds` condition and the unverified boundary-trigger path. |
| E8 | refinement | `sc-readmission-blocked-after-purge-tombstone` | `Exercised: yes` while F27 and the finite-class purge are untested. | Applied: `partial`. |
| E9 | refinement | `sc-consent-gate-admits-capture`, `sc-pending-unit-publishes-next-ready-pass`, `sc-unit-cap-refuses-whole-never-prefix` | `Exercised: yes` while a named required fault is unconstructed. | Applied: `partial`, naming the missing fault. |
| E10 | refinement | `sc-harness-classes-have-no-production-publisher` | The existing-check text called the `manifest.rs` comment stale after it was corrected. | Applied. |
| E11 | refinement | `sc-harness-classes-have-no-production-publisher` | The successor named only the consent bound. | Applied: the reach obligation points at the E2 gap. |
| E12 | refinement | `spec-integration.md` | Two rows listed witness names as tests. | Applied: the rows name the owning tests. |
| E13 | refinement | `catalog.md` | The semantics line counted invalidated records without saying so. | Applied. |
| E14 | gap | AC15 process kill | `sc-delete-retry-recovers-crash-between-stores` covers the in-process F23 cut only. | Queued: a crash-consistency test that kills the process between the Kernel commit and the memory-store delete and retries in a new process. |
| E15 | gap | F5 and F29 | Both need a hook inside publication or the job; `before_publish` runs before the preview. | Queued: one hook seam, the top item of the leverage ranking in `fault-map.md`. |
| E16 | refinement | `sc-curator-capture-requires-finite-live-deadline` | The replay test depends on a 200 ms deadline and a 300 ms wall-clock sleep. | Applied to `Fault/timing angle`; the clock seam stays an open question for `/testing:test-strategy`. |
| E17 | bias | `sc-delete-retires-before-memory-store-delete` | Every `session.delete` fails closed while the Kernel is starting, including sessions with no captures, and no caller retries. | For a human: whether capture-off deployments should carry this cost (OQ12). |
| E18 | bias | `existing-checks.md` | The ticket says the inventory is "audited from `unaudited`", while METHOD routes adequacy verdicts to `/testing:invariant-test-review` and `/low-level-systems:defensive-assertions-and-invariant-guards`. | For a human: every entry stays `unaudited`, per METHOD, until those reviews run. |
| E19 | bias | `sc-tool-call-line-carries-no-outcome` | The slug says the line carries no outcome, and the line carries status and excerpts since U6. | For a human: the slug stays stable because the U6 ticket names it as the one shape both sites share; rename or invalidation is an owner choice. |
| E20 | bias | `sc-capture-resource-bounds` | One tool output above about 16 MiB refuses its whole window on every pass, while R10 says siblings still publish. | For a human: whether R10's wording or the window bound changes. |

## Documentation verification findings carried here

The fresh verification of `docs/source-capture.md` found that a
secret-bearing unit, a pending unlink, and a purge tombstone each reject the
whole capture batch during batch ingest, with no named unit, so the memo
records nothing and the unit is offered again on every pass. The authoring
group for `sc-unprovenanced-text-stores-sensitive-or-refuses-secrets` found
the same by reading `ingest_exact_artifacts_with`. The document and
`sc-readmission-blocked-after-purge-tombstone` state this behavior; a test
of a secret-bearing unit inside a screened batch is listed under "None
found" in [`existing-checks.md`](existing-checks.md).

## Semantics distribution

| Dimension | Count |
| --- | --- |
| Check `always` | 39, of which 3 are liveness records with stated bounds (one pass, `ceil(N / 64)` calls, one retry) |
| Check `always(!X)` | 4 |
| Type | 40 safety, 3 liveness |
| Reachability | 17 default-production, 26 explicit-config-only |
| Status | 41 active, 2 invalidated |

## Biases for a human

E17 to E20 above, plus the specification's own: the integration seams
capture turns on (shared Kernel writer concurrency, the request-path receipt
wait, the shutdown drain) rest on Part B records and the C1 to C4
constraints rather than on existing-behavior records.
