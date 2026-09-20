# Portfolio evaluation

The records in this part were reconstructed from the specification and the
code, so no lens passes produced them and no fresh-context evaluation of a
discovery portfolio ran. In its place, each change that introduces records is
reviewed before its pull request opens by independent reviewers that have not
seen the design reasoning, through five lenses: over-engineering, complexity,
testing, Rust code review, and Rust design review. This file records what those
reviews found and how each finding was dispositioned.

## Production canonical resolution

| Finding | Class | Disposition |
| --- | --- | --- |
| `Run.subject` was an `Option` that was `None` only before `open`, with an unreachable refusal arm | refinement | fixed: the subject is an owned local returned from the opening block and passed to `bind_proposal` |
| `proposal_target` accepted a decision target whose registry row is not a decision; the coordinator was safe only because `open` ran the broker first | gap | fixed: the target refuses `ExpectationChanged` when the row's `object_kind` is not `decision`; test asserts it |
| Two refusal codes for one malformed-row class inside `resolve_descriptor` | refinement | fixed: a missing identity field refuses `Unsupported` like the sibling decode checks |
| Docs on `resolve_descriptor`, `bind_proposal`, `MEMORY_CLASSES`, and `originating_decision` dropped the contract the old text stated | refinement | fixed: contracts restored naming each refusal and the `Refused`-vs-`Kernel` split |
| `related_memories` decoded the class twice for one row | refinement | fixed: `expectation` calls `originating_decision` with the class it already parsed |
| The new integration test file duplicated the broker fixture | refinement | fixed: the tests moved into `memory_reviewer_broker.rs` and reuse its fixture |
| The `commit_token` oracle used the same formula as the implementation over a decision with one commit | gap | fixed: a second commit appends a decision event so the last change differs from creation |
| No coordinator-level witness exercised `resolve_subject` through `open` for a canonical subject | gap | fixed: `a_canonical_subject_resolves_through_its_decision_and_abstains_for_a_remote_model` |
| The scheduler gate test derived its expectation from the gate constant, so flipping the gate changed nothing | gap | fixed: a constant assertion on the gate plus a fixed expected task list |
| Canonical before/after equality was asserted only on the Git path | gap | fixed: tracked registry rows compared before and after in both broker tests |
| The wrong-kind owner refusal at the broker is `Scope`, not a kind-specific code, because the evidence object is unscoped | bias | kept: the Kernel judges scope before kind; `proposal_target` supplies the kind refusal, and the evidence file records the ordering |
| `PRODUCTION_SELECTION_OPEN` is a compile-time constant rather than an operator switch | bias | kept: opening is a reviewed code change with its own witness, not a deployment action |

## Private result expiry

| Finding | Class | Disposition |
| --- | --- | --- |
| A reconciliation error returned before Kernel maintenance, so one unparsable pin could starve capture expiry and staging cleanup for up to seven days | gap | fixed: the pass records `healthy = false` and continues into Kernel maintenance; an unparsable owner is skipped rather than failing the listing |
| `in_progress_memory_reviewer_receipts` took a `LIMIT` ordered by run deadline, so overflow would omit the newest receipts and release holds their settlements still needed | gap | fixed: the query is unbounded; every in-progress receipt holds a pending job under the store's own bounds |
| `memory_reviewer_result_is_selected` ignored the selection's project digest and store incarnation, so a same-target job elsewhere could keep a hold alive | gap | fixed: both predicates added from the hold's binding |
| Memory Store reads were interleaved with Kernel releases inside one loop | refinement | fixed: orphans are collected first, then released |
| `DeadlineClock` enum encoded one integer floor | refinement | fixed: `load_staged_review` takes the instant directly |
| `read_selected_review_input` carried an unused `now`; `ActiveReviewHold` carried an unread `expires_at` | refinement | fixed: both removed |
| `updated_at_ms` is not covered by the receipts' immutability trigger, so `completed_at_ms` rests on `WHERE state = 'in_progress'` clauses | bias | kept: every write site is gated on the in-progress state today; a trigger or dedicated column would change the Memory Store baseline and belongs with the owner's next baseline change |
| Capture `retain_until` still moves to the review expiry at transfer | bias | kept: a retention floor, not a visibility path; recorded as an open question on the record |
| The selected read's owner check moved from the Kernel to the daemon and had no witness; the reconciler's generation and project scoping had none either | gap | fixed: tests mutate the stored owner and class after selection, orphan a hold through takeover, and query the selection question across digests and generations |
| `completed_at_ms` was asserted only as earlier than the deadline, and no test swept after a completed selection | gap | fixed: exact completion time asserted and re-asserted after a sweep at the queue deadline |
| No test placed the sweep inside the settlement window | gap | fixed: `before_completion_for_test` runs the sweep; the completion is fenced and the settlement releases the hold |
| No test reopens the stores between envelope and sweep | bias | kept: the rows are SQLite writes already committed; a reopen witness is queued under the U1 qualification owner |

## Broker-free recovery

