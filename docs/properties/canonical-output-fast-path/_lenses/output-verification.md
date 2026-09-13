# Output verification lens

## Scope and evidence

The supplied [plan](../../../plans/2026-09-13-0030-perf-canonical-output-direct-frame-plan.md)
defines the evidence scope. It is absent from HEAD; its older source anchors
are not reused. All numbered references below are verified through
`git show HEAD:<path>` at `2e4433e6b511ae74944df8a9669c428e73915d29`.
Unrelated working-tree changes are excluded. No tests or measurements run here.

Failure/degradation: `crates/daemon/src/served_json.rs:121-164` serializes
once, sorts every object, then always allocates B. Serialization failure exits
before sorting/copying. The selected change adds no transport or callback.

Liveness: `crates/daemon/src/lib.rs:12360-12431` measures, checks cancellation,
reserves, checks cancellation, then writes synchronously. Preserve these cuts;
add no unbounded eventuality. Cancellation remains best effort, not retraction
after settlement (`docs/host-wire-protocol.md:765-798`).

Bounded history: `git log -8` over selected daemon/host/ring modules and
`git log -6` over serializer/dispatch files identify `32829851` and `04a82e31`.
Their inspected diffs establish span serialization and reduced key-decoding
work, not an incident or independently reproduced performance result.

## Reused properties and fault map

All records remain active claims under test; existing checks remain unaudited.

| Property | Reachability | Check and enabling state |
| --- | --- | --- |
| [B1][b1] | default-production | `always`: frozen canonical bytes, hashes and fingerprints agree across retained, typed, edited and cached shells. |
| [T1][t1] | default-production | `always`: preserve admission and reclamation bounds under saturation, abort and wraparound. |
| [T2][t2] | default-production | `always`: preserve atomic arena copies under concurrent peer writes; no references into peer-writable payload. |
| [T3][t3] | test-only | `always`: direct publication either commits declared length or aborts; retain charges through completion. |
| [T4][t4] | test-only | `sometimes`: independently witness queued direct output, finished handler and uncommitted publication. |

T3/T4 remain HP1 obligations, not deliverables. The only `output_from_writer`
caller is `crates/host-runtime/tests/support/mod.rs:492-508`; no test invokes
`direct_fill`. Public writer failures do not exercise it. Security and transport
obligations remain unchanged: complete-frame publication and unknown-outcome
handling (`docs/host-wire-protocol.md:314`).

### prepared-output-diagnostics-preserved

Type: safety
Reachability: default-production
Status: active
Exercised: partial - Existing length/failure checks exist; proposed cases are unrun.
Guarantee: Preparation preserves measured lengths and path-specific failure diagnostics.
Check: `always` - Successful writes equal measured length; cap refusal precedes
reservation; pre-write cancellation/denial writes nothing; write failure returns
no Response.
Compare exact error variants, codes and length fields because all are observable.
Fault/timing angle: Cancellation before/after reservation; partial writer failure.
Required faults and enabling state: Cap crossing, inconsistent segment lengths,
reservation denial, and failure during JSON, envelope or segment writes.
Confidence: high - [Evidence](#diagnostic-evidence) establishes the production seam.
Existing check: `crates/daemon/tests/prepared_output.rs:118-233` and
`crates/daemon/src/lib.rs:17823-17939`; all unaudited.
Impact: Misclassified errors or incomplete output escape preparation.
Open questions: None.

#### Diagnostic evidence

`crates/daemon/src/dispatch.rs:237-277` maps JSON destination failures to
`Write`; envelope serde failures remain `Serialize` at `:343-367`.
Preserve that distinction. `:280-330` and `:386-408` distinguish known totals
from the first over-cap counting chunk; do not replace reported length with
eventual total. Extend existing public `measure`/`write_to` and private
`settle_prepared_with` tests, not production APIs. The partial-terminal test
uses a local variable, not a ring observation.
Cover positive short writes, `Interrupted`, zero progress, invalid accepted
counts, underfill and failure after accepted bytes. Assert exact error fields
and reservation/write counts, not only `is_err()`.

## Test strategy

- Reuse `crates/daemon/src/transform.rs:13714-13794` literal/hash/segment and
  frozen-corpus oracles. Exhaust all six three-key permutations using decoded
  keys; offsets increase exactly for the identity permutation. Include escaped,
  empty, prefix and Unicode keys, scalar edge cases, nested disorder and two
  unordered siblings. A short-circuit mutation must fail.
- Keep `crates/daemon/src/served_json.rs:170-251` single-visit checks. Add a
  test-only failing `Serialize` at private `encode`: assert the error and no
  finalization/copy. Production `WireMessage` construction expects success
  (`crates/daemon/src/transform.rs:164-166`); do not invent a production failure.
- Replace slope-only evidence (`crates/daemon/tests/served_json_passthrough_allocations.rs:58-85`)
  with an absolute per-call B allocation oracle: canonical N-byte misses remove
  one B allocation and N logical copy bytes without replacement storage.
  Use large scalars/small metadata, allocation sizes and test-only branch
  instrumentation. Use nonallocating, thread-owned recording in a dedicated,
  filtered single-test process to exclude other libtest threads. A mutex
  between tests is insufficient. Always-copy
  must fail this negative control. Neither negative control runs here.
- Preserve retained/typed/edited cold misses and warm hits, 1/65 blocks and
  100/1,000 messages. Record events, requested bytes, peak live bytes, A capacity,
  explicit copies, CPU and latency separately, including complete construction.
  Keep ten AB/BA process pairs and replay provenance.
- `crates/daemon/benches/hot_path.rs:1-8` measures in-process work;
  `crates/host-runtime/benches/ipc_budget.rs:24-28` uses echo. Host latency needs
  a retained, reproducible `direct_host` driver through real transforms including
  `serve_native`, timed send-to-correlated-terminal. Its seam exists at
  `crates/daemon/tests/support/direct_host.rs:359-371`; that benchmark does not.
  Keep echo separate. Green functional tests establish no speedup.

## Handoff and gates

Preserve U0 → U1 → U4 and every original verification command in the plan.
For U1 daemon edits, CI additionally requires predecessor/retired/schema guards,
“Incident history and verifier gates,” “Workspace graph has no sibling checkout
or metadata stub,” fuzz format/check, “lint and test (stable),” and
“native addon package (Bun),” including payload/tarball smoke and Rust-mode E2E
(`.github/workflows/ci.yml:116-302`, `:376-425`, `:470-591`, `:668-739`).
Miri/Valgrind remain path-conditional; no unsafe transport edit is selected.
This lens alone is outside Rust/native path filters
(`.github/workflows/ci.yml:55-102`).

Portfolio self-check prioritizes diagnostic and allocation gaps over duplicate
boundary records. Fresh-context evaluation was blocked by the subagent depth
limit; independent evaluation remains outstanding.

[b1]: ../../hot-path-optimization/latency-audit/catalog.md#derived-artifacts-are-ownership-independent
[t1]: ../../hot-path-optimization/latency-audit/catalog.md#arena-residency-is-bounded-by-admission-and-one-punch-batch
[t2]: ../../hot-path-optimization/latency-audit/catalog.md#arena-payload-copies-keep-the-address-derived-atomic-shape
[t3]: ../../hot-path-optimization/latency-audit/catalog.md#direct-frame-publishes-declared-length-or-nothing-and-holds-its-charges
[t4]: ../../hot-path-optimization/latency-audit/catalog.md#direct-frame-outlives-its-handler-before-publication
