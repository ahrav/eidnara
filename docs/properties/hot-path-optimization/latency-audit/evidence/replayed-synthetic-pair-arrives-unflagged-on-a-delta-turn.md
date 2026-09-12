# replayed-synthetic-pair-arrives-unflagged-on-a-delta-turn

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The executed witness below supplements that history.

## Discovery trigger

B2's outside clause states that the historian, the projection-cache charge,
and the native attach read the un-normalized request while the pass reads the
normalized clone. Those two views differ for a downstream observer only when
the normalization actually produced a clone, a historian firing was prepared
on that pass, and the request arrived on the lane where the two views
disagree. The existing replayed-pair test uses a full array with no firing, so
B2 can pass without the divergent observer ever being reached.

## Evidence trail

- [`normalize_synthetic_todo_ingress`][normalize] clones only when a
  non-synthetic message carries a [`synthetic_todo_`][todo-prefix] call or
  result id; otherwise it returns `None` and no clone exists.
- [`expand_transform_tail_delta`][expand] runs when `parsed.tail_delta` is an
  object with `after`; it reattaches the prefix through
  [`reattach_messages_prefix`][reattach-call] from the projection cache and
  deep-copies the native prefix ([`:4228-4231`][native-deep]).
- [`prepare_historian_fire`][historian-fire] is called with `&parsed` on the
  Emergency95 arm ([`:8245-8247`][prepare-a]) and on the ordinary arm
  ([`:8336-8338`][prepare-b]); it returns `no_models` without firing when
  [`cfg.model_chain.is_empty()`][no-models], and [`model_chain`][cfg-models]
  defaults to empty.
- `serve_native` gates the native attach ([`:8406`][native-gate]) and requires
  the `OpencodeAiSdk` profile ([`:7946-7947`][native-profile]).
- [`compaction_enabled`][cfg-compaction] defaults to `true`; with it off the
  pass returns through `apply_additive_only` before normalization.
- [`tail_reclaim`][tail-reclaim] is `true` for every shipping profile, so a
  bust pass can freeze a todo pair. The
  [replayed-pair test][t-collapsed] builds a collapsed replay of that pair
  without the `synthetic` marker at `:27303-27318`, on a full array, without a
  firing.
- The plugin sends both [`method` and `kind`][plugin] and uses the delta
  protocol on steady turns; whether it replays a frozen pair unflagged on a
  delta turn is inferred from the protocol, not observed.

## Failure scenario

Not a violation; a coverage gap. On a full-array turn the historian's message
filter keeps the replayed pair as a zero-block `BoundaryMsg`; on a delta turn
the rebuilt prefix already carries the flag and the filter drops it. Without a
delta-turn pass that both clones and prepares a firing, no test distinguishes
a design that widens the normalized view to the historian from the HEAD
design.

## Timing windows and dependencies

None in time. The situation needs four independent enabling conditions on one
pass: a delta body whose prefix reattaches, a clone-producing message, a
prepared firing (configured `model_chain`), and `serve_native` on.

## What a test must construct

A prior bust pass that freezes a todo pair; a harness replay of the pair
without the `synthetic` marker, as the existing test constructs; a `tail_delta`
body so [`expand_transform_tail_delta`][expand] reattaches the prefix; a
configured `model_chain` so [`prepare_historian_fire`][historian-fire] passes
the `no_models` gate; the `OpencodeAiSdk` profile with `serve_native` on. The
marker records the four preconditions at the pass and asserts them; it does
not assert observer agreement, which is B2's check. No existing check covers
the delta-turn form.

The clone precondition is the input condition, a non-synthetic message
carrying a `synthetic_todo_` id, not the clone
`normalize_synthetic_todo_ingress` produces at HEAD, so the marker fires under
a shared-view design as well.

## Investigation log

### Q: Does the harness replay a frozen pair unflagged on a delta turn?

- Sources examined: the [replayed-pair test][t-collapsed], the plugin
  [discriminators][plugin], [`expand_transform_tail_delta`][expand].
- Findings: The test comment at `:27303` states that OpenCode represents the
  pair as one tool part on replay. The plugin's delta protocol carries the
  suffix only, so a replayed pair in the suffix would arrive unflagged. No
  production trace was supplied.
- Missing evidence: A captured delta-turn body from the plugin after a bust.
- Conclusion: unresolved, needs a captured plugin body; the catalog's
  `medium` confidence stands.

