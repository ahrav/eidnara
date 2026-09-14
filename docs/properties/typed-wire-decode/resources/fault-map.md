# Resource fault and enabling-state map

System: daemon typed-wire resource paths.
HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
[Source register](source-register.md) records supplied evidence and limits.
All constructions below are recommendations for handoff. None runs here.

## Availability

| Fault or state | Existing seam | Availability and limit |
| --- | --- | --- |
| Plain/escaped text and dense native/ignored nodes | `crates/daemon/tests/parse_charge_covers_typed_decode.rs:90-199` | Input builders and peak counter exist; text-heavy combined tree cases are missing. |
| Malformed JSON, typed errors, duplicate keys, raw tokens | `crates/daemon/src/lib.rs:19712-20118` | Corpus and both decoders exist; post-change duplicate message/block fallback must be witnessed separately. |
| Zero or partially free reserve | `crates/daemon/tests/parse_charge_covers_typed_decode.rs:224-333` | Allocation refusal probes exist; successful fake charges are unbacked. |
| Real held scratch bytes | `crates/daemon/src/lib.rs:20242-20276,20414-20470` | TestPool uses ByteBudget and a distinct held charge. Ring-level competing-request barrier is not supplied. |
| Frozen cases and additive ceiling neighbours | `crates/daemon/src/lib.rs:19569-19645,20188-20229` | Freeze original A1-A3 byte/capacity/outcome receipts; length caps and node floor stay fixed. Add new string-ceiling cases without replacing old ones. |
| Projection and alias lifetimes | `crates/daemon/src/wire.rs:958-1238,1749-1792` | Manual estimate and pointer-sharing controls exist; no continuous full-path allocator ledger. |
| Cache removal with live lease | `crates/daemon/src/lib.rs:20624-20675,23311-23353` | Retained owner and charge observations exist; not a transient-construction bound. |
| Canonical serializer workspace | `crates/daemon/src/served_json.rs:121-141` | Code shows metadata and two buffers; attributable peak/charge instrumentation is missing. |
| Above-facade-cap escaped key | `crates/daemon/src/lib.rs:12153-12166,15837-15860,16144-16161` | Probe-before-meter ordering is verified and conflicts with A1 at `docs/properties/hot-path-optimization/latency-audit/catalog.md:159-165`; allocation magnitude is unmeasured. |
| Real host admission outcome | `crates/daemon/tests/direct_host.rs:48-69` | Existing Unix FixtureProcess; request-level resource trace/barrier requires downstream work. |
| 40/200 combined operation measurement | `crates/daemon/benches/hot_path.rs:69-115` | Existing corpus/projection scaffold; new operation group, manifest, and schedule are missing. |

Crash, power loss, network partition, election, and remote-service outage
faults are N/A to these local resource claims. They are not simulated with
unrelated failures merely to increase fault coverage.

## Required faults by property

| Record | Required non-vacuous construction | Observation |
| --- | --- | --- |
| [R1](evidence/decode-footprint-covers-both-lanes-combined-peak.md) | Both actual tree stages, direct decode, escaped/plain strings, dense nodes, payload Values, failed prefixes and fallback | Continuous peak compared with selected footprint; coefficient selection is a separate accepted design procedure. |
| [R2](evidence/retained-accounting-follows-typed-ownership.md) | Every surviving Value family, spare capacity, canonical text, shared and distinct backing, owner after eviction | Independent ownership ledger and live allocation identity. |
| [R3](evidence/frozen-admission-outcomes-and-boundaries-stay-stable.md) | Original A1-A3 cases, fixed length-cap/node-floor neighbours, exact held-pool schedule, and additive string-ceiling cases | Original byte/capacity/outcome receipts; terminal count before client filtering; independent dispatch/store observers. |
| [R4](evidence/decode-and-projection-stay-within-resident-pool.md) | Near-ceiling full request, nonempty projection, both probe sizes, fallback and cleanup, other owners | Time-correlated logical demand and covering reservations under existing pool/ownership rules; layout observations are lower bounds, not RSS. |
| [R5](evidence/message-decode-allocation-gate-has-isolated-scope.md) | Fixed 40-message plugin-shaped subtree and production decoder with validated scope | Exact events and high water against raw `messages` JSON byte length. |
| [R7](evidence/resource-witnesses-reach-independent-preconditions.md) | Each of the twelve resource markers below | Independent sometimes checks, never candidate safety result or error code alone. |

