# hv-single-history_segment-skips-lookahead-discard

## Discovery trigger

Task item 4 asks about output that drops all but one item. Tracing what happens to
a one-history_segment output showed it takes a different path through the discard-last
protection than a two-history_segment output: it skips the protection entirely,
because the guard is written as a length test.

## Evidence trail

The protection, `crates/daemon/src/history_summarizer_validate.rs:539-558`:

```rust
if !options.in_emergency && !options.force_keep_last_history_segment && history_segments.len() >= 2 {
    let last_end = history_segments.last().map(|c| c.end_message).unwrap_or(chunk.end_index);
    // TypeScript uses numeric ordinal distance here. ...
    let lookahead_distance = chunk.end_index.saturating_sub(last_end);
    let previous_end = history_segments.get(history_segments.len().saturating_sub(2)).map(|c| c.end_message);
    let pop_would_split_arc = previous_end.is_some_and(|end| {
        boundary_splits_completed_tool_arc(end.saturating_add(1), &chunk.completed_tool_arcs)
    });
    if lookahead_distance <= BOUNDARY_HEALING_SLACK && !pop_would_split_arc {
        history_segments.pop();
        discarded_last = true;
    }
}
```

`BOUNDARY_HEALING_SLACK = 2` at `:19`.

With `history_segments.len() == 1` the whole block is skipped. `discarded_last` stays
`false` (initialised at `:538`), so:

- The single history_segment publishes regardless of lookahead distance.
- `keep_side_channel` (`:1086-1098`) does not take its early-return
  (`:1091-1093`), so facts, events, primers, and observations from this run are
  ALSO kept, unlike the multi-history_segment weak-lookahead case which suppresses
  every side channel.

The purpose the protection serves is stated in the type itself,
`ValidatedChunk.discarded_last` (`:235-237`):

> True when the provisional last history_segment was intentionally withheld so it can
> be re-derived with real lookahead in the next run.

And in `ValidateOptions.in_emergency` (`:81-84`):

> When true, emergency recovery favors fast raw-history reduction over the
> highest-quality final boundary for the newest history_segment.

So the design intent is explicit: a boundary chosen without lookahead is
provisional and should not freeze into durable coverage unless recovery speed
matters more. A one-history_segment run gets the freeze without the option.

Interaction that suggests the guard may be deliberate: popping the only
history_segment would leave `history_segments` empty, and then the forward-progress check
at `:565-570` computes `last_new_end = history_segments.last().map(...).unwrap_or(0)`
which is `0`, so `0 < offset` and the run REJECTS with "no forward progress". So
`>= 2` may exist to avoid turning a weak-lookahead single-history_segment run into a
rejection. Verified this reading by reading `:560-570`:

```rust
let offset = prior_history_segments.last().map(|c| c.end_message.saturating_add(1)).unwrap_or(chunk.start_index);
let last_new_end = history_segments.last().map(|c| c.end_message).unwrap_or(0);
if last_new_end < offset { return Err(...); }
```

Confirmed: with an empty vector this rejects for any `offset > 0`. No comment
states this as the reason for `>= 2`, which is why the open question below is a
design question rather than a defect claim.

## Failure scenario

Chunk 200..=240, a single coherent topic. The model correctly produces one
history_segment covering it all and ending exactly at the chunk end:

```
<output><history_segments>
<history_segment start="200" end="240" title="Migration rollout plan" ...><p1>...</p1><p2>...</p2><p3>...</p3><p4/></history_segment>
</history_segments><meta><unprocessed_from>241</unprocessed_from></meta></output>
```

Lookahead distance is `240 - 240 = 0`. The model had zero messages after its
chosen boundary, so it could not know whether ordinal 241 continues the same topic.

Gate: check 7 passes; check 8 passes; check 9 passes; check 10 passes; contiguity
passes; `unprocessed_from == 241 == chunk_end + 1` passes at `:1063`;
`:539` is skipped for length; check 22 passes since `240 >= offset`.

Published. Coverage now ends at 240 with a boundary the model guessed.

Compare the two-history_segment case with identical lookahead: the last history_segment
is popped at `:555`, coverage stops at the previous end, and the next run
re-derives the boundary with real lookahead. That is the protection working. The
one-history_segment case gets the opposite treatment for the same evidential
situation.

Consequence: if ordinal 241 does continue the topic, the durable history_segment
boundary now bisects one narrative unit, and coverage monotonicity means it cannot
be moved back. The next run starts at 241 mid-topic. The raw text survives
(`history_summarizer.rs:1727`), so this is a boundary-quality failure, not data loss.