| Finding | Class | Disposition |
| --- | --- | --- |
| A resumed run acquired an empty execution hold when the lost run's hold had been released on an unsettled exit, so revalidation refused an admissible sealed result as `NotCovered` | gap | fixed: adoption extends the execution hold over the payload's disclosed inputs before revalidating; the released-hold window is tested for both the admissible and the retired-input case |
| The resume path's hold acquisition swallowed store errors as an empty hold | gap | fixed: only a Kernel refusal yields an empty hold; a store error returns from `prepare` |
| A resumed run with no hold at all abstained `expectation_changed`, which reads as lineage drift | refinement | fixed: it abstains `budget_exhausted`, the cutoff having passed before retention could be recovered |
| `Verdict::from_hold` judged a backing limit as a durable change while the broker judges it transient | gap | fixed: it maps through `broker::hold_refusal` |
| `settle` and `adopt` shared a copied prologue | refinement | fixed: `Settlement::open` returns the opened state to both |
| `Verdict::from_refusal` ended in a wildcard | refinement | fixed: every code is named |
| A `failed` or `cancelled` marker with no sealed row completes the receipt `unknown` | bias | kept: the ticket admits a new attempt only after a proven `not_dispatched` terminal, and the run's transcript is gone; the terminal literal is an open question on `unknown-dispatch-does-not-authorize-resend` |
| Rows staged before the record existed refuse on read for the rest of their hold | bias | kept: fails closed with no migration, stated on `ReviewStagedRow::dependencies` |
| `Prepared` carries a `resuming` flag beside a sentinel subject the resume path never reads | refinement | kept: an enum would fork `Run` construction for one field; queued with the coordinator's next structural change |
| The selected read inverts `Verdict` back to a `RefusalCode` by hand; `PolicyUnion::decode` walks `Value` by hand | refinement | kept: both are local and exhaustive; a `Refused(code)` verdict and a serde wire type are queued as refinements with no behavior change |
| No Kernel negative case for the staging rules; no negative control on the marker join; no test of a live settlement with no completed marker; no coordinator-level adoption to `Published` | gap | fixed: seven refused staging specs, a record joined to a marker index the ledger lacks, a `Failed`-only marker settlement, and a coordinator run that adopts the row a crashed settlement sealed |
| No adopt case with a `temporary_capture` member under the empty execution hold | bias | kept: the member path is shared with the live read's `TemporaryCapture` expectation; queued in `existing-checks.md` |

## Observational routes

| Finding | Class | Disposition |
| --- | --- | --- |
| The observational-harness comment named a consumer that does not exist in the tree | refinement | fixed: the comment states the mechanism only |
| `OBSERVATIONAL_HARNESS` was `pub` with no Rust consumer outside the crate | refinement | fixed: `pub(crate)` |
| The wire document did not name the `cli` value whose behavior the daemon defines, so the TypeScript consumer had no contract to mirror | gap | fixed: one sentence in the bind section; the value stays a scoping claim, no operation or literal of the protocol changes |
| The worker test's comment claimed observation while its view was hand-built | gap | fixed: the comment states the empty-view claim; the catalog record says the composition is two tests |
| No protocol-3 read through a `cli` route | gap | fixed: `review.list`, `review.read`, and two malformed bodies answer byte-equal through the observer and the ordinary route |
| The empty-view worker construction repeated `worker_for` | refinement | fixed: `worker_with_projects` serves both |
| `values()` consumers in bind and unbind remain unfiltered | bias | kept: session-liveness and note-capability decisions are route-local and must see observers; the security check found no read an observer gains or any scheduling an ordinary route escapes |

## Exact integers at the client seam

| Finding | Class | Disposition |
| --- | --- | --- |
| The number-or-bigint split compared `String(value)` with the lexeme, which keys on shortest-digit printing rather than exactness: 2^60 became a `bigint` and 10^20 stayed a `number`, contradicting the twenty-digit refusal | gap | fixed: the split is `Number.isSafeInteger`; every lexeme outside the safe range is a `bigint` or refused, and the tests pin 2^60 and 10^20 |
| `module-wire.ts` declared its own `I64_MIN` and `U64_MAX` | refinement | fixed: imported from `exact-json` |
| Exports with no caller outside tests | refinement | fixed: `exactIntegerWithin`, `isWireInteger`, and the count and i64 bounds are module-private |
| The bigint-to-number branch in `exactIntegerWithin` was unreachable from the reviver | refinement | fixed: removed |
| No routed body with a lexeme past 64 bits reached a caller as `invalid_response_body` | gap | fixed: asserted with the diagnostics checked for the token |
| A reviver walks the value recursively, so nesting a few thousand levels deep fails as invalid JSON where the plain parse accepted it | bias | kept: the daemon's serializer nests no deeper than 128; recorded in `existing-checks.md` |
| The observer bind in the identity test changes `session`, so the separate cache slot for `consumerIdentity: null` under an identical identity is not shown | bias | kept: `routeOpen` does not cache; the slot question belongs to managed `call`, which does not take the option |
| Every integer lexeme could become a `bigint`, as the decision's wording admits | bias | kept: converting only lexemes outside the safe range preserves every existing consumer's values; recorded on the evidence page |
| Decoding every body exactly made the `transform` recipe refuse as `malformed` whenever an inserted message value carried an integer past 2^53, and the failure was sticky for that session; the values are handed to OpenCode, whose `JSON.stringify` rejects a `bigint` | gap | fixed: exact decoding is a per-request response mode; `hostStatus` and `request(..., { exactIntegers: true })` select it and every other body decodes as `JSON.parse` does; asserted at the host client and on the stream path |
| `exactU64` and `exactI64` returned an unsafe integer-valued `number` unchanged, so `formatExactInteger(exactU64(2 ** 60))` printed `1152921504606847000` as exact | gap | fixed: any in-range value outside the safe range returns as the `bigint`; pinned with 2^60 |
| The reviver returned the rounded double when `JSON.parse` handed it no source text, so a runtime below the engine floor would pass 2^53+1 through `exactCount` as 2^53 | gap | fixed: an integer-valued unsafe double with no lexeme refuses the body; pinned by wrapping `JSON.parse` |

## Gaps queued

- A production-class positive witness cannot be constructed until an owner
  supplies a Remote-eligible decision representation. Queued under the owner of
  the policy-validating broker.
- Class tightening and wrong-scope decisions at the coordinator seam remain
  unconstructed; see `existing-checks.md`.
