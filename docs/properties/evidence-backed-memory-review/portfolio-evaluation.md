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
| `MEMORY_CLASSES`, `related_memories::CLASSES`, and `decision_derived` enumerated the same class set independently, so a class added to one list alone would be selected without resolving through its decision, or resolve without being selectable | gap | fixed: `related_memories::CLASSES` is `MEMORY_CLASSES`; a `const` assertion in `selection.rs` requires every walked class to be decision-derived; `the_selected_classes_are_exactly_the_decision_derived_classes` checks the reverse direction over every `OccurrenceClass` |

## Private result expiry

| Finding | Class | Disposition |
| --- | --- | --- |
| A reconciliation error returned before Kernel maintenance, so one unparsable pin could starve capture expiry and staging cleanup for up to seven days | gap | fixed: the pass records `healthy = false` and continues into Kernel maintenance; an unparsable owner is skipped rather than failing the listing |
| `in_progress_memory_reviewer_receipts` took a `LIMIT` ordered by run deadline, so overflow would omit the newest receipts and release holds their settlements still needed | gap | fixed: the query is unbounded; every in-progress receipt holds a pending job under the store's own bounds |
| The reconciler's selection question ignored the selection's project digest and store incarnation, so a same-target job elsewhere could keep a hold alive | gap | fixed: both predicates come from the receipt's recorded selection and the live incarnation |
| The selection question was one query per live review hold, and `memory_reviewer_receipts` is indexed by state alone, so each pass walked every complete receipt once per hold for as long as selected results kept their holds | gap | fixed: `selected_memory_reviewer_results` lists once per pass the selected triples of the live holds' candidates, bounded by the hold cap rather than the receipt ledger, and holds are filtered against the set, matching the pending set |
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
| `OBSERVATIONAL_HARNESS` was `pub` with no Rust consumer outside the crate | refinement | reverted: the wire test in `crates/daemon/tests/memory_reviewer_wire.rs` binds through `daemon::OBSERVATIONAL_HARNESS`, so a change to the value cannot leave that test binding an ordinary route; the constant is `pub` |
| The operations document listed `host.status` among the reads an observer makes under project authorization | gap | fixed: `host.status` is a route-free channel-0 operation (wire document 7.6) and is stated as unaffected |
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
| `exactU64` and `exactI64` returned an unsafe integer-valued `number` unchanged, so `formatExactInteger(exactU64(2 ** 60))` printed `1152921504606847000` as exact | gap | fixed: a `number` is a wire integer only when it is a safe integer, so an unsafe double is refused; only a `bigint`, which only an integer lexeme produces, carries a value outside the safe range; pinned with 2^60 |
| A decimal or exponent spelling that evaluates to an unsafe integer (`9007199254740993e0`, `9007199254740993.0`) decoded as the rounded double 2^53, which `exactCount` then accepted as the exact boundary; `-0` passed every domain although serde reads it, like those spellings, as `f64` | gap | fixed: the same safe-integer rule refuses the rounded double, and `-0` is excluded by `Object.is`; pinned in `exact-json.test.ts` and the routed body in `client.test.ts` |
| The reviver returned the rounded double when `JSON.parse` handed it no source text, so a runtime below the engine floor would pass 2^53+1 through `exactCount` as 2^53 | gap | fixed: an integer-valued unsafe double with no lexeme refuses the body; pinned by wrapping `JSON.parse` |

## Review command

| Finding | Class | Disposition |
| --- | --- | --- |
| The absent-connection-file diagnostic keyed on `ENOENT`, which the host client never throws; it wraps the miss as `ConnectionFileError` `not_found` | gap | fixed: keyed on the error's name and code; a `stat_failed` still reports only the error name |
| `--project` was resolved after the connection opened, so a missing path printed the connection-file message | gap | fixed: resolved before connecting, exit 2, tested with the real `realpathSync.native` on relative, symlinked, and subdirectory paths |
| `HostCallError.code` is peer-controlled and reached stderr raw | gap | fixed: `printableLine` on the code; tested with an escape sequence |
| No real-host positive read: a completed receipt with a nonempty-span proposal rendered by `review show` against the hermetic host | gap | kept open: the direct-host fixture has no path that publishes a proposal from TypeScript; the selected read is reached in `memory_reviewer_wire.rs` and decoded from the same wire shape in the unit tests; recorded on `shared-path-fixture-reaches-selected-readable-proposal` for the owner |
| Missing cases: UTF-8 byte caps versus character counts, staged-candidate target, empty limitations, exact-full page then empty follow-up, absent activation state, a stdout that throws, every outcome and reason, timeout and generation change | gap | fixed: added |
| Per-field `jsonInteger` mapping through three shapes | refinement | fixed: one `JSON.stringify` replacer turns every `bigint` into a raw token |
| A hand-written connection interface and a per-request timeout beside the connect-time one | refinement | fixed: `Pick<HostClient, ...>`; the connect-time timeout alone |
| An unreachable `default` terminal text and five exported vocabularies with no importer | refinement | fixed |
| Support and contradictions were bounded on their sum; the Kernel's `check_references` bounds each list at 256 and only the daemon's producer (`steps.rs`, `coordinator.rs`) keeps the sum under 256 before staging | gap | fixed: each list is bounded at 256 as the Kernel admits it; the sum check refused a proposal the Kernel can stage |
| `--json` output passes C1 and bidi characters through `JSON.stringify`'s escaping | bias | kept: JSON output is machine output; the README says so and text output strips them |
| Production size is about 770 lines against a 500 target | bias | kept: the closed vocabularies and the field-by-field decoder are the substance; the ticket's hard maximum is 1,000 and the status command shares the connection and decoding with list and show |

## Cumulative response budgets

| Finding | Class | Disposition |
| --- | --- | --- |
| The frame-count refusal recorded one chunk less than the byte-overflow refusal | gap | fixed: it records the whole remaining bound like every overflow; tested |
| A daemon `ResponseAllowance` mirrored the ledger's field for field with a conversion at the seam | refinement | fixed: the ledger type is the one type; the sender clamps to its own constants |
| `ResponseUsage::UNKNOWN` duplicated `Default` | refinement | fixed |
| No test cancelled mid-body, no test asserted the bytes read before a timeout, no accounting on compressed and undecodable refusals, no exhaustion after an unterminated row, no nonzero usage across reopen | gap | fixed: added at the disclosure, sender, and ledger seams |
| The ledger's usage bounds check repeats the schema `CHECK` as a typed refusal | bias | kept: the typed `InvalidRequest` is the contract the caller sees; production callers cannot exceed it |
| Two const assertions pin the sender's constants to the ledger's | bias | kept: the sender's constants are the wire document's per-response bounds; the assertion states the coupling where it is relied on |
| The raw budget is clamped in the sender and the text budget only in the decoder | bias | kept: each bound is clamped where it is enforced; `decode_message_within` is the public entry point that owns the text contract |
| The ledger tests that committed successive markers without terminals now close each attempt first | refinement | fixed: matches what a live run does; the unterminated case is exercised on purpose in the new tests |
| No timing or memory measurement of the padded-response path | bias | kept: the byte counts the tests assert are the resource evidence; no latency or quality threshold is claimed, per the ticket |

## Gaps queued

- A production-class positive witness cannot be constructed until an owner
  supplies a Remote-eligible decision representation. Queued under the owner of
  the policy-validating broker.
- Class tightening and wrong-scope decisions at the coordinator seam remain
  unconstructed; see `existing-checks.md`.
- A TypeScript-driven positive `review show` against the hermetic host needs
  a fixture control that publishes a proposal with nonempty spans; queued
  under the direct-host fixture owner.