## Timing windows and dependencies

The window is evidential, not temporal: it is the state where the model's chosen
terminal boundary sits within `BOUNDARY_HEALING_SLACK` ordinals of the chunk end.

Two conditions must also hold, both defaults: `in_emergency` false and
`force_keep_last_history_segment` false. `ValidateOptions::default()`
(`:103-114`) sets both false, and `force_keep_last_history_segment` is documented at
`:94-96` as the explicit-wrapup flag, so the ordinary firing path has both false.

Dependency on chunk size: a chunk that yields only one history_segment is more likely
when the chunk is small, which is exactly the state a freshly-triggered history_summarizer
run is in. So this is not an exotic corner.

## What a test must construct

Direct unit test, mirroring the existing `discard_last_progress_guard_boundary_k1_vs_k2`
at `:1633` but with one history_segment:

1. Chunk `200..=240`, dense, all anchorable.
2. A single history_segment `200..=240` with valid tiers and
   `<unprocessed_from>241</unprocessed_from>`.
3. Assert the property: whenever `chunk.end_index - last.end_message <= 2` with
   both flags false, `validated.discarded_last` is true OR the last history_segment is
   absent. Today `discarded_last` is false and the history_segment is present, which is
   the finding.

A table is the right shape, since the axes are small and known: history_segment count
in `{1, 2, 3}` crossed with lookahead distance in `{0, 1, 2, 3}` crossed with
`in_emergency` and `force_keep_last_history_segment`. The existing test covers one
row of it (`len == 2`, distances 1 and 2). Filling the `len == 1` row is the
increment.

Important: the test must also assert the side-channel consequence, because it is
the part most likely to surprise. With one history_segment and weak lookahead, facts
survive; with two, `:1091-1093` suppresses them. Asserting both makes the
asymmetry explicit rather than incidental.

## Investigation log

### Q: Is the `>= 2` guard intentional, given that popping the only history_segment would trip the forward-progress reject?

- Sources examined: `history_summarizer_validate.rs:539` (the guard), `:560-570` (the
  forward-progress check and its `unwrap_or(0)`), `:235-237` (the
  `discarded_last` doc comment), `:544-546` (the only comment inside the
  discard-last block, which explains ordinal distance and not the length guard),
  `:1633-1651` (`discard_last_progress_guard_boundary_k1_vs_k2`), the golden case
  "discard-last suppressed for k1 progress guard".
- Findings: The mechanism is real: popping to empty rejects at `:565-570`. The
  existing test name contains "progress guard", and the golden case name says
  "suppressed for k1 progress guard", so the interaction between discard-last and
  forward progress is a recognised, tested concern. That is circumstantial evidence
  the `>= 2` is deliberate. But both the test and the golden case operate on a
  two-history_segment input where the guard's effect is about which history_segment
  survives, not about a one-history_segment input skipping the block entirely. No
  comment anywhere states "we require two so that popping cannot empty the set".
- Missing evidence: a design note. The scope map established there is no history_summarizer
  specification outside `history_summarizer*.rs`, so no such note exists.
- Conclusion: needs human input. If deliberate, the correct outcome for a
  weak-lookahead single-history_segment run needs stating: publish it (today's
  behaviour), reject the run, or defer the whole run. The third is plausible and is
  not currently expressible.

### Q: Does the emergency flag reach this path by default, making the protection normally off anyway?

- Sources examined: `history_summarizer_validate.rs:81-84`, `:103-114`
  (`ValidateOptions::default`), `:94-96` (`force_keep_last_history_segment`'s doc),
  `history_summarizer_chunk.rs:777` and `:506` (where `validate_options` is populated),
  `history_summarizer.rs:912`, `:930`, `:1633` (the option's carriage through the request
  structs).
- Findings: `Default` sets `in_emergency: false` and
  `force_keep_last_history_segment: false`. The option is threaded through request
  structs rather than derived inside the gate, so the gate is honest about it being
  caller-supplied.
- Missing evidence: which callers set `in_emergency` true, and how often that state
  occurs in practice. That is the firing-decision lens's territory
  (`lib.rs:4808-5184`).
- Conclusion: resolved with answer for this record — the protection is ON by
  default, which is what makes its absence for a single history_segment meaningful.
  The frequency of `in_emergency` is deferred to the firing lens.
