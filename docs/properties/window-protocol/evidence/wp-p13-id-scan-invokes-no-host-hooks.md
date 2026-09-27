# wp-p13-id-scan-invokes-no-host-hooks

## Discovery trigger

Record WP-P13 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. Revision 3 adds a backward id scan over the host array,
including covered slots the capture never walks. TE19's guard covers only
captured data.

Exercised status: yes - "crosses planted proxies, accessors, and revoked
proxies without invoking any hook" runs `scanMessageIds` and `messageIdFilter`
across hostile hops with trap counters at zero, and "declines a matched
boundary that fails the hostile walk instead of picking another" runs a pass
whose matched boundary fails the walk; both ran in the #883 (head `d7712d75`)
and #884 (head `f8734c12`) `bun run check:repo` gates. The hostile
covered-slot traversal is constructed at the primitive level, not inside a
discovery pass (see the investigation log).

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Recorded runs: #883 (head `d7712d75`, tests at `:102` and `:235` there) and
  #884 (head `f8734c12`) `bun run check:repo ok`. `scanMessageIds` reads each
  hop with `readOwnDataProperty`, so a proxy, revoked proxy, or accessor reads
  as no id.
- The hostile test plants a proxy message, a proxy `info`, an accessor `id`,
  an accessor `info`, a revoked message proxy, and a revoked `info` proxy at
  indices 1 to 6, and an accessor on array index 7, all above the one readable
  match at 0; it asserts `scanMessageIds` returns 0 for the target and -1 for
  an absent id, `traps` 0, the filter charged `host.length * 4`, and the
  filter holding only `fnv1a32("target")`.
- The matched-boundary test installs a `parts` getter on the matched message
  and asserts `getterCalls` 0, no second body, one discovery cursor, the host
  unchanged, and `boundary` still `m-6`.
- The transform-edit-responses catalog records TE19 as extended by #832
  (WP-P13) at `f2442b2f`.

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

A plugin or extension installs a proxy on a covered message. A scan that reads
`messages[i].info.id` runs the trap, which can mutate the array or throw
during capture.

## Timing windows and dependencies

None; the scan is synchronous.

## What a test must construct

Plant a proxy, an accessor, and a revoked proxy at each hop between the end
and the match; count traps; assert zero and the correct index; make the
matched message fail the walk and assert a decline without another anchor.

## Investigation log

### Q: Is the hostile traversal constructed through a discovery pass?

- Sources examined: `rust-mode-window.test.ts:123`, `:256`.
- Findings: The traversal is exercised at the primitive level; the pass-level
  test covers a hostile matched boundary, not hostile covered slots during
  discovery.
- Missing evidence: A pass-level discovery run across hostile covered slots;
  #883's Evidence names only the two tests above.
- Conclusion: unresolved, needs a pass-level witness or an explicit owner
  acceptance of the primitive-level witness (needs human input).

### Q: Does the filter retain id strings?

- Sources examined: `rust-mode-window.test.ts:166`.
- Findings: The filter is a `Uint32Array` of hashes with no id strings; an
  unaffordable filter is refused.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; it ran in the #883 and #884 `bun run
  check:repo` gates.
