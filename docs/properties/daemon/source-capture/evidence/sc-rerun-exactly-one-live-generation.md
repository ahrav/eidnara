# sc-rerun-exactly-one-live-generation

## Discovery trigger

Part B record on #1066 (comment 2), requirement R5 and decision D10: a
`raw_tool_spans` identity includes `result_revision`, so a re-run of one
native tool call is a new lineage, and nothing at baseline retired the
earlier generation. U7 (#1086) landed the reconciliation and named this
record as its obligation, with C8 for the in-commit recheck and OQ11 for
the lookup cost.

## Evidence trail

`crates/daemon/src/harness_sources.rs`:

- `ToolCall` names a call by `project_id`, `harness`, `session_id`,
  `parent_message_id`, `tool_call_id`; `tool_generation` reads the call
  and `result_revision` by field name.
- `publish_screened` calls `plan_reruns` over the admitted units, then
  commits one `KeyedWrite` from `Reruns::intent` and `Reruns::apply`
  ahead of the units through `publish_batch`.
- `plan_reruns` reads each call's live generations in one preview
  (`live_generations`, narrowed on `tool_call_id`) and marks an older
  generation `Superseded`, `Kept` when its receipt replays, or
  `Conflicting` (`PublishError::IdentityReused`) when its key holds
  another digest.
- `Reruns::apply` re-reads the calls inside the commit; a newer live
  generation or a different predecessor set is `KernelError::Conflict`;
  otherwise `retire_descriptors` retires the planned set.
- `Reruns::intent` keys `source-rerun-retire:{digest}` over
  `RERUN_RETIREMENT_FORMAT` (`source-rerun-retire.v1`), each call's
  fields, its successor, and its sorted retirement set.
- The `publish_screened` doc states the precondition: the stale and
  first-generation decisions come from the preview and hold while every
  `raw_tool_spans` publication is serialized by capture's commit gate;
  `SourcePublisher::publish` and `publish_batch` do not reconcile.

`crates/kernel/src/source_descriptor.rs`:
`Envelope::live_source_descriptors_with` is the in-transaction narrowed
read; it sees the commit's own writes and retirements.

`crates/daemon/src/source_capture.rs`: `CaptureJob::publish` takes
`commit_gate` before `publish_locked` calls `publish_screened`. The only
production producer of `raw_tool_spans` is this path; `claim_sources.rs`
publishes `canonical_claims` and `promoted_memory`, `git_reconcile.rs`
publishes `git_commits`.

## Failure scenario

Restart with an empty memo, then generation 2 publishes beside live
generation 1. Or a redelivered generation 1 after generation 2 retires
generation 2 or republishes 1. Or a reversed delivery (2 then a
never-published 1) leaves 1 live. Or the retirement commits separately
from the successor, so a reader sees zero or two live generations.

## Timing windows and dependencies

- Between the preview and the commit, another writer could publish a
  newer generation. The in-commit re-read covers every call that
  retires. A first-generation unit with no live predecessor is decided
  only in the preview; it is safe because the commit gate serializes
  every `raw_tool_spans` publication of the store in one process.
- A store written before U7 can hold two live generations of one call;
  it converges only when the newest generation is delivered again.
- `publish` and `publish_batch` called directly publish tool units with
  no reconciliation. `a_rerun_retirement_rechecks_the_call_inside_its_commit`
  constructs two live generations exactly that way.

## What a test must construct

- One `tool_call_id` delivered in order, reversed (2 then 1),
  duplicated (1, 2, 1), error-to-success, and with fewer blocks; read
  live descriptors per call and the predecessor's
  `invalidated_commit_seq` against the successor's `created_commit_seq`.
- A restart: empty the memo between generations (F3).
- Two generations in one batch; a window showing published 1 beside new 2.
- Distinct `tool_call_id`s with equal text (legal witness).
- Inside one commit: a newer live generation and a missing predecessor.

Landed tests construct all of these (see the catalog record).

## Investigation log

### Q: Source and cost of the predecessor lookup (OQ11)
- Sources examined: `ticket-1086.md`; `plan_reruns`, `live_generations`,
  `Envelope::live_source_descriptors_with`; PR #1100 body;
  `rerun_lookup_cost_at_ten_thousand_live_tool_descriptors`.
- Findings: the default landed: a filtered live inventory narrowed in
  the store on `tool_call_id`, one pass per batch in the preview and one
  in the commit, no Kernel index and no schema change. The ignored
  measurement test builds 10 000 live descriptors and times a 32-call
  re-run window. The PR records a 245 ms median on a quiet host against
  the 500 ms criterion. The number is the PR's record; this agent did not
  run the test.
- Missing evidence: none for the decision. Cost is linear in live tool
  descriptors; U4's 30-day expiry bounds that population.
- Conclusion: resolved with answer (U7: filtered narrowed inventory, no
  index).

### Q: Does a re-run keep its `callID` (OQ3)
- Sources examined: `ticket-1086.md` design decisions; `ToolCall`;
  `a_rerun_keeps_one_live_generation_across_a_restart`; U5 trace fixture
  `crates/daemon/testdata/opencode-tool-outcome-trace.json` (per PR #1099).
- Findings: U7 treats a new `callID` as another call and a kept `callID`
  with a new `state.time.end` as a newer generation. Both readings are
  handled. The capture-path test re-runs an OpenCode call in place with a
  new `state.time.end`.
- Missing evidence: no pinned trace shows which reading OpenCode uses for
  an edited re-run; both readings are safe under this record.
- Conclusion: resolved with answer for this record (U7); the native
  behavior itself is not pinned.

### Q: Do stores written before U7 or direct `publish_batch` callers break the guarantee?
- Sources examined: PR #1100 "Recorded and left as written";
  `publish_screened` doc; production callers of `publish_batch`.
- Findings: only `publish_screened` reconciles. No production caller
  publishes tool spans through `publish` or `publish_batch`.
- Missing evidence: none.
- Conclusion: resolved with answer; the guarantee holds for commits made
  through `publish_screened` under the commit gate after U7.
