# Portfolio evaluation: fold authority (M1 and M2)

Evaluator: an independent evaluator on a different model vendor, fresh
context, METHOD step 5. Tree: `fold-authority/m2-exit` at `d7b330113`.
Scope: all 28 records, the existing-check inventory, the fault map, and the
28 evidence files, with the specification (#834 body and comments) and
targeted source inspection. This is a portfolio evaluation, not a
test-adequacy or merge verdict. Pure line-number drift is excluded; a
separate mechanical pass corrected it. The disposition of every finding is
in the last section.

References to `catalog.md`, `fault-map.md`, and `evidence/` are relative to
this directory. The evaluation read `d7b330113`; its `file:line` references
were carried to `0ff62b29a` (#859 PR C) through the diffs of #859 PR B
(`b45416ac0`) and #859 PR C (`bdf564e3a` through `78fc312db`), so
they cite `0ff62b29a`. The findings table records what the evaluator saw at
`d7b330113`; the disposition table records what holds at `0ff62b29a`.

## Findings

| ID | Lens | Finding with evidence | Classification |
| --- | --- | --- | --- |
| H1 | Harness fit | FA-N04 has an explicit bound, so `liveness` with `always-or-unreached` is admissible, but its recovery schedule mixes uninterrupted completion with death between commits, and calls `MAX_CAS_RETRIES = 8` an attempt budget while the loop starts at zero and permits a final attempt after eight retries (`crates/daemon/src/transform.rs:2019-2027`, `:2045-2064`, `:2110-2115`). | refinement |
| H2 | Implementability | FA-FM17 assumes `Emergency95` under stored native authority reaches the second preparation caller. Native preparation returns `Complete` (`crates/daemon/src/lib.rs:9433-9437`), which settles in the first unit regardless of pressure (`:8918-8942`); the second caller follows a completed `Busy` wait (`:8957-8974`, `:9079-9082`). High usage alone cannot construct the stated window. | refinement |
| H3 | Harness fit | Some occurrence markers depend on the verdict they precede: FA-FM03 requires the loader to report `Unresolved`; FA-FM34 requires commits with the expected authority and selection count. FA-FM09 also requires a changed authority and first file, which excludes its cited unchanged-proposal control (`packages/cli/src/commands/setup-opencode-authority.test.ts:439-462`). | refinement |
| I1 | Implementability | FA-N09 places its invariant at every `promote_derived` call, but correct code also receives stale or revoked proposals and rejects them (`crates/daemon/src/lib.rs:2152-2159`); its own witness calls promotion with older versions and unknown or removed generations (`:21740-21748`, `:21780-21792`). | refinement |
| I2 | Implementability | FA-N11's check makes missing parts unconditionally invalid and matching parts evaluable. A bust rebuilds valid parts even when prior parts are missing; an invalidated baseline stays invalid on a non-bust despite matching parts; prefix disagreement also invalidates (`crates/daemon/src/tail_hygiene.rs:1097-1142`, constructed at `:2327-2349`). | refinement |
| I3 | Coverage balance | FA-N13 is `Exercised: yes` while its atomic-admission clause lacks the over-budget success-path witness: the oracle stops at assembly (`crates/daemon/src/history_summarizer_chunk.rs:1835-1884`), and the handler test covers only the indivisible refusal (`crates/daemon/src/lib.rs:36290-36314`). FA-E04 requires a mixed system and noise range while its oracle uses plain text messages (`history_summarizer_chunk.rs:1749-1760`). | refinement |
| I4 | Implementability | FA-E08's reachability label rests on the folding gate, not on construction of its known-empty retained state; its evidence leaves that production state unresolved, and the unit test supplies `Some(&[])` directly (`crates/daemon/src/transform.rs:13345-13349`). | refinement |
| C1 | Coverage balance | FA-N14's universal bound is contradicted by its evidence, rather than merely lacking a larger test. The bound table excludes both `Unbounded` fields from its sum and assigns an unenforced 12 KiB allowance to `history_summarizer` (`crates/daemon/src/transform_meta_bound.rs:316`, `:323`, `:363`, `:374-390` at `d7b330113`; those entries are gone at `0ff62b29a`); covered system contents accumulate without a byte cap (`crates/daemon/src/transform.rs:6318-6341` at `d7b330113`). The relationship map claimed metadata stays under 128 KiB. | gap |
| C2 | Wildcard | The final M2 acceptance criterion (affected catalogs updated, #824 amendments recorded) has no closure record, and WP-P25 and WP-P06 in `docs/properties/window-protocol/catalog.md` remain active and require window pruning, the opposite of FA-N12's retained omitted rows. | gap |
| C3 | Coverage balance | FA-N12 names snapshot consistency and deltas but omits quantitative requested-read bounds, explicit deletion postconditions, and the re-adoption-counter clause. The completed, provisional, completed test asserts final identities only (`crates/daemon/src/transform.rs:14628-14669`); the counter assertion belongs to a separate completed-tail edit (`:14680-14694`). | gap |
| C4 | Coverage balance | The specification's M2 measurements ask for transaction duration, connection wait, aborts, and retries across delta writes, receipt maintenance, reset, and descent. FA-N12 records only delta-commit and snapshot-read timings; the driver reports aggregate elapsed times (`crates/memory-store/src/lib.rs:33789-33876`). | gap |
| C5 | Harness fit | Several required constructions remain unowned: the authority reset and pass crash cut, a firing across route teardown, TUI startup under `warn`, and the first-commit metadata refusal. The derived-state SIGKILL test covers a different boundary (`crates/daemon/tests/derived_state_crash_cut.rs:102-119`). | gap |
| W1 | Wildcard | The fixed 28-record framing places surviving obligations inside wholly `invalidated` records, for example native output preservation in FA-E01 and command and tool gating in FA-E02. Filtering work by `Status: active` loses these clauses. | bias |
| W2 | Wildcard | Evidence leans toward directly constructed internal states: budget-edge tests use a 30,000-character fingerprint (`crates/daemon/src/history_summarizer_chunk.rs:1776-1782`), and the metadata matrix seeds summarizer state for an active firing even under native authority, where the retained selection is empty (`crates/daemon/src/transform_meta_bound.rs:1195-1221`, `:1239-1251`, `:1284-1285`). These are useful stress inputs with a different reachability claim from production-generated states. | bias |

## Verification note

The evaluator read METHOD, every record, and every evidence file; located
named tests with `git grep`; and inspected the witnesses of eleven
`Exercised: yes` records (FA-N02, FA-N03, FA-N09, FA-N10, FA-N11, FA-N12,
FA-N13, FA-E04, FA-E07, FA-E08, FA-E14). Each constructs the state its record
claims, with the exceptions recorded as I3 and I4. The surviving clauses of
four invalidated records hold in the code: FA-E01 serves m0 and m1 and then
ingress (`crates/daemon/src/transform.rs:3064-3075`); FA-E04 keeps range
identities and the missing-identity refusal
(`crates/daemon/src/history_summarizer_chunk.rs:119-150`); FA-E13 keeps
file-arm flag enforcement and resolved-value precedence
(`packages/opencode-plugin/src/shared/conflict-detector.ts:106`,
`:350-363`); FA-E14 omits an absent observation
(`packages/opencode-plugin/src/plugin/rpc-handlers.ts:542-545`, `:789-792`).
FA-N05's `unreachable` names code locations, which METHOD allows. All 39
fault-map rows were read: no marker asks for the forbidden outcome; H2 and H3
are reachability and independence problems. No tests were executed; the
historical gate results are supplied by the PR descriptions.

## Disposition

| Finding | Disposition |
| --- | --- |
| H1 | Applied: FA-N04's Check splits (a) fault-free recovery, which completes at one invocation's exit within at most nine attempts (the counter runs 0 through 8 under `MAX_CAS_RETRIES = 8`, `crates/daemon/src/transform.rs:2019`, `:2027`, `:2113`) and one authority reset, from (b) the post-kill obligation, one reopened pass without a second reset. `Exercised: partial` names (b) as the missing half. |
| H2 | Applied: FA-N05 and FA-FM17 describe the native high-pressure first-unit state. No witness exists: the one native handler test runs at 90 percent (`crates/daemon/src/lib.rs:24445-24447`). The rerun caller follows only a completed `Busy` wait, which native authority never produces, so its coverage is recorded as structural through the shared gate. |
| H3 | Applied: FA-FM03 asserts the raw rejected construct and the prior admitted load; FA-FM34 asserts the seeded N, W, authority intent, and firing state, and leaves commit results to the oracle. FA-FM09 is now the changed-proposal case and FA-FM40 the already-matching control (FA-FM01 to FA-FM09 had no free number); the intro range then read `FA-FM01` to `FA-FM40` and now reads `FA-FM01` to `FA-FM42`. |
| I1 | Applied: FA-N09's Check holds on successful installation (provenance and ordering); a rejected promotion leaves the retained state unchanged or absent; an unaccepted transform promotes nothing, and an accepted commit whose boundary read fails still promotes. |
| I2 | Applied: FA-N11's Check lists bust or fresh refresh, sticky invalidation on a non-bust, key join, and prefix comparison in order, and states that matching keys permit evaluation without guaranteeing validity; its Guarantee is qualified. FA-E07's Guarantee carries the excluded-prefix qualification. |
| I3 | Applied at `d7b330113`: FA-N13 and FA-E04 were `Exercised: partial`, naming the missing shrink, reserve, and publish construction and the mixed system and noise range. Closed by #859 PR C: `an_over_budget_history_reserves_and_publishes_the_longest_fitting_prefix` (`crates/daemon/src/lib.rs:36318`) reserves and publishes a chunk the identity budget shrank, with identities a real transform derived, and `a_generated_history_fires_the_longest_whole_block_prefix_within_the_identity_budget` (`crates/daemon/src/history_summarizer_chunk.rs:2169`) asserts the exact selection over generated ranges with system and noise turns. FA-N13 is `Exercised: yes`; FA-E04 stays `Exercised: partial` because the proptest places a mixed range only probabilistically and counts no such case. G3 is closed. |
| I4 | Applied: FA-E08's Reachability and Exercised fields separate verified helper-state exercise from unresolved production reachability; the open question stays. |
| C1 | Applied at `d7b330113`: FA-N14 stayed partial with the owner decision open. Decided and implemented: the owner moved covered system messages out of `meta` into `covered_system_messages` rows and capped legacy history segments at 512 (#859 PR B, `covered_systems_grow_in_m0_while_the_stored_meta_stays_fixed`, `crates/daemon/src/transform.rs:26398`; `state_sync_refuses_a_result_over_the_legacy_segment_cap`, `crates/memory-store/src/lib.rs:25100`), and #859 PR C replaced the declared 12 KiB summarizer allowance with an exhaustive, enforced inventory (`every_history_summarizer_field_has_an_enforced_bound`, `crates/daemon/src/transform_meta_bound.rs:588`) and asserts the sum with the selection is at most 384 KiB (`:739-743`). The contradiction C1 named is closed. FA-N14 stayed `partial` for the worst-case composite commit until `e02b22383`. A store-side abandon path that stored `last_failure` uncut was found before `6267f66d4`; `6267f66d4` (#859 PR C) closed it, witnessed by `an_abandon_keeps_a_failure_detail_within_its_serialized_bound` (`crates/memory-store/src/lib.rs:25850`). A later design review found that summarizer details, the task-list setter, and the synthetic task-list pair checked raw values that redaction could lengthen past their bounds; `6fc85742f` (#859 PR C) checks the redacted forms, with a secret-bearing test for each (FA-FM42). `186f3c076` moves final shaping into the store, so a substituted secret earns its receipt and every prefix is stable under both redactors, bounds `last_recut` at 1,024 bytes, and checks the seeded pair and acknowledged watermarks in their stored form; `e02b22383` commits the composite record (`composition_witness_a_meta_with_every_field_near_its_bound_commits_and_reloads_within_the_total`, `crates/daemon/src/transform_meta_bound.rs:978`), a synthetic stress state. A later design review found the recorded `last_render_config` bound, four request inputs plus 256 bytes, undercounted the render identity; `78fc312db` (#859 PR C) charges all five inputs plus 512 framing bytes and measures a real pass's stored identity (`a_render_identity_from_five_escaped_inputs_at_their_bound_fits_its_allowance`, `crates/daemon/src/transform_meta_bound.rs:1342`). FA-N14 is `Exercised: yes`, and G1 is closed. |
| C2 | Applied: `prune_block_identities` has no code reference at `d7b330113` or `0ff62b29a`. In `docs/properties/window-protocol/catalog.md`, #905's own reconciliation invalidates WP-P25 and rewrites WP-P06 and WP-P16 around the withdrawal record; WP-P25 names WP-P06 as successor for the publication fence, and this catalog's relationship map links FA-N12 as the identity-row retention that replaced the prune. Queued: G2. |
| C3 | Queued: G4. |
| C4 | Queued: G5. |
| C5 | Queued: G6. |
| W1 | Surfaced: bias 1 below. |
| W2 | Surfaced: bias 2 below. Applied: the Exercised text of FA-N13, FA-E04, and FA-N14 labels the budget-edge fixtures and the native plus active-firing matrix cell as synthetic stress states. Reduced by #859 PR C: FA-N13's reserve and publish witness and FA-E04's proptest do not use the 30,000-character fingerprints, and FA-N13's handler test derives its identities from a real transform. Extended by `e02b22383`: FA-N14's composite witness commits a directly constructed record with every field near its bound, and FA-N14's Exercised text labels it a synthetic stress state. |

## Gaps queued

These are proposed queue entries, not tracker items.

- G1 - FA-N14 bound closure. Closed by #859 PR B and PR C: distinct
  covered system contents (now rows outside `meta`), the legacy segment
  count (capped at 512), and an enforced byte bound for each field, with the
  owner's decision in place of a narrowed contract; the store's abandon path
  cuts `last_failure` since `6267f66d4`
  (`an_abandon_keeps_a_failure_detail_within_its_serialized_bound`), and
  details, the task-list setter and capture, and the synthetic pair meet
  their bounds after redaction since `6fc85742f`, under both redactors and
  shaped by the store since `186f3c076`. The worst-case composite commit
  exists since `e02b22383`
  (`composition_witness_a_meta_with_every_field_near_its_bound_commits_and_reloads_within_the_total`).
  G1 is closed.
- G2 - M2 acceptance bookkeeping: the window-protocol records WP-P06, WP-P16,
  and WP-P25 are reconciled by #905; the #824 amendment receipt is
  pending owner approval.
- G3 - Selection lifecycle: a production-shaped identity-byte shrink,
  reserved and published (FA-N13); a mixed system and noise range for
  FA-E04. Closed by #859 PR C (see I3).
- G4 - FA-N12 sub-obligations: read-work bounds, deletion isolation, and
  counter semantics. Reuse
  `requested_identity_reads_do_not_grow_with_the_identity_table`
  (`crates/memory-store/src/lib.rs:23944`) and
  `delete_session_sweeps_every_session_table_once_and_scopes_notes`
  (`crates/memory-store/src/lib.rs:21223`).
- G5 - M2 measurement evidence: per-operation transaction duration,
  connection wait, aborts, and retries.
- G6 - Remaining boundary witnesses: the authority reset and pass crash
  cut, a firing across route teardown, TUI startup under `warn`, and the
  first-commit refusal.

## Biases for a human

1. W1 - The fixed 28-record framing keeps surviving obligations inside
   wholly `invalidated` records, such as FA-E01's native output clause and
   FA-E02's command and tool gating. Decide whether those clauses stay
   there or move into active successor records, so that filtering by
   `Status: active` does not drop them.
2. W2 - Much of the evidence uses synthetic stress states: 30,000-character
   fingerprints at the budget edge and summarizer state seeded for an active
   firing under native authority, with an empty selection. They are useful
   stress inputs, but their reachability claim
   differs from production-generated states.
3. C1 - Accepting uncapped content-bounded fields in FA-N14 would have
   changed #834's constraint C3. The owner decided instead to move covered
   system messages into rows and cap legacy segments (#859 PR B); C3 stands
   unchanged.
