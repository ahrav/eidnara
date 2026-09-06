# documented-close-order-has-a-production-driver

## Discovery trigger

`docs/shm-transport.md:63` (source tree; not at HEAD) states a seven-stage close ordering as
implemented behaviour, and the traceability record marks the corresponding
requirement `PASS` on the strength of a contract test. The type that encodes
that ordering was traced to its callers to establish which shipping code obeys
it.

## Evidence trail

`crates/shm-transport/src/lifecycle.rs` encodes the ordering faithfully.
`CloseState` (lines 4-27) declares eleven variants whose doc comments track the
documented stages almost word for word — `Quiescing` is "New admission has
stopped", `DrainingPublished` is "Already-published frames are draining",
`RevokingJsOnEnv` is "The environment thread detaches JavaScript aliases"
(reworded from "JavaScript aliases are being detached on environment thread"
by the #131 comment pass),
`AwaitingRustScopes`, `ReleasingSamples`, `DroppingTransport`, `Joined`.
`advance` permits exactly one edge per step (valid pairs at lines 59-77) and
treats `Joined` and `Quarantined` as terminal (line 80). `mark_prepared`
(lines 41-47) and `must_fail_closed` (lines 49-51) encode the
no-TCP-replay-after-preparation rule (formerly cited to
`docs/shm-transport.md:61`; the trimmed post-#131 document no longer
carries that sentence — unresolved, needs a current normative source for the
documented ordering).

A repository-wide search for `CloseState`, `Lifecycle::new`, `mark_prepared`,
and `must_fail_closed`, excluding `docs/` and `target/`, returns matches in
exactly two files:

- `crates/shm-transport/src/lifecycle.rs` — the definition itself.
- `crates/shm-transport/tests/contract.rs` — lines 272-335, all inside
  `lifecycle_accepts_only_diagram_edges_and_quarantine_is_terminal` (declared at
  line 272).

No production caller exists. `Lifecycle` is constructed only by the test.

The two real close paths each implement their own ordering:

- **Addon.** `close` (`packages/shm-native/src/lib.rs:1546`) calls
  `close_channel` (`:407-413`), which sets `channel.closed = true`, aborts every
  registered producer reservation via `detach_producer(...)?.abort()`, detaches
  every active lease, then detaches stranded references. `force_close` (`:1579`)
  calls `quarantine_channel` (`:415`), which additionally calls
  `enter_quarantine()` on both rings (`:421-422`) before the same detach
  sequence. Neither calls `Lifecycle::advance`.
- **Host.** The endpoint thread wraps `run_endpoint` in `catch_unwind`
  (`crates/host-runtime/src/ring_transport.rs:331-342`) and then takes no branch at
  all on the result: the unconditional `admission.release()` at `:360` follows
  regardless of outcome (the former clean/unclean custody disposition was
  deleted with `shm_provider.rs`). This is a disposition decision, not an
  ordered teardown, and it does not call `Lifecycle::advance` either.
  At HEAD: The thread does branch on the outcome at HEAD: a panic increments the counter, cancels, and sends a `Corrupt` close at `:343-349`, and the disposition chooses `admission.quarantine()` at `:358` when either ring is quarantined or `admission.release()` at `:360` otherwise.
  At HEAD: `close_channel` now sets `closed`, sends `setup::goodbye`, and delegates the sweep to `detach_all_aliases` (`:386-405`), which aborts producers, detaches leases, then detaches stranded views and returns the first failure instead of stopping at it.

The documented "drains published data" stage has no counterpart in the addon
path. `close_channel` **aborts** producer reservations rather than committing or
draining them, and **detaches** active leases rather than waiting for them to be
received. Published-but-unreceived frames are not polled.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

A future change reorders the addon close path — for example, detaching stranded
references before active leases, or dropping the mapping before the last alias
is revoked. `lifecycle_accepts_only_diagram_edges_and_quarantine_is_terminal`
still passes, because it exercises a model that the changed code does not touch.
The traceability row still reads `PASS`. The documentation still describes the
original order. Nothing in the tree contradicts any of the three, and the
divergence is invisible until a use-after-free surfaces at runtime.

## Timing windows and dependencies

None. This is a static reachability question: does any non-test caller advance
the machine. The answer does not depend on scheduling, faults, or state.

The dependency worth naming is that this property is what makes several other
close-ordering claims unverifiable. Any property whose evidence is the
lifecycle contract test inherits this gap, because that test proves the model's
edges rather than the shipping paths' behaviour.

## What a test must construct

A reachability assertion, not a state construction:

1. Assert that at least one non-test caller advances `Lifecycle`. Today this
   fails by inspection, so the check is a static one — for example, a test that
   would fail if `Lifecycle` were referenced only from `tests/`.
2. Failing that, the alternative is to make the shipping paths the object of
   test. That requires an observable trace of stage transitions from
   `close_channel`, `quarantine_channel`, and the host disposition branch, and
   an assertion that the observed sequence is an accepted path through
   `CloseState`.
3. Either way, a case that pins the `DrainingPublished` stage is needed, because
   that is the stage with no implementation in the addon path. The construction
   is a channel closed while a committed frame has not been received, asserting
   the documented disposition for that frame.

## Investigation log

### Q: Is the state machine intended to become the driver, or is it a specification artifact? If specification-only, which code is normative for close ordering?

- Sources examined: `crates/shm-transport/src/lifecycle.rs` in full;
  repository-wide search for `CloseState`, `Lifecycle::new`, `mark_prepared`,
  `must_fail_closed` excluding `docs/` and `target/`;
  `crates/shm-transport/tests/contract.rs:379-444`;
  `packages/shm-native/src/lib.rs:407-424` and `:1546-1594`;
  `crates/host-runtime/src/ring_transport.rs:324-367`;
  `docs/shm-transport.md` (the close narrative formerly at `:59-65` is
  gone from the trimmed post-#131 document).
- Findings: the machine is a complete and internally consistent encoding of the
  documented ordering, with per-edge validation and correct terminality. It has
  no production caller. The two shipping close paths implement partial,
  differently-shaped teardowns and neither references it. The pre-#131
  `//! Checked close state machine` module doc comment is gone at HEAD
  (`lifecycle.rs` now opens with `use std::fmt;`), so nothing
  distinguishes specification from implementation.
- Missing evidence: no plan document, comment, or issue reviewed for this part
  states whether the machine is intended to be wired in. The `advance` API
  takes `&mut self`, which suits a driver rather than a validator, but that is
  suggestive, not decisive. I did not read `run_endpoint` in full, so whether
  the host path performs an internal drain before returning its boolean is not
  established here; what is established is that it does not advance the machine.
- Conclusion: needs human input. The mechanical facts are settled — two
  referencing files, no production driver, and one documented stage with no
  counterpart in the addon path — but which artifact is normative for close
  ordering is a decision the tree does not record.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 43, `:358-373` now `:407-413`: `close_channel` now sets `closed`, sends `setup::goodbye`, and delegates the sweep to `detach_all_aliases` (`:386-405`), which aborts producers, detaches leases, then detaches stranded views and returns the first failure instead of stopping at it.
  - line 51, `:276` now `:360`: The thread does branch on the outcome at HEAD: a panic increments the counter, cancels, and sends a `Corrupt` close at `:343-349`, and the disposition chooses `admission.quarantine()` at `:358` when either ring is quarantined or `admission.release()` at `:360` otherwise.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 5, `docs/shm-transport.md:63` (the seven-stage close ordering): The trimmed document carries no close-ordering narrative; `:63` now describes the diagnostics error class.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
