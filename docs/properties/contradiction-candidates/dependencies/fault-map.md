# Fault map: P5 dependency surfaces

HEAD `3a3b916b7`. For each fault class: whether the repository can produce it
today, which records need it, and the cheapest valid oracle.

## Fault classes and availability

| Class | Available at HEAD | Seam |
| --- | --- | --- |
| F1 oversize input (bytes, atoms) | yes | construct a `&str`; pure function |
| F2 large same-project domain (> 8192 decisions) | yes, slow | commit decisions in a fixture (`kernel_routes.rs:1958` does this for the daemon cap) |
| F3 scope shapes: no term, non-exact operator, redacted term, redacted filter, NULL scope | yes | `insert_scope` with `ScopeTermSpec`; `SECRET` fixture for redaction (`kernel_scope.rs:235`) |
| F4 corrupt payload in another domain | yes | raw SQL `UPDATE decisions SET decision_payload=...` (`kernel_canonical_read.rs:448`) |
| F5 held reader pool | `test-support` only | `hold_readers_for_test` (`open.rs:614`) |
| F6 deadline crossing inside a statement | partial | `EvalBudget` with a near deadline plus a slow statement; no injection point found for `claim_facts` statements |
| F7 parent interrupt on a linked budget | yes | `EvalBudget::linked` then `parent.cancel()` |
| F8 busy publication slot | yes | hold `capture.slot.try_acquire_owned()` in the test (`source_capture.rs:1592`) |
| F9 panic inside the capture closure | `cfg(test)` hooks | `before_publish`, `between_screen_and_commit` (`source_capture.rs:783`, `:841`) |
| F10 kernel unavailable or starting during a pass | yes | `KernelOpenCoordinator` state in daemon tests (`source_capture_tests.rs:473`) |
| F11 capture budget exhausted | yes | advance past `CAPTURE_BUDGET` or cancel (`:1081`) |
| F12 baseline text change | yes | a frozen prior baseline fixture (`baseline.rs:486-489`) |
| F13 clock advanced past `retain_until` | yes | `expire_harness_captures(now)` takes `now` as an argument |
| F14 pin taken between selection and recheck | yes | in-module test `local_file.rs:797` |
| F15 foreign citation on capture evidence (decision event, decision, edge) | yes, unconstructed | `record_admission` with `evidence_id` on a live decision, or `insert_decision` with `evidence_id` |
| F16 refused capture inside a page | yes | `local_file.rs:895` |
| F17 project-root mismatch, unknown body field, NotModule authority | yes | `memory_reviewer_wire.rs:15`, `:238` |
| F18 store read failure during authority lookup | not found | no seam found for `memories_authority_for_route` failure |
| F19 observational (`cli`) route beside a participating route | yes | `lib.rs:35926` |
| F20 disallowed action, empty allowlist, context-researcher agent | yes | `tools.test.ts:1162`; agent string in `toolContext` |
| F21 toolchain Unicode or SQLite version change | out of tree | a different toolchain build |

## Per-record required faults

