# Fold authority (M1 and M2)

This part catalogs fold authority and session metadata bounds, as specified
by [#834](https://github.com/ahrav/eidnara/issues/834) and landed by
milestones M1 (tickets #852 to #856) and M2 (tickets #857 to #859). It
carries the 28 records of the specification: FA-E01 to FA-E14 (existing
behavior at `265df096` that the milestones preserve or deliberately
invalidate) from #834's first comment, and FA-N01 to FA-N14 (new
obligations, each with a falsifier) from the specification body. The record
text follows the specification; `Reachability`, `Exercised`, `Confidence`,
and `Existing check` are re-derived here against the landed code and the
recorded PR runs.

## Evidence boundary

Code references are verified at `0ff62b29a`, the code tip of #859 PR C
(`fold-authority/m2-exit-bounds`), unless a record names another tree;
package references are verified at `d7b330113`, the package tree all three
slices share (no file under `packages/` changes between PR A and PR C).
#859 lands in three stacked slices: PR A (`fold-authority/m2-exit`) on
`main` at `74e347d9e`, PR B on PR A, and PR C on PR B. `main` holds #905 and
#906 with their review commits (merges `21681e0ea` and `74e347d9e`). A
reference the specification cites at `265df096` that moved or was deleted is
corrected in the record's evidence file. Every other `file:line` was carried
to `0ff62b29a` (or `d7b330113`) through the diffs from the tree it was last
verified at, then checked against the current line. A record that names an
earlier commit of the stack as a state (`286ffaf81` or `d7b330113` for PR A,
`b45416ac0` for PR B, `bdf564e3a` through `78fc312db` for PR C) describes the
tree before a later slice changed it. The pre-rebase commits stay reachable
under the `fold-authority/*-prerebase` branches.

`Exercised` cites the recorded run of the PR that supplies it. The recorded
runs are the gate tables of the PR descriptions:

| PR | Ticket | Branch | Head at read | Base | Merge on `main` |
| --- | --- | --- | --- | --- | --- |
| #898 | #852 | `fold-authority/m1-loader-parity` | `4c964f93a` | `main` | `dd5dcc170` |
| #899 | #853 | `fold-authority/m1-conflict-disposition` | `566a35c10` | `main` | `f653cecc4` |
| #900 | #854 | `fold-authority/m1-setup-doctor` | `2effd440f` | `main` | `38b816f33` |
| #903 | #855 | `fold-authority/m1-daemon-authority` | `ccab18208` | `main` | `09f76a917` |
| #904 | #856 | `fold-authority/m1-plugin-authority` | `0023302db` | `main` | `73bf811de` |
| #905 | #857 | `fold-authority/m2-identity-rows` | `5d9ff4581` | `main` | `21681e0ea` |
| #906 | #858 | `fold-authority/m2-derived-state` | `f6dde3fa3` | `main` | `74e347d9e` |
| #859 PR A | #859 | `fold-authority/m2-exit` | `fd0b52aa5` | `main` (`74e347d9e`) | open |
| #859 PR B | #859 | `fold-authority/m2-exit-covered-systems` | `b45416ac0` | `fold-authority/m2-exit` | open |
| #859 PR C | #859 | `fold-authority/m2-exit-bounds` | the commit that adds this catalog | `fold-authority/m2-exit-covered-systems` | open |

The #859 runs are recorded in the gate blocks of the three #859 PR
descriptions, which are being opened now. Each block names its command, the
commit it ran at, and its result counts:

- PR A: the full workspace test run at `fd0b52aa5` (6,124 passed, 0 failed,
  66 ignored), with fmt, clippy, rustdoc, and the comment-marker script
  green. PR A holds `286ffaf81`, `d7b330113` (comments and the regenerated
  TUI tree only), and `fd0b52aa5` (the bound for the archive fold sequence).
- PR B: the full workspace test run at `b45416ac0` (6,131 passed, 0 failed,
  66 ignored), with the same gates green.
- PR C: the full workspace test run at `0ff62b29a` (6,156 passed, 0 failed,
  66 ignored), with the same gates green. PR C holds `bdf564e3a`,
  `6267f66d4` (the store-side cut of the abandon path's failure detail),
  `6fc85742f` (bounds checked on redacted forms for stored diagnostics, the
  task-list setter and capture, and the synthetic task-list pair),
  `186f3c076` (final shaping of metadata text in the store, prefixes stable
  under both redactors, a 1,024-byte `last_recut` bound, and stored-form
  state-sync checks), `e02b22383` (the composite metadata witness and
  caller-side and receipt-retention tests), `78fc312db` (the render identity
  bound counts all five request inputs), `0ff62b29a` (bounds for the
  summarizer's withdrawal and abandon records), and the commit that adds
  this catalog, which changes no code, so the run at `0ff62b29a` stands for
  PR C's head.
- TypeScript: `bun run check:repo` (4,849 passed, 0 failed) and the plugin
  smoke check green, on the package tree `d7b330113` the three slices share.

A test named green in PR A's run also ran green in PR B's and PR C's runs
unless the record says otherwise.

## Scope

The records cover five surfaces: the configuration loaders (Rust
`crates/daemon/src/config.rs`, the OpenCode plugin's `config/`, the Pi
plugin's loader, and the CLI's mode resolution); conflict disposition, setup,
and doctor in the CLI and the plugin; the daemon's durable per-session fold
authority and its native gate; the plugin's status surfaces; and the
session's durable metadata, its identity rows, derived state held in the
daemon cache, and the selected-identity budget. Canonical window-protocol
records live in `docs/properties/window-protocol/`; this catalog links shared
obligations to those records. WP-E10 shares FA-E03's durable-text guard.

## Reachability classes

- `default-production` - reached by a default install. An empty summarizer
  chain is the default, so native authority is the default path.
- `explicit-config-only` - reached only with an admitted, non-empty
  summarizer chain, so the session's stored authority is Eidnara folding.
- `test-only` - reached only by a test seam.

Each record states the evidence for its label.

## Index

| ID | Record | Type | Check | Surface | Status | Exercised | Owner |
| --- | --- | --- | --- | --- | --- | --- | --- |
| FA-E01 | [`fa-e01-additive-retains-ingress-identities`](#fa-e01-additive-retains-ingress-identities) | safety | `always` | daemon | invalidated | partial | #903 (#855), #905 (#857); output-shape witness unowned |
| FA-E02 | [`fa-e02-raw-mode-controls-host-surfaces`](#fa-e02-raw-mode-controls-host-surfaces) | safety | `always` | plugin, pi | invalidated | yes | #898 (#852); #904 (#856) last TypeScript gate; refusal text for an empty chain (needs human input) |
| FA-E03 | [`fa-e03-whole-meta-refuses-oversize`](#fa-e03-whole-meta-refuses-oversize) | safety | `always` | store | active | partial | #905 (#857), #859 PR A, #859 PR C; first-commit refusal witness unowned (shared with WP-E10); covered-row refusal witness unowned |
| FA-E04 | [`fa-e04-selected-identities-follow-the-whole-range`](#fa-e04-selected-identities-follow-the-whole-range) | safety | `always-or-unreached` | summarizer | invalidated | yes | #859 PR C, #859 PR A, #905 (#857) |
| FA-E05 | [`fa-e05-empty-chain-refusal-is-late-and-change-gated`](#fa-e05-empty-chain-refusal-is-late-and-change-gated) | safety | `always` | daemon | invalidated | partial | #903 (#855), #904 (#856); #859 PR A; ignored no-fire commit error unowned |
| FA-E06 | [`fa-e06-route-snapshots-and-background-work-have-different-lifetimes`](#fa-e06-route-snapshots-and-background-work-have-different-lifetimes) | reachability | `sometimes` | daemon | active | partial | #903 (#855); #859 PR A; firing across route teardown unowned |
| FA-E07 | [`fa-e07-empty-baseline-and-absent-baseline-have-permissive-helper-semantics`](#fa-e07-empty-baseline-and-absent-baseline-have-permissive-helper-semantics) | safety | `always` | daemon | active | yes | #906 |
| FA-E08 | [`fa-e08-empty-served-history-means-unserved-and-cold`](#fa-e08-empty-served-history-means-unserved-and-cold) | safety | `always` | daemon | active | yes | #906; production reachability of a known-empty history unowned |
| FA-E09 | [`fa-e09-missing-identities-have-consumer-specific-policies`](#fa-e09-missing-identities-have-consumer-specific-policies) | safety | `always` | daemon, store | active | partial | #905; empty-selected-set witness unowned |
| FA-E10 | [`fa-e10-rust-model-resolution-is-user-jsonc-only`](#fa-e10-rust-model-resolution-is-user-jsonc-only) | safety | `always` | daemon | invalidated | partial | #898 (#852); #859 PR A; final-component symlink witness unowned |
| FA-E11 | [`fa-e11-typescript-expands-user-config-and-rejects-module-keys`](#fa-e11-typescript-expands-user-config-and-rejects-module-keys) | safety | `always` | plugin, pi | invalidated | yes | #898 (#852); #904 (#856) last TypeScript gate |
| FA-E12 | [`fa-e12-cli-mode-resolution-does-not-use-the-plugin-loader`](#fa-e12-cli-mode-resolution-does-not-use-the-plugin-loader) | safety | `always` | cli | invalidated | yes | #898 (#852), #900 (#854); #904 (#856) last TypeScript gate |
| FA-E13 | [`fa-e13-conflicts-disable-and-repair-toward-off`](#fa-e13-conflicts-disable-and-repair-toward-off) | safety | `always` | plugin, cli | invalidated | yes | #899 (#853), #900 (#854); #904 (#856) last TypeScript gate; TUI startup witness unowned |
| FA-E14 | [`fa-e14-native-status-conflates-pruning-with-folding`](#fa-e14-native-status-conflates-pruning-with-folding) | safety | `always` | plugin | invalidated | yes | #899 (#853), #904 (#856) |
| FA-N01 | [`fa-n01-config-derived-authority`](#fa-n01-config-derived-authority) | safety | `always` | daemon, plugin, pi, cli | active | yes | #898 (#852); #859 PR A; #904 (#856) last TypeScript gate; "never `false`" reading (needs human input) |
| FA-N02 | [`fa-n02-first-commit-adopts-authority`](#fa-n02-first-commit-adopts-authority) | safety | `always` | daemon, store | active | yes | #903 (#855); #859 PR A; two-route concurrent adoption unowned (refinement) |
| FA-N03 | [`fa-n03-authority-change-requires-quiescent-bind`](#fa-n03-authority-change-requires-quiescent-bind) | safety | `always-or-unreached` | daemon, store | active | yes | #903 (#855); #859 PR A; busy summarizer through a reopened handler unowned |
| FA-N04 | [`fa-n04-quiescent-change-completes`](#fa-n04-quiescent-change-completes) | liveness | `always-or-unreached` | daemon, store | active | partial | #903 (#855); #859 PR A; post-kill obligation (b), a kill between the reset commit and the pass commit, unowned |
| FA-N05 | [`fa-n05-native-authority-gates-fold-work`](#fa-n05-native-authority-gates-fold-work) | safety | `unreachable` | daemon | active | partial | #903 (#855); #859 PR A; native high-pressure first-unit witness unowned (the rerun caller has structural coverage only); wrapup snapshot and boundary counters (needs human input) |
| FA-N06 | [`fa-n06-native-state-stays-additive`](#fa-n06-native-state-stays-additive) | safety | `always` | daemon, store | active | partial | #903 (#855), #905 (#857), #906 (#858); #859 PR A; "bootstrap" writer (needs human input) |
| FA-N07 | [`fa-n07-conflict-disposition-is-authoritative`](#fa-n07-conflict-disposition-is-authoritative) | safety | `always` | plugin, cli | active | partial | #899 (#853), #904 (#856); TUI startup under `warn` unowned; TUI reading of a load failure (needs human input) |
| FA-N08 | [`fa-n08-repair-uses-the-proposed-document`](#fa-n08-repair-uses-the-proposed-document) | safety | `always-or-unreached` | cli | active | yes | #900 (#854), #904 (#856); rollback-first failure report (needs human input) |
| FA-N09 | [`fa-n09-only-accepted-results-publish-derived-state`](#fa-n09-only-accepted-results-publish-derived-state) | safety | `always-or-unreached` | daemon | active | yes | #906; reference model unowned |
| FA-N10 | [`fa-n10-unknown-served-history-defers-hints`](#fa-n10-unknown-served-history-defers-hints) | safety | `always-or-unreached` | daemon | active | yes | #906, #859 PR A |
| FA-N11 | [`fa-n11-baseline-validity-requires-a-matching-join`](#fa-n11-baseline-validity-requires-a-matching-join) | safety | `always-or-unreached` | daemon | active | yes | #906; transform-driven diagnostics-only commit unowned |
| FA-N12 | [`fa-n12-identity-observations-survive-window-loss`](#fa-n12-identity-observations-survive-window-loss) | safety | `always-or-unreached` | daemon, store | active | yes | #905 |
| FA-N13 | [`fa-n13-selected-identities-have-a-byte-budget`](#fa-n13-selected-identities-have-a-byte-budget) | safety | `always-or-unreached` | summarizer | active | yes | #859 PR C, #859 PR A |
| FA-N14 | [`fa-n14-module-meta-is-message-independent`](#fa-n14-module-meta-is-message-independent) | safety | `always` | daemon, store | active | yes | #859 PR C, #859 PR B, #859 PR A, #905 (#857), #906 (#858), #903 (#855); covered lists stored in `meta` before #859 PR B (needs human input) |

Owner lists the PR whose recorded run supplies the status, then, after a
semicolon, the evidence still unowned or the decision that needs human input.
Semantics: 17 `always`, 9 `always-or-unreached`, 1 `sometimes`, 1
`unreachable`, 0 `reachable`.

Exercise distribution: 18 `yes`, 10 `partial` (FA-E01, FA-E03, FA-E05,
FA-E06, FA-E09, FA-E10, FA-N04, FA-N05, FA-N06, FA-N07), 0 `not yet`.
Status: 19 `active`, 9 `invalidated` (FA-E01, FA-E02, FA-E04, FA-E05, FA-E10,
FA-E11, FA-E12, FA-E13, FA-E14).

## Records

### fa-e01-additive-retains-ingress-identities

Type: safety
Reachability: default-production - the additive path now runs whenever the
stored authority is native (`crates/daemon/src/transform.rs:3207-3208`), and
an empty chain, the default, selects native; the replaced identity clause is
unreachable at HEAD because the additive path writes no identity.
Status: invalidated
Invalidated: #855 (PR #903, `8b1e04944`) moves identity insertion,
provisional-tail removal, and tail and basis re-adoption onto the folding
path, so the additive path retains no identity; FA-N06 replaces the
identity-retention clause. #857 (PR #905, `871ebfb08`) then removes the
identity map from the record entirely. The output clause survives: an
additive pass serves m0, m1, then the ingress messages unchanged.
Exercised: partial - the replacement is witnessed: #903's
`native_authority_skips_every_fold_step_even_with_a_live_chain` asserts an
empty identity table after native passes (also green in #859 PR A's run at
`fd0b52aa5`). No named test asserts the surviving output clause.
Guarantee: Accepted compaction-off passes prepend m0/m1, preserve ingress
messages, and retain non-provisional identities.
Check: `always` - replaced clause (not checked at HEAD): identity keys become
`(previous - provisional) ∪ eligible-new`. Surviving clause: on every
accepted additive pass the output's first two messages are the m0 and m1
synthetic units and the rest equals the ingress messages in order.
Fault/timing angle: Repeated appends or a replaced native slice.
Required faults and enabling state: Native authority; several passes with
distinct mids; a native slice replacing the history.
Confidence: high -
[evidence](evidence/fa-e01-additive-retains-ingress-identities.md). The
output construction (`transform.rs:3054-3064`), the scalar-only ingress
update on the additive path (`:2904` calls `apply_ingress_scalars`,
`:5583-5598`), and the folding-only `apply_ingress_identities` (`:5602`)
were read at `0ff62b29a`. Correction: the spec's existing check
`compaction_mode_projection_cache_reclassifies_synthetic_prefix` (D`lib.rs:42489`
at `265df096`) was deleted on `main` by `252d9e179` and does not exist at
HEAD.
Existing check: `crates/daemon/src/fold_authority_handler_tests.rs:46`
`native_authority_skips_every_fold_step_even_with_a_live_chain` (empty
identity table via `assert_native_state`, `:19-29`); `:645`
`native_metadata_does_not_grow_with_the_message_count`. None for the output
clause.
Impact: High at `265df096`: compaction-off accumulated durable metadata. At
HEAD the surviving clause matters for native serving: a changed ingress
message would be served under the host's own compaction.
Open questions:
- Does a test assert the additive output is m0, m1, then ingress? None found
  at HEAD. Unresolved, needs an output-shape assertion on a native pass.

### fa-e02-raw-mode-controls-host-surfaces

Type: safety
Reachability: default-production - at HEAD compaction-off mode is the
default, because an empty chain makes `isCompactionEnabled` false
(`packages/opencode-plugin/src/config/agent-disable.ts:21-29`).
Status: invalidated
Invalidated: PR #898 (#852) replaces the "raw boolean only" clause with the
chain clause of FA-N01 (`bd592e4a0`), and its merged history withdraws
folding on an unresolved admission (`f5b10fefe`,
`packages/opencode-plugin/src/config/fold-authority.ts:137-146`). The clauses
"off omits `eidnara_reduce`" and "fold commands refuse while their
definitions stay registered" survive unchanged. #900 and #904 consume the
predicate through `foldAuthorityOf` and do not change it.
Exercised: yes - the `bun run check:repo` gates of #898 through #904 ran the
updated predicate test, the tool-registry tests (including an empty chain
registering the compaction-off tool set), and the command refusal tests; the
plugin source at HEAD is `d7b330113`'s, which differs from #904's head only
in comments and generated TUI source, and #859 PR A's `check:repo` ran on it.
Guarantee: At `265df096`, TypeScript compaction mode depended only on the raw
`compaction.enabled` boolean, not model availability; off omitted
`eidnara_reduce` and refused fold commands while keeping their definitions
registered.
Check: `always` - the predicate equals `compaction?.enabled !== false` and a
non-empty normalized chain; when it is false the registry omits exactly
`eidnara_reduce`, the commands stay registered with the "Unavailable"
description, and flush, recomp, and wrapup refuse without a daemon call.
`always` because the mode is resolved once per startup on every
configuration.
Fault/timing angle: An empty chain with an absent or true compaction
setting.
Required faults and enabling state: An empty configuration, explicit
`false`, a model with each compaction setting, a blank model.
Confidence: high -
[evidence](evidence/fa-e02-raw-mode-controls-host-surfaces.md). Read at HEAD
and at `265df096`. Corrections: `agent-disable.ts:19-23` is now `:21-29`,
`tool-registry.ts:34-48` is `:38-48`, `hook.ts:223` is `:226`, Pi
`index.ts:84,349-355` is `:13,349-356`, and
`compaction-accessor-guard.test.ts:66` is `:67`.
Existing check: `packages/opencode-plugin/src/config/compaction-accessor-guard.test.ts:67`
"isCompactionEnabled requires a summarizer chain and a compaction setting
that is not false"; `packages/opencode-plugin/src/plugin/tool-registry.test.ts:127`
and `:154` "an empty summarizer chain registers the compaction-off tool set";
`packages/opencode-plugin/src/hooks/context/command-handler.test.ts:144`
"refuses /${command} without a daemon call when compaction is off".
Impact: High. Before #898, "enabled" did not establish folding capability.
Open questions:
- The refusal notification and the command descriptions say
  `compaction.enabled: false` even when the cause is an empty chain
  (`hooks/context/command-handler.ts:505-513`,
  `features/builtin-commands/commands.ts:7-8`), and the test pins that text.
  Should the text name both causes? Needs human input.
- The TUI comment at `packages/opencode-plugin/src/tui/index.tsx:1098` (at
  `286ffaf81`) said `isCompactionEnabled` defaults to `true` after a
  config-load failure, while `isCompactionEnabled({})` is `false`. Resolved:
  `d7b330113` removed the comment.

### fa-e03-whole-meta-refuses-oversize

Type: safety
Reachability: default-production - every transform commit serializes
`ModuleMeta` and scans it through `json_content`
(`crates/memory-store/src/lib.rs:11034-11041`, `:3028-3038`), whose
single-pass preparation applies `ensure_durable_text_bound` first (`:4303`,
`:4577-4583`, 512 KiB at `:429`).
Status: active
Exercised: partial - #905's `a_refused_commit_writes_no_identity_row`
commits an oversized `meta` over an existing row and asserts `InputLimit`,
unchanged identity rows, and an unchanged row version (green in #905's run
and in #859 PR C's run at `0ff62b29a`). #859 PR A's matrix asserts every
cell stays under the guard. No witness refuses a first transform commit and
checks the row stays absent. Since #859 PR B the commit also writes
covered-system rows, scanned in meta-shaped documents that each pass the same
guard inside the transaction; no test refuses an oversized covered-system
document or checks the covered rows after a durable-text refusal.
Guarantee: Transform metadata is one bounded durable text field; an
oversized first commit is refused and leaves the row absent.
Check: `always` - the guard accepts a byte length at most 524,288 and rejects
larger with `InputLimit`. The `meta` guard runs while the commit is prepared,
before the transaction executes; covered-system document validation runs
inside the transaction, after the `cache_state` write, and its refusal rolls
the transaction back. Either way a refused first commit leaves no
`cache_state` row, and a refused later commit leaves the row version,
identity rows, and covered-system rows unchanged. Every commit passes
through the guard.
Fault/timing angle: Ordinary history growth crossed the limit at
`265df096`; at HEAD, FA-N14 keeps ordinary growth below it, so the guard is
the recoverable backstop.
Required faults and enabling state: A fresh store; exact-boundary inputs; a
field or restored legacy value past the bound; a covered-system document
past the bound.
Confidence: high -
[evidence](evidence/fa-e03-whole-meta-refuses-oversize.md). The guard chain
and the refusal test were read at `0ff62b29a`. The `meta` guard runs in
preparation (`crates/memory-store/src/lib.rs:11034-11041`) before
`write.execute` opens the transaction (`:11102`); the covered-system row scan
(`prepare_document`, `:4832-4856`; `apply_covered_system_message_delta`,
`:5025-5127`) runs inside that transaction after the `cache_state` write
(`:11120-11127`), so its refusal rolls the transaction back and the durable
rows stay as they were. Cross-reference:
[WP-E10](../window-protocol/catalog.md#wp-e10-durable-meta-bound-refuses-the-cliff)
catalogs the same guard for the window protocol and records that the refusal
half had no witness; #905's store test now supplies one for a later commit.
Corrections to the spec's citations (at `265df096`): the constant moved from
S`:427` to `:429` and the guard from `:3998-4004` to `:4577-4583`; the
existing check D`transform_meta_bound.rs:20-99` was deleted by #833
(`7a8fb84b`); the growing fields S`:1668`, `:1876`, `:2014` left the record
(identity map by #905, fingerprints and baseline parts by #906).
Existing check: `crates/memory-store/src/lib.rs:33757`
`a_refused_commit_writes_no_identity_row`; `:32079`
`a_prepared_content_field_that_grows_past_the_durable_bound_on_redaction_is_refused`
(exact boundary on prepared content); `:31906`
`state_sync_metadata_scan_failure_rolls_back_earlier_writes` (oversized
stored meta refused inside a state sync); `:34279`
`covered_system_rows_past_one_scan_document_split_and_retire_every_receipt`
(covered rows split across scan documents, #859 PR C);
`crates/daemon/src/transform_meta_bound.rs:1273` (guard not reached, `:1265-1268`).
Impact: High. Without the guard an oversized record would be written; with
it, a record that keeps growing repeats a refused commit and the plugin
serves raw history.
Open questions:
- Does a witness refuse a first transform commit and leave the row absent?
  No. Unresolved, needs a transform-level first-commit refusal test; shared
  with WP-E10's open question.
- Does a refused commit leave the covered-system rows unchanged? No test
  checks it. Unresolved, needs a refusal, by the `meta` guard and by an
  oversized covered-system document, that compares the rows before and
  after.

### fa-e04-selected-identities-follow-the-whole-range

Type: safety
Reachability: explicit-config-only - firing assembly needs an Eidnara
authority and a non-empty chain (`crates/daemon/src/config.rs:181-185`).
Status: invalidated
Invalidated: #859 PR A (`fold-authority/m2-exit`, `fd0b52aa5`) charges each
flushed block's serialized identities against a 256 KiB budget, so the
selected range now shrinks to fit; FA-N13 replaces the "no separate byte
budget" clause. The other clauses survive: the selection equals the
identities of every non-synthetic message in the chunk's range, and a
missing identity is a no-fire.
Exercised: yes - for the surviving clauses, #859 PR C's recorded run at
`0ff62b29a` (6,156 passed) ran
`a_generated_history_fires_the_longest_whole_block_prefix_within_the_identity_budget`,
64 generated histories of user, assistant, tool-exchange, system, and
blank-user noise turns in random order. Its expected selection takes every
message from the first non-system message through the last message of the
fitting prefix, so system and noise messages between text blocks are in it,
and `BudgetCase::assert_matches_oracle` asserts the firing's selection
equals it exactly, in order. The property generates 64 cases whose turns
include system and blank-user noise roles; whether a given case places such
a message inside the selected range is probabilistic, and no assertion
counts those cases. The same run ran the fixed
oracle tests and
`history_summarizer_boundary_construction_matches_owned_reference`, which
asserts `MissingBlockIdentity` for a range with an unidentified message.
The fixed oracle cases are synthetic stress states with directly
constructed 30,000-character fingerprints
(`crates/daemon/src/history_summarizer_chunk.rs:1749-1782`).
Guarantee: Firing assembly copies every non-synthetic message's identity
vector inside the selected range with no separate byte budget.
Check: `always-or-unreached` - surviving clauses: a successful assembly's
selection equals, in order, the identities of the non-synthetic messages
from the chunk start through its end; a message in range without a stored
vector returns `MissingBlockIdentity`. Replaced clause: no byte budget.
Fault/timing angle: Noise-heavy inputs, same-role runs, an oversized first
block.
Required faults and enabling state: Non-empty chain, eligible range, supplied
identity vectors; a range containing system and noise messages.
Confidence: high -
[evidence](evidence/fa-e04-selected-identities-follow-the-whole-range.md).
At `265df096` the selection was a filter over messages in
`[start_index, end_index]` (D`history_summarizer_chunk.rs:698-715`, read
there). At `0ff62b29a` it is built from each flushed block's message list
(`:119-150`, `:386-395`), which carries pending system and noise messages
into the next block (`:233-267`, `:284`, `:302`, `:337`); the missing check
returns at `:1026-1029`.
Existing check: `crates/daemon/src/history_summarizer_chunk.rs:2169`
`a_generated_history_fires_the_longest_whole_block_prefix_within_the_identity_budget`
(#859 PR C, mixed roles); `:1960`, `:1981`, `:1995` (exact selection through
the oracle, `:1910-1914`);
`crates/daemon/src/lib.rs:18613`
`history_summarizer_boundary_construction_matches_owned_reference` (missing
identity at `:18831-18836`); `crates/daemon/src/history_summarizer_chunk.rs:2814`
`budget_stop_and_tool_only_ranges_are_recorded` (range, not identities).
Impact: High at `265df096`: a rendered-token budget did not bound durable
identity bytes. At HEAD a selection that skipped a message in range would
leave it unfenced at publication.
Open questions:
- Does a test compare the builder-derived selection with the range filter on
  a range holding system or noise messages? Resolved: yes, since #859 PR C,
  through the generated-history proptest; it does not count the mixed
  ranges it generates.

### fa-e05-empty-chain-refusal-is-late-and-change-gated

Type: safety
Reachability: explicit-config-only - the surviving late path needs a stored
Eidnara authority with an empty live chain (`cfg` from `effective_config`,
`crates/daemon/src/lib.rs:5406`), which requires removing the chain after
adoption; the default empty chain adopts native and stops at the gate.
Status: invalidated
Invalidated: PR #903 (#855) replaces it with the early `native_authority`
gate of FA-N05 (`crates/daemon/src/lib.rs:9423-9434`). The trigger-then-
`no_models` order, the change-gated `record_no_fire`, and the ignored commit
error survive only as defense for a stored Eidnara authority whose live chain
is empty (a stall that `session.status` reports since #904).
Exercised: partial - `no_fire_reason_is_durable_change_gated_and_cleared_by_fire`
(binding with a chain, live configuration without one: records `no_models`
once, a repeat leaves `row_version` unchanged) and
`session_status_names_a_stalled_eidnara_summarizer_in_the_authority_prefix`
ran in the PR #903 gate and #859 PR A's run at `fd0b52aa5`. No test forces the
ignored commit error or the trigger-false arm with an empty live chain.
Guarantee: At `265df096`, Idle preparation evaluated the trigger before
rejecting an empty chain; identical repeated no-fire reasons did not rewrite
metadata; the commit error was ignored.
Check: `always` - under an applied Eidnara authority with an empty live
chain: a non-firing trigger returns `trigger_false` or `busy`
(`crates/daemon/src/lib.rs:5553-5576`); a firing trigger returns `no_models`
(`:5578-5584`); an unchanged stored reason causes zero commits and a failed
commit leaves the pass result unchanged (`:5895-5912`). `always` because
every such pass runs the path.
Fault/timing angle: Chain removal after adoption, repeated turns, a lost
commit race on the no-fire write.
Required faults and enabling state: A stored Eidnara authority; an empty live
chain; both trigger outcomes; a repeated reason; a forced commit failure.
Confidence: high -
[evidence](evidence/fa-e05-empty-chain-refusal-is-late-and-change-gated.md).
Read at HEAD. Corrections: callers `lib.rs:9014`, `:9150` are now `:8924`,
`:9077`; the entry `:9451-9468` is `:9423-9434`;
`history_summarizer_chunk.rs:619-623` is `:908-912`;
`history_summarizer.rs:1655-1657` is `:1831-1833`; the checks at
`lib.rs:42058` and `:39354` are at `:41167` and `:37569`.
Existing check: `crates/daemon/src/lib.rs:41167`
`no_fire_reason_is_durable_change_gated_and_cleared_by_fire`; `:37569`
`session_wrapup_no_models_is_terminal_and_retains_command`;
`crates/daemon/src/fold_authority_handler_tests.rs:1037`
`session_status_names_a_stalled_eidnara_summarizer_in_the_authority_prefix`.
Replacement: FA-N05.
Impact: High at `265df096`: an empty-chain session paid for preparation on
every firing turn. At HEAD, medium: a stalled Eidnara session never folds and
only the stall status says so.
Open questions:
- Does a failed `record_no_fire` commit leave the pass unaffected?
  Unresolved, needs a forced commit failure.

### fa-e06-route-snapshots-and-background-work-have-different-lifetimes

Type: reachability
Reachability: explicit-config-only - unequal captured configurations on one
session need a configuration edit between two binds
(`SessionBinding.config`, `crates/daemon/src/lib.rs:227-241`).
Status: active
Exercised: partial - PR #903 (#855) gate and #859 PR A's run at `fd0b52aa5`:
`a_sibling_binding_keeps_the_change_pending_until_a_quiescent_bind` and
`a_sibling_bound_during_the_change_keeps_it_pending` hold two routes with
unequal captured configurations on one session;
`no_fire_reason_is_durable_change_gated_and_cleared_by_fire` shows
preparation reading a live chain that differs from the captured one. No test
holds a firing across route teardown, and no test combines the three.
Guarantee: One session can hold sibling route bindings with unequal captured
configurations while summarizer preparation reads the live configuration and
an admitted firing outlives route teardown.
Check: `sometimes` - within one campaign, observe two participating
bindings on one session with unequal captured configurations, a preparation
read (`effective_config`, `crates/daemon/src/lib.rs:5406`) that sees an edited
live chain, and a firing task spawned by `spawn_history_summarizer_firing`
(`:6194`) still running after `unbind_route` (`:4487`) removed its route.
`sometimes` because this is enabling state to reach, not an invariant.
Fault/timing angle: A configuration edit between binds; a route close while
a firing awaits its producer.
Required faults and enabling state: A changed mtime, two route handles, a
non-empty chain, a held producer.
Confidence: high -
[evidence](evidence/fa-e06-route-snapshots-and-background-work-have-different-lifetimes.md).
Read at HEAD. The durable authority (FA-N02, FA-N03) decides which path runs;
it does not unify captured configurations, and one firing still reads the
live chain beside the captured `memory_enabled` (`lib.rs:5640`).
Corrections: `lib.rs:211-251` is now `:227-263`; `:5722` is `:5406`;
`:5924-5925` is `:5640`; `config.rs:2158` is `:2350`.
Existing check: `crates/daemon/src/fold_authority_handler_tests.rs:245`,
`:778`; `crates/daemon/src/lib.rs:41167`;
`crates/daemon/src/config.rs:2350`
`mtime_cache_reuses_unchanged_reads_and_invalidates_on_mtime_change`.
Impact: High. Route mode alone is not a session-wide authority fact, which
is why FA-N02 stores one.
Open questions:
- Does a firing held at its producer complete after its only route unbinds?
  Unresolved, needs a teardown witness with a blocked producer.

### fa-e07-empty-baseline-and-absent-baseline-have-permissive-helper-semantics

Type: safety
Reachability: explicit-config-only - corrected from `default-production`.
At `265df096` the folding path ran whenever `compaction_enabled` held
(default true). Since `8b1e04944` (#903) it runs only when the stored
authority folds (`crates/daemon/src/transform.rs:3207-3208`), which needs a
non-empty user-tier chain (`crates/daemon/src/config.rs:181-185`).
Status: active
Exercised: yes - #906's recorded workspace gate runs
`durable_scalars_join_retained_parts_by_generation`, which evaluates
present-but-empty parts at the matching generation (clause 1) and refreshes
with no previous baseline on a non-bust pass (clause 2, generation 1,
evaluable, parts present), with matching non-empty parts as the control.
Guarantee: An empty measured prefix matches every current sequence whose
excluded prefix has the recorded length and digest, and a refresh with no
previous baseline creates an evaluable baseline.
Check: `always` - `same_measured_prefix` over parts with an empty list
returns `Some(0)` for every current sequence whose excluded prefix has the
recorded length and digest (with `excluded_prefix_len` 0 and the digest of
the empty slice, every current sequence);
`refresh_tail_hygiene_baseline(measured, _, None, _, _)` yields generation 1,
evaluable, not invalidated, with parts at generation 1. Absent parts under a
durable baseline never reach `same_measured_prefix`. `always` because both
are pure helper contracts that hold on every call.
Fault/timing angle: Baseline absence or genuinely empty retained parts.
Required faults and enabling state: Empty parts at the matching generation;
no previous baseline; a non-empty measurement control; a missing-parts
control.
Confidence: high -
[evidence](evidence/fa-e07-empty-baseline-and-absent-baseline-have-permissive-helper-semantics.md).
Read `same_measured_prefix` (`crates/daemon/src/tail_hygiene.rs:1036-1069`)
and `refresh_tail_hygiene_baseline` (`:1080-1165`) at `0ff62b29a`. Correction
to the spec (at `265df096`, `tail_hygiene.rs:983-1043`): the helper takes
`BaselineParts` and compares an excluded-prefix digest that `e15a09a6` added
after `265df096`, so "matches every current sequence" now holds for every
sequence whose excluded prefix matches. The spec's
`transform.rs:4881-4893` is `crates/daemon/src/transform.rs:4953-4976`; a
non-bust pass with no durable baseline still leaves it absent rather than
calling refresh with `None`.
Existing check: `crates/daemon/src/tail_hygiene.rs:2278`
`durable_scalars_join_retained_parts_by_generation` (#906); `:2148`
`defer_delta_and_boundary_advance_are_additive` and `:2210`
`non_append_mutation_invalidates_until_a_bust` (non-empty baselines; spec
`:2035`, `:2079` at `265df096`).
Impact: High. Missing retained parts cannot masquerade as an empty baseline;
FA-N11 now carries that obligation at the join.
Open questions: None.

### fa-e08-empty-served-history-means-unserved-and-cold

Type: safety
Reachability: explicit-config-only - corrected from `default-production`
for the same reason as FA-E07: hint admission and divergence run only past
the native gate (`crates/daemon/src/transform.rs:3207-3208`). The label
rests on that gate, not on a constructed state: whether a production folding
pass can retain a known-empty served history is unresolved (see Open
questions).
Status: active
Exercised: yes - #906's recorded workspace gate runs
`a_new_hint_defers_only_when_its_target_may_have_been_served_outside_a_bust`
("known empty" row: an empty retained vector does not defer) and
`first_divergence_classifies_each_boundary_kind_and_ignores_appends` (an
empty old sequence reports nothing). This is verified exercise of the
helper state only: both tests supply the empty vector directly
(`crates/daemon/src/transform.rs:13347`,
`crates/daemon/src/divergence.rs:164-165`), and no Handler test promotes an
empty served history, so production reachability of the state is
unresolved.
Guarantee: An empty served-fingerprint vector suppresses divergence reporting
and does not establish prior service for hint deferral.
Check: `always` - `first_divergence(&[], new)` returns `None`;
`user_hint_deferred(text, Some(&[]), block_id, false)` returns false; a newly
decided hint is deferred iff its text is non-empty, the pass is not a bust,
and either the retained history is absent (FA-N10) or it names the target
block or, for block 0, the bare mid. `always` because both are pure helper
contracts.
Fault/timing angle: A first pass, or a retained history that is genuinely
empty. Absence is a separate state since #906 and no longer reaches this
contract.
Required faults and enabling state: Empty and matching fingerprints; a
block-zero bare-mid match; bust and non-bust controls; an absent-history
control.
Confidence: high -
[evidence](evidence/fa-e08-empty-served-history-means-unserved-and-cold.md).
Read `first_divergence` (`crates/daemon/src/divergence.rs:42-48`),
`user_hint_deferred` (`crates/daemon/src/transform.rs:7906-7924`), and the
admission call (`:4212-4217`) at `0ff62b29a`. Correction to the spec (at
`265df096`): `user_hint_target_was_served` (`transform.rs:8072-8079`) read
`meta.served_output_fingerprint`; #906 replaced it with `user_hint_deferred`
over the retained history, and the admission at `:4150-4156` is now
`:4212-4217`.
Existing check: `crates/daemon/src/divergence.rs:124`
`first_divergence_classifies_each_boundary_kind_and_ignores_appends` (empty
old row at `:164-165`); `crates/daemon/src/transform.rs:13329`
`a_new_hint_defers_only_when_its_target_may_have_been_served_outside_a_bust`
(#906).
Impact: Medium. A known-empty history that established prior service would
defer every first hint.
Open questions:
- Can a production folding pass promote an empty served vector, or is the
  known-empty state reachable only in tests? Unresolved, needs a pass whose
  served output has no blocks.

### fa-e09-missing-identities-have-consumer-specific-policies

Type: safety
Reachability: explicit-config-only - transform enforcement runs on the
folding path (`crates/daemon/src/transform.rs:3684`, past `:3207-3208`);
publication needs an admitted firing, which needs a configured chain.
Status: active
Exercised: partial - #905's recorded workspace gate runs
`publish_rejects_a_selected_message_whose_identity_row_is_gone` (missing
row), `a_completed_tail_that_turns_provisional_is_removed_and_re_adopted_exactly`
(a transform over a deleted row adopts the new vector with no error),
`a_window_that_drops_a_selected_message_fences_the_publication_out_and_keeps_its_rows`
(the window that drops a selected mid records the withdrawal, the
publication is fenced out, and the identity rows are unchanged), and
`publish_rejects_a_firing_whose_selected_message_left_the_window`;
the same run includes the existing
`selected_range_identity_drift_during_await_rejects_without_cooldown`
(changed row), `publish_history_summarizer_chunk_rejects_recut_epoch_mismatch_as_conflict`
(epoch), and `tail_identity_extension_during_await_still_publishes` (legal
unrelated tail). No test constructs a firing with an empty selected set.
Guarantee: Transform identity enforcement skips a missing entry; publication
rejects a missing or changed selected identity and a mismatched epoch.
Check: `always` - in `enforce_block_identity` a mid with no stored row
continues (`crates/daemon/src/transform.rs:5485-5487`) and produces no
re-adoption and no `FrozenTargetDrift`; publication refuses an empty selected
set (`crates/memory-store/src/lib.rs:14138-14143`), then a firing whose
`withdrawn_selected_mid` is set (`:14144-14148`), then any selected mid whose
stored vector is absent or differs (`:14149-14172`), then a revert-epoch
mismatch as `CasConflict` (`history_publication_fence_tx`, `:6645-6650`,
called at `:14174-14183`). `always` because each consumer
applies its policy on every evaluation.
Fault/timing angle: Identity removal or edit during a firing's producer
await, or a reset during a firing.
Required faults and enabling state: A configured firing; a selected mid
absent or changed; an empty selected set; a mismatched epoch; a legal
unrelated-tail control.
Confidence: high -
[evidence](evidence/fa-e09-missing-identities-have-consumer-specific-policies.md).
Read both consumers at `0ff62b29a`; the `lib.rs` line references were
refreshed after `44f3159`, the base merge, and the abandon-path cut moved
the publish transaction down by 40 lines. Corrections to the spec (at `265df096`):
S`:11825-11859` is now `crates/memory-store/src/lib.rs:14124-14183`, and the
fence reads `lookup_block_identity_rows` in the publish transaction instead
of the hydrated `meta.block_identity_by_mid` (#905); D`transform.rs:5364-5370`
is now `:5485-5487`; the `IdentityDrift` error and
`identity_drift_requires_reject` (spec `:5437-5444`) were deleted by
`6477c9f27` (window-protocol #833, D25), so a stored-versus-new mismatch now
re-adopts or refuses with `FrozenTargetDrift`.
Existing check: `crates/memory-store/src/lib.rs:25938`
`publish_rejects_a_selected_message_whose_identity_row_is_gone` (#905);
`crates/daemon/src/history_summarizer.rs:4055`
`selected_range_identity_drift_during_await_rejects_without_cooldown` (spec
`:3037`); `:4109` `tail_identity_extension_during_await_still_publishes`;
`crates/memory-store/src/lib.rs:30032`
`publish_history_summarizer_chunk_rejects_recut_epoch_mismatch_as_conflict`;
`crates/daemon/src/transform.rs:14628`
`a_completed_tail_that_turns_provisional_is_removed_and_re_adopted_exactly`
(#905); `:20444`
`a_window_that_drops_a_selected_message_fences_the_publication_out_and_keeps_its_rows`
(#905); `crates/memory-store/src/lib.rs:25993`
`publish_rejects_a_firing_whose_selected_message_left_the_window` (#905).
Impact: High. Removing identity rows weakens transform validation while
blocking publication; a fence that accepts a missing row publishes stale
content.
Open questions:
- The empty-selected-set refusal has no witness; a firing with an empty
  `selected_range_identities` is needed. Unresolved, needs that test.

### fa-e10-rust-model-resolution-is-user-jsonc-only

Type: safety
Reachability: default-production - every daemon bind resolves configuration
through `effective_for_project` (`crates/daemon/src/lib.rs:4744`).
Status: invalidated
Invalidated: PR #898 (#852) replaces the `.jsonc`-only discovery
(`discover_tier`, `crates/daemon/src/config.rs:401-408`, now `.jsonc` then
`.json`) and the array-only fallbacks (`chain_models`, `:1064-1082`, now a
string or an array) under FA-N01. Surviving clauses: no variable
substitution (`:470`), the 1 MiB bound and final-component symlink rejection
(`:412`, `read_bounded_bytes` `:427-459`), user-tier-only chain keys
(`tier_class`, `:762-778`; `:829-837`), module-model precedence
(`:868-883`), and the empty-chain default with compaction on (`:142-144`).
Exercised: partial - `tier_policy_ignores_project_models_and_rejects_project_lowering`
(`config.rs:1509`),
`module_model_keys_replace_the_plugin_chain_only_when_the_module_model_is_set`
(`:2237`), `compaction_enabled_defaults_true_and_is_user_tier_only`
(`:1616`), and `an_oversized_tier_file_is_ignored_with_a_warning` (`:1420`)
cover the surviving clauses and ran green in #859 PR A's run at `fd0b52aa5`; the
replaced clauses are pinned by `jsonc_only_discovery_fails_the_fixture`
(`:2588`) and the fixture rows (PR #898). No test witnesses the
final-component symlink rejection.
Guarantee: Rust discovers `.jsonc` tiers only, parses without variable
substitution, bounds a tier to 1 MiB and rejects a final-component symlink,
and derives the chain only from user-tier keys with module-model precedence
and array-only fallbacks.
Check: `always` - per load. At HEAD the surviving clauses hold per load; the
replaced clauses are asserted by FA-N01.
Fault/timing angle: JSON-only files, literal variables, malformed tiers,
mtime edits.
Required faults and enabling state: Disk-backed tiers covering those forms;
a symlinked tier.
Confidence: high -
[evidence](evidence/fa-e10-rust-model-resolution-is-user-jsonc-only.md). The
read path, tier classes, and chain arms were read at `0ff62b29a`.
Corrections to the spec's `265df096` citations: defaults `config.rs:123-125`
are now `:142-144`; `:274-404` is `:310-492`; `:325-370` is `:412-459`;
`:675-691` is `:762-778`; `:772-824` is `:859-883`; the checks `:1317`,
`:1424`, `:2045`, `:2158` are now `:1509`, `:1616`, `:2237`, `:2350`.
Existing check: `crates/daemon/src/config.rs:1509`
`tier_policy_ignores_project_models_and_rejects_project_lowering`; `:1616`
`compaction_enabled_defaults_true_and_is_user_tier_only`; `:2237`
`module_model_keys_replace_the_plugin_chain_only_when_the_module_model_is_set`;
`:2350` `mtime_cache_reuses_unchanged_reads_and_invalidates_on_mtime_change`;
`:1420` `an_oversized_tier_file_is_ignored_with_a_warning`; `:2390`
`unreadable_and_malformed_tiers_warn_while_missing_tiers_stay_silent`.
Impact: High. Rust resolution can disagree with plugin configuration.
Open questions:
- No test constructs a symlinked tier: unresolved, needs a symlink witness
  through `effective_for_env`.

### fa-e11-typescript-expands-user-config-and-rejects-module-keys

Type: safety
Reachability: default-production - every plugin boot loads both tiers
through `loadPluginConfigDetailed` (`packages/opencode-plugin/src/index.ts:77`).
Status: invalidated
Invalidated: PR #898 (#852) replaces two clauses under FA-N01: the module keys
are declared on the summarizer schema
(`packages/opencode-plugin/src/config/schema/eidnara.ts:121-141`), so they
are accepted, not rejected; and chain-key values are screened before
substitution (`screenUserTier`, `config/fold-authority.ts:97-121`, called at
`config/index.ts:137-140`), so a chain reference is excluded, not expanded.
Surviving clauses: `.jsonc` before `.json` (`shared/jsonc-parser.ts:227-243`),
user-tier substitution before parsing for the rest of the document
(`config/index.ts:141-147`), string-or-array fallbacks
(`config/schema/agent-overrides.ts:54-57`), and the project sanitizer's
strip list (`config/project-security.ts:23-39`, `:67`), now extended with the
two module keys and `context_limit_tokens`.
Exercised: yes - `config/config-paths.test.ts:66`, "keeps {env:} and {file:}
expansion enabled for user config" (`config/index.test.ts:689`), "leaves
{env:} and {file:} tokens literal in project config and warns" (`:715`), and
"keeps history_summarizer model selection user-owned when project config
tries to override it" (`:760`) cover the surviving clauses; the fixture rows
through `:1114` and `packages/pi-plugin/src/config/index.test.ts:765` pin the
replaced ones. All ran green in PR #904's `bun run check:repo` gate.
Guarantee: TypeScript discovers `.jsonc` before `.json`, expands user-tier
variables before parsing, accepts string-or-array fallbacks, rejects the
module keys as unknown, and its project sanitizer strips model, fallback,
disallowed-tool, hidden-agent escalation, disable, and cost-cap fields.
Check: `always` - per load. At HEAD the surviving clauses hold per load; the
replaced clauses are asserted by FA-N01.
Fault/timing angle: Configuration disagreement before startup.
Required faults and enabling state: The same variants as FA-E10; project
override controls.
Confidence: high -
[evidence](evidence/fa-e11-typescript-expands-user-config-and-rejects-module-keys.md).
The loader, screen, schema, and sanitizer were read at `0ff62b29a`.
Corrections to the spec's `265df096` citations: `config/index.ts:108-115` is
now `:141-147`; `config/project-security.ts:23-36` is `:23-39`; `:512-564`
is `:515-567`; the checks `config/index.test.ts:695` and `:740` are now
`:715` and `:760`.
Existing check: `packages/opencode-plugin/src/config/config-paths.test.ts:66`;
`packages/opencode-plugin/src/config/index.test.ts:689`, `:715`, `:760`,
`:1114`; `packages/pi-plugin/src/config/index.test.ts:765`.
Impact: High. A plugin-only reading of a key changes the chain the daemon
does not see.
Open questions:
- No sanitizer unit test names the module keys; the fixture row "a model
  only in the project tier" witnesses their effect. Resolved; recorded in
  the evidence file.

### fa-e12-cli-mode-resolution-does-not-use-the-plugin-loader

Type: safety
Reachability: default-production - every `eidnara setup` resolves modes
through `readEidnaraModes` (`resolveWriterModes`,
`packages/cli/src/commands/setup-opencode.ts:60-65`, called at `:554`).
Status: invalidated
Invalidated: PR #898 (#852) replaces the raw lenient read and the
compaction-only forwarding under FA-N01: `readEidnaraModes` calls the plugin's
`loadUserTierConfigDetailed` (`packages/cli/src/lib/eidnara-modes.ts:21`) and
`compactionEnabledFor` forwards the summarizer block (`:54-64`). PR #900
(#854) replaces "off-mode setup leaves native compaction untouched" under
FA-N08: native folds write `{ auto: true }` and leave `prune` as found
(`NATIVE_FOLDS_COMPACTION`, `setup-opencode.ts:267`). Surviving clause:
Eidnara-folds setup writes `auto` and `prune` false
(`EIDNARA_FOLDS_COMPACTION`, `:266`).
Exercised: yes - "derives every mode from the shared config through the plugin
loader" (`packages/cli/src/lib/eidnara-modes.test.ts:24`), "forwards the
summarizer chain after substitution and reference exclusion" (`:53`), the
writer tests (`packages/cli/src/commands/setup-opencode.test.ts:670`, `:689`),
and the command tests (`setup-opencode-authority.test.ts:136`, `:165`,
`:221`) ran green in PR #904's `bun run check:repo` gate.
Guarantee: CLI mode resolution reads raw lenient JSONC and forwards only the
compaction block; off-mode setup leaves native compaction untouched; on-mode
setup writes `auto` and `prune` false.
Check: `always` - per resolution and write proposal. At HEAD the surviving
clause holds per Eidnara-folds write; the replaced clauses are asserted by
FA-N01 and FA-N08.
Fault/timing angle: Invalid configuration or stale native settings.
Required faults and enabling state: A missing or malformed file; explicit
`false`; native `auto` already false.
Confidence: high -
[evidence](evidence/fa-e12-cli-mode-resolution-does-not-use-the-plugin-loader.md).
The resolver, writer, and setup selection were read at `0ff62b29a`.
Corrections to the spec's `265df096` citations: `lib/eidnara-modes.ts:15-48`
is now `:17-64`; `commands/setup-opencode.ts:99-101,150-168` is now
`:112-114` and `:163-175`; the check `lib/eidnara-modes.test.ts:21` is now
`:24`, renamed; `commands/setup-opencode.test.ts:632` and `:651` are now
`:670` and `:689`.
Existing check: `packages/cli/src/lib/eidnara-modes.test.ts:24`, `:53`,
`:68`, `:121`; `packages/cli/src/commands/setup-opencode.test.ts:670`
("leaves compaction untouched when compactionEnabled=false and turns it off
once mode is on", now driving the writer with an explicit patch), `:689`;
`packages/cli/src/commands/setup-opencode-authority.test.ts:136`, `:165`,
`:221`.
Impact: High. Off mode does not restore a disabled native authority.
Open questions: None.

### fa-e13-conflicts-disable-and-repair-toward-off

Type: safety
Reachability: default-production - every plugin boot with `enabled` runs
`detectConflicts` (`packages/opencode-plugin/src/index.ts:144`).
Status: invalidated
Invalidated: PR #899 (#853) replaces "any detected conflict disables server
participation and stops TUI initialization" with the disposition of FA-N07
(`index.ts:149-155`; `tui/index.tsx:1110-1116`) and "compaction repair writes
only `false`" with target-value repair (`fixConflicts`,
`shared/conflict-fixer.ts:110`, writes `:120-145`); PR #900 (#854) carries
the target values into doctor (`packages/cli/src/commands/doctor-opencode.ts:571-574`),
under FA-N08. Surviving clauses: `OPENCODE_DISABLE_AUTOCOMPACT` forces `auto`
false in the file-based arm (`shared/conflict-detector.ts:361`), and a
supplied resolved configuration bypasses it (`:106`); the flag is now also
reported unresolved by source (`:197-204`).
Exercised: yes - the file-based arm (`shared/conflict-detector.test.ts:1043`,
rows `:1064`, `:1079`), the resolved-arm bypass (`:743`), the `warn` boot
(`src/index.entry.test.ts:203`), and target-value repair
(`shared/conflict-fixer.test.ts:659`, `:689`) ran green in PR #899's and PR
#904's `bun run check:repo` gates.
Guarantee: Any detected conflict disables server participation and stops TUI
initialization; compaction repair writes only `false`; the environment flag
forces `auto` false in the file-based fallback, which a supplied resolved
configuration bypasses.
Check: `always` - per disposition. At HEAD the surviving clauses hold per
detection; the replaced clauses are asserted by FA-N07 and FA-N08.
Fault/timing angle: Mixed conflicts, inline override, environment-forced
settings.
Required faults and enabling state: Both modes; file and resolved arms;
conflicting plugins; an uneditable winning layer.
Confidence: high -
[evidence](evidence/fa-e13-conflicts-disable-and-repair-toward-off.md). The
detector, fixer, boot, and TUI branches were read at `0ff62b29a`.
Corrections to the spec's `265df096` citations: `conflict-detector.ts:79-143`
is now `:88-161`; `:94` is `:106`; `:295-297` is `:360-363`; `index.ts:111-113`
is `:144-155`; `tui/index.tsx:1055-1063` is `:1106-1121`;
`conflict-fixer.ts:87-100` is `:89-104`; `:128-159` is `:118-145`; the checks
`conflict-detector.test.ts:498-558` are now the `describe` at `:480` (rows
`:502`, `:558`); `conflict-fixer.test.ts:248` is `:290`; `:598-672` is `:705`
onward.
Existing check: `packages/opencode-plugin/src/shared/conflict-detector.test.ts:480`
("compaction-off mode matrix", rows `:502`, `:558`); `:743`; `:1043`;
`shared/conflict-fixer.test.ts:290`, `:659`, `:689`, `:705` ("compaction-off
mode parity", "does NOT flip compaction.auto to false when compaction-off"
at `:706`); `src/index.entry.test.ts:203`.
Impact: High. Without a warning-only disposition a user with no summarizer
loses the plugin over a host setting.
Open questions:
- TUI startup under `warn` has no runtime witness: unresolved, needs a TUI
  startup harness (shared with FA-N07).

### fa-e14-native-status-conflates-pruning-with-folding

Type: safety
Reachability: default-production - at HEAD compaction-off mode is the
default (empty chain, `packages/opencode-plugin/src/config/agent-disable.ts:21-29`),
so the field and label render under the default configuration. Correction:
the label `explicit-config-only` at `265df096` assumed an explicit `false`.
Status: invalidated
Invalidated: PR #899 (#853) replaces "native activity is `auto || prune`" with
`auto` alone and replaces "the TUI labels anything but explicit false as
native compaction" with three states (native, none, unknown), per FA-N07 and
D2c. The clause "an absent observation omits the field" survives in both
RPCs. PR #904 (#856) adds the fold-authority status beside it without
reintroducing either replaced clause.
Exercised: yes - the `bun run check:repo` gates of #899 through #904 ran the
RPC test (both RPCs agree; `auto = false, prune = true` gives `false`;
`auto = true` gives `true`; absence gives `undefined`; Eidnara folds gives
`undefined`) and the TUI label test; the plugin source at HEAD is
`d7b330113`'s, which differs from #904's head only in comments and generated
TUI source, and #859 PR A's `check:repo` ran on it.
Guarantee: At `265df096`, off-mode status reported native activity as
`auto || prune`; an absent observation omitted the field; the TUI labelled
anything but explicit false as native compaction.
Check: `always` - while Eidnara compaction is off, `native_compaction_active`
equals the host's resolved `compaction.auto`
(`packages/opencode-plugin/src/plugin/rpc-handlers.ts:789-792`) and is absent
when no observation exists (`:542-545`), identically in `sidebar-snapshot`
and `status-detail`; the TUI suffix is "compaction owner unknown" for
absence (`packages/opencode-plugin/src/tui/compaction-off.ts:9-12`). `always`
because every poll renders it.
Fault/timing angle: `auto` false with `prune` true; an unavailable host
observation.
Required faults and enabling state: Eidnara compaction off; each
observation value and absence; an Eidnara-folds control.
Confidence: high -
[evidence](evidence/fa-e14-native-status-conflates-pruning-with-folding.md).
Read at HEAD and at `265df096`. Corrections: `rpc-handlers.ts:764-769` is now
`:789-792`, `:528-531` is `:542-545`, the handlers at `:829-864` are at
`:865-908`, and `rpc-handlers.test.ts:470-522` is `:582-651`.
Existing check: `packages/opencode-plugin/src/plugin/rpc-handlers.test.ts:582`
"the snapshot carries the host's compaction ownership only while Eidnara
compaction is off"; `packages/opencode-plugin/src/tui/compaction-off.test.ts:75`
"names the owner from compaction.auto alone and marks an absent observation
unknown"; #904's `rpc-handlers.test.ts:179` and `:246` (pending and stalled
payloads). Replacement: FA-N07.
Impact: Medium. Displayed activity did not prove a fold authority existed.
Open questions:
- The doc comment on `CompactionOwnership`
  (`packages/opencode-plugin/src/plugin/rpc-handlers.ts:385-388`) said at
  `286ffaf81` that `nativeActive` covers `compaction.auto` or
  `compaction.prune`, while the code reads `auto` alone. Resolved: the
  comment was corrected at `d7b330113` and now says `nativeActive` reads
  `compaction.auto`; the behavior is correct.

### fa-n01-config-derived-authority

Type: safety
Reachability: default-production - the default configuration has an empty
chain and `compaction_enabled: true` (`crates/daemon/src/config.rs:142-144`),
so every daemon bind resolves the predicate through
`ConfigCache::effective_for_project` (`crates/daemon/src/lib.rs:4744`) and
every plugin boot through `loadPluginConfigDetailed`
(`packages/opencode-plugin/src/index.ts:77`).
Status: active
Exercised: yes - one 49-row fixture
(`packages/opencode-plugin/src/config/__fixtures__/fold-authority-parity.json`)
runs through the daemon's production file path in
`every_fixture_row_matches_through_the_configuration_cache`
(`crates/daemon/src/config.rs:2539`), green in #859 PR A's run at `fd0b52aa5`
(6,124 passed); through the OpenCode loader in "every row matches through the
production loader" (`packages/opencode-plugin/src/config/index.test.ts:1114`)
and the Pi loader in "every row matches through the Pi loader"
(`packages/pi-plugin/src/config/index.test.ts:765`), both green in PR #904's
`bun run check:repo` gate. PR #898 (#852) owns all three and their negative
controls.
Guarantee: Both production loaders derive the same ordered chain, the same
admission outcome, and the same boolean from equivalent inputs.
Check: `always` - for every fixture row both loaders return the row's
`expected_chain`, `expected_admission`, and `expected_eidnara_folds`, where
the boolean is `admitted && compaction.enabled !== false &&
chain.nonempty`, with module precedence, ECMAScript trimming, deduplication,
substitution outside the chain keys, `.jsonc`-then-`.json` selection,
last-admitted recovery, and user-tier restrictions preserved. `always`
because every resolution feeds the authority decision. Falsifier: Rust
selects a model while TypeScript selects native folding, or a pre-parsed or
raw read passes the fixture.
Fault/timing angle: Loader recovery across a rejected reload; substitution
elsewhere in the document; excluded references inside chain keys.
Required faults and enabling state: Malformed tiers; mixed-type fallback
arrays; unknown keys inside the authority blocks; `{env:` or `{file:` inside
a chain key; a substituted or string `compaction.enabled`; `.json`-only
files; project-only models; a rejected reload after an admitted load.
Confidence: high - [evidence](evidence/fa-n01-config-derived-authority.md).
Both loaders, the fixture and its `contract` field, and the negative controls
(`config.rs:2573`, `:2588`; `index.test.ts:1179`) were read at `0ff62b29a`.
Contract-versus-code: D1a says a rejected admission is "never `false`"; since
`f5b10fefe`, which merged with #898 after its description, both loaders
return `false` for an unresolved configuration
(`config.rs:181-185`; `withdrawUnresolvedFoldAuthority`,
`packages/opencode-plugin/src/config/fold-authority.ts:137-146`) and carry
`Unresolved` beside it, and the fixture contract encodes that reading. The
consumers found read the admission first
(`crates/daemon/src/fold_authority.rs:105-113`;
`packages/cli/src/commands/setup-opencode.ts:618-628`;
`packages/cli/src/commands/doctor-opencode.ts:539-542`).
Existing check: `crates/daemon/src/config.rs:2539`
`every_fixture_row_matches_through_the_configuration_cache`; `:2546`
`excluded_chain_values_warn_with_their_key`; `:2573`
`the_pre_parsed_merge_seam_fails_the_fixture`; `:2588`
`jsonc_only_discovery_fails_the_fixture`; `:2599`
`a_rejected_reload_keeps_the_last_admitted_configuration`; `:2664`
`a_first_load_rejection_keeps_the_tier_outside_the_authority_blocks`;
`:2711`
`an_unresolved_configuration_withdraws_fold_authority_from_the_retained_documents`;
`:2741` `retained_project_documents_are_bounded_and_the_user_document_survives_churn`;
`packages/opencode-plugin/src/config/index.test.ts:1114`, `:1124`, `:1143`,
`:1158`, `:1179`; `packages/pi-plugin/src/config/index.test.ts:765`.
Impact: High. The daemon and the plugin select different fold authorities for
one session.
Open questions:
- Does "`false` with a separate `Unresolved` admission" satisfy the falsifier
  "either side turns a rejected admission into `false`"? D1a says "never
  `false`"; the landed code and fixture say `false` plus `Unresolved`, and
  every consumer found gates on the admission (needs human input).

### fa-n02-first-commit-adopts-authority

Type: safety
Reachability: default-production - every transform pass plans its authority
from the store (`load_fold_authority`, `crates/memory-store/src/lib.rs:13237`;
`fold_authority::plan`, `crates/daemon/src/fold_authority.rs:95`) and stamps
the adoption into its commit (`stamp_fold_authority`,
`crates/daemon/src/transform.rs:2154`, called at `:2746` and `:3390`); a fresh
session under any admitted configuration adopts on its first committing pass.
Status: active
Exercised: yes - PR #903 (#855) gate and #859 PR A's run at `fd0b52aa5` (6,124
passed): `the_first_committing_pass_adopts_over_a_state_sync_row` (a real
`state_sync` row, both intents),
`a_losing_first_adopter_converges_on_the_winner` (a rival commit between the
loser's plan and commit), `a_fold_artifact_written_after_the_plan_replans_the_adoption`
(row-version fence; the PR records a failing negative control without it),
`an_unresolved_configuration_adopts_nothing`,
`legacy_rows_adopt_by_their_fold_artifacts`, the 128-case
`the_transition_table_follows_the_session_authority_rules`, and
`a_non_boolean_fold_authority_is_a_serde_error_on_every_read`.
Guarantee: Only the first accepted committing pass adopts the
binding-derived authority for an unadopted session; competing routes converge
on one value.
Check: `always` - at every committed transform, `eidnara_folds` changes from
`None` to `Some(v)` only when the committing pass planned `adopt = Some(v)`
from an admitted intent at the row version it loaded, and never changes from
`Some(_)` outside the authority reset. `always` because every committing
pass passes through `stamp_fold_authority`. Falsifier: the losing pass
establishes its authority, a default `false` blocks adoption of `true`, or
row existence is taken as adoption.
Fault/timing angle: A rival adoption or artifact write between the plan's
`load_fold_authority` and the pass commit; a state-sync or diagnostic write
that creates the row before any transform.
Required faults and enabling state: Two bindings with different intents on
one session; a forced CAS loss after the plan; a row created by state sync;
an unresolved admission; keyless rows with and without fold artifacts; a
non-boolean stored value.
Confidence: high - [evidence](evidence/fa-n02-first-commit-adopts-authority.md).
The field's serde default and skip (`crates/memory-store/src/lib.rs:2460-2465`),
the SQL read and Serde error (`:13200-13243`), the planner's adopt arms
(`fold_authority.rs:105-112`), the admission-aware intent
(`crates/daemon/src/config.rs:181-185`), and the row-version fence
(`transform.rs:2158-2163`) were read at HEAD; the witnesses were run in the
cited gates. The competing adopter is a direct store commit from the
pre-commit hook, not a second handler pass.
Existing check: `crates/daemon/src/fold_authority_handler_tests.rs:135`
`the_first_committing_pass_adopts_over_a_state_sync_row`; `:163`
`a_losing_first_adopter_converges_on_the_winner`; `:742`
`a_fold_artifact_written_after_the_plan_replans_the_adoption`; `:203`
`an_unresolved_configuration_adopts_nothing`; `:329`
`legacy_rows_adopt_by_their_fold_artifacts`;
`crates/daemon/src/fold_authority.rs:200`
`the_transition_table_follows_the_session_authority_rules`;
`crates/memory-store/src/lib.rs:29612`
`a_non_boolean_fold_authority_is_a_serde_error_on_every_read`.
Impact: High. Routes that read different authorities for one session mix
fold coordinates.
Open questions:
- Is a rival commit from the pre-commit hook a sufficient stand-in for two
  concurrent first passes through two routes? Resolved: it moves the row
  version exactly as a second pass would, so the fence is exercised; a
  two-route interleaving is a refinement.

### fa-n03-authority-change-requires-quiescent-bind

Type: safety
Reachability: explicit-config-only - a change needs an adopted session and a
later bind whose admitted configuration derives the other authority
(`fold_authority.rs:113-130`), which only a configuration edit or a second
configuration root produces.
Status: active
Exercised: yes - PR #903 (#855) gate and #859 PR A's run at `fd0b52aa5`:
`a_quiescent_bind_changes_authority_in_both_directions_through_the_reset`
(epoch + 1 per change), `a_sibling_binding_keeps_the_change_pending_until_a_quiescent_bind`,
`a_sibling_bound_during_the_change_keeps_it_pending` (sibling inserted after
the plan; the PR records a failing negative control without the fence),
`a_busy_summarizer_keeps_the_change_pending_across_a_restart` (binding table
cleared, store kept), `an_emergency_rerun_after_publication_keeps_the_change_pending`,
`an_ordinary_recomp_reset_preserves_the_adopted_authority`, and the store
tests for the replacement write, a busy summarizer, and a pending
publication on an Idle session.
Guarantee: A disagreeing bind changes authority only with no sibling
binding, an Idle summarizer, and no pending publication, validated inside
the reset transaction, and the replacement value is written in that
transaction.
Check: `always-or-unreached` - whenever `eidnara_folds` changes from
`Some(a)` to `Some(b)`, the same transaction advanced `revert_epoch`, emptied
core and metadata, deleted identity rows, and observed summarizer Idle and no
pending publication row (`crates/memory-store/src/lib.rs:13272-13283`, `:13301`,
`:13328`), and the change ran under the bindings mutex with no participating
sibling (`crates/daemon/src/lib.rs:301-313`). `always-or-unreached` because
most sessions never change authority. Falsifier: authority changes without
the epoch-fenced reset; a blocked change alters the serving mode; an ordinary
recomp reset changes authority.
Fault/timing angle: A sibling binds, or the summarizer leaves Idle, or a
publication becomes pending, between the plan and the reset.
Required faults and enabling state: An adopted authority; a changed
configuration; each quiescence blocker alone; a sibling bind interleaved
after the plan; a recomp reset; an emergency rerun.
Confidence: high -
[evidence](evidence/fa-n03-authority-change-requires-quiescent-bind.md). The
planner's change and pending arms, the fence, the in-transaction re-check,
the replacement write beside the epoch advance, and recomp's preservation
(`reset_session_for_recomp`, `crates/memory-store/src/lib.rs:13213`) were read
at HEAD. The "restart" in the busy-summarizer witness clears the binding
table in place rather than reopening the handler.
Existing check: `crates/daemon/src/fold_authority_handler_tests.rs:219`
`a_quiescent_bind_changes_authority_in_both_directions_through_the_reset`;
`:245` `a_sibling_binding_keeps_the_change_pending_until_a_quiescent_bind`;
`:778` `a_sibling_bound_during_the_change_keeps_it_pending`; `:289`
`a_busy_summarizer_keeps_the_change_pending_across_a_restart`; `:656`
`an_emergency_rerun_after_publication_keeps_the_change_pending`; `:314`
`an_ordinary_recomp_reset_preserves_the_adopted_authority`;
`crates/memory-store/src/lib.rs:29658`
`the_authority_reset_writes_its_replacement_and_ordinary_resets_keep_the_authority`;
`:29691` `a_busy_summarizer_refuses_the_authority_reset`; `:29718`
`a_pending_publication_refuses_the_authority_reset_of_an_idle_session`.
Impact: High. A change under a live firing publishes old coordinates into a
session that claims the new authority.
Open questions:
- Does a reopened handler over a store with a non-Idle summarizer keep the
  change pending? Unresolved, needs a busy-summarizer case through the
  `restarted` helper; the store-level refusal covers the durable check.

### fa-n04-quiescent-change-completes

Type: liveness
Reachability: explicit-config-only - requires the same disagreeing
configuration as FA-N03 on a quiescent session.
Status: active
Exercised: partial - obligation (a) only. PR #903 (#855) gate and #859 PR A's run
at `fd0b52aa5`:
`authority_changes_survive_restarts_and_serve_first_passes` (folded session,
reopen, native change, reopen, Eidnara change on `big_messages_from(40)`
committing with no coverage and epoch + 1),
`a_descended_target_whose_binding_disagrees_resets_to_a_first_pass`, and the
100 seeded histories of `bounded_operation_histories_follow_the_authority_model`.
Obligation (b) is not exercised: no test kills
the process between the reset commit and the same pass's commit, and the
final slice is not headed by an OpenCode compaction summary.
Guarantee: With quiescence maintained and storage available, one disagreeing
bind applies the reset and the next accepted transform starts as a first
pass on the current array.
Check: `always-or-unreached` - two bounded obligations. (a) Fault-free
recovery: after interference stops (no rival committer, no sibling binding,
no durable quiescence blocker), with quiescence and an admissible request
maintained, the first transform invocation that plans the change completes
at that invocation's exit: it commits one authority reset, then commits the
pass with `coverage_ordinal` `None` and `revert_epoch` advanced by exactly
one. The bound is one call of `apply_once_with_estimator`
(`crates/daemon/src/transform.rs:2013`): the attempt counter starts at 0
(`:2019`), each retry requires it below `MAX_CAS_RETRIES = 8` (`:81`,
`:2027`, `:2061`, `:2094`, `:2113`), and the committed reset takes one
increment (`:2045-2054`), so the call makes at most nine attempts, with the
counter at 0 through 8, and one authority reset. (b) Post-kill: after a
process death between the reset commit and the pass commit, and a reopen,
one reopened pass completes without a second authority reset.
`always-or-unreached` because the change is optional; both bounds are in
attempts and invocations, not time.
Fault/timing angle: Process death after the reset commit and before the
pass's own commit; a restart before the next transform.
Required faults and enabling state: A quiescent adopted session; a
disagreeing admitted bind; a native slice; for (a), interference stopped
before the invocation; for (b), a test-only kill point after
`reset_session_for_authority` commits, then a reopen.
Confidence: medium - [evidence](evidence/fa-n04-quiescent-change-completes.md).
The attempt loop, the reset's full clearing
(`crates/memory-store/src/lib.rs:13285-13355`),
and the empty binding table after reopen were read at HEAD. The restart
witnesses fall between transform calls, so the kill window inside one call
is argued from the reset leaving no coverage, not tested.
Existing check: `crates/daemon/src/fold_authority_handler_tests.rs:840`
`authority_changes_survive_restarts_and_serve_first_passes`; `:591`
`a_descended_target_whose_binding_disagrees_resets_to_a_first_pass`; `:219`
`a_quiescent_bind_changes_authority_in_both_directions_through_the_reset`;
`:1030` `bounded_operation_histories_follow_the_authority_model`.
Impact: High. A transition that stays pending forever, or a first pass that
reuses pre-change history.
Open questions:
- Does a first pass after a kill between the reset and the pass commit serve
  the current slice without pre-change state? Unresolved, needs a test-only
  kill point and a reopen.

### fa-n05-native-authority-gates-fold-work

Type: safety
Reachability: default-production - an empty chain is the default, so a fresh
session adopts native (`DaemonConfig::eidnara_folds`,
`crates/daemon/src/config.rs:181-185`) and every non-subagent pass reaches the
gate (`crates/daemon/src/lib.rs:9423-9434`).
Status: active
Exercised: partial - PR #903 (#855) gate and #859 PR A's run at `fd0b52aa5`:
`native_authority_skips_every_fold_step_even_with_a_live_chain` (first-unit
caller, live chain non-empty, preparation full-load count unchanged, trigger
timings zero, no producer start or connect, no `last_no_fire`, a repeat
writes no row) and `wrapup_is_refused_under_native_authority`. That
first-unit witness runs at 45,000 of 50,000 tokens, 90 percent
(`crates/daemon/src/lib.rs:24440-24442`), below `Emergency95`; no handler
test under native authority runs at or above 95 percent. The second caller
(`lib.rs:9077`) is not a reachable native state: native preparation returns
`Complete` (`:9428-9432`), which settles in the first unit whatever the
pressure (`:8918-8942`), and the rerun follows only a completed `Busy` wait
(`:8957-8974`), which native authority never produces. Both callers call
the one gated function (`:8924`, `:9077`), so the second caller is covered
structurally by the shared gate, not by a witness. Wrapup's snapshot and
boundary work are not counted.
Guarantee: Under native authority the shared preparation entry returns
before its store load, trigger evaluation, or firing, from both callers, and
`session.wrapup` refuses before snapshot or boundary work.
Check: `unreachable` - under an applied native authority,
`prepare_history_summarizer_fire` (`crates/daemon/src/lib.rs:5299`) and
`record_no_fire` (`:5895`) never execute from either caller (`:8924`,
`:9077`), and the wrapup handler returns `reason = "native_authority"` before
the transform snapshot read (`:7306`). `unreachable` because these are named
code points that must not run. Falsifier: live configuration bypasses stored
authority; any `record_no_fire` commit.
Fault/timing angle: A configuration change after binding that makes the live
chain non-empty; an Eidnara-intent sibling over a stored native session.
Required faults and enabling state: A stored native authority with a
non-empty live chain; a first-unit pass, including one at or above 95
percent of the context limit (the native high-pressure first-unit case);
a `session.wrapup` call. The rerun caller has no native construction: it
needs a completed `Busy` wait, which native preparation never returns.
Confidence: high -
[evidence](evidence/fa-n05-native-authority-gates-fold-work.md). The gate is
the first statement of the shared entry and reads the pass's applied
authority (`crates/daemon/src/transform.rs:2117-2122`), not the live
configuration; both callers call it. The wrapup refusal follows an entry
`store.load` (`lib.rs:7264`), which the record's "before snapshot or
boundary work" allows.
Existing check: `crates/daemon/src/fold_authority_handler_tests.rs:46`
`native_authority_skips_every_fold_step_even_with_a_live_chain`; `:107`
`wrapup_is_refused_under_native_authority`; `:245` and `:778` (native
diagnostics while a change is pending).
Impact: High. Unauthorized folding and hidden segment creation beside the
harness's own compaction.
Open questions:
- Does a native pass at or above 95 percent settle in the first unit with
  `native_authority`? Unresolved, needs that first-unit witness. The rerun
  caller is not reachable under native authority, so its coverage is the
  shared gate, not a witness.
- Should wrapup's snapshot and boundary reads be counted? Needs human input:
  PR #903 lists the counters as follow-up coverage pending an owner decision.

### fa-n06-native-state-stays-additive

Type: safety
Reachability: default-production - native is the default authority, and
every native non-subagent pass takes the additive path
(`crates/daemon/src/transform.rs:3207-3209`).
Status: active
Exercised: partial - PR #903 (#855) gate and #859 PR A's run at `fd0b52aa5`:
`assert_native_state` (no identity rows, no `tail_hygiene_baseline`, no
coverage, no folded sequence) runs after native passes in
`native_authority_skips_every_fold_step_even_with_a_live_chain`,
`a_quiescent_bind_changes_authority_in_both_directions_through_the_reset`,
`authority_changes_survive_restarts_and_serve_first_passes` (from a folded
session with identity rows), `a_resent_descent_after_a_change_to_native_is_acknowledged_as_a_replay`,
`native_metadata_does_not_grow_with_the_message_count`, and every native
step of the bounded histories; the store test
`a_native_authority_state_sync_keeps_the_session_free_of_fold_coordinates`
covers state sync with seeds. The "bootstrap" writer the record names is not
identified by any witness.
Guarantee: Native-mode accepted metadata contains no fold coverage, identity
map entries, served fingerprints, or tail baseline; shared ingress scalars
and additive injections remain.
Check: `always` - after every commit to a session whose stored authority is
`Some(false)`: no `block_identities` rows, `coverage_ordinal` and
`tail_hygiene_baseline` absent, `folded_history_segment_seq == 0`, and no
derived state promoted (`derived: None`, `transform.rs:3162`), while
`newest_live_block_id` and `last_usage` follow ingress. `always` because
every native writer must hold it. Falsifier: identity adoption runs through
shared ingress; seeding repopulates forbidden state.
Fault/timing angle: The authority reset into native, state sync with seeds,
lineage descent, and the additive passes that follow.
Required faults and enabling state: A large prior folding state followed by
a legal native transition; then additive passes, a seeded state sync, and a
descent edge.
Confidence: high - [evidence](evidence/fa-n06-native-state-stays-additive.md).
The additive path calls only `apply_ingress_scalars` (`transform.rs:2904`,
`:5583`); `apply_ingress_identities` (`:5602`) is called only on the folding
path (`:4159`); native state sync empties the segment batch
(`crates/memory-store/src/lib.rs:11434-11439`). Served fingerprints and
baseline parts are not `ModuleMeta` fields after #905 and #906.
Existing check: `crates/daemon/src/fold_authority_handler_tests.rs:46`,
`:219`, `:840`, `:555`, `:645`, `:1030` (through `assert_native_state`,
`:19`); `crates/memory-store/src/lib.rs:25162`
`a_native_authority_state_sync_keeps_the_session_free_of_fold_coordinates`;
`crates/daemon/src/transform_read_bound.rs:756`
`native_pass_reads_are_bounded_independent_of_stored_rows`; folding-path
counterpart `crates/daemon/src/transform.rs:14628`
`a_completed_tail_that_turns_provisional_is_removed_and_re_adopted_exactly`.
Impact: High. Native sessions would keep message-proportional state that no
reader uses.
Open questions:
- Does "bootstrap" mean the state-sync seed boundary (witnessed) or the
  lineage `AlreadyBootstrapped` disposition (not constructed under native
  authority)? Needs human input.

### fa-n07-conflict-disposition-is-authoritative

Type: safety
Reachability: default-production - every plugin boot with `enabled` runs
`detectConflicts` (`packages/opencode-plugin/src/index.ts:137-155`), and with
the default empty chain an OpenCode `compaction.auto = false` left by an
earlier setup is the `warn` state.
Status: active
Exercised: partial - PR #899 (#853) owns the detector matrix
(`packages/opencode-plugin/src/shared/conflict-detector.test.ts:502`), the
mixed rows (`:558`), the unresolved-source tests (`:578`, `:589`, `:606`),
the fixer (`shared/conflict-fixer.test.ts:659`), both status RPCs
(`plugin/rpc-handlers.test.ts:582`), the TUI label
(`tui/compaction-off.test.ts:75`), and the entry-bundle boot "a configuration
with no fold authority boots with a warning and keeps the RPC server"
(`src/index.entry.test.ts:203`); PR #904 (#856) adds the pending and
root-mismatch `warn` (`shared/fold-authority-status.test.ts:282`,
`plugin/rpc-handlers.test.ts:179`). All ran green in #899's and #904's
`bun run check:repo` gates. No runtime witness covers TUI startup under
`warn` (`tui/index.tsx:1110-1116`); #899 records typecheck and the
compiled-TUI freshness check as its only coverage, and no test asserts the
sidebar stays up.
Guarantee: Detection produces one desired compaction target, one actionable
patch, its unresolved entries, and one disposition, consumed consistently by
server boot, TUI, setup, doctor, and formatting.
Check: `always` - native folds with resolved `auto = false` alone give `warn`
with desired target `auto = true` (`shared/conflict-detector.ts:117-121`);
the actionable patch carries `{ auto: true }` only when the config files
control the setting, and an `OPENCODE_DISABLE_AUTOCOMPACT`, inline
`OPENCODE_CONFIG_CONTENT`, or external host-layer override yields an empty
patch plus a source-specific `unresolved` entry (`splitCompactionPatch`,
`:172-195`; `compactionOverrideSource`, `:197-204`); any DCP, OMO, or
Eidnara-folds `auto`/`prune` conflict gives `disable`, dominating `warn`;
otherwise `none` (`conflictDisposition`, `:166-170`); each
consumer branches on that value only; `native_compaction_active` equals the
host's resolved `compaction.auto` and is absent without an observation
(`plugin/rpc-handlers.ts:543-545`, `:790-791`). `always` because every boot
and CLI run evaluates it. Falsifier: a warning disables RPC or the sidebar;
prune or absence reports native compaction active.
Fault/timing angle: Mixed conflict classes; a missing host observation;
environment or inline sources a file edit cannot change.
Required faults and enabling state: Warn plus DCP or OMO; `auto = false,
prune = true`; an absent observation; `OPENCODE_DISABLE_AUTOCOMPACT` or
inline `OPENCODE_CONFIG_CONTENT` forcing `auto = false`; for #904, a daemon
summary with a pending target or a different configuration root.
Confidence: high -
[evidence](evidence/fa-n07-conflict-disposition-is-authoritative.md). The
detector, fixer, boot, TUI, setup, doctor, diagnostics, and RPC consumers
were read at `d7b330113`. At `286ffaf81` two comments disagreed with the
code, and `d7b330113` corrected both: the TUI said `isCompactionEnabled`
"defaults to `true`" after a load failure (`tui/index.tsx:1098` there), but
it is `false` for `{}` (`config/agent-disable.ts:21-29`), and `d7b330113`
removed the comment; `CompactionOwnership` said `nativeActive` reads `auto`
or `prune` (`plugin/rpc-handlers.ts:385-386` there), and it now says
`auto`, which the code reads alone (`plugin/rpc-handlers.ts:791`).
Existing check: `packages/opencode-plugin/src/shared/conflict-detector.test.ts:502`
(seven mode-by-host rows asserting disposition, patch, and flags); `:558`
(native `warn` plus DCP or OMO gives `disable` and keeps `{ auto: true }`);
`:578`, `:589`, `:606` (environment, inline, and host-only `auto = false`:
`warn`, an empty patch, and one unresolved entry naming the source); `:625`
("formats a warning without claiming Eidnara is disabled");
`shared/conflict-fixer.test.ts:659`, `:679`, `:689`;
`plugin/rpc-handlers.test.ts:582`, `:179`; `tui/compaction-off.test.ts:75`;
`src/index.entry.test.ts:203`; `shared/fold-authority-status.test.ts:282`;
`plugin/conflict-warning-hook.test.ts:198` (a warning replaces one the other
disposition left).
Impact: High. A `warn` that disables loses memory features for a user with
no summarizer; a false "native compaction" label hides a session with no
fold authority.
Open questions:
- After a configuration-load failure the TUI detects under native folds
  (`tui/index.tsx:1098-1106`) and shows `warn`, while the server refuses to
  start (`index.ts:77`). Is native the intended TUI reading of a load failure,
  or should it be unresolved (needs human input)?
- TUI startup under `warn` has no runtime witness: unresolved, needs a TUI
  startup harness.

### fa-n08-repair-uses-the-proposed-document

Type: safety
Reachability: explicit-config-only - reached only when a user runs
`eidnara setup` (`runSetup`, `packages/cli/src/commands/setup-opencode.ts:486`)
or `eidnara doctor` (`runDoctor`, `packages/cli/src/commands/doctor-opencode.ts:279`;
repairs only with `--force`).
Status: active
Exercised: yes - PR #900 (#854) owns the 21 command-level setup tests in
`packages/cli/src/commands/setup-opencode-authority.test.ts`, each in isolated
`HOME`, XDG, and `PATH` directories: keep versus remove (`:230`), thrown
write failure (`:438`, `:465`), environment-forced `auto = false` (`:357`),
SIGKILLed child between the writes (`:407`, `:424`), declined repairs
(`:337`, `:347`), and unloadable proposal or project tier (`:369`, `:323`);
and doctor's "treats a configuration that does not load as unresolved and
leaves host settings alone" (`doctor-opencode.test.ts:441`). All ran green in
#900's and #904's `bun run check:repo` gates.
Guarantee: Setup normalizes and validates its proposed document before
deriving authority, requesting consent, or choosing the host patch; repaired
files are re-detected; consent never changes the authority decision; a
loader failure is unresolved, not native.
Check: `always-or-unreached` - at setup or doctor completion: the authority
printed and the host patch applied equal `foldAuthorityOf` of the proposed
document validated by `loadUserTierConfigText`; an unresolved proposal or
project tier writes no host file; a declined repair writes no compaction key
and leaves the authority unchanged; "Written, restart required" appears only
when the read-back matches both files; doctor leaves host files unchanged
for an unresolved configuration. `always-or-unreached` because the path runs
only on an explicit command.
Fault/timing angle: The window between the two file writes; a declined
repair; environment or inline sources a file edit cannot change.
Required faults and enabling state: Keep-chain versus remove-all-fields; a
thrown failure between the writes; `OPENCODE_DISABLE_AUTOCOMPACT` forcing
`auto = false`; a process kill between the writes; an unloadable proposal and
project tier.
Confidence: high -
[evidence](evidence/fa-n08-repair-uses-the-proposed-document.md). The setup
order (`setup-opencode.ts:603-628` before consent at `:643-692`), freshness
checks (`:726-742`), authority-ordered writes (`:784-795`), rollback and
read-back (`:816-834`, `:899-920`), and doctor's unresolved branch
(`doctor-opencode.ts:79-98`, `:539-542`) were read at `0ff62b29a`.
Correction to #900's description: `betweenWrites` is not fixed after the
OpenCode write; `e414db3e0`, merged with #900, orders the writes by
authority, and `setup-opencode-authority.test.ts:407` and `:424` cover both
orders.
Existing check: `packages/cli/src/commands/setup-opencode-authority.test.ts:136`,
`:151`, `:165`, `:176`, `:197`, `:221`, `:230`, `:253`, `:262`, `:276`,
`:295`, `:306`, `:323`, `:337`, `:347`, `:357`, `:369`, `:407`, `:424`,
`:438`, `:465`; `packages/cli/src/commands/doctor-opencode.test.ts:359`
(authority line and native `auto = true` repair without a registered
plugin), `:398` (native repair beside a blocking DCP conflict), `:441`.
Impact: High. Setup leaves zero or two configured fold authorities, or
reports a partial write as done.
Open questions:
- D2 says a failure between the writes is reported as "written, restart
  required" with a read-back; setup rolls back first and reports "rolled
  back" or a mismatch. #900 records the deviation for the owner, and the
  falsifier holds; does the owner accept it (needs human input)?

### fa-n09-only-accepted-results-publish-derived-state

Type: safety
Reachability: explicit-config-only - only the Eidnara folding path proposes
derived state. `apply_once` returns through `apply_additive_only` unless the
session's stored authority folds (`crates/daemon/src/transform.rs:3207-3208`),
the additive result carries `derived: None` (`:3162`), and a session adopts
Eidnara folds only from a configuration with a non-empty user-tier chain
(`DaemonConfig::eidnara_folds`, `crates/daemon/src/config.rs:181-185`).
Status: active
Exercised: yes - #906's recorded `cargo test --workspace --all-features
--locked` gate runs the promotion family:
`committing_and_no_write_passes_promote_under_a_charged_lease_and_a_restart_forgets`
(commit and no-write promotion, charged prior lease, restart absence),
`a_pass_that_loses_every_compare_and_swap_publishes_nothing` (rival on every
attempt), `a_sibling_route_waits_for_promotion_and_a_delete_revokes_the_paused_incarnation`,
`a_session_purged_while_its_pass_awaits_promotion_stays_absent`,
`a_caller_cancelled_after_the_commit_still_promotes_once_the_worker_finishes`,
`a_committed_pass_whose_boundary_read_fails_still_promotes`,
`a_worker_lost_between_commit_and_promotion_leaves_the_commit_and_no_derived_state`,
and the child-process SIGKILL cut
`a_process_killed_between_commit_and_promotion_keeps_the_commit_and_forgets_the_derived_state`.
Each also ran green in the #859 PR A run at `fd0b52aa5` (6,124 passed).
Guarantee: Only an accepted transform promotes complete immutable derived
state, including a successful no-write pass; losers publish nothing; caller
cancellation after commit still promotes when the worker finishes; process
death between commit and promotion leaves absence.
Check: `always-or-unreached` - on every successful installation by
`promote_derived` (`crates/daemon/src/lib.rs:2152-2188`): provenance, the
installed state's `accepted_row_version` is the proposing pass's committed
row version or, for a pass that wrote nothing, the row version it read,
re-read as still current at acceptance
(`crates/daemon/src/derived_state.rs:70-92`), and the pass's snapshot
generation is still InFlight or Ready; ordering, the installed
`(revert_epoch, accepted_row_version)` is not below the retained one. After
a rejected promotion the retained state is unchanged (an unknown,
superseded, or removed generation, or an older pair, `lib.rs:2153-2160`) or
absent (a state over the retained-byte limit, `:2171-2176`). An unaccepted
transform promotes nothing: an error returns before promotion (`:9242`),
and a no-write proposal whose read version is no longer current is not
accepted. An accepted commit whose later response construction fails still
promotes: the post-commit boundary read failure is carried in the output
and becomes the error only at `finished()`, after promotion (`:9243-9244`,
`crates/daemon/src/transform.rs:1742-1747`). `always-or-unreached` because
promotion runs only on the folding path, which a default configuration
never enters.
Fault/timing angle: CAS loss; caller cancellation after the commit; a sibling
route's pass on the same session waiting in the lane; session delete, purge,
route replacement, or recomp `begin` during the pause between commit and
promotion; eviction; process death or worker unwind in that pause; a
post-commit boundary-read failure.
Required faults and enabling state: A folding session with a retained prior
state; a pause at the `promote_derived:<session>` attempt hook (test builds)
or at the fixture halt point
(`EIDNARA_FIXTURE_HALT_BEFORE_DERIVED_PROMOTION`, `direct-host-fixture`
feature); a rival committer on every attempt; a second route bound to the
session; a real `session.delete`; SIGKILL of the fixture child.
Confidence: high -
[evidence](evidence/fa-n09-only-accepted-results-publish-derived-state.md).
Read at `0ff62b29a`: `ProposedDerivedState::accept`
(`crates/daemon/src/derived_state.rs:70-92`), `DerivedState::supersedes`
(`:30-33`), `TransformSnapshotCache::lease_derived` and `promote_derived`
(`crates/daemon/src/lib.rs:2141-2186`), the generation check (`:2241-2249`),
`remove` clearing derived state (`:2251-2257`), the lease taken before `begin`
in `first_unit` (`:8899-8900`), and promotion before `finished()`
(`:9242-9244`; `promote_derived_state` `:9247-9276`). The verification
strategy's reference model `(epoch, accepted_version, derived_payload,
lease)` is not built; the tests assert outcomes directly.
Existing check: `crates/daemon/src/lib.rs:35175`
`committing_and_no_write_passes_promote_under_a_charged_lease_and_a_restart_forgets`;
`:35399` `a_pass_that_loses_every_compare_and_swap_publishes_nothing`;
`:35437` `a_committed_pass_whose_boundary_read_fails_still_promotes`; `:35501`
`a_sibling_route_waits_for_promotion_and_a_delete_revokes_the_paused_incarnation`;
`:35603` `a_session_purged_while_its_pass_awaits_promotion_stays_absent`;
`:35623`
`a_worker_lost_between_commit_and_promotion_leaves_the_commit_and_no_derived_state`;
`:35651`
`a_caller_cancelled_after_the_commit_still_promotes_once_the_worker_finishes`;
`:21722`
`derived_state_keeps_the_newest_acceptance_of_a_live_generation_under_the_shared_budget`
(cache unit: older epochs and versions ignored, unknown, superseded, and
removed generations refused, exact lease charge, shared eviction);
`crates/daemon/src/derived_state.rs:122`
`newer_epochs_and_versions_supersede_and_keys_select_by_epoch_and_generation`;
`:148` `a_no_write_proposal_is_accepted_only_while_its_read_version_is_current`;
`crates/daemon/tests/derived_state_crash_cut.rs:76`
`a_process_killed_between_commit_and_promotion_keeps_the_commit_and_forgets_the_derived_state`.
All #906.
Impact: High. Speculative or older-incarnation derived state steers later
hint deferral, divergence reporting, and tail-hygiene decisions.
Open questions:
- Resolved: when the active-lease budget refuses (`MAX_ACTIVE_SNAPSHOT_LEASES`
  8, `crates/daemon/src/lib.rs:890`), `lease_derived` returns `None` and the
  pass reads absence. Its accepted proposal carries no baseline parts, and
  since `96aad0baf` (#906 on `main`) promotion keeps the retained parts of
  the same revert epoch instead of replacing them with absence
  (`:2161-2170`). Witnesses:
  `a_pass_refused_a_lease_keeps_the_retained_baseline_parts` (`:35255`) and
  `a_promotion_without_parts_keeps_the_retained_parts_of_its_epoch_only`
  (`:35288`).
- Is the direct-assertion test family accepted in place of the reference
  model the verification strategy names? (needs human input)

### fa-n10-unknown-served-history-defers-hints

Type: safety
Reachability: explicit-config-only - hint admission runs inside `apply_once`
past the native gate (`crates/daemon/src/transform.rs:3207-3208`, admission
`:4212-4217`); the additive path decides no hint and carries no derived
state (`:3162`).
Status: active
Exercised: yes - #906's recorded workspace gate runs the predicate table
`a_new_hint_defers_only_when_its_target_may_have_been_served_outside_a_bust`
(unknown, unknown plus bust, known-empty, served block, served whole message,
other block, empty-hint cases), the fixture lifecycle
`a_new_hint_defers_after_a_restart_forgets_what_was_served` (with retained
history a `SOFT+` pass admits the hint; after a fixture restart the same
hint is deferred and absent, a repeated `SOFT+` keeps it absent, and the next
`HARD` serves it once), and
`divergence_reports_nothing_against_an_unknown_served_history`. #859 PR A's run
adds `a_new_hint_is_skipped_while_every_deferral_slot_is_taken`.
Guarantee: Missing retained fingerprints mean unknown, not known-empty;
unknown defers a new non-empty hint unless the pass independently busts.
Check: `always-or-unreached` - at hint admission `user_hint_deferred` returns
true for a non-empty hint on a non-bust pass whenever the retained served
history for the session's revert epoch is absent, and false for a known-empty
history, an empty hint, or a bust; when 16 deferrals are already pending the
hint is skipped as `DeferralsFull` rather than admitted. Neither branch lets
unknown history authorize an immediate prefix mutation. Divergence against an
unknown history reports nothing. `always-or-unreached` because admission runs
only on the folding path.
Fault/timing angle: Daemon restart, eviction, `remove`, lease-budget refusal,
or a revert-epoch mismatch before a non-busting pass.
Required faults and enabling state: A folding session with no retained
derived state and an eligible non-empty hint on a non-bust pass; a
known-empty control; a bust control.
Confidence: high -
[evidence](evidence/fa-n10-unknown-served-history-defers-hints.md). Read
`user_hint_deferred` (`crates/daemon/src/transform.rs:7906-7924`),
`prior_served` (`:7926-7931`), `DerivedState::served_for`
(`crates/daemon/src/derived_state.rs:14-16`), the admission call and the
deferral cap (`crates/daemon/src/transform.rs:4212-4227`,
`MAX_PENDING_USER_HINT_BLOCK_IDS` `:148`), and the divergence call sites
(`:3543-3544`, `:3621-3622`, `:5125-5126`) at `0ff62b29a`.
Existing check: `crates/daemon/src/transform.rs:13329`
`a_new_hint_defers_only_when_its_target_may_have_been_served_outside_a_bust`
(#906); `crates/daemon/tests/eval_surface_ledger.rs:442`
`a_new_hint_defers_after_a_restart_forgets_what_was_served` (#906);
`crates/daemon/src/transform.rs:13384`
`divergence_reports_nothing_against_an_unknown_served_history` (#906);
`crates/daemon/tests/eval_surface_ledger.rs:533`
`a_new_hint_is_skipped_while_every_deferral_slot_is_taken` (#859 PR A).
Impact: High. Unknown history treated as empty lets a new hint mutate a
prefix the provider may have cached, a cache-stability violation.
Open questions: None.

### fa-n11-baseline-validity-requires-a-matching-join

Type: safety
Reachability: explicit-config-only - tail-hygiene refresh runs inside
`apply_once` past the native gate (`crates/daemon/src/transform.rs:3207-3208`,
join `:4953-4976`); `session.status` reports a tail-hygiene object only when
the durable row holds a baseline, which only a folding pass writes
(`crates/daemon/src/lib.rs:6881-6899`, write at
`crates/daemon/src/transform.rs:4963`).
Status: active
Exercised: yes - #906's recorded workspace gate runs the join table
`durable_scalars_join_retained_parts_by_generation` (no baseline gives a
fresh generation-1 baseline; missing and wrong-generation parts invalidate,
stay invalid without a bust, and recover on a bust; matching and
present-but-empty parts evaluate), the status test
`status_reads_hygiene_validity_joined_with_the_retained_parts` (missing,
matching, a diagnostics-only commit, the wrong generation, a restart), the
key test `newer_epochs_and_versions_supersede_and_keys_select_by_epoch_and_generation`
(epoch and generation selection), and the process-kill cut, whose restarted
fixture reports the baseline invalid. The status test makes its
diagnostics-only commit with a direct `store.commit`, not a transform pass.
Case (4), a disagreeing prefix, is constructed by
`non_append_mutation_invalidates_until_a_bust`
(`crates/daemon/src/tail_hygiene.rs:2210`), and the join table's
still-invalid and rebuilt steps construct cases (2) and (1) (`:2327-2349`).
Guarantee: Baseline consumers join durable scalars with retained parts by
session identity, reset epoch, and baseline generation; on a non-bust pass,
scalars without matching parts are not evaluable and invalidated; no durable
baseline, or a bust, is fresh; matching parts, including present-empty
parts, permit evaluation but do not by themselves make the baseline valid.
Check: `always-or-unreached` - both consumers take parts only from
`DerivedState::parts_for` (`crates/daemon/src/derived_state.rs:18-28`),
which returns them only for the session's cache entry, a matching revert
epoch, and a matching baseline generation. `refresh_tail_hygiene_baseline`
(`crates/daemon/src/tail_hygiene.rs:1080-1165`) then applies these cases in
order: (1) bust or fresh refresh - a cache-busting pass, or a pass with no
durable baseline, builds generation `previous + 1` (1 with none), evaluable,
from the measured parts, whatever the prior parts (`:1105-1133`); (2) sticky
invalidation on a non-bust - a durable baseline already
`generation_invalidated` stays invalid with no parts, even when the parts
match (`:1097-1104`); (3) key join - otherwise, missing or wrong-generation
parts give `evaluable: false`, `generation_invalidated: true`, and no parts,
before any prefix comparison (`:1135-1140`); (4) prefix comparison -
matching parts, including an empty list, reach `same_measured_prefix`, and a
disagreeing prefix also invalidates (`:1141-1143`); only an agreeing prefix
evaluates. Cases (1) and (2) are disjoint, so the code's order, which tests
(2) first (`:1097`), is the same decision. Matching keys permit evaluation;
they do not guarantee validity. `session.status` reports evaluable only when
the durable baseline is evaluable, not invalidated, and parts are retained
at the key (`crates/daemon/src/lib.rs:6881-6899`); it makes no prefix
comparison. A row-version advance that leaves the generation unchanged keeps
validity. `always-or-unreached` because both consumers see a baseline only
on folding sessions.
Fault/timing angle: Daemon restart, eviction or `remove`, an unrelated
row-version advance (diagnostics-only commit), a reset that advances the
revert epoch, a lease-budget refusal.
Required faults and enabling state: Durable scalars with missing, with
wrong-generation, and with genuinely empty parts, separately; a
diagnostics-only commit between promotion and the read; a restart.
Confidence: high -
[evidence](evidence/fa-n11-baseline-validity-requires-a-matching-join.md).
Read `refresh_tail_hygiene_baseline`
(`crates/daemon/src/tail_hygiene.rs:1080-1165`, the parts filter
`:1136-1140` ahead of `same_measured_prefix` `:1141`), `parts_for`
(`crates/daemon/src/derived_state.rs:18-28`), `prior_baseline_parts` and
`carried_derived_state` (`crates/daemon/src/transform.rs:7933-7956`), and the
status join (`crates/daemon/src/lib.rs:6881-6899`) at `0ff62b29a`. A
non-bust pass does not write its joined result durably: it evaluates it for
that pass's decisions (`crates/daemon/src/transform.rs:4966-4976`), so
"invalidated until a bust" holds because parts appear only from a bust or a
first refresh.
Existing check: `crates/daemon/src/tail_hygiene.rs:2278`
`durable_scalars_join_retained_parts_by_generation` (#906);
`crates/daemon/src/lib.rs:35724`
`status_reads_hygiene_validity_joined_with_the_retained_parts` (#906);
`crates/daemon/src/derived_state.rs:122`
`newer_epochs_and_versions_supersede_and_keys_select_by_epoch_and_generation`
(#906); `crates/daemon/src/lib.rs:35175`
`committing_and_no_write_passes_promote_under_a_charged_lease_and_a_restart_forgets`
(restart then non-bust stays invalid; a render-config `HARD` restores it;
#906); `crates/daemon/tests/derived_state_crash_cut.rs:76` (#906).
Impact: High. Missing parts read as an empty prefix produce false hygiene
decisions: a nudge or directive computed from a baseline that no longer
describes the served prefix.
Open questions:
- No witness makes the diagnostics-only commit through a transform pass; is
  the direct `store.commit` in the status test enough? (needs human input)

### fa-n12-identity-observations-survive-window-loss

Type: safety
Reachability: explicit-config-only - identity reads and writes belong to the
folding path (`enforce_block_identity` `crates/daemon/src/transform.rs:3684`,
`apply_ingress_identities` `:4159`, both past the native gate `:3207-3208`);
the additive path writes no identity rows. Publication needs a configured
firing.
Status: active
Exercised: yes - #905's recorded `cargo test --workspace --all-features
--locked --no-fail-fast` gate runs the store and daemon identity family:
`requested_identity_reads_do_not_grow_with_the_identity_table` (300 mids at
1k and 100k rows), `block_identity_deltas_keep_omitted_rows_and_scan_only_the_rows_they_write`,
`a_rejected_commit_writes_no_identity_row` (CAS loss),
`a_refused_commit_writes_no_identity_row` (durable-text refusal),
`a_failure_after_each_identity_mutation_rolls_the_whole_commit_back`,
`a_delta_that_writes_and_deletes_one_mid_is_refused`,
`identity_histories_match_a_per_session_reference_map` (100 seeded histories
of 32 operations with reopen), `transform_snapshot_resists_commit_between_state_and_overlay_reads`,
the descent and reset tests, the provisional-tail round trip, and
`a_window_that_drops_a_selected_message_fences_the_publication_out_and_keeps_its_rows`
(the dropped mid's row survives the window and its publication is fenced
out).
All ran green again in #906's and #859 PR A's runs.
Guarantee: Accepted identity changes are CAS-coupled upsert deltas; during
ordinary folding-window updates omitted mids survive until reset, deletion,
or native adoption, which clears legacy identity rows in the adopting
transaction; descent preserves observations. CAS and replay checks precede
identity mutation, and a later scan, serialization, or write failure rolls
back the transaction, preserving the previous durable state.
Check: `always-or-unreached` - at `commit_transform` the delta is applied
after both checks that return `Replay` (row-version CAS and history-segment
sequence, `crates/memory-store/src/lib.rs:11059-11089`) and after the
`cache_state` write, inside the same transaction (`:11117-11119`); the only
per-mid delete is the delta's `deletes` set (`:4951-4956`), which the
transform fills only for the provisional tail
(`crates/daemon/src/transform.rs:5610-5612`); requested-mid reads run inside
the snapshot read (`crates/memory-store/src/lib.rs:8772`); publication
refuses a selected mid whose row is absent or differs
(`:14149-14171`). `always-or-unreached` because identity writes happen only
on folding sessions.
Fault/timing angle: CAS loss; restart; covered edits; missing selected rows;
a commit between the state read and the identity read; failure injected
after an identity mutation before commit.
Required faults and enabling state: A large identity history with a small
incoming window; injected failure after the delete, between two upserts, and
after the upserts; a CAS-losing commit; an oversized meta; a delta that
writes and deletes one mid; descent and recomp reset.
Confidence: high -
[evidence](evidence/fa-n12-identity-observations-survive-window-loss.md).
Read `lookup_block_identity_rows` and `apply_block_identity_delta`
(`crates/memory-store/src/lib.rs:4863-4979`), `commit_transform` (`:10883`, delta at `:11117-11119`), `load_transform_snapshot_with_hook`
(`:8727-8772`), descent's copy (`:12961-12970`), reset
(`delete_block_identities` `:4981-4983`, called at `:13328`), and
`WindowIdentities` (`crates/daemon/src/transform.rs:5394-5429`) at
`0ff62b29a`.
Existing check: `crates/memory-store/src/lib.rs:23909`
`requested_identity_reads_do_not_grow_with_the_identity_table`; `:33485`
`block_identity_deltas_keep_omitted_rows_and_scan_only_the_rows_they_write`;
`:33571` `a_rejected_commit_writes_no_identity_row`; `:33757`
`a_refused_commit_writes_no_identity_row`; `:33603`
`a_failure_after_each_identity_mutation_rolls_the_whole_commit_back`;
`:33797` `a_delta_that_writes_and_deletes_one_mid_is_refused`; `:33977`
`identity_histories_match_a_per_session_reference_map`; `:21896`
`transform_snapshot_resists_commit_between_state_and_overlay_reads`;
`:34074` `descent_copies_block_identities_and_recomp_reset_clears_them`;
`:33925` `descent_copies_identity_rows_without_their_scan_owner`; `:33826`
`receipt_retirement_reads_at_most_one_row_beyond_the_released_ones`;
`:25938` `publish_rejects_a_selected_message_whose_identity_row_is_gone`;
`crates/daemon/src/transform.rs:14628`
`a_completed_tail_that_turns_provisional_is_removed_and_re_adopted_exactly`;
`:14673` `mid_turn_tail_stays_provisional_and_re_adopts_completed_tail`;
`:20444`
`a_window_that_drops_a_selected_message_fences_the_publication_out_and_keeps_its_rows`.
All #905.
Impact: High. Deleted or loser-written identities weaken publication fencing
and transform validation.
Open questions:
- #905's test review lists follow-ups not taken: vary core and metadata in
  the rollback test and compare receipts; compare the source's
  `(mid, scan_version)` pairs across descent; exact-vector oracles in the
  meta-bound and revision 3 tests. Are these tracked? (needs human input)

### fa-n13-selected-identities-have-a-byte-budget

Type: safety
Reachability: explicit-config-only - firing assembly runs only on a session
whose stored authority is Eidnara, and Eidnara folds only with an admitted,
non-empty summarizer chain (`DaemonConfig::eidnara_folds`,
`crates/daemon/src/config.rs:181-185`); the default chain is empty
(`:142`).
Status: active
Exercised: yes - #859 PR C's recorded run at `0ff62b29a` (workspace tests,
6,156 passed) ran
`an_over_budget_history_reserves_and_publishes_the_longest_fitting_prefix`:
800 alternating user and assistant messages, each mid carrying 100 control
characters, go through a real transform, so the stored identities are the
ones the pass derives. The test computes the longest fitting prefix of whole
blocks from those stored identities with the shared prefix oracle, requires
it to stop before the last block, and asserts the reservation commits
(`AwaitingProducer`, `firing_seq` 1) with exactly that selection, the chunk
range `1..=end`, the fingerprint of an unbudgeted build of that range, and a
prompt that ends at `end`; after the producer returns it asserts one
published history segment over `1..=end`, an Idle state, and one published
firing. The same run ran the three assembler oracle tests at budget - 1,
exact, and + 1 (`an_over_budget_selection_stops_at_the_longest_prefix_of_whole_blocks`,
`an_indivisible_first_block_over_the_identity_budget_no_fires`,
`escaped_and_unicode_mids_are_charged_as_they_serialize`), the 64-case
`a_generated_history_fires_the_longest_whole_block_prefix_within_the_identity_budget`
proptest over generated roles, escaped mid prefixes, and identity sizes, and
`an_indivisible_block_over_the_identity_budget_no_fires_and_reserves_nothing`
(4,000 escaped mids through a real transform, no producer start, durable
firing state Idle with no selection). The fixed-case oracle tests use
directly constructed 30,000-character fingerprints
(`crates/daemon/src/history_summarizer_chunk.rs:1776-1782`), a synthetic
stress state; the handler tests do not.
Guarantee: Every admitted firing fits a fixed serialized-byte identity budget
including mids, escaping, structure, and all block identities; admission and
the exact bounded selection commit atomically.
Check: `always-or-unreached` - for every `Fire` outcome,
`serde_json::to_vec(&selected_range_identities).len()` is at most
`SELECTED_IDENTITY_BUDGET_BYTES` (262,144); the chunk is the maximal
contiguous prefix of whole blocks that both the token check and the identity
check admit, with the token check's first-block exception, and
`flush_current_block` evaluates the token limit before identity admission
(`crates/daemon/src/history_summarizer_chunk.rs:387-395`); when the token
check admits the full candidate, the identity check selects its longest
prefix whose serialized selection fits; the chunk's text and fingerprint
equal an unbudgeted build of that prefix; a first block
whose selection alone exceeds the budget returns `IdentityBudget` and leaves
the durable firing state Idle, `firing_seq` unchanged, and the selection
empty. `always-or-unreached` because assembly never runs without an Eidnara
authority. Falsifier: the identity vector is truncated while the chunk keeps
the larger interval, or an indivisible refusal leaves a reservation.
Fault/timing angle: Tiny rendered messages with large identity vectors; mids
whose JSON escaping multiplies their length; the placeholder rebuild after
repeated chunk failures (`history_summarizer_chunk.rs:981-997`).
Required faults and enabling state: A non-empty chain and an eligible range;
one shrinkable selection; one indivisible oversized first block; selections
at budget - 1, exact, and + 1; escaped and Unicode mids; a shrunk chunk
carried through reservation and publication.
Confidence: high -
[evidence](evidence/fa-n13-selected-identities-have-a-byte-budget.md). The
budget constant (`crates/daemon/src/history_summarizer_chunk.rs:97`), the
per-entry charge from `"[]"` with separators (`:119-150`), the charge inside
`flush_current_block` after the token check (`:386-395`), the first-block
refusal (`:139-142`), the `IdentityBudget` return before any prompt work
(`:999-1006`), and the reservation handing the assembled selection to `fire`
(`crates/daemon/src/history_summarizer.rs:1857-1865`) were read at
`0ff62b29a`. Reattachment reuses the stored selection and rebuilds only text
(`crates/daemon/src/lib.rs:5072-5084`), so it never re-selects.
Existing check: `crates/daemon/src/history_summarizer_chunk.rs:1960`
`an_over_budget_selection_stops_at_the_longest_prefix_of_whole_blocks`;
`:1981` `an_indivisible_first_block_over_the_identity_budget_no_fires`;
`:1995` `escaped_and_unicode_mids_are_charged_as_they_serialize`; `:2169`
`a_generated_history_fires_the_longest_whole_block_prefix_within_the_identity_budget`
(#859 PR C); all four run through `BudgetCase::assert_matches_oracle`
(`:1887`), which also asserts the store reserves nothing (`:1875-1883`), and
compare with the shared `identity_prefix_oracle` (`:1369-1376`);
`crates/daemon/src/lib.rs:36285`
`an_indivisible_block_over_the_identity_budget_no_fires_and_reserves_nothing`;
`:36313` `an_over_budget_history_reserves_and_publishes_the_longest_fitting_prefix`
(#859 PR C).
Impact: High. An over-budget selection pushes the record toward the 512 KiB
durable-text guard and a repeated refused commit; truncating only the
identities would weaken the publication fence.
Open questions:
- Does any test reserve and publish a shrunk chunk with its exact selection?
  Resolved: yes, since #859 PR C,
  `an_over_budget_history_reserves_and_publishes_the_longest_fitting_prefix`
  (`crates/daemon/src/lib.rs:36313`).

### fa-n14-module-meta-is-message-independent

Type: safety
Reachability: default-production - every accepted transform commit shapes
and serializes `ModuleMeta` and passes it through the durable-text guard
(`crates/memory-store/src/lib.rs:11034-11041`, `:429`), under either
authority.
Status: active
Exercised: yes - #859 PR C's recorded run at `0ff62b29a` (6,156 passed) ran
`composition_witness_a_meta_with_every_field_near_its_bound_commits_and_reloads_within_the_total`.
It builds one `ModuleMeta` with every field near its recorded bound
(`worst_case_module_meta`): free text of `\u0001` characters that escape to
six bytes each, every collection full, integers at their extremes, the
worst-case summarizer state, the worst-case synthetic task-list pair, and a
selection of 262,014 serialized bytes, within 256 bytes of the 262,144-byte
budget. `last_recut` and `pending_rewrite_last_failure` each carry 1,008
bytes of secret-bearing text, near their 1,024-byte bound. It commits that record with its identity delta through the
store, then asserts each stored field is within its recorded bound, the
stored selection within the budget, the whole record within the recorded
total plus the budget and under 512 KiB, the row length equal to the stored
text, and the reload equal to the committed record except the two shaped
fields, which reload redacted within their bounds. At `e02b22383`, before
the render identity allowance grew, the commit reported 371,060 stored
bytes against 115,457 recorded plus the budget. The record is
a synthetic stress state: no pass produces every field at its bound at once.
The same run ran the `(N, W)` matrix
`module_meta_size_is_independent_of_message_count_and_window_size` (`(1k,
300)`, `(20k, 300)`, `(20k, 5,000)` under both authorities, with and
without an active firing, every commit accepted, metadata equal within 1%
after digit-width normalization, guard not reached, and negative controls
that must fail, including a string that grows only in escaping cost);
`every_metadata_field_has_a_recorded_bound_within_the_headroom` (every
`ModuleMeta` field listed by exhaustive destructuring, each with a finite
bound, the sum under 128 KiB, and the sum plus the selection budget at most
384 KiB); `every_history_summarizer_field_has_an_enforced_bound`; the
per-field cap tests at the bound and one past it; the store-shaping tests
`a_secret_bearing_summarizer_detail_is_stored_within_its_bound_with_its_detection_recorded`,
`a_committed_last_recut_is_stored_within_its_bound`, and
`a_revert_keeps_last_recut_within_its_bound_when_a_surviving_id_is_long`;
the stored-form state-sync test
`state_sync_refuses_values_whose_stored_form_passes_their_bound`; and the
caller-side pair tests
`a_replacement_pair_over_its_bound_after_redaction_clears_the_persisted_pair`
and `a_replacement_pair_near_its_bound_persists_and_reloads_within_it`. The
run also ran
`a_render_identity_from_five_escaped_inputs_at_their_bound_fits_its_allowance`,
which sends a real transform pass with all five request inputs (render
config, provider, model, system prompt hash, upgrade state) at 256 `\u0001`
characters and asserts the stored `last_render_config` holds all 1,280 of
them, adds at most 512 framing bytes, and serializes within its recorded
bound. The #859 PR C description records, for each new test, the
negative control that makes it fail with its target broken.
#859 PR B's run at `b45416ac0` (6,131 passed) ran
`covered_systems_grow_in_m0_while_the_stored_meta_stays_fixed`. In the
matrix, active-firing cells seed summarizer state and then load the
selection from the first 100 window mids' identity rows (`:1195-1221`); the
final assertion requires the Eidnara cell to retain 100 selected identities
and the native cell zero, since native passes write no identity rows
(`:1239-1251`). The native active-firing cell is also a synthetic stress
state: an active summarizer under native authority is a state native passes
do not produce (`:1284-1285`).
Guarantee: Admitted metadata has a fixed serialized bound independent of N
and W, below the 512 KiB guard; every field is scalar or independently
byte-capped.
Check: `always` - at durable-write admission, the serialized `meta` of every
accepted commit stays under the sum of recorded field bounds (under 128 KiB)
plus the 256 KiB selected-identity budget, at most 384 KiB and below
524,288 bytes; covered system messages are rows of `covered_system_messages`
outside `meta`. The `(N, W)` matrix detects regressions but does not prove
the bound. `always` because every commit writes the record. Falsifier:
metadata grows with N or W, or a field's serialized value exceeds the bound
the table charges for it.
Fault/timing angle: Long native sessions; stalled folding; an active
oversized selection; many distinct covered system messages; state-sync,
plugin-supplied, or producer-supplied values at their caps; text that
escaping or redaction lengthens.
Required faults and enabling state: The `(N, W)` matrix under both
authorities and firing states; a restored growing field as a negative
control; each capped field at its cap and one past it, including secrets
whose placeholders lengthen a value at its bound; many folds with distinct
system messages; a state sync at 512 legacy segments and one past; one
commit with every field near its bound and a near-budget selection.
Confidence: high -
[evidence](evidence/fa-n14-module-meta-is-message-independent.md). Read at
`0ff62b29a`: the matrix (`crates/daemon/src/transform_meta_bound.rs:1163-1270`,
`:1273`); the bound table `recorded_metadata_bounds` (`:610-729`), which
charges `history_summarizer` at `history_summarizer_bound()` (`:273-483`,
inventory macro `:242-249`, `ModuleMeta` destructured at `:637`),
`legacy_history_segment_seqs` at 512 sequences (`:624`, `:675`),
`last_recut` and `pending_rewrite_last_failure` at their 1,024-byte bounds
(`:652`, `:661`), `last_render_config` at the five request inputs plus
`RENDER_IDENTITY_FRAMING_BYTES` = 512 (`:640`, `:754`), and
`pending_rewrite` as an exact inventory of `PendingRewriteState`
(`:653-658`), and `archive_fold_seq` as an integer (`:674`); inside the
summarizer inventory, `withdrawn_selected_mid` at the escaped size of a mid
at its 128-byte ingress bound (`:476`) and `last_abandon` as an exact
inventory of `HistorySummarizerAbandon` (`:477-481`); both sums
(`:734-743`); the worst-case
summarizer (`:485-595`); the composite witness (`worst_case_module_meta`
`:822-975`, test `:978-1072`). Store-owned shaping: `commit_transform`
passes the record through `shape_stored_meta` before serializing it
(`crates/memory-store/src/lib.rs:10982`; `:6094-6167`), which scans
`last_failure`, `last_no_fire`, every `NoFire` detail,
`pending_rewrite_last_failure`, and `last_recut` with `write.content` when a value is over its bound or a
redactor would change it, so the field's receipt records any detection, and
keeps `redacted_prefix_within_serialized_bytes` of the result
(`shaped_meta_text`). That prefix is within the bound and both the durable
and the transaction redactors leave it unchanged; a prefix a redactor would
change is cut back to the start of that redactor's earliest finding in it
(`earliest_finding`), so each scan removes a whole finding and the record
keeps its bound when state sync re-prepares it with the transaction
redactor. The daemon's `bounded_detail`
(`crates/daemon/src/history_summarizer.rs`) caps a summarizer detail at
`MAX_RAW_SUMMARIZER_DETAIL_BYTES`, 64 KiB of raw text, so a credential near
the 512-byte bound reaches the store's scan whole and is redacted before the
cut (`a_credential_that_crosses_the_detail_bound_is_redacted_whole`).
`NoFire::new` (`crates/memory-store/src/summarizer_timeline.rs:94-95`) and
`pending_rewrite_detail` (`crates/daemon/src/transform.rs:6831-6848`) keep a
raw prefix; `record_no_fire` compares the stored form
(`crates/daemon/src/lib.rs:5903-5907`).
`MAX_PENDING_REWRITE_DETAIL_BYTES` and `MAX_LAST_RECUT_BYTES` (1,024 each)
live in the store (`crates/memory-store/src/lib.rs:6039`, `:6041`), and the
descent, reset, and revert-truncation writers cut `last_recut` to the stable
prefix (`:12706-12712`, `:13288-13299`, `:13505-13510`).
`MAX_SUMMARIZER_DETAIL_BYTES` is at `:6007-6009`, re-exported by the daemon.
The task-list setter and bust capture accept a state only when its raw and
its redacted form both meet both task-list bounds (`todo_state_within_bounds`;
`set_todo_state` `:10309-10311`; `crates/daemon/src/injection.rs:214-222`);
the synthetic pair is frozen only when `SyntheticTodo::admitted` holds: its
stored form fits 24 KiB under the longest serialized anchor, and
`injection_pending_after_capture` applies the same decision, so a refused
pair leaves no injection pending (`synthetic_todo_pair_within_bounds`,
`crates/memory-store/src/lib.rs:6147-6152`, measured by `stored_json_len`,
the longer of the durable and transaction scans, `:6155-6160`;
`crates/daemon/src/transform.rs:6977-6979`). State sync checks the seeded
pair and acknowledged watermarks with `stored_json_len`
(`crates/memory-store/src/lib.rs:6295-6299`, `:6379-6383`), beside the
caps (`:5976-5980`, `:6141-6143`, `:6164`) and their check after redaction
(`:6398`, `:6402-6448`); the legacy count refusal (`:11505-11534`). The
summarizer state stores a SHA-256 chunk fingerprint and model-chain digest
(`crates/daemon/src/history_summarizer.rs:166-179`) and refuses a harness or
run id over 128 serialized bytes and keeps a 48-byte session slug
(`:1616`, `:1621-1624`, `:1834-1843`, `:1895-1901`); the reservation
identity refusal (`crates/memory-store/src/lib.rs:668-690`,
`:13808-13814`); the covered-system rows
(`crates/memory-store/baseline.sql:40-58`, delta applied in the commit at
`crates/memory-store/src/lib.rs:11120-11127`, read with the snapshot at
`:8773`, removed by reset at `:13329`, and absent from a descent target at
`:12962-12964`); the request-identity bound
(`crates/daemon/src/transform.rs:149`, `:2236-2241`), the hint deferral cap
(`:148`, `:4218-4222`), and the mid bound (`crates/daemon/src/wire.rs:215`,
`:285-290`). The store's
`abandon_history_summarizer_run_if_matching_with_publish_failure` (`crates/memory-store/src/lib.rs:13701`) redacts its caller's detail and
keeps the stable prefix within `MAX_SUMMARIZER_DETAIL_BYTES`
(`:13715-13722`, stored at `:13768-13770`). Its daemon callers pass uncut
`publish rejected: {reason}` and `memory_reviewer handoff failed: {error}`
details (`crates/daemon/src/history_summarizer.rs:737-743`, `:756-762`,
`:773-779`, `:794-806`, `:2598-2605`), and a fence reason can quote a
128-byte control-character mid (`crates/memory-store/src/lib.rs:14165-14169`),
so the store-side cut keeps that path within the 514 bytes the inventory
charges.
Existing check: `crates/daemon/src/transform_meta_bound.rs:1273`
`module_meta_size_is_independent_of_message_count_and_window_size`; `:978`
`composition_witness_a_meta_with_every_field_near_its_bound_commits_and_reloads_within_the_total`
(#859 PR C, `e02b22383`); `:1342`
`a_render_identity_from_five_escaped_inputs_at_their_bound_fits_its_allowance`
(#859 PR C, `78fc312db`); `:732`
`every_metadata_field_has_a_recorded_bound_within_the_headroom`; `:588`
`every_history_summarizer_field_has_an_enforced_bound`; `:1371`
`request_identity_strings_over_their_bound_are_refused_before_any_read`;
`:91` `a_hundred_thousand_message_session_commits_a_three_hundred_message_window`;
`crates/daemon/src/transform.rs:26370`
`covered_systems_grow_in_m0_while_the_stored_meta_stays_fixed` (#859 PR B);
`:21623` `a_replacement_pair_over_its_bound_after_redaction_clears_the_persisted_pair`
and `:21659` `a_replacement_pair_near_its_bound_persists_and_reloads_within_it`
(#859 PR C);
`crates/memory-store/tests/production_redaction.rs:968`
`a_secret_bearing_summarizer_detail_is_stored_within_its_bound_with_its_detection_recorded`
(#859 PR C);
`crates/memory-store/src/lib.rs:24390`
`state_sync_refuses_anchors_and_watermarks_over_their_bounds` (with the todo
serialized-length cap, #859 PR C); `:24632`
`state_sync_refuses_values_whose_redacted_form_passes_their_bound` and
`:25037` `state_sync_refuses_a_result_over_the_legacy_segment_cap` (#859 PR
B); `:24756` `a_redacted_detail_cut_keeps_its_length_through_another_redaction`,
`:24789` `a_committed_last_recut_is_stored_within_its_bound`, `:24809`
`a_revert_keeps_last_recut_within_its_bound_when_a_surviving_id_is_long`,
`:24849` `state_sync_refuses_values_whose_stored_form_passes_their_bound`,
`:24944`
`an_abandon_keeps_a_secret_bearing_detail_within_its_bound_after_redaction`,
`:24984` `set_todo_state_refuses_a_state_whose_redacted_form_passes_its_bound`,
and `:25787` `an_abandon_keeps_a_failure_detail_within_its_serialized_bound`
(#859 PR C); `:26716`
`a_reservation_identity_over_its_serialized_bound_is_refused_before_any_write`
(#859 PR C); `:34174`
`covered_system_rows_round_trip_in_ordinal_order_and_retire_their_receipts`,
`:34349` `covered_system_content_is_stored_as_the_meta_scan_redacts_it`,
`:34385` `reset_and_delete_remove_covered_system_rows_and_their_receipts`,
and `:34415` `descent_leaves_the_target_without_covered_system_rows` (#859
PR B); `crates/daemon/src/history_summarizer.rs:4243`
`a_producer_start_failure_records_a_bounded_detail`, `:4275`
`a_secret_bearing_start_failure_stays_within_its_bound_once_stored`, `:4397`
`a_run_id_over_the_producer_identity_bound_is_a_start_failure`, `:4432`
`a_harness_over_the_producer_identity_bound_writes_nothing`, `:6372`
`a_producer_session_id_keeps_a_bounded_slug`, `:6333`
`chunk_fingerprint_uses_id_kind_and_byte_length`, and `:2721`
`chunk_failures_count_per_chunk_and_ignore_provider_errors` (#859 PR C);
`crates/memory-store/src/summarizer_timeline.rs:490`
`a_detail_is_cut_by_its_serialized_length` (#859 PR C);
`crates/daemon/src/injection.rs:773`
`a_captured_state_whose_redacted_form_passes_its_bound_reads_as_an_empty_list`
and `:793` `a_synthetic_pair_over_its_bound_after_redaction_is_refused`
(#859 PR C);
`crates/daemon/src/wire.rs:975` `empty_and_reserved_message_ids_are_rejected`
(mid length at `:986-996`);
`crates/daemon/tests/eval_surface_ledger.rs:533`
`a_new_hint_is_skipped_while_every_deferral_slot_is_taken`;
`crates/daemon/src/fold_authority_handler_tests.rs:645`
`native_metadata_does_not_grow_with_the_message_count`.
Impact: High. A record that grows with the session crosses the guard, and
every later pass fails its commit.
Open questions:
- Resolved: does any test compose every capped field near its cap with a
  full selection and commit it? Yes, since `e02b22383` (#859 PR C):
  `composition_witness_a_meta_with_every_field_near_its_bound_commits_and_reloads_within_the_total`
  (`crates/daemon/src/transform_meta_bound.rs:978`). The record is a
  synthetic stress state.
- Resolved: the store's abandon path wrote `last_failure` without the
  512-byte cut before `6267f66d4`. `6267f66d4` (#859 PR C) cuts it in the store,
  so every caller meets the bound, and
  `an_abandon_keeps_a_failure_detail_within_its_serialized_bound` (`crates/memory-store/src/lib.rs:25787`) witnesses it.
- Resolved: a design review found that a value meeting its bound on input
  could pass it once redaction replaced a secret with a longer placeholder,
  for summarizer failure and no-fire details, the task-list setter, and the
  synthetic task-list pair. `6fc85742f` (#859 PR C) checked the redacted
  forms of all three; the next item records how `186f3c076` completed it.
  Each case has a secret-bearing test at its bound, listed above.
- Resolved: the bounds of `6fc85742f` held under the durable redactor only,
  and a daemon-side cut kept the redacted text, so the store's scan found no
  secret to record. `186f3c076` (#859 PR C) moves final shaping into the
  store, keeps prefixes stable under both redactors, bounds `last_recut` at
  1,024 bytes, and checks the seeded pair and acknowledged watermarks in
  their stored form; `e02b22383` adds the composite and caller-side pair
  witnesses.
- Resolved: a design review found that the recorded `last_render_config`
  bound, four request inputs plus 256 bytes, undercounted the render
  identity, which carries all five request inputs and its framing.
  `78fc312db` (#859 PR C) charges five inputs plus 512 framing bytes,
  updates the composite fixture to match, and adds
  `a_render_identity_from_five_escaped_inputs_at_their_bound_fits_its_allowance`,
  which measures the identity a real pass stores.
- A row written before #859 PR B holds its covered system list inside
  `meta`. `ModuleMeta` at HEAD has no such field and no migration copies the
  list into rows, so the first load drops it and m0 stops rendering those
  instructions until the systems are covered again. Is that accepted for
  existing sessions? (needs human input)
- Resolved: the owner decided the two content-bounded fields of #859 PR A.
  #859 PR B moved covered system messages out of `meta` into
  `covered_system_messages` rows and capped legacy history segments at 512
  per session (`MAX_LEGACY_HISTORY_SEGMENTS`), and #859 PR C replaced the
  declared 12 KiB summarizer allowance with an enforced, inventoried bound.
  The bound table has no `Unbounded` or `Configured` entry.

## Relationship map

- Invalidated baselines and successors: FA-E01 by FA-N06 (the output clause
  survives) and then by #857's identity rows; FA-E02 by FA-N01 (the
  `eidnara_reduce` omission and the refusing fold commands survive); FA-E04
  by FA-N13 (range coverage and the missing-identity no-fire survive);
  FA-E05 by FA-N05 (the late `no_models` path survives for a stalled
  Eidnara session); FA-E10, FA-E11, and FA-E12 by FA-N01; FA-E13 by FA-N07
  and FA-N08; FA-E14 by FA-N07's status clause.
- Shared mechanism, fold authority rule: FA-N01, FA-N07, FA-N08, FA-E02,
  FA-E12, and FA-E13 read one rule, `enabled !== false` and a non-empty
  admitted chain, through the Rust `DaemonConfig::eidnara_folds`, the
  plugin's `isCompactionEnabled`, and the CLI's `foldAuthorityOf`; the shared
  fixture pins the loaders to each other.
- Shared mechanism, durable authority: FA-N02, FA-N03, FA-N04, FA-N05, and
  FA-N06 read and write the per-session authority row; FA-N05's gate is the
  point FA-E05 moved from, and FA-E06's route and firing lifetimes are the
  enabling state FA-N03's quiescent bind checks.
- Shared mechanism, accepted derived state: FA-N09 publishes the state that
  FA-N10 and FA-N11 join against; FA-E07 and FA-E08 are the empty and absent
  cases those joins keep.
- Shared mechanism, identity rows: FA-N12, FA-E09, FA-N13, and FA-E04 read
  the block identity rows; FA-N13's budget bounds the selection FA-N14 counts
  inside the record. The covered-system rows of #859 PR B share the identity
  rows' scan and receipt scheme (`ScanOwnedRows`) and their CAS-coupled
  delta, but carry no identity record of their own here; FA-N14 and FA-E03
  cover them.
- Shared mechanism, the durable-text guard: FA-E03 and WP-E10
  (`docs/properties/window-protocol/`) share `ensure_durable_text_bound`,
  which also scans the covered-system rows #859 PR B moved out of `meta`;
  FA-N14 bounds every `meta` field under 128 KiB, and under 384 KiB with the
  selected identities, which keeps the guard a recoverable refusal.
- Canonical records elsewhere: the window protocol's WP-E04 (the identity
  table), WP-P06 (the withdrawal record), WP-E10 (the guard), and the summary
  supersession part
  hold those obligations; this catalog links its shared obligations to them.
- Window protocol after #905 (#857), which removes window pruning: WP-P25 is
  invalidated and names WP-P06 as successor for the publication fence, and
  WP-P06 and WP-P16 describe the withdrawal record a window commits when it
  drops a selected mid, with identity rows kept
  (`docs/properties/window-protocol/catalog.md`). FA-N12 carries the
  identity-row retention that replaced the prune.
