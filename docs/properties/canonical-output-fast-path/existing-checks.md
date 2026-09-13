# Existing checks for canonical-output fast-path properties

## Inventory boundary

This inventory covers the selected canonicalizer, served construction/cache,
and prepared-output preservation surfaces. It reuses [B1 and T1–T4][relations]
rather than reinventorying unrelated transport or storage suites. All checks
below have status **unaudited**. Source inspection is not test exercise or an
adequacy verdict. No test, benchmark, mutation, build, or CI gate ran here.

All numbered links use source verified through `git show HEAD:<path>` at
`2e4433e6b511ae74944df8a9669c428e73915d29`. The settled plan's baseline is
`4980f8af3bb90d58b19b80a38a227fb6363a8b33`. Older catalog execution reports
and drifted source anchors are not imported as current evidence.

## Canonicalizer and guards

| Existing check | Assertion or guard, message, and observation limit | Status |
| --- | --- | --- |
| [Formatter stack and field guards][formatter] | `expect` messages: `serde closes an open object`, `serde keys belong to an object`, `serde values belong to an object`, `key began`. They require local stack/field presence, not complete span geometry. | unaudited |
| [Sorter guards][sort] | Fewer than two fields returns before decoding. Unescaped sortedness avoids sorting; decoded keys require `serde emits valid string keys`. No changed flag exists at HEAD. | unaudited |
| [canonical_encoding_visits_each_serialize_implementation_once][once] | Shared nested counter equals 2 and output equals a literal. No custom assertion message. It does not independently count root visits or failing prefixes. | unaudited |
| [canonical_encoding_preserves_nested_scalars_and_decoded_key_order][scalars] | Output equals Value-round-trip bytes and differs from direct struct encoding for each scalar/container fixture. No custom assertion message. Value ordering is feature-conditional. | unaudited |
| [canonical_encoding_orders_prefix_and_escaped_keys_like_decoded_strings][keys] | Value and reversed-map inputs equal expected bytes; direct reversed bytes differ. Assertion context: `{case}`. No changed-flag or stable equal-key assertion. | unaudited |
| [Constructor serialization expectation][constructor] | `CK wire message values must always serialize`; block fallback uses `CK wire blocks must always have a JSON representation`. These do not model a production failing generic serializer. | unaudited |

None found: changed-permutation flag checks, all-six-permutation checks,
stable duplicate-key probes, explicit aggregate-flag checks, direct unchanged
table-to-A comparisons, or error-before-finalization probes. None found:
complete span-geometry guards. This absence does not require adding a general
validator; real-table identity tests and compact punctuation reasoning target S3.

## Served ownership and cache

| Existing check | Assertion or guard, message, and observation limit | Status |
| --- | --- | --- |
| [served_canonical_shell_bytes_and_segments_are_frozen][shells] | Original, latent-edit, typed, and edited shells equal literal canonical bytes, SHA-256 and identity text. Actual served-segment writes equal the literal frame and measured length. No custom assertion message. B1 owns these guarantees. | unaudited |
| [served_canonical_frozen_corpus_matches_value_reference_for_both_shells][corpus] | Retained/typed shells equal the Value reference and prepared bytes; block identity digest equality agrees with structural equality. No custom assertion message. The reference assumes sorted Value maps and unique keys. | unaudited |
| [served_fingerprint_fallback_preserves_complete_identity_and_first_match][receipts] | A separate structural reference checks positional/fallback receipt reuse and first match. Signed-zero cases distinguish structural equality from serialized spelling. No custom assertion message. Not a whole-message hash oracle. | unaudited |
| [served_serialization_has_no_value_round_trip_and_fallback_reuses_receipt_helper][source-test] | Lexically excludes `serde_json::to_value` from construction; requires one lazy identity-index initialization and one shared receipt-helper call; excludes alternate lookup/receipt expressions. No custom assertion message. Not a runtime traversal counter. | unaudited |
| [Cached/fresh differential][differential] | Rebuilds fresh output in tests and asserts equal canonical slices with `serialized output cache drift`. This reference work must be outside a hit-path call-count interval. | unaudited |
| [serialized_output_cache_reuses_steady_state_and_matches_fresh_bytes][cache-test] | Token-estimator calls stay unchanged; replay reports 0 serialized and 4 reused items and equals fresh bytes. No custom assertion message. No independent constructor/encoder spy. | unaudited |
| [serialized_output_cache_tag_overlay_invalidates_only_its_message][overlay] | 1 serialized and 3 reused items; output equals fresh tagged output. No custom assertion message. | unaudited |
| [serialized_output_cache_drop_invalidates_only_the_target][drop] | 1 serialized and 3 reused items; output equals fresh dropped output. No custom assertion message. | unaudited |
| [serialized_output_cache_fold_refreshes_prefix_and_reuses_tail][fold] | 2 serialized and 2 reused items; output equals fresh folded output. No custom assertion message. | unaudited |
| [serialized_output_cache_deep_charge_counts_message_metadata_and_none_rows][retained] | Charges exceed byte-only estimates and omitted-row-free estimates, with a 5% manual-fixture tolerance. Messages include `served:None row must carry a charge` and `serialized-output estimate left 5% fixture tolerance: retained={retained} expected={expected}`. Not a constructor peak bound. | unaudited |
| [serialized_output_cache_revert_epoch_bump_evicts_session][epoch] | Matching epoch has entries; changed epoch has none and default stats. No custom assertion message. | unaudited |

