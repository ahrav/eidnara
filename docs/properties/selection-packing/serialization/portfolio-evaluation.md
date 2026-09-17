# Serialization portfolio evaluation

Discovery seeks properties; evaluation seeks flaws in the set. This pass was
run at RP2.8.U4b (PR #674, head `546cacad`, then verified against the merged
head `1eff586e`) by an evaluator that had not taken
part in the discovery: it was given `../../METHOD.md`, `catalog.md`,
`existing-checks.md`, `fault-map.md`, the `packing.serialization.` rows of
`../marker-ledger.md`, `../README.md`, `crates/daemon/src/packing/`
(`serialize.rs`, `mod.rs`, `render.rs`, `accounting.rs`),
`crates/daemon/src/projection_gates.rs`, `crates/daemon/src/dispatch.rs`,
`crates/daemon/tests/packing_serialize.rs`,
`crates/daemon/tests/support/packing.rs`, and CC9 of
`../../search-projection/construction-contracts.md`, and did not open
`evidence/`. It ran `cargo test --locked -p daemon --test packing_serialize`
(12 pass, 1 ignored; see the base note below),
`cargo test --locked -p daemon --doc packing` (1 doctest and 3 `compile_fail`,
pass), and
`grep -rn 'finalize(\|prepare_optional(\|PackingLimits::from_manifest' crates --include=*.rs`,
which finds `finalize` and `prepare_optional` defined in
`crates/daemon/src/packing/` and called only from `crates/daemon/tests/`, and
`PackingLimits::from_manifest` called from `packing_serialize.rs` only; every
other hit is a hasher's `.finalize()`. `grep -rn 'packing::' crates/daemon/src`
finds `pub mod packing` (`lib.rs:44`) and an unrelated `retrieval::packing`
import. `test-only` holds for records 1, 2, and 4. The disposition is ours.

Base note. The working tree moved from `546cacad` to `fcc0401d` while this
pass ran; that commit adds a post-write budget poll to `finalize`, a
`guard_calls::on_write` seam, one test, one marker row, and one fault-map row.
The test run above was against that tree. At `546cacad` the test file declares
12 `#[test]` functions, one ignored, so the expected count there is 11 pass,
1 ignored. The disposition landed after the merge of `rp28/u4a-accounting`
(`3e32dec9`), so every line reference below was re-verified against the merged
head `1eff586e`, whose `prepare_optional` is the accounting branch's; where
`fcc0401d` already closes a finding, the finding says so.

Four lenses were applied: harness fit, coverage balance, implementability, and
a wildcard pass that questioned the framing itself.

## Disposition summary

| Category | Count | Status |
| --- | --- | --- |
| refinement | 8 | applied to the catalog, existing-checks, fault-map, and marker-ledger |
| gap | 5 | queued; one closed by `fcc0401d`, one needs a human decision |
| bias | 5 | require human judgment, listed below |

## Refinements applied

1. **`packing-body-serialized-once-through-the-guard` claims a reservation
   no test observes.** The Guarantee says the body is "reserved to exactly
   the measured length" (`catalog.md`, the record's Guarantee). The
   reservation is
   `Vec::with_capacity(measured.len())` at
   `crates/daemon/src/packing/serialize.rs:235`; the test asserts
   `body().len() == rendered` (`crates/daemon/tests/packing_serialize.rs:136`)
   and never reads capacity, so `Vec::new()` would pass. What the check does
   prove is that the guard's write verified the measured length
   (`crates/daemon/src/dispatch.rs:362-394`, `LengthMismatch`). The Guarantee
   now says "written exactly once through `MeasuredOutput::write_to`, which
   verifies the measured length" and drops "reserved to exactly".
2. **Same record: the transport arm is unreachable under manifest bounds.**
   `PackingLimits::from_manifest` refuses `packing_rendered_bytes` past
   `MAX_WIRE_BODY_BYTES` (`serialize.rs:44-49`), `prepare_optional` refuses a
   closed render past `max_rendered_bytes`
   (`crates/daemon/src/packing/mod.rs:746`), and the body is the ledger text
   (`serialize.rs:228`), which adjustment only shortens. So
   `SerializationBound::Transport` (`serialize.rs:231`) cannot be returned
   from `PackingLimits`-derived bounds; only a hand-built `AccountingBounds`
   past the cap reaches it. The Guarantee, the Existing check line, the
   fault-map row (`fault-map.md:13`), and the ledger row now say the marker
   fires at the guard only and that this arm is `always-or-unreached` from the
   packing path, not merely "no fixture constructs" it.
