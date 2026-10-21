# fa-e01-additive-retains-ingress-identities

## Discovery trigger

Existing-behavior record FA-E01 of specification #834 (first comment),
describing `265df096`: accepted compaction-off passes prepend m0 and m1,
serve ingress unchanged, and retain every non-provisional identity, so a
compaction-off session still accumulated durable identity metadata. The
spec's D3b moves identity adoption to the folding path; #855 records the
replacement for this catalog ("FA-E01 ... is replaced. The additive path
retains none.").

Exercised status: partial - the replacement state (no identity under native
authority) is witnessed by #903's handler test, green again in #859 PR A's run at
`fd0b52aa5`. The surviving output clause has no named witness.

## Evidence trail

References are verified at `0ff62b29a` unless another tree is named.

- Path selection: `apply_once` sends a pass whose stored authority is native
  to `apply_additive_only` (`crates/daemon/src/transform.rs:3217-3218`),
  except a native subagent lineage switch, which reaches the protocol
  passthrough. Native is what an empty chain selects
  (`crates/daemon/src/config.rs:181-185`, default chain `:142`), so the path
  is default-production at HEAD. At `265df096` it ran only with
  `compaction.enabled: false`, hence the record's explicit-config label.
- Output: `apply_additive_only` (`transform.rs:2710`) builds the served list
  as the m0 unit, the m1 unit, then every ingress message cloned in order
  (`:3064-3074`). This is the surviving clause.
- Ingress update: the additive path calls only `apply_ingress_scalars`
  (`:2914`; body `:5593-5608`: newest-live block and ordinal, last usage).
  Identity insertion, provisional-tail removal, tail and basis re-adoption,
  and the re-adoption counter live in `apply_ingress_identities` (`:5612`),
  called only on the folding path.
- #855 (PR #903, `8b1e04944`) made that split. #857 (PR #905, `871ebfb08`)
  removed `ModuleMeta.block_identity_by_mid`; identities are rows written as
  explicit deltas on the folding path, so "retain" no longer describes a
  record field on either path.
- Witness of the replacement:
  `native_authority_skips_every_fold_step_even_with_a_live_chain`
  (`crates/daemon/src/fold_authority_handler_tests.rs:46`) runs native passes
  and calls `assert_native_state` (`:19-29`), which asserts an empty
  `block_identities` table, no tail baseline, no coverage, and a zero folded
  sequence, while newest-live and usage scalars update. It ran in #903's
  workspace gate and in #859 PR A's run at `fd0b52aa5`.
  `native_metadata_does_not_grow_with_the_message_count` (`:645`) checks the
  1k and 20k native records within 1% after a simulated OpenCode slice.
- Correction: the spec's existing check
  `compaction_mode_projection_cache_reclassifies_synthetic_prefix`
  (D`lib.rs:42489` at `265df096`) was deleted on `main` by `252d9e179`
  ("Project every request from its full input and delete the projection
  cache"), which is after `265df096` and an ancestor of HEAD. The spec's
  other citations (D`transform.rs:2562-3036`, `:2757-2764`, `:2895-2917`,
  `:5478-5509`, `:3050-3052`) describe `265df096`; their HEAD counterparts
  are the lines above.

## Failure scenario

At `265df096`: a long compaction-off session writes one identity entry per
message into metadata until the 512 KiB guard refuses the commit. At HEAD,
for the surviving clause: a native pass that altered or dropped an ingress
message would change what the host's own compaction sees.

## Timing windows and dependencies

None for the output clause. The replacement depends on the stored authority
read inside the pass's compare-and-swap (#903).

## What a test must construct

A native-authority session over several passes with distinct mids and a
replaced native slice; assert the output is m0, m1, then the ingress
messages in order, and that no identity row exists.

## Investigation log

### Q: Does a test assert the additive output is m0, m1, then ingress?

- Sources examined: `git grep` over `crates/daemon` for additive and native
  test names; `transform.rs` tests at `:15358`, `:15467`, `:15586`,
  `:15652`; the handler tests in `fold_authority_handler_tests.rs`.
- Findings: These tests check recorded reasons, history retention, retries,
  and native state. None asserts the served list's shape.
- Missing evidence: An output-shape assertion on a native pass.
- Conclusion: unresolved, needs that assertion.

### Q: Which record replaces the identity clause?

- Sources examined: #834 FA-N06; PR #903 "Replaced property clauses".
- Findings: FA-N06 (native-state-stays-additive) states native metadata holds
  no identity entries; #903 names FA-E01 replaced.
- Missing evidence: None.
- Conclusion: resolved with answer: FA-N06, landed by #855 (PR #903).

## Update for #1060 at `a15da13a4`

Ticket #1061; PR #1068 (head `f04c54e4d`, `cargo +1.98 nextest run
--profile ci -p daemon --lib`, 1,871 passed). References are verified at
`a15da13a4`.

- The additive branch is selected from applied authority
  (`crates/daemon/src/transform.rs:3341-3342`). The extra guard covers only a
  lineage-switched subagent pass; Pi sends `is_subagent: false`.
- The additive output pushes m0 and m1, then every submitted message
  (`transform.rs:3187-3199`).
- `a_native_pi_pass_serves_m0_m1_then_every_row_and_starts_no_summarizer`
  (`crates/daemon/src/pi_native_serving_tests.rs:652`) serves a window of
  every closed Pi role (`:312` `every_closed_role_rows`) with a leading
  `eidnara:compactionSummary:` row and a paired tool call under a native
  binding. It asserts two `eidnara:synthetic:` user rows, then the submitted
  rows unchanged and in order (`:574`), a null `boundary`, and the recipe
  `insert(2)` then `keep(source: input, start: 0, count: n)` (`:594`).
- `the_same_pi_windows_differ_by_binding_alone` (`:710`) serves the same
  windows under an Eidnara binding: the short window is served the same way
  while only the Eidnara binding evaluates the fold trigger, and the long
  window folds.

### Q: Does a test assert the additive output is m0, m1, then ingress?

- Sources examined: `pi_native_serving_tests.rs:574-610`, `:652-707`;
  `transform.rs:3187-3199`.
- Findings: Yes. The recipe assertion pins the shape exactly; a recipe that
  re-inserted copies of the submitted rows fails it. Retained `Arc`
  identity for kept rows follows from `keep(source: input)`; the codec seam
  checks pointer identity (`crates/daemon/src/codec/pi.rs:2422`).
- Missing evidence: None.
- Conclusion: resolved with answer.