None found: independent positive-hit constructor/encoder observation or direct
Arc-identity assertions for every S6 artifact in this scoped inventory. None
found: a combined C2 campaign marker distinguishing canonical misses, typed and
edited disorder, positive hits in both callers, and completed prepared replay.
[Live-tail lookup][tail-lookup] increments hits for omitted `Some(None)` as
well as positive values; [synthetic lookup][synthetic-lookup] flattens omissions
into misses. Existing counters cannot prove a warm positive tail item.

## Prepared output and settlement

| Existing check | Assertion or guard, message, and observation limit | Status |
| --- | --- | --- |
| [json_measurement_matches_small_and_facade_sized_bytes][json-test] | Measured lengths and actual destination bytes equal the serde reference for small and 900 KiB payload fixtures. No custom assertion message. | unaudited |
| [transform_segments_preserve_existing_golden_bytes][segments-test] | Two exact segments produce literal envelope bytes and matching measured length. No custom assertion message. Actual Served segments are checked by the B1 shell test above. | unaudited |
| [cached_bytes_copy_only_after_destination_reservation][reserve-test] | Measurement writes nothing; the fixture marks reservation before writes and compares exact bytes. Writer guard: `copy occurred before reservation`. This public fixture does not observe host admission. | unaudited |
| [exactly_at_wire_cap_succeeds_without_destination_allocation][cap-test] | Exact source measures at the cap; a counting sink accepts the same total. No custom assertion message. | unaudited |
| [cap_plus_one_and_arithmetic_overflow_fail_before_write][overflow-test] | Inconsistent segment lengths yield exact `BodyTooLarge { len, max }` and `LengthOverflow`. No custom assertion message. | unaudited |
| [destination_failure_retains_no_partial_terminal][failure-test] | A positive short write followed by `injected serializer failure` returns `Write`, accepts some bytes, and leaves a local terminal unset. No custom assertion message. Not a ring-publication witness. | unaudited |
| [inconsistent_source_reports_length_mismatch_without_emission][mismatch-test] | Asserts both measured/written lengths, destination prefix bytes, and a local unset terminal. No custom assertion message. The destination has bytes despite the test name. | unaudited |
| [Length, cap, and error guards][length-guards] | Checked totals/CountingWriter reject overflow and cap crossing; `write_to` rejects length mismatch. Error text includes `prepared body length {len} exceeds wire cap {max}`, `prepared body length overflowed`, and `prepared body length mismatch: measured {measured}, wrote {written}`. | unaudited |
| [BoundedWriter guards][bounded] | Rejects excess requested writes with `WriteZero: prepared body exceeded measured length`; rejects impossible accepted counts with `InvalidData: destination reported an invalid write length`. This is an existing writer contract, not added scope. | unaudited |
| [Transform-envelope validation][envelope] | Requires an object with `messages: null`; otherwise `InvalidTransformEnvelope`. Existing error display says `transform envelope must contain a null wire_messages field`. Preserve the literal; do not silently repair it in this optimization. | unaudited |
| [json_settlement_streams_into_the_reservation_without_a_prior_encoding][settlement-test] | Exact bytes and reservation length; first event is reserve; all later events are writes and there is more than one. Messages: `every post-reservation event is a write` and `serde streamed into the reservation ({writes} writes); a single write_all would mean a fully encoded body existed before it`. | unaudited |
| [production_settlement_error_and_stream_skip_reservation][skip-test] | Error/Streamed outcomes do not become Response and reserve zero times. No custom assertion message. | unaudited |
| [production_settlement_cancellation_and_denial_emit_no_body][cancel-test] | Existing cuts return `request_cancelled`, with 0 then 1 reservations; denial returns `output_unavailable`. No custom assertion message. These assertions do not pin every diagnostic message or every write count. | unaudited |