[normalize]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L2083-L2100
[t-collapsed]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L27269-L27270
[expand]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4151-L4245
[reattach-call]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4188-L4190
[native-deep]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4228-L4231
[historian-fire]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4994
[no-models]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L5189-L5196
[native-profile]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L7946-L7947
[prepare-a]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8245-L8247
[prepare-b]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8336-L8338
[native-gate]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8406
[todo-prefix]: ../../../../../crates/daemon/src/injection.rs#L187-L189
[tail-reclaim]: ../../../../../crates/daemon/src/healing.rs#L130-L139
[cfg-models]: ../../../../../crates/daemon/src/config.rs#L119
[cfg-compaction]: ../../../../../crates/daemon/src/config.rs#L121
[plugin]: https://github.com/ahrav/eidnara/blob/9132344/packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L759-L761

## Executed delta witness

Implementation base: `bf6b9d5fad969fa29da852a1dd9f1de569732197`.
Execution date: 2026-09-11.

[`unflagged_synthetic_delta_prepares_historian_and_native_output`][witness]
uses the handler entry, fixture memory store and existing scripted producer.
It establishes these independent conditions on one delta pass:

1. Compaction is enabled and the configured model chain is nonempty.
2. A real todowrite call/result boot pass returns HARD and freezes a pair.
3. The delta names the boot fingerprint and reattaches two prefix messages.
   The response reports two reused projection messages.
4. Both suffix carriers arrive with `meta.synthetic == false` and carry the
   frozen pair's call ID. Eighty ordinary messages precede the pair, placing
   its ordinals 83 and 84 in the protected tail.
5. The profile is `opencode-aisdk`, `serve_native` is true, and the handler
   reports `historian.fired == true` on that pass.

After those assertions the test emits the constant marker
`replayed-synthetic-pair-arrives-unflagged-on-a-delta-turn: reached`.
It also checks nonempty native output, waits for the producer to start, then
releases the scripted output block and waits for the historian to become idle.
Observer equivalence is checked separately by the
[B2 reference and delta comparisons](synthetic-normalization-is-scoped-to-the-pass.md#pass-local-view-evidence).

### Characterization and limitations

`cargo test -p daemon --lib --locked unflagged_synthetic_delta_prepares_historian_and_native_output -- --nocapture`
passed before the production refactor (1 test); its marker was reached and
the historian completed. The post-refactor `synthetic` filter passed all 20
matching tests, including this witness.

Fixture development first omitted the authored tool result and hit the
tool-adjacency assertion. Adding that result fixed the fixture. A second
attempt placed the replay inside the selected chunk; the baseline refused
assembly with `MissingBlockIdentity { message_id: "replay-call" }`. Moving
the replay into the protected tail makes the required firing reachable.
Neither refusal is changed by the optimization.

This is a constructed handler witness, not a captured production plugin body.
The default-production reachability class and medium confidence remain.
Test adequacy remains unaudited. No benchmark or measurement campaign ran;
parent measurement and whole-repository landing gates remain separate.

[witness]: ../../../../../crates/daemon/src/lib.rs#L22859

## Third-turn prefix and production-prompt checks

The [witness][witness] also sends a third delta after the second turn's
historian completes. The third turn reuses all 84 prefix messages, including
the replay at ordinals 83 and 84. It asserts that expansion uses the projection
cache rather than snapshot fallback and restores both normalized flags.

The reference prefix comes from full projection of the same second-turn
messages with only the replay carriers' typed flags set, followed by the
existing prefix-reattachment function. This matches the clone-era mechanism.
The actual expanded ingress equals that reference. Its boundary messages omit
the pair and its chunk input ordinals omit 83 and 84. A full raw observer input
with those two flags cleared retains two empty boundary messages and both
ordinals instead. That baseline difference is asserted, not erased.

Two independent fixture handlers run the same first two turns. One receives
the third delta; the other receives the full reference with the same
reattached observer shape. The scripted producer captures both actual firing
prompts. The first prompt contains ordinary message 3 and excludes the replay
carrier's unique text. The third prompt advances the chunk start and also
excludes that text. Both captured prompts, third-turn served messages and
native bytes agree across the handlers. Native suffix responses are expanded
against the prior native output before comparison.

The extended witness passed individually before the scan/helper cleanup and
again in the post-cleanup `synthetic` filter (21 matching tests). The original
execution record above remains the two-turn characterization result. This
follow-up adds no production trace or performance claim.
