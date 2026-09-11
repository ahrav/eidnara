# Shared selection and pressure accounting

## Provenance

- Work item: https://github.com/ahrav/eidnara/issues/420.
- Parent specification: https://github.com/ahrav/eidnara/issues/350.
- Characterization baseline: `afac1d1f`, branch `perf/shared-selection-inputs`.
- Owner decision supplied in the controller task: remove the dead
  `memory_update_count > 40` SOFT arm and retire its reachability marker.
  Production composition sets the count to zero. No writer is introduced.
- The same instruction retires the global baseline campaign, citing owner
  comments on #410, #411, #413, and #414 and the W1/W2 invalidation in merged
  #409. Per-ticket correctness checks still apply. This report makes no
  latency, allocation-count, or production-representativeness claim.
- Discovery evidence in the four linked records remains a historical snapshot.
  The current implementation and execution evidence below supersedes its
  unresolved questions about input sharing and estimator routing, not its raw
  observations or unrelated open questions.

## Implementation and bounds

`SelKind<'a>::ToolCall.input` is `Cow<'a, Value>`. Both production constructors,
[`sel_item_from_flat`][selection] and [`sel_kind_for_flat`][historian], borrow
the input inside the projected wire block. Selection clones copy that borrow.
The three consumers use references for arc grouping, todo capture, and
historian summaries. Standalone owned fixtures use `Cow::Owned`; the frozen
selection reference remains byte-identical. Its generator adapts only the
candidate input representation.

The borrow lifetime propagates through selection items and boundary messages,
but not through persisted wire types. No new process-local cache or retained
input allocation is added. Existing projection-owned wire and `tool_input`
copies remain; this work removes their downstream selection copies, not the
projection's own copies. Token-cache generations, capacity, key domains, and
`RETAINED_BYTES_BOUND` are unchanged.

[`soft_pressure_refold`][pressure] owns the frozen-unit lookup, placeholder
gate, and both comparisons. Its estimator argument is the pass's injected
function. Tag minting and fallback nudge derivation use the existing
`cached_estimate_tokens` entry point, as W3 permits. Stored tag counts avoid
recounting, and serialization needs no token estimate. No forwarding arguments
or alternate memo are added to those helpers.

The sidecar prefix loop proves `order ⊆ messages.keys()` while copying the
prefix. If every prefix key has metadata, suffix insertion preserves that
invariant and map membership equals order membership. Sparse cached metadata
breaks the premise, so that path retains the original order scan. Rebuilding
all missing metadata was tested and rejected: it caused the giant degraded
snapshot to exceed its expected retained charge. The [merge check][sidecar]
preserves both the healthy and sparse cases.

Initial production diff against `afac1d1f`: 135 lines, 80 additions and 55
deletions. This counts changed
Rust source outside test modules, excluding the removed documentation of two
`cfg(test)` wrappers. Benchmark fixture adapters add another 12 changed lines
and are not production code. All source changes are inside `crates/daemon/`.

## Characterization before production edits

Commands use `--locked` and run from the repository root.

- `cargo test -p daemon --locked --test selection_differential`: 18 passed.
- `cargo test -p daemon --locked --lib soft_pressure_classification_matches_frozen_thresholds`:
  passed with 48 real SOFT evaluations, before replacing the predicate.
  The fixture first corrected its expected response action: a pressure refold
  enters through SOFT but returns HARD. The frozen predicate was unchanged.
- `cargo test -p daemon --locked --lib selection_input_shares_projected_wire_value`:
  failed on pointer identity, after value equality passed.
- `cargo test -p daemon --locked --lib transform_has_no_global_estimator_bypass`:
  failed on all four direct calls: frozen m0, composed m1, tag mint, and nudge.
  An initial fixture error in the test-module delimiter was corrected before
  collecting this red result.

## Verification after implementation

The [threshold comparison][thresholds] measures m0 at 499 and 500 tokens, m1
at 74, 75, and 76 tokens, and budgets 0, -1, 365, 370, 375, 380, 385, and
100,000. Negative budgets are helper robustness cases, not admitted request
values. The three constant witness assertions record budget-only pressure,
ratio-only pressure, and neither, independently of the candidate result.
An injected spy checks the exact m0 and m1 texts and cached/direct parity.
The original direct-tokenizer reference retains its dead arm with count zero;
no test pretends that an update-count crossing is production-reachable.

Focused commands `cargo test -p daemon --locked --lib <filter>` passed:

| Filter | Passed | Ignored |
| --- | ---: | ---: |
| `transform::` | 287 | 3 manual timing tests |
| `selection::` | 38 | 0 |
| `boundary::` | 29 | 0 |
| `injection::` | 18 | 0 |
| `wire::` | 11 | 0 |
| `codec::` | 40 | 0 |
| `token_cache::` | 6 | 0 |