None found: a complete same-variant/same-partition error matrix, direct
CountingWriter first-crossing versus eventual-length comparison, or complete
settlement message assertions. The public tests are not evidence for T3/T4.
No new transport fault campaign is part of this handoff.

## Allocation and measurement

| Existing check or measurement surface | What it observes and what it does not | Status |
| --- | --- | --- |
| [passthrough_shell_canonicalization_allocates_independently_of_key_count][alloc] | Global event difference for 1/65 blocks, divided by 64, must be at most 8. Message: `{per_block} allocation events per passthrough block (small {small_events}, large {large_events})`. Also checks Value-reference bytes. Fixed B cancels from the slope. | unaudited |
| [parse_charge_covers_typed_decode PeakAlloc fixture][peak-alloc] | Tracks requested-layout live/peak bytes process-wide; a mutex serializes its readers but does not exclude harness allocations. This is an allocator reuse lead, not a canonical-constructor check or owner-thread observer. | unaudited |
| [hot_path cold/warm transform benchmarks][bench] | Existing in-process transform cells distinguish fresh and primed output caches. [100/1,000-message constants][counts] are controlled inputs, not empirically established production frequencies. | unaudited |
| [direct_host request seam][host-seam] | Sends a real correlated request and returns parsed response JSON. It is reusable support, not a retained latency benchmark driver. | unaudited |
| [ipc_budget echo benchmark wiring][echo] | Uses the echo fixture. It is a transport control, not real-transform user-visible latency evidence. | unaudited |

None found at scoped HEAD: an isolated canonicalizer benchmark cell, a
full-ServedMessage-constructor benchmark cell, an absolute B allocation/copy
oracle, returned-A lifetime observation, constructor peak/slack measurement,
or a real-transform host latency benchmark driver. No C1 campaign-wide
accumulated situation assertion was found either.

The allocator search finds declarations only in the parse-charge fixture at
line 69 and the canonicalizer fixture at line 31. None is installed in the unit binary
at HEAD. The integration canonicalizer facade cannot measure the private full
constructor. U0 must establish an isolated filtered in-crate observer using
the existing transform module's private access, reusing compatible fixture
logic after inspection. This resolves the seam route, not its implementation
or peak measurement. No public constructor wrapper is needed; if compatible
test-only observation cannot be built, stop U0 for seam approval.

### U0 harness landing

Plan U0 lands the surfaces below at harness revision `c1dafa76`. The [baseline record](evidence/u0-baseline-measurement.md)
retains the captured numbers and provenance. Per the method contract every
check keeps status `unaudited`; the execution column records only that a run
happened on this revision, not an adequacy verdict.

