# fa-n08-repair-uses-the-proposed-document

## Discovery trigger

Proposed record FA-N08 of #834 (D2, D2b; C1; user stories 1 to 3 and 8).
Ticket #854 implements it; PR #900 lands it on top of #899's disposition and
patch. Surface: cli.

Exercised status: yes - 21 command-level setup tests (the merged #900 holds
21; its description counts 18) and three doctor tests construct every
required fault and ran green in #900's and #904's `bun run check:repo` gates.

## Evidence trail

Package references are verified at `d7b330113`, the package tree of #859
PR C's head; no file under `packages/` changes after it. Between #904 (`57a820bb6`) and `d7b330113` the package tree takes the
review changes merged through `main` (fold-authority warning delivery and
reconciliation); `d7b330113` itself changes only comments and generated TUI
source, and package contents are equal across the three #859 slices.

- Order in `runSetup` (`packages/cli/src/commands/setup-opencode.ts:486`):
  the picker runs first; `proposeEidnaraConfig` (`:290`) builds the text;
  `loadUserTierConfigText` validates it through the plugin's user-tier
  pipeline; `foldAuthorityOf` derives the authority (`:603-613`). An
  unresolved proposal or project tier stops setup before any prompt or host
  edit (`:618-628`). DCP consent (`:643-650`), conflict detection, and repair
  consent (`:662-692`) follow. The host target is the authority's patch
  (`EIDNARA_FOLDS_COMPACTION` and `NATIVE_FOLDS_COMPACTION`, `:266-267`;
  selection `:652-657`).
- Consent does not change the authority: a declined repair sets
  `keepNativeCompaction` and writes no compaction key (`:684-690`, `:693`);
  `authority` is never reassigned after `:613`.
- Freshness: setup stops when the Eidnara file changed while a prompt was open
  (`:726-732`) and re-admits the project tier (`:733-742`).
- Writes: a snapshot precedes them (`:743-757`). Under Eidnara folds the
  Eidnara document is written first, under native folds the OpenCode file
  first, with `betweenWrites` between them (`:784-795`). A caught failure
  restores the snapshot and prints the read-back (`:816-834`).
- Re-detection and read-back: after the writes setup re-detects
  (`:835-845`, `reportRemainingConflicts` `:422-440`) and reports "Written,
  restart required" only when `reportReadBack` (`:899-920`, called at `:846-850`) finds the Eidnara
  file equal to the proposal and the OpenCode file holding the target values.
- Doctor: `resolveFoldAuthorityForDoctor`
  (`packages/cli/src/commands/doctor-opencode.ts:79-98`) maps a thrown load or
  an unresolved admission to `unresolved`; doctor then skips detection and
  fails the check (`:522`, `:539-542`). Under native folds a blocked
  disabling repair still runs the `auto = true` repair (`nativeEnablementOnly`,
  `:62-76`; `:560-574`) and re-detects (`:589-597`).

Witnesses (`packages/cli/src/commands/setup-opencode-authority.test.ts`,
#900): fresh native setup (`:136`), native dry run (`:151`), model setup
(`:165`), keep versus remove (`:230`), native repair keeping `prune` (`:221`),
edits during prompts (`:262`, `:276`), blocked host edits (`:323`, `:369`),
declined repairs (`:337`, `:347`), environment-forced `auto = false`
(`:357`), SIGKILLed child between the writes (`:407`, `:424`), thrown failure
between the writes (`:438`, `:465`). Doctor
(`packages/cli/src/commands/doctor-opencode.test.ts`): `:359`, `:398`,
`:441`.

Gate results of the runs cited, as recorded:

- #900 (base `6b3d08e81`, merged head `2effd440f`, merge `38b816f33`):
  `bun run check:repo`, smoke, comment markers pass.
- #904 (branch head `57a820bb6`): `bun run check:repo`, smoke, markers pass.

## Failure scenario

Setup derives the patch from the file on disk rather than the document it is
about to write, so choosing "remove every chain field" leaves
`compaction.auto = false`. Or a write that fails halfway is reported as done,
and the next OpenCode start runs with zero or two fold authorities.

## Timing windows and dependencies

The window between the two file writes. A thrown failure there rolls back; a
killed process cannot. The write order leaves an Eidnara-folds setup with the
new chain plus the old host setting (a `disable` conflict doctor reports) and
a native setup with `auto = true` plus the old chain.

## What a test must construct

Keep-chain and remove-all picker runs; a declined repair in both directions;
an environment-forced `auto = false`; a thrown failure and a SIGKILL between
the writes; an unloadable proposal and project tier; a doctor run over an
unloadable configuration.

## Investigation log

### Q: Does the landed failure report match D2?

- Sources examined: D2 in #834; #900's "Deviation from the spec text";
  `setup-opencode.ts:816-834`; tests `setup-opencode-authority.test.ts:438`,
  `:465`.
- Findings: D2 says a failure between the writes is reported as "written,
  restart required" with a read-back. Setup rolls back first, prints the
  read-back, and reports "rolled back" or a partial rollback; it prints
  "Written, restart required" only when both files match. #900 records this
  for the owner. The falsifier (a partial write reported as success) holds.
- Missing evidence: Owner acceptance of the deviation.
- Conclusion: needs human input.

### Q: Does #900's description of `betweenWrites` match the code?

- Sources examined: #900's description; `setup-opencode.ts:482-483`,
  `:784-795`; `git log` of the branch.
- Findings: No. The description places the hook after the OpenCode write and
  before the Eidnara write. `e414db3e0` ("Keep a fold authority through
  setup's write window and its read-back"), merged with #900, orders the
  writes by authority, and the hook's doc comment says "regardless of write
  order". Tests `setup-opencode-authority.test.ts:407` and `:424` cover
  both orders.
- Missing evidence: None.
- Conclusion: resolved with answer; the code and tests are authoritative.
