# Search recovery completion handoff

## Scope

The completion coordinator is `SearchSelection::recover_slice`. It uses the
existing replacement builder, selector, retirement receipts, lifecycle record,
and hook gate. It does not introduce another queue or change the kernel schema.

Construction and selection return `Selected`. A separate scheduled slice checks
the canonical inventory, retires the bound predecessor, reconciles the paired
local and kernel prefix at the fixed target, removes the construction copy,
releases its capture, and records `Current`. Withholding that slice prevents
completion. Failed construction and selection return their existing owned
failure objects.

The lifecycle record retains the operation identity, target, authorization,
start time, deadline, and consumed allowance. An explicitly authorized recovery
is a separate operation; it does not amend those fields in the disabled
operation. Missing or stale gate evidence cannot authorize that transition.
The disabled handoff supplies the predecessor's deregistration evidence when
the old consumer has already left.
An authorized recovery keeps one level of prior handoff. The handoff it embeds
has its own `prior_disabled` cleared, so repeated aborted recoveries do not
nest older control records, and completed cleanup drops the level it kept.
Retirement reads only that one level.

A disabled handoff may be the operation that produced the live selection, or an
unfinished follow-up recorded above it whose `selected_generation` names that
selection. Recovery accepts both. In the follow-up case the live consumer is the
certified predecessor's, the request must name a different consumer, and a
follow-up consumer that already registered must be reused so its checkpoint
does not stay behind.

Construction registers its consumer unless the intent inherits the disabled
handoff's consumer. A request that names any other registered consumer fails
with the kernel's `Conflict` rather than adopting that consumer's checkpoint.

## Record compatibility

This branch changes the durable control record in two ways: `Current` is a new
schema-4 record, and an active intent may carry `prior_disabled` (also copied
into the family certificate at selection). Both `LifecycleIntent` and
`DisabledIntent` reject unknown fields, so a daemon built before this branch
reads either shape as `Unavailable` or "bootstrap corrupt". On that older
binary the gate disables on every admission, `protected_generations` fails and
blocks reclamation, and `disable()` and `record()` return `Unavailable`.

Rolling back to an older binary after any operation has completed or any
authorized recovery has started is therefore one-way. Recovery on the older
binary requires removing `search-lifecycle/intent.json` and any
`search-families/<digest>/bootstrap.json` written by this branch, then
rebuilding. Decide the rollback policy before wiring these writers into the
daemon; no production caller writes these records yet.

`Current` records completion history. It is not a query authorization or a
persistent inventory proof. A new selection handle cannot serve from that
record alone. Reopening checks the current canonical inventory and paired
prefix. Final egress authorization remains canonical.

Completed observations use a fresh caller budget, even after the construction
deadline. They verify the durable selector, live gates, operation binding, and
mutable family against canonical state. They neither charge an episode nor
perform retirement, acknowledgement, construction cleanup, or a Current write.
A verified active prefix may extend beyond the completed operation's fixed
target without changing that operation's record.

Active completion charges compare the expected record under the lifecycle lock.
The selected family must match the attempt, consumer, staged digest, and fixed
target before a charge. Local verification and the exact target check precede
old-family retirement. Private-family removal shares the lifecycle cleanup
primitive, including the ownership check, admission checks, lease acquisition,
and revocation barrier; the certificate remains recorded until cleanup succeeds.

## Local evidence

`crates/daemon/tests/search_replacement/recovery.rs` exercises:

- Positive-lag catch-up with an independent five-class occurrence, revision,
  span, tombstone, and byte ledger, plus a suppressed-slice negative control.
- Complete search-state removal after confirmed outbox pruning.
- All five identity mismatches and corruption of each source class.
- Pending restart and one real dispatcher/JobTable fixture result alongside
  remaining Pending work. The deterministic engine is test evidence, not
  approval of a production model.
- Explicit Disabled recovery with unfinished and completed construction,
  both registered-pending and deregistered predecessors, and rejected missing
  authorization and stale evidence.
- Explicit recovery from a disabled follow-up above a live selection, with the
  follow-up consumer both unregistered and registered, including the refusal of
  a different consumer while the registered one is unfinished.
- Three consecutive aborted recoveries that keep one prior handoff level and a
  fixed record size, then complete.
- Recovery requests refused as `DeadlineExpired` or `AllowanceExhausted` with
  the disabled record unchanged.
- Construction refusing to adopt a registered consumer the intent does not
  inherit (`crates/daemon/tests/search_replacement.rs`).
- Real child-process cuts at pinned construction, partial selection, final
  acknowledgement, and Current publication, including the rename and directory
  sync boundaries. Every case reopens twice and checks the retained envelope.
- Interrupted export/catch-up and same-lineage restore from fresh authority.
- A moving-tip negative control that preserves the old consumer and fixed
  target rather than acknowledging a newer tip.
- Historical-completion observation with unchanged accounting and no writes,
  including a legitimately extended local prefix and expired-budget, stale-gate,
  and wrong-selector refusals. The historical fixture shifts only timestamps
  after real construction completes; it does not depend on a fast build or sleep.
- A raced operation with no debit to the replacement record, immutable binding
  and local-prefix failures before retirement, and cleanup revocation and changed
  hold failures that retain owned files and capture. Process-cut tests assert
  exact consumed counts and read-only replay after Current.

These tests assume a finite admitted inventory, retained canonical source
bytes, healthy dependencies, and a quiet completion window. Bounds are explicit
fixture gates. Synchronous filesystem calls can outlive cancellation if the
filesystem stops responding. Process kill is not power-loss evidence.

## External acceptance cells remain unmet

This branch is not production enablement or release approval.

| Cell | Status and owner handoff |
| --- | --- |
| Real-engine FTS query after Current | Unmet. No production FTS implementation or virtual FTS table exists in the inspected Rust source. SELECT/LIKE checks do not substitute for lexical execution. The lexical owner must integrate and run the bounded real-engine fixture. |
| Exact-lookup proof invalidation after restart | Unmet. No production exact-lookup coverage-proof seam exists in the inspected Rust source. The lookup owner must bind its proof lifetime to fresh inventory coverage and test invalidation across these cuts. No parallel proof mechanism is introduced. |
| Lexical frequency evidence | Externally deferred to the lexical work in issues #366/#392; not claimed by this coordinator. |
| Numeric, resource, high-water, corpus/protocol, and both-harness approval | Unmet here. RP2.9 owns these approvals, including approval of the live decoded-heap observer. Logical charges and fixture evidence are not high-water measurements. |
| Parent prerequisite receipts | External. Migration, Stage 1, source freeze, and fresh-host access from both harnesses remain coordinator-owned acceptance evidence. |
| Release chain and release gate | External. The release owner must supply the owner-hosted `1.0.0-rc.1` chain under `rc` and reconcile the absent root `release:check` script. |
| Independent pre-publication reviews | Six independent review lenses ran: simplicity, complexity, test strategy, Rust design, durable writes, and bounded resources. Verified findings were addressed, and focused checks were rerun. These reviews do not approve the external acceptance cells above. |

The implementation request is issue #388; its parent contract is issue #347.