| Surface | What it observes | Status | Execution evidence |
| --- | --- | --- | --- |
| `crates/daemon/tests/support/alloc_recorder.rs` | Thread-owned, fixed-capacity, non-allocating ledger over `System`: alloc/realloc/dealloc events with pointers and sizes, cumulative requested bytes, peak live bytes, growth-chain, release, and buffer-provenance queries. Windows serialize on a process mutex. | unaudited | used by the tests and driver below |
| `crates/daemon/tests/support/served_output_fixtures.rs` | Retained ASCII, retained escaped/Unicode/prefix, retained large payload, typed shell, and one-edited-block populations at 1 and 65 blocks, the independent byte and canonicality oracles, and the expected return-buffer kind per population. | unaudited | used by the tests and driver below |
| `canonical_miss_return_buffer_provenance_is_classified` | Independent canonicality oracle, byte oracle, returned-buffer provenance (fresh exact-N allocation, growth chain, or unattributed), liveness at return, exactly one exact-N allocation for the copy path, and no output-sized storage outside the returned chain for the ownership path. | unaudited | passes on the baseline revision |
| `full_constructor_observation_covers_receipts_hashing_and_arc_conversion` | Peak and events across the complete constructor through the `served_message_for_test` test-support entry, including the exact `Arc<[u8]>` payload allocation. | unaudited | passes on the baseline revision |
| `recording_excludes_other_threads_and_tracks_growth_chains` | Foreign-thread exclusion, growth-chain attribution, and rejection of a shrunk buffer. | unaudited | passes on the baseline revision |
| `crates/daemon/examples/canonical_output_evidence.rs` | Release driver: allocation cells, canonicalizer and full-constructor timing with per-sample setup outside both clocks, cold/warm transform timing and CPU, served-order and cache-counter frequencies, provenance, explicit host-latency-unmeasured status. | unaudited | ran 10 processes on the baseline revision |
| `scripts/perf/canonical-output-paired-runs.sh` | Frozen ten-run or ten-pair AB/BA process schedule with isolated worktree builds, resolved feature lines, and a provenance sidecar. | unaudited | ran in `baseline` mode |

The in-crate `--lib` observer route is infeasible: `crates/daemon/src/lib.rs`
declares `#![forbid(unsafe_code)]`, so no `GlobalAlloc` can be declared in the
daemon crate. The full-constructor observer uses the `test-support` entry
`daemon::transform::served_message_for_test`, matching the existing
`canonical_served_bytes_for_test` pattern. It is compiled only with the
`test-support` feature and is not part of the production API. A real-host
transform driver is not retained; host latency remains unmeasured.

## Suspiciously quiet areas and handoff

Prioritize constant allocation costs hidden by slopes, escape-aware identity,
empty/single-field inputs, late disorder, error finalization, and cache-hit
reference-work contamination. Successful bytes do not prove any of those
resource or control-flow properties. No result here proves lower latency.

Route all listed tests to `/testing:invariant-test-review`; route production
guards separately to `/low-level-systems:defensive-assertions-and-invariant-guards`.
Each catalog record goes to `/testing:test-strategy` for the cheapest valid
observation. Keep plan U0 → U1 → U4 and its verification contract; consult
the current [.github/workflows/ci.yml][ci] for required implementation checks.
This documentation pass neither runs nor replaces those checks.

[relations]: catalog.md#relationships-and-handoff
[formatter]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L42-L81
[sort]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L144-L164
[once]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L170-L193
[scalars]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L195-L215
[keys]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L217-L252
[constructor]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L164-L224
[shells]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L13713-L13757
[corpus]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L13759-L13794
[receipts]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L13797-L13939
[source-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L13942-L13969
[differential]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L4862-L4890
[cache-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L28295-L28320
[overlay]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L28323-L28358
[drop]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L28361-L28396
[fold]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L28399-L28424
[retained]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L28427-L28581
[epoch]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L28585-L28602
[tail-lookup]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L11127-L11131
[synthetic-lookup]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L10405-L10422
[json-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/prepared_output.rs#L15-L30
[segments-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/prepared_output.rs#L32-L52
[reserve-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/prepared_output.rs#L54-L99
[cap-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/prepared_output.rs#L117-L129
[overflow-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/prepared_output.rs#L131-L163
[failure-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/prepared_output.rs#L165-L204
[mismatch-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/prepared_output.rs#L206-L234
[length-guards]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/dispatch.rs#L237-L330
[bounded]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/dispatch.rs#L416-L458
[envelope]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/dispatch.rs#L103-L118
[settlement-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L17823-L17867
[skip-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L17869-L17892
[cancel-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/lib.rs#L17894-L17939
[alloc]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/served_json_passthrough_allocations.rs#L10-L86
[peak-alloc]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/parse_charge_covers_typed_decode.rs#L25-L87
[bench]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/benches/hot_path.rs#L320-L397
[counts]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/benches/hot_path.rs#L33-L35
[host-seam]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/support/direct_host.rs#L359-L371
[echo]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/host-runtime/benches/ipc_budget.rs#L24-L28
[ci]: ../../../.github/workflows/ci.yml
