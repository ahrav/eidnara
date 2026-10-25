# p5dep-claim-fact-bounds-are-refused-before-any-row-is-read

HEAD `3a3b916b7`. Surface S3.

## Discovery trigger

Plan R2: summaries "through `claim_facts_at_within_budget`
(`crates/kernel/src/claim_facts.rs:230-247`), bounded by `TooManyClaims`".
The task names the claim "bounds refused before rows are read". Also the
plan-versus-code disagreement D1: `ClaimFacts` carries no summary text.
Lenses: resource boundaries; protocol contracts.

## Evidence trail

- `claim_facts.rs:38` `MAX_CLAIM_OBJECT_ID_BYTES = 512`.
- `claim_facts.rs:41-44` `ClaimFactBounds { max_claims: NonZeroUsize,
  max_causal_payload_bytes: NonZeroU64 }`.
- `claim_facts.rs:46-60` `ClaimFactsError::{TooManyClaims, DuplicateClaim,
  NotADecision, MalformedRequiredField, IncarnationMismatch, Kernel}`.
- `claim_facts.rs:230-247` `claim_facts_at_within_budget`: `let limit =
  budget.acquire_limit(); limit.run(|| self.claim_facts_inner(..))`.
  `AcquireLimit::run` (`open.rs:1543-1549`) calls `check()` first, so an
  exhausted budget returns `Deadline` before `claim_facts_inner`.
- `claim_facts.rs:249-285` `claim_facts_inner`: `check_claim_bounds` at
  `:258`, `requested < 0` at `:259`, `reader_with_limit` at `:261` (first
  reader acquisition), `IncarnationMismatch` at `:268-270` under the reader
  guard, `load_claims_in_tx` at `:271`, final `limit.check()` at `:274`.
- `claim_facts.rs:287-304` `check_claim_bounds`: `len > max_claims` ->
  `TooManyClaims`; empty or > 512-byte id -> `InvalidInput`; duplicate ->
  `DuplicateClaim`. No store access.
- `claim_facts.rs:146-156` `ClaimFacts { object, decision: ClaimDecisionFacts,
  own_admission, lineage_admission, served: ServedStanding, occurrences,
  excluded_representations, causality, causal_record }`.
- `claim_facts.rs:90-97` `ClaimDecisionFacts { decision_id, decision_kind,
  proposition_id, scope_id, anchor_id, evidence_id }`: identifiers only.
- `crates/kernel/src/slice/mod.rs:25-28` `DecisionPayload { summary,
  rationale }` lives on `DecisionRow`, returned by
  `decisions_for_objects_as_of[_within_budget]` (`slice/read.rs:221-243`)
  and inside `PagedDecision.decision` from `decision_page_within_budget`.
- Tests: `crates/kernel/tests/kernel_claim_facts.rs:466-540+`
  (`TooManyClaims` with two ids under `max_claims = 1`, `DuplicateClaim`,
  `NotADecision`, `FutureSnapshot`, oversized causal payload);
  `crates/retrieval/tests/claims.rs:993-1014` exhausted budget before read.
- `test-support` counter: `controlled_acquisitions_for_test`
  (`open.rs:1650-1652`) increments per limited acquisition, so "zero
  acquisitions before refusal" is observable.

## Failure scenario

If `check_claim_bounds` moved after `reader_with_limit`, an oversized request
would take a pooled reader and, under a busy pool, wait up to the budget
before refusing; the refusal would cost a reader slot. For P5 this matters
less than D1: the plan's data path for summaries through this API does not
exist, so a P5 design built on it would compile against a type with no
`summary` field.

## Timing windows and dependencies

Reader acquisition is the only wait; the bound check precedes it.

## What a test must construct

- `max_claims + 1` distinct ids; assert `TooManyClaims` and
  `controlled_acquisitions_for_test` unchanged.
- A budget with `cancel()` called before the call; assert
  `Kernel(Deadline)` and no acquisition.
- A 513-byte id; assert `InvalidInput`.

## Investigation log

### Q: D1. Which API supplies decision summary text for P5?

- Sources examined: `claim_facts.rs:85-156`, `slice/mod.rs:25-28`,
  `slice/read.rs:126-243`, `kernel_routes/read.rs:231-242`.
- Findings: `ClaimFacts` has no text. `DecisionRow.payload.summary` is
  returned by `decisions_for_objects_as_of` (unbudgeted),
  `decisions_for_objects_as_of_within_budget` (reader-wait budget), and
  `decision_page_within_budget` (reader-wait budget, store-wide page).
  `claim_facts` does supply `ServedStanding` and `ClaimDecisionFacts.scope_id`,
  which can filter live, served decisions before summaries are fetched.
- Missing evidence: the P5 author's intended composition.
- Conclusion: needs human input. The plan's R2 sentence is contradicted by
  the code; the code is the fact.
