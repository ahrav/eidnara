# replayed-synthetic-pair-arrives-unflagged-on-a-delta-turn

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

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

[normalize]: ../../../../../crates/daemon/src/transform.rs#L2083-L2100
[t-collapsed]: ../../../../../crates/daemon/src/transform.rs#L27269-L27270
[expand]: ../../../../../crates/daemon/src/lib.rs#L4151-L4245
[reattach-call]: ../../../../../crates/daemon/src/lib.rs#L4188-L4190
[native-deep]: ../../../../../crates/daemon/src/lib.rs#L4228-L4231
[historian-fire]: ../../../../../crates/daemon/src/lib.rs#L4994
[no-models]: ../../../../../crates/daemon/src/lib.rs#L5189-L5196
[native-profile]: ../../../../../crates/daemon/src/lib.rs#L7946-L7947
[prepare-a]: ../../../../../crates/daemon/src/lib.rs#L8245-L8247
[prepare-b]: ../../../../../crates/daemon/src/lib.rs#L8336-L8338
[native-gate]: ../../../../../crates/daemon/src/lib.rs#L8406
[todo-prefix]: ../../../../../crates/daemon/src/injection.rs#L187-L189
[tail-reclaim]: ../../../../../crates/daemon/src/healing.rs#L130-L139
[cfg-models]: ../../../../../crates/daemon/src/config.rs#L119
[cfg-compaction]: ../../../../../crates/daemon/src/config.rs#L121
[plugin]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L759-L761
