# wp-p17-interior-omission-is-constructed

## Discovery trigger

Record WP-P17 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. Ticket #832 pairs WP-P01 with this marker.

Exercised status: yes - the "interior window omission" case of "declines
${name} during the await with no candidate write and no promotion" asserts a
window at boundary index 6, removes an interior window message during the
await, and asserts the decline; it ran in the #883 (head `d7712d75`) and #884
(head `f8734c12`) `bun run check:repo` gates.

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Enabling-state assertions before the mutation: `bodies[1].boundary` equals
  `{ mid: "m-6", sequence: 3 }` and `bodies[1].native_messages` has length 4
  (window `m-6..m-9` of ten).
- Mutation: `output.messages.splice(8, 1)` then `push(message("rebuilt",
  "m-late"))`, keeping length 10.
- Outcome: `output.messages` equals the mutated array, contains no candidate,
  `boundary` stays `m-6`, `chargedBytes` 0.
- The same family runs prefix deletion, same-length reorder, and root
  rebinding as WP-P22's shapes; the interior omission is the only case that
  keeps the host length and the boundary slot unchanged, so a length or head
  check alone would miss it.
- The steady-state test "sends the declared window in both representations and
  publishes the recipe at boundaryIndex + i" (`rust-mode-window.test.ts:182`)
  is the encoder-side control: it asserts `native_messages` equals
  `first.slice(6)` exactly.
- #832 PR one (#883) is the owner (ticket #832 Property and Verification
  Obligations: "a window with boundary index > 0 and an omitted interior tail
  message is constructed and rejected").

Gate results of the runs cited here, as recorded in the PR descriptions:

- #883 (#832 PR one; head `d7712d75`, base
  `window-protocol/m1-daemon-revision-3`): `bun install --frozen-lockfile`,
  `bun run check:repo`, fmt, clippy, markers, fixture build ok;
  fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17 skip, 0 fail
  (no addon_unavailable skips).
- #884 (#832 PR two; head `f8734c12`, base
  `window-protocol/m1-plugin-revision-3`): `bun install --frozen-lockfile`,
  `bun run check:repo`, fmt, clippy, markers, fixture build ok;
  fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17 skip, 0 fail
  (no addon_unavailable skips).

## Failure scenario

Not applicable to a marker; see WP-P01.

## Timing windows and dependencies

During the pending response.

## What a test must construct

Capture a window at a nonzero index, remove an interior window message while
keeping the length, and assert the enabling state before the fault.

## Investigation log

### Q: Does the case test encoder omission or host mutation?

- Sources examined: `rust-mode-window.test.ts:296-299`.
- Findings: Host mutation during the await; the encoder always sends the
  captured copy.
- Missing evidence: None.
- Conclusion: resolved with answer: host mutation; the encoder path is covered
  by the steady-state membership assertion.

### Q: Is the enabling state asserted before the fault?

- Sources examined: `rust-mode-window.test.ts:319-322`.
- Findings: Yes: the in-test comment reads "Enabling state: the window starts
  past index 0 and the host changes during the await" and the two `expect`
  calls precede `mutate(output)`.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; it ran in the #883 and #884 `bun run
  check:repo` gates.