The selection differential also passed after the ownership change. The final
sharing test checks all 48 tool-call inputs in `selection-golden.json`, including
projection-field equality and pointer identity through both consumers and a
selection clone. The
placeholder/absent-m0 test checks exact call counts and two warm cache hits.
The tag-mint test checks only three new counts, no recount of stored nudge
rows, 64 fallback nudge counts, and no work when every block is tagged. The
serialization test preserves bytes across fresh and cached builds with no
token-cache calls. The giant degraded native snapshot test passes with the
sparse-order fallback.

`cargo clippy -p daemon --all-targets --locked -- -D warnings` passes.
Scoped rustfmt and `git diff --check` pass. A broad daemon library run during
development reported 1083 passed, three failed, and four ignored: the rejected
full-sidecar rebuild and two dreamer deadline tests. After removing the rebuild,
the giant snapshot passes. Both deadline tests pass when run together with
filter `dreamer_run_task_bounds_`; their failure under suite load remains
reported, not treated as a green full-suite run.

Workspace/Bun gates, six independent reviews, and any draft PR are controller
work. No git mutation or tracker edit is performed here. The source scan is a
lexical guard over production `transform.rs`, not a proof of the complete
transitive call graph. No new contention or cache-capacity campaign is run.

## Parent integration and review disposition

Verification base: `8a188111`, including parent review commits `763766be` and
`8a188111`. The controller reports completion of all six mandatory reviews and
no confirmed production correctness blocker. No nested review runs here.

Both documentation conflicts are resolved without dropping the parent guard
or verifier fixes. Live source and test anchors are re-resolved after the merge
and fixture edits. Pinned discovery snapshots, historical lens findings, and
historical execution results remain unchanged.

The SOFT test searches for the 74-, 75-, and 76-token composed bodies once per
target, not once per case. Each of the 48 cases still uses a separate real
store, composes m1, independently checks its direct count, drives the actual
pass through the injected estimator, and compares against the frozen oracle.
The oracle is byte-identical to the reapplied stash. Measurement-gate tests
also assert that m0 is counted first and non-placeholder m1 last.

The scan is named
`production_transform_module_has_no_global_estimator_bypass`. It still scans
the full production transform module, including imports and the named SOFT,
serialization, tag-mint, and nudge sections. A `tokenizer::` import is rejected
even when aliased; unusual whitespace remains a limitation of the lexical
check. No transitive handler-wide coverage is claimed.

The injected estimator remains a deterministic dependency: equal text must
produce equal counts. It exposes classification and measurement order to tests;
production supplies the existing cache-backed function. Byte-length estimator
fixtures exercise that seam and do not promise the real tokenizer's results.
The m0 measurement precedes the placeholder gate, including on placeholder
passes. No global-estimator substitution or estimator-order change is made.

The architecture remains separate where behavior differs: boundary tool-result
names remain empty while transform selection retains real names. `Cow` borrows
the exact existing wire value; wrapping an owned-value clone in an `Arc` would
keep the copy unless the wire owner also changed. `FlatBlock.tool_input`
remains part of the serialized projection artifact. The constant
`M1Composition.memory_update_count` field remains for the frozen baseline oracle
and its zero-count assertions; no production decision reads it. Sparse-sidecar
fallback uses a transient boolean and the existing order vector, with no extra
set or retained cache.

After integration, `cargo test -p daemon --locked --lib <filter>` passes for
`transform::` (288 passed, three manual timing tests ignored), `selection::`
(38), `boundary::` (29), `injection::` (18), `wire::` (11), `codec::` (40),
`token_cache::` (6), and `differential_goldens::` (4). The prefix-differential
corruption check, full-versus-delta normalization check, three-turn synthetic
witness, and giant degraded-snapshot check each pass separately. The transform
suite includes the parent's authoritative-view overlay regression.

`cargo test -p daemon --locked --test selection_differential` passes all 18
tests. The frozen selection reference, bound golden verifier file, parent
overlay guard and its regression test, and both prefix-differential helpers
are unchanged from their respective reference versions. Scoped formatting,
daemon all-target clippy with `-D warnings`, the comment-marker check, and
`git diff HEAD --check` pass.

The final production diff against `8a188111` is 131 lines: 76 additions and
55 deletions, with the same test/documentation exclusions as above. The
benchmark adapters account for 12 additional non-production lines. Full
workspace/Bun gates and index resolution remain controller work. The worktree
contains no conflict markers; the unmerged index is deliberately untouched.

[selection]: ../../../../../crates/daemon/src/transform.rs#L6352
[historian]: ../../../../../crates/daemon/src/lib.rs#L16629
[pressure]: ../../../../../crates/daemon/src/transform.rs#L6315
[sidecar]: ../../../../../crates/daemon/src/codec/opencode.rs#L2071
[thresholds]: ../../../../../crates/daemon/src/transform.rs#L24318
