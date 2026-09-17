# Required-phase portfolio evaluation

Discovery seeks properties; evaluation seeks flaws in the set. This pass was
run at RP2.8.U2 (PR #668, head `e6ed3878`) by an evaluator that had not taken
part in the discovery: it was given `../../METHOD.md`, `catalog.md`,
`existing-checks.md`, `fault-map.md`, `crates/daemon/src/packing.rs`,
`crates/retrieval/src/packing/`, both `packing_required.rs` test files, and
`crates/retrieval/AGENTS.md`, and did not open `evidence/`. It ran
`cargo test --locked -p daemon --test packing_required` (12 tests, pass),
`cargo test --locked -p retrieval --test packing_required` (4, pass),
`cargo test --locked -p daemon --lib packing::` (4, pass),
`cargo test --locked -p daemon --doc packing` (2 `compile_fail`, pass), and
`grep -rn 'prepare_required\|packing::' crates --include=*.rs`, which confirmed
`test-only` for every record. The disposition is ours.

Four lenses were applied: harness fit, coverage balance, implementability, and
a wildcard pass that questioned the framing itself.

## Disposition summary

| Category | Count | Status |
| --- | --- | --- |
| refinement | 5 | applied to the catalog and existing-checks |
| gap | 4 | queued; two need a human decision first |
| bias | 3 | require human judgment, listed below |

## Refinements applied

1. **`packing-required-phase-precedes-optional-work` claimed request order at
   the daemon layer across stages.** `prepare_required` reads every row, then
   judges and admits, then loads and verifies, then reserves, so a `Missing`
   on request 2 is returned before a `Stale` on request 1. Only the pure layer
   (`admit_required`) is strictly request-ordered. The Guarantee now says
   "within its stage" and names the stage order; Confidence notes that no
   daemon test crosses stages with faults on two requests.
2. **Same record: `optional_events() == 0` cannot fail at this base.**
   `PackingTrace` has no public way to push `StageEvent::Optional`; the only
   push is the private `required()` (`crates/daemon/src/packing.rs:155`).
   Exercised is now `partial`, with the structural clause named.
3. **Same record: the retrieval-call oracle detects a self-reporting packer
   only.** `note_retrieval_call` is called by the test fixture alone; a packer
   that called `lexical::retrieve` directly would leave the counter at 1. The
   Check now says what the counter is an oracle for and names the
   `crates/retrieval/AGENTS.md` rule as the guard against the other case.
4. **Same record: the "another length" fault is refused by the digest, not
   the SQL guard.** The test rewrites `payloads.byte_length` with the bytes,
   so the row's `PayloadRef` follows the rewrite and `fetch_payload`'s
   predicate passes; `PayloadRef::verify` refuses. The fault entry now says
   so and points at the retrieval test that drives the SQL predicate,
   `payload_fetch_is_length_guarded_in_sql_and_verified_by_digest_afterwards`.
5. **`existing-checks.md` omitted `materialized_bytes_are_never_printed`.**
   Added as `unaudited`; no record here owns the logging rule it asserts.

## Gaps queued

1. **No record that every required request yields exactly one materialized
   item or a refusal.** `prepare_required` zips `requests`, `selected`, and
   `report.occurrences` (`crates/daemon/src/packing.rs:326`), and
   `crates/retrieval/src/eligibility.rs:234` guards the verdict count with a
   `debug_assert_eq` only, so a short kernel batch in release would drop
   trailing required items silently, which is the Impact records 2 and 3 name.
   Only `required_cost_at_the_limit_succeeds_and_one_above_fails_without_truncation`
   asserts two items for two requests, incidentally. A one-to-one record is
   due when U3 gives the phase a consumer.
2. **Duplicate identities in a required set are undefined.** `read_selected`
   documents that a repeated identity yields a repeated entry
   (`crates/retrieval/src/packing/mod.rs:152`); `admit_required` and
   `reserve_required` neither dedupe nor refuse, so one occurrence is loaded
   and charged twice. Needs a human decision on whether a required set is a
   set before a record can say which behavior is correct.
3. **No liveness record for phase termination.** All three records are
   safety. A budget with no deadline holds through `with_conn`
   (`crates/daemon/src/packing.rs:240`), and the tested cancellation uses a
   30 s deadline (`crates/daemon/tests/packing_required.rs:764`). The catalog
   states the ceiling; no property bounds the phase in time. Queued behind
   the production caller, which decides whether a deadline-free budget can
   reach the phase at all (at this base it cannot).
4. **The production estimator's byte-to-cost mapping is unchecked.**
   `TokenizerEstimator::cost` runs `String::from_utf8_lossy`
   (`crates/daemon/src/packing.rs:103`) before counting, so invalid UTF-8 is
   replaced first; record 3 charges through the injected `ByteEstimator` and
   the 64 KiB test uses ASCII. A record on the production profile belongs
   with the route that selects it (U5a).
   Superseded at U4a: the estimator is gone. The lossy conversion is
   `render::required_fragment` (`crates/daemon/src/packing/render.rs`), the
   exact profile counts through `tokenizer::estimate_tokens`, and
   `../accounting/` owns the profile's records; the route that selects a
   profile is still U5a.

## Biases for a human

1. **`high` confidence on three records over a function with no production
   caller.** The Impact lines presuppose a route that does not exist at this
   base; the confidence is in the harness, not the system, and U5a can
   invalidate both the reachability class and the trace-based oracles.
2. **Zero is an accepted budget.** `from_budget(Some(0.0))` is `Ok(0)`
   (`crates/daemon/src/packing.rs:419`) and `reserve_required` then refuses
   any non-empty set as `OverBudget`. Whether an empty budget is a valid
   request or a malformed one is a design call the record makes silently.
3. **Nine markers in `fault-map.md`, fired by construction only.** The
   semantics distribution is `always` 3, `sometimes` 0, `reachable` 0;
   `../marker-ledger.md` attributes each marker to a test that constructs
   its precondition, and no test names a marker literal. That is the
   ledger's stated convention, but it means marker coverage is a reading of
   the test source rather than an observed firing. Whether the U-series
   tests should emit the literals is a scoping decision.
