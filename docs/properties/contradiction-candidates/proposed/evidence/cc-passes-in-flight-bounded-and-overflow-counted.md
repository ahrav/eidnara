# cc-passes-in-flight-bounded-and-overflow-counted

Verified at the #1167 branch tip (base `39a2eb3f9`).

## Discovery trigger

Passes hold Kernel readers and up to 4 MiB of summaries; an unbounded number
would grow with the publication rate.

## Evidence trail

- `crates/daemon/src/contradiction_candidates/pass.rs:112` `Candidates::offer`
  takes the store's one permit with `try_acquire_owned`; a busy permit counts
  `refused_pass_busy` and drops the publication, releasing its charge.
- `crates/daemon/src/contradiction_candidates/pass.rs:136` `run` raises the
  budget interrupt on cancel and awaits the blocking closure, so shutdown's
  tracker wait covers the pass.
- `crates/daemon/src/contradiction_candidates/pass.rs:165` `finish` catches a
  panic, counts `worker_panicked`, and records the end before `ActivePass`
  releases the permit and `active`.

## Failure scenario

A queue or a second permit would let a burst of publications start one pass
each; a lost permit on panic would stop every later pass.

## Timing windows and dependencies

A publication refused busy gets no rows until a later publication carries the
same descriptors; rows are derived.

## What a test must construct

A parked pass and a second publication; a cancel at a descriptor; a panicking
pass hook; shutdown while a pass is parked; all in
`crates/daemon/src/candidate_pass_tests.rs`.

## Investigation log

### Q: P and its scope (OQ5-11)

- Sources examined: the evidence trail above.
- Findings: one permit on `Candidates`, one `Candidates` per handler and store.
- Missing evidence: none.
- Conclusion: resolved with answer.