3. **Same record: the counter oracle's scope.** `guard_calls` counts every
   `measure` and `write_to` on the thread (`dispatch.rs:249`, `:364`), so
   `(1, 1)` proves the packer made one guard measurement and one guard write,
   not that the returned body is the bytes that write produced. The body
   equality with the ledger text is what closes that, and only jointly. The
   Check line now says what the counters are an oracle for. Also, the
   reachability evidence "`grep -rn 'finalize('` finds the packing module and
   its tests" was not what that command prints: it also matches about seventy
   hasher `.finalize()` calls. The line (`catalog.md:84`) now cites the
   `packing::` import search and the test-only call sites; records 2 and 4
   inherit it by "as above".
4. **`packing-adjustment-removes-last-admitted-within-the-cap` overstates
   ledger equality.** "The rebuilt ledger equals the ledger an admission of
   the remaining groups would have closed" held only when no group was
   skipped. `CostedGroup.index` keeps the partition position and
   `render::group_fragments` labels entries `GroupOpen(index)` with it
   (`crates/daemon/src/packing/render.rs:255`), so
   a rebuild after a skip keeps a gap in its labels while a fresh admission of
   the remaining set re-indexes from zero. The text is unaffected because the
   open fragment renders `first_fused`, not the index (`render.rs:235`). Both
   twin comparisons use a fixture with no skipped group
   (`packing_serialize.rs:170`, `:759`). The Guarantee (`catalog.md:149`) is
   narrowed to body equality, with ledger equality when nothing was skipped
   and the label difference named otherwise.
5. **`packing-limits-fail-closed-at-manifest-parse` carries one reachability
   class over two.** The parse refusals (`PackingUnapproved`, `MissingLimit`,
   `UnknownLimit`, `NonNumericLimit`;
   `crates/daemon/src/projection_gates.rs:368-407`) run in production through
   `crates/daemon/src/projection_admission.rs:97` when a manifest carries the
   group, so `explicit-config-only` is right for them. The conversion refusals
   `Zero`, `OutOfRange`, and `Absent` (`serialize.rs:36-49`, `:57-86`) and the
   four bound mappings have no caller outside `packing_serialize.rs`, so they
   are `test-only` at this base. The version-disagreement refusal
   (`projection_gates.rs:346`) precedes the packing branch and is inherited
   manifest behaviour, not a packing property; and the source tripwire
   (`packing_serialize.rs:711`) is a static name check. The Reachability line
   now gives the class per clause, and the Guarantee marks the version clause
   as inherited.
6. **`packing-output-byte-identical-across-cache-states` is exercised under
   the byte profile only.** `identity_hex` and `refusal_at_token_edge` both
   go through `admit`, which fixes `byte_profile()`
   (`packing_serialize.rs:318-330`, `crates/daemon/tests/support/packing.rs:36`,
   `str::len` with zero headroom). The exact tokenizer, the profile whose
   counts the cache actually saves and whose `delta` depends on the anchored
   tail (`render.rs:130-144`), runs across cache states nowhere. Exercised now
   reads `partial`, naming the byte profile's sweep and the exact tokenizer as
   not varied.
7. **Same record: "concurrent misses" are not constructed.** The four
   threads spawn (`packing_serialize.rs:359`) after sequential calls that
   refill the cache following `rotate()` (`:355`), so every thread hits. The
   miss race the cache documents (`crates/daemon/src/token_cache.rs:176`)
   needs `clear()` immediately before the scope. The Fault/timing angle
   (`catalog.md:264`) now says concurrent reads that hit, and the fault-map
   row (`fault-map.md:15`) no longer lists the thread scope as a miss seam.
8. **`existing-checks.md` names the wrong location for the guard tests.**
   Row one cited "`crates/daemon/src/dispatch.rs` guard tests"
   (`existing-checks.md:9`); `dispatch.rs` has `#[cfg(test)]` helpers and no
   `#[test]`. The row now names `crates/daemon/tests/prepared_output.rs`:
   `cached_bytes_copy_only_after_destination_reservation`,
   `cap_plus_one_and_arithmetic_overflow_fail_before_write`,
   `inconsistent_source_reports_length_mismatch_without_emission`, and
   `destination_failure_retains_no_partial_terminal`, and a cross-reference
   row for `crates/daemon/src/token_cache.rs`
   `a_count_cached_under_one_revision_is_not_served_under_another`, which the
   accounting part owns and record 4 leans on.