R6 is invalidated for category mismatch. Its accepted obligation is
[EG1](evidence-gates.md#eg1-decode-projection-payoff), not a fault-mapped runtime
property. Both sizes, both artifacts, and stop-within-noise remain mandatory.

## Independent coverage markers

Each of the twelve names below is unchanged, constant, and globally prefixed.
Evaluate an independent `sometimes(preconditions_m)` for each, with its own
predicate, occurrence floor of one, and result. Their conjunction is only a
completion rollup. No single aggregate `sometimes` replaces those checks.
For rows naming hostases, retain per-hostase receipts and require all hostases;
one favorable hostase cannot certify the row. Missing rows mean incomplete.

| Constant marker | Independent qualifying preconditions |
| --- | --- |
| `typed-wire-resources-direct-owned` | Frozen valid unpaged bytes reach the production typed decode and materialize nonempty owned messages. |
| `typed-wire-resources-tree-combined` | Measurement starts before actual `from_slice::<Value>`; tree allocation identities remain in the live trace as consuming `from_value` begins; both plain and escaped text cases are recorded. |
| `typed-wire-resources-fallback-prefix` | Duplicate message-envelope and block-envelope cases each reach a typed prefix and an ensuing tree conversion, with one continuous trace; neither is inferred from final lane alone. |
| `typed-wire-resources-escaped-scratch` | Independently inspected escaped key/value inputs exercise scratch creation, with short and large strings recorded. |
| `typed-wire-resources-late-failure` | Known valid large text/dense prefixes are consumed before the supplied late type error or malformed suffix is encountered, and cleanup is observed. |
| `typed-wire-resources-held-pool` | A fixed fitting body reaches a reservation attempt while a distinct owner holds known bytes, leaving less than the fixed required shortfall; record acquisition and overlap, not just `queue_full`. |
| `typed-wire-resources-after-holder-release` | The same body's next attempt occurs after the independent holder drops and the budget returns its bytes. Success remains a separate assertion. |
| `typed-wire-resources-frozen-boundaries` | All original frozen A1-A3 cases and fixed length-cap/node-floor neighbours are submitted at original numeric capacities and schedules; new string-ceiling observations are additive, never replacement witnesses. |
| `typed-wire-resources-probe-above-cap` | Raw bytes exceed 1 MiB, contain a long escaped top-level key, and reach the cap probe before meter creation; a sub-cap control is also recorded. |
| `typed-wire-resources-projection-overlap` | Decoded input allocations remain live while nonempty canonical text and projection workspace exist; include rebuilding and prefix-reuse cases. |
| `typed-wire-resources-surviving-owner` | A shared ingress/native owner survives cache removal; a separate equal-content allocation and copy-on-write case are also present. |
| `typed-wire-resources-messages-scope` | Exactly 40 nonempty mixed messages and frozen raw JSON bytes enter a production-type decoder with validated attribution independent of measured threshold values. |

The resource campaign requires these twelve rows. A diagnostic run records which
rows it does not attempt and cannot claim completion of R7. A missing marker
means investigate generator coverage versus unreachable preconditions; it
does not mean assert `sometimes(peak_exceeded)` or relax the safety check.

### Preserved evidence receipt outside R7

`typed-wire-resources-measurement-pair` retains its original name under EG1.
It requires raw before and after records for both 40 and 200 messages, with
all four cell identities present; it requires no speedup. This is manifest
completeness, not runtime `sometimes` or liveness. No marker name is renamed.

## Leverage ranking by cheapest valid oracle

1. Freeze admission bytes, numeric capacities, and receipts using existing
   TestPool and dispatch seams. This catches witness drift without a load test.
2. Extend the existing isolated peak observation to actual combined tree,
   escaped text, and late-error intervals. Keep one baseline across stages.
3. Reuse retained-size and pointer-sharing controls with an independent
   ownership ledger for the final model, including surviving payload Values.
4. Validate messages-only attribution before enforcing allocation ratios.
   A small subtree check is cheaper than an invalid whole-body comparison.
5. Observe complete entry/decode/projection demand against the existing logical
   pools. Use producer-side terminal observation for refusal integration;
   managed-client settlement cannot count duplicate terminal frames.
6. Execute the predeclared 40/200 operation measurement after its manifest and
   acceptance rule are settled. Source inspection cannot substitute for it.

This ranks observation cost, not implementation priority or proven adequacy.
Test form and instrumentation design remain with the named handoff owners.

## Marker status after the typed-wire U1 execution, 2026-09-13

Each marker below is reported on its own; the rollup is a summary only.

| Constant marker | Status | Witness |
| --- | --- | --- |
| `typed-wire-resources-direct-owned` | fired | `whole_request_decode_fits_its_resident_charge` decodes the frozen 40 and 200 bodies typed and asserts the message count. |
| `typed-wire-resources-tree-combined` | fired | `parse_charge_covers_text_heavy_peaks_on_both_lanes` records the tree lane from `decode_metered::<Value>` through `from_value`, plain and escaped. |
| `typed-wire-resources-fallback-prefix` | partial | `parse_charge_covers_a_failed_typed_prefix_and_its_tree_fallback` covers a duplicate message-envelope key after a 4 MiB prefix in one trace; the block-envelope duplicate is not constructed. |
| `typed-wire-resources-escaped-scratch` | fired | 64 KiB and 4 MiB escaped text on both lanes; the escaped 900 KiB key in the probe tests. |
| `typed-wire-resources-late-failure` | partial | Only the duplicate-key failure after a large prefix is constructed; a late type error and a malformed suffix are not. |
| `typed-wire-resources-held-pool` | fired | `a_drained_pool_refuses_a_fitting_body_as_transient_and_records_the_shortfall` (`crates/daemon/src/lib.rs`, existing unit test): a backed `TestPool` holds `footprint / 2` for a distinct owner, the fitting body's metered decode is refused as transient, and the shortfall marker records needed, charged, and capacity. `a_pool_with_room_for_the_prefix_only_refuses_before_the_large_string_is_unescaped` and the drained-pool probes prove refusal ordering only: `Granting.held` starts at zero and grants unbacked `ByteCharge::none()`, and `Drained` returns `None` for every charge, so neither has an independent holder. |
| `typed-wire-resources-after-holder-release` | fired | The same `a_drained_pool_refuses_a_fitting_body_as_transient_and_records_the_shortfall` drops the holder, then decodes the same body with a fresh meter and asserts the decoded `kind` separately. |
| `typed-wire-resources-frozen-boundaries` | fired | `frozen_corpus_footprints_replay_with_only_string_charge_changes` at original capacities; `text_heavy_admission_ceiling_witnesses` is additive. |
| `typed-wire-resources-probe-above-cap` | partial | `byte_cap_admits_a_facade_sized_body_without_body_proportional_allocation` covers the sub-cap control; the above-cap magnitude is unmeasured. |
| `typed-wire-resources-projection-overlap` | fired | `decode_and_projection_fit_the_declared_pool` keeps the request live through projection and checks shared shells; prefix reuse is covered by `wire.rs` sharing tests. |
| `typed-wire-resources-surviving-owner` | unfired | No eviction-with-live-owner sequence runs in this execution. |
| `typed-wire-resources-messages-scope` | fired | `message_decode_stays_within_the_allocation_budget` over the frozen 40-message bytes. |