| Record | Required faults | Enabling state |
| --- | --- | --- |
| p5dep-analyzer-refuses-oversize-input-before-retention | F1 | bounds `max_input_bytes`, `max_atoms` at the test's choice |
| p5dep-analyzer-keeps-original-bytes-and-is-pure | none (F1 variants for content) | mixed-case, diacritic, NFC/NFD inputs |
| p5dep-analysis-epoch-and-identity-pin-the-contract | F21 | a build; the pinned literal |
| p5dep-domain-decision-read-matches-domain-and-kind-in-sql | F4 | two domains at one snapshot |
| p5dep-scope-prefilter-is-a-superset-of-the-scope-verdict | F3 | scopes of each shape with decisions |
| p5dep-daemon-bounds-rows-after-the-scope-filter | F2; large payloads for the byte cut; F4 variant (missing decision row) for `CorruptCanonicalRow` | a project with > 8192 rows |
| p5dep-kernel-domain-read-has-no-row-bound | F2 | > 8192 same-project `memory` decisions; an assertion that `visible_domain_decisions_as_of_in_scope` returns more than `MAX_READ_ROWS` rows before `read_visible` bounds them (`read.rs:191`) |
| p5dep-claim-fact-bounds-are-refused-before-any-row-is-read | over-bound id lists; exhausted budget | `controlled_acquisitions_for_test` counter |
| p5dep-budget-exhaustion-stops-a-budgeted-kernel-read | F5, F6, F7 | `test-support` build |
| p5dep-decision-page-returns-an-oversize-decision-alone | a payload larger than `max_payload_bytes` | decisions with known sizes |
| p5dep-response-never-waits-on-the-capture-job | F8, F10, F11 | `source_capture_enabled: true`, `memory_enabled: true`, non-subagent session |
| p5dep-capture-slot-is-single-and-released-on-every-exit | F8, F9, shutdown cancel | same |
| p5dep-capture-batch-is-bounded-per-batch | 33-unit window; > 64 KiB window; 64 KiB + 1 unit; pending documents | same, plus `inject_docs` config for the documents batch |
| p5dep-capture-job-ends-in-one-closed-vocabulary-count | F9, F10, F11, delete epoch moved, publish refusal | same |
| p5dep-baseline-digest-change-closes-the-activation-gate | F12 | a store at the prior baseline; an activation record naming the old digest |
| p5dep-harness-sweep-retires-owners-before-evidence | F13, F14, F16 | harness_capture rows (capture on, or direct `SourcePublisher` in a Kernel fixture) |
| p5dep-cited-evidence-survives-the-harness-sweep | F13, F15 | an expired capture whose evidence a live decision's event cites |
| p5dep-review-reads-parse-strict-bodies-after-the-envelope | F17, F18 | a bound route; a receipt |
| p5dep-observational-routes-read-and-take-no-background-part | F19 | two routes on one root |
| p5dep-memory-tool-actions-are-closed-and-allowlisted | F20 | fake transport that counts calls |

## Coverage checks to add

Each asserts a precondition, never the violation:

- `p5dep_cov_kernel_rows_exceed_daemon_cap`: `sometimes(visible.rows.len() >
  MAX_READ_ROWS)` observed before the heap (the reachability record itself).
- `p5dep_cov_scope_shape_each_kept`: `sometimes` for each of the five scope
  shapes appearing in one filtered read's input.
- `p5dep_cov_budget_crossed_inside_statement`: `sometimes(deadline passed
  while a budgeted statement was executing)` for `claim_facts`; without it the
  progress-handler branch is never exercised.
- `p5dep_cov_slot_busy_at_handoff`: `sometimes(RefusedQueueFull counted)`.
- `p5dep_cov_two_batches_in_one_job`: `sometimes(window batch and documents
  batch both published in one job)`.
- `p5dep_cov_sweep_page_full`: `sometimes(pending.len() > 64 at one sweep)`.
- `p5dep_cov_cited_capture_expired`: `sometimes(an expired harness capture
  with a live foreign citation was a sweep candidate)`.
- `p5dep_cov_observer_newest_on_root`: `sometimes(the newest binding on a root
  is observational while an older participating binding exists)`.

## Leverage ranking (cheapest valid oracle first)

1. S1 records: pure functions; property tests over generated strings with
   byte-substring and determinism oracles. No fault injection.
2. S8 record: fake transport; count calls. Already has a harness.
3. S2 superset record: one Kernel fixture with five scope shapes and the
   scope algebra as the oracle (`ProjectScope::names_project`).
4. S6 cited-evidence record: a Kernel fixture that publishes one descriptor,
   writes one decision event citing its evidence, advances `now`, sweeps, and
   reads `invalidated_commit_seq` on both rows. Cheap and currently
   unconstructed; highest information per cost in this set.
5. S3 claim-fact bounds: existing test pattern plus the acquisition counter.
6. S2 daemon bound and S2 reachability witness: existing over-cap fixture
   plus one probe.
7. S4 records: existing integration suite; add a one-terminal-per-job
   assertion across the exit matrix.
8. S3 budget exhaustion inside a statement: needs an injection point or a
   deliberately slow statement; most expensive.
9. S5: existing tests suffice; the deployment step is operator-side.
10. S7 `store_unavailable`: needs a seam not found at HEAD (F18).