## Gaps queued

1. **A budget that ends inside the fitting write returned the preparation.**
   At `546cacad` `finalize` polled the budget once per pass and returned `Ok`
   after the write and the hash with no second poll, while `prepare_optional`
   polls after its close (`mod.rs:731`). Record 2's "refuses with `Deadline`
   before any measurement" was true and the Fault/timing angle "none" was
   not. Closed by `fcc0401d`, which adds the poll (`serialize.rs:286`), the
   `on_write` seam, the test
   `a_budget_that_ends_while_the_body_is_written_refuses_the_preparation`,
   and the `packing.serialization.budget_ended_during_write` marker; the
   record's Fault/timing angle (`catalog.md:176`) names the window.
2. **`PackingLimits.deadline` has no consumer.** It is parsed and converted
   (`serialize.rs:26`, `:84`), tested for its value, and read by nothing;
   `finalize` takes the caller's `EvalBudget` and never calls `bounded_by`
   (`crates/kernel/src/applicability/checkout.rs:188`). No record says the
   packing deadline bounds the phases. Queue behind U5a, which owns the
   caller that would apply it.
3. **An approval flag with no packing group is accepted silently.**
   `projection_gates.rs:407` refuses `packing.is_some() && !approved` only; a
   manifest that keeps `search_projection.packing.approved: true` after its
   group is removed parses, and `PackingLimits::from_manifest` yields
   `Absent`. Q4 rules the group needs the flag, not that the flag needs the
   group. Needs a human decision before a record can say which is correct.
4. **`PackingFailure::LengthMismatch` and `Write` have no test and no
   reach.** `serialize` maps them (`serialize.rs:237-243`), but an `Exact`
   source written into a `Vec` cannot short-write or fail. They are
   `always-or-unreached` at this base; either a record says so or the
   variants wait for a destination that can fail.
5. **Adjustment after a skipped group is unexercised.** Every fixture admits
   all three groups; no scan skips a middle group, so "last-admitted" and
   "highest partition index" coincide trivially. `prepare_optional` builds
   `admitted` in index order (`mod.rs:747-753`), so the property should hold
   with a gap in the indexes; a fixture that skips group 1 and admits group 2,
   then adjusts, would show it and would exercise the label difference in
   refinement 4.

## Biases for a human

1. **`high` confidence on three records over functions with no production
   caller.** As in the required-phase evaluation: the Impact lines presuppose
   a route U5a has not landed, so the confidence is in the harness. U5a can
   change the reachability class, the counter oracle (a caller that measures
   on another thread defeats a thread-local counter), and the profile.
2. **Every record is `always`.** The distribution is `always` 4, all others
   0. The transport arm (refinement 2) and the two failure variants (gap 4)
   are `always-or-unreached`, a semantics the catalog never uses. The set
   reads as if every arm were exercised when two are dead at this base.
3. **A zero pass cap is an accepted approved limit.** `size()` admits
   `packing_adjustment_passes: 0` (`serialize.rs:83`) and the zero sweep
   asserts it (`packing_serialize.rs:586-601`). Under it every serialized-bytes
   overflow is a hard refusal with no repair. Whether that is a valid
   configuration or a malformation is a design call the record makes
   silently.
4. **The identity binds the body only.** `PreparationIdentity` is
   `Sha256::digest(&body)` (`serialize.rs:282`) with no domain separation and
   nothing from the profile revision or the bounds. Two profiles that admit
   the same body share an identity. Whether the apply-time digest must also
   bind what produced the body belongs to the U5a design.
5. **Global cache seams in a parallel test binary.** `cost_cache::clear` and
   `rotate` act on the process-wide cache (`token_cache.rs:97`, `:220-231`)
   while the other tests in `packing_serialize.rs` run in parallel, and
   `test_cache_guard` is crate-private (`token_cache.rs:238`). Harmless
   today because every count is a pure function, so a cleared entry only
   recomputes; an assertion on hit counts anywhere in this binary would
   flake. Whether the seams should take the guard is a scoping decision.
