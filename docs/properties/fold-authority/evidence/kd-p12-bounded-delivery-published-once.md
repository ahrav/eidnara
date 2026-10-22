# kd-p12-bounded-delivery-published-once

## Discovery trigger

Proposed obligation KD-P12 of #1060's Property Catalog section B (KTD5,
KTD6, R11, R12). Tickets #1061 and #1064; PRs #1068 and #1071. Surface:
plugin, Pi, and daemon.

## Evidence trail

All references are verified at `07ea883b5`, the head of #1071, which holds
#1068 to #1071.

- `packages/e2e-tests/tests/pi-rust-delivery.test.ts:109`: per turn, m0 is
  `<session-history></session-history>` with one `<project-memory>` block
  (`:163-166`), each memory line appears once in the whole request
  (`:167-169`), and the prompt is last (`:170`); a memory seeded between
  turns two and three appears on turn three; after Pi's compaction the
  summary reaches the next request as often as Pi stored it (`:208-212`).
- `packages/e2e-tests/tests/rust-native-delivery.test.ts:13`: the block once
  per request over three turns; the guidance in the system prompt names the
  tag, so the tag count runs over the messages and the line count over the
  whole body.
- Budget clause: `git diff 08de9051a..07ea883b5` adds no budget constant and
  raises none.

## Failure scenario

Reusing an old m0 after a memory HARD, or splicing a delta into m1 and m0
together, would serve a memory twice.

## Timing windows and dependencies

A memory created between passes.

## What a test must construct

An admitted verified memory; a second memory between turns; a Pi
compaction.

## Investigation log

### Q: Where are the bounds enforced?

- Sources examined: #1060 R11.
- Findings: The kernel read's `memory_budget_tokens`, the user-profile
  budget, the m1 row cap, capture admission, the invocation budget, and the
  daemon's resident reservation; #1068 to #1071 change none of them.
- Missing evidence: None.
- Conclusion: resolved with answer.
