# kd-p11-model-free-delivery

## Discovery trigger

Proposed obligation KD-P11 of #1060's Property Catalog section B (KTD5).
Ticket #1064; PR #1071. Surface: plugin, Pi, and daemon.

## Evidence trail

All references are verified at `a15da13a4`, the head of #1071, which holds
#1068 to #1071.

- `packages/e2e-tests/tests/pi-rust-delivery.test.ts:109`: each turn adds
  exactly one provider request (`:160`), Pi's compaction adds two (`:196`),
  and the fixture reports `historySummarizerLive() === false` and
  `backendCounters().started === 0` (`:214-215`).
- `packages/e2e-tests/tests/rust-native-delivery.test.ts:13`: one main request
  per turn (`:48`), at most one title request (`:67`), zero starts (`:69`).
- `packages/e2e-tests/tests/pi-smoke.test.ts:58`: zero starts (`:82-83`).
- The only `runContextResearcher` caller is OpenCode's command handler
  (`packages/opencode-plugin/src/hooks/context/command-handler.ts:342`); Pi's
  ContextResearcher entry is `/eidnara-aug`
  (`packages/pi-plugin/src/commands/eidnara-aug.ts`, registered at
  `packages/pi-plugin/src/index.ts:508`).
- `historySummarizerLive` reports only a firing in flight; the start
  counter is the cumulative oracle.
- The request oracle counts per class: one request per completed turn,
  plus the host's own compaction and title requests. A bare "requests equal
  turns" equality fails on correct code at the Pi compaction (`:196`) and
  on OpenCode's title request (`:67`).

## Failure scenario

A context hook that called a model per turn would add a request per turn
and a cost the user did not configure.

## Timing windows and dependencies

The end-to-end sessions run at low pressure; the 96 percent summarizer gate
is FA-N05's daemon witness (#1068).

## What a test must construct

A stored memory; several turns; one Pi compaction.

## Investigation log

### Q: Is the no-summarizer check vacuous at low pressure?

- Sources examined: the tests above; FA-N05.
- Findings: At low pressure the summarizer would not fire either way; the
  end-to-end check proves delivery adds no call. The pressure case is
  FA-N05's.
- Missing evidence: None.
- Conclusion: resolved with answer.
