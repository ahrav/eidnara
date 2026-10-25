# p5dep-decision-page-returns-an-oversize-decision-alone

HEAD `3a3b916b7`. Surface S3.

## Discovery trigger

Plan OQ5-1 considers paging summaries under a budget. The task names the
claim "a single oversize decision is returned alone". Doc at
`crates/kernel/src/slice/read.rs:121`. Lens: resource boundaries; C7; D2.

## Evidence trail

- `slice/read.rs:119-125` doc: at most `max_rows` rows examined live or
  invalidated; live ones returned with registry rows as at `requested`;
  stops before a live decision whose payload would pass `max_payload_bytes`;
  "a single decision larger than the bound is returned alone"; `next` is the
  last examined id while decisions remain.
- `slice/read.rs:126-133` signature `(requested, after: Option<&str>,
  max_rows: NonZeroUsize, max_payload_bytes: NonZeroU64, budget:
  &EvalBudget) -> Result<DecisionPage, KernelError>`.
- `:139-158` key query: `SELECT object_id, length(decision_payload),
  invalidated_commit_seq IS NULL OR ?1<invalidated_commit_seq FROM decisions
  WHERE created_commit_seq<=?1 AND object_id>?2 ORDER BY object_id LIMIT ?3`
  with `?3 = max_rows + 1` (to detect "more remain"). No `domain_id`, no
  `scope_id` predicate.
- `:159-170` the loop over `keys.iter().take(max_rows)`: for a live row, if
  `!live.is_empty() && bytes + size > max_payload_bytes` break; else add. The
  `!live.is_empty()` guard is the "returned alone" rule.
- `:172` `next = (examined < keys.len()).then(|| keys[examined - 1].0)`;
  comment: the byte cut never stops before the first live row and `max_rows`
  is nonzero, so `examined >= 1` whenever rows remain.
- `:173-214` loads decisions, object states, folds; `CorruptCanonicalRow` if
  counts disagree; sorts by `object_id`.
- Test `crates/kernel/tests/kernel_slice.rs:1600-1760+`: 5 decisions, 2
  retired; `rows(3)` examines 3 and returns 1 live with `next =
  decision-object-3`; `one = 1 byte` bound returns `decision-object-1` alone;
  twin bounds `both - 1` and `both` separate a cut before and after the second
  live row; a page of only invalidated rows returns nothing live and moves the
  cursor; exactly `max_rows` remaining ends without a cursor.
- Consumers at HEAD: only tests (grep across `crates/`).

## Failure scenario

Without the "alone" rule, a decision whose payload exceeds the bound would
never be returned and the cursor would stall on it (or skip it silently),
losing a live decision from any page-based enumeration. For P5, a page
enumeration with a byte bound smaller than one summary would otherwise never
reach later decisions.

Store-wide scope: for a store with many projects and domains, paging to one
project's `memory` decisions examines every row in `object_id` order; with
`max_rows` per call, the number of calls grows with the whole store.

## Timing windows and dependencies

Reader acquisition under the budget (see the budget record); one transaction
per page.

## What a test must construct

- A decision with payload > `max_payload_bytes`, first in a page; assert it is
  returned alone with the next cursor after it.
- A page whose first live row fits and whose second does not; assert the cut.
- For P5: decisions in two domains and two scopes interleaved by `object_id`;
  assert the page contains both (demonstrating the absence of a filter).

## Investigation log

### Q: D2. Is a domain- and scope-filtered page needed for P5?

- Sources examined: `slice/read.rs:139-146`; `admission.rs:3472-3478` (the
  domain predicate the served read uses).
- Findings: the page API has no filter; the served read has the filter but no
  bound. Neither alone meets OQ5-1.
- Missing evidence: the P5 author's choice.
- Conclusion: needs human input.
