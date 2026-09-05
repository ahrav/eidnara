# client-a-a-failed-pong-enqueue-retires-the-generation-as-a-local-fault

## Discovery trigger

Task 4 asked whether the client upholds the duties the protocol places on a peer,
including answering probes, and "whether a bug in the client could make a
well-behaved host retire the generation." The `Pong` path is the only probe duty
and its result is discarded with `let _ =`.

The first revision of this record was titled
`client-a-a-dropped-pong-is-never-observable-to-the-client`. Its finding was
that `send_control` had one failure exit, the encode branch, that returned `Err`
without retiring, so a `Pong` could be silently dropped and the client would
carry on believing it was healthy. That revision left one question open: whether
`encode_owned_frame` can fail at all for an empty body. The portfolio
disposition read `wire.rs` and answered it. The encode branch is impossible for
a `Pong`, so the only surviving failure exits both retire the generation. The
record was reframed to the guarantee the code actually provides: a failed `Pong`
enqueue is never merely dropped; it retires the whole generation with a local
admission code. What the earlier finding got right survives as the impact line:
nothing records that a probe went unanswered.

## Evidence trail

All references are at HEAD `e16e39e`. The catalog record still carries the
line numbers of the revision it was written against; every one below has been
re-verified and the HEAD line is what is cited.

The obligation, `docs/host-wire-protocol.md:277`:

> | 8 | `Pong` | required | consumer to host; echoes Ping control identity and
> flags |

and `:296`: "| `Ping` | `0 / 0 / nonzero` | empty | client returns matching
Pong |". `Ping` is listed "required" at `:276` as the "host to consumer liveness
probe".

The client's entire implementation of that duty, in
`crates/host-runtime/src/client.rs`:

```
1316:            FrameType::Ping => {
1317:                // `Pong` echoes `Ping` flags exactly.
1318:                let _ = self.send_control(
1319:                    FrameType::Pong,
1320:                    header.flags,
1321:                    FrameId::control(header.corr),
1322:                    None,
1323:                );
1324:            }
```

`send_control` (`:1249-1292`) has four exits:

| Exit | Site | Retires? | Code |
| --- | --- | --- | --- |
| already retired | `:1256-1258` | already retired | `generation_retired` (`:2287-2293`) |
| encode failed | `:1259-1265` | **no** | `encode_failed` |
| byte charge failed | `:1268-1275` | yes, `:1269` | `control_capacity_exhausted` |
| channel full | `:1283-1290` | yes, `:1284` | `control_capacity_exhausted` |

The encode exit is the one that does not retire, and it cannot run for a
`Pong`. `send_control` passes `Vec::new()` as the body at `:1259`.
`encode_owned_frame` (`wire.rs:543-572`) has exactly two `Err` returns: the
`MAX_BODY_LEN` check at `:549-553` and the `u32::try_from(body_len)` at `:555`.
Both need a non-empty body; an empty `Vec` passes both. So the disjunction over
`send_control`'s failure exits, restricted to the `Ping` arm, is
{already retired, charge failed, channel full}, and every member either was
already retired or retires now.

Both retiring branches call `retire("control_capacity_exhausted")`. `retire`
(`:1569-1577`) swaps `retired` to true, sets `closed`, settles every pending
request with the code, clears routes, and cancels the token. The code names the
admission failure, not the probe; nothing in `retire`'s signature carries the
frame type that triggered it.

The two resources the branches guard are sized together. `control_tx` is an
`mpsc::channel(CLIENT_CONTROL_QUEUE_FRAMES)` (`:361`, 32 slots at `:59`) and
`control_budget` is `ByteCounter::new(CLIENT_CONTROL_QUEUED_BYTES)` (`:374`),
where `CLIENT_CONTROL_QUEUED_BYTES = CLIENT_CONTROL_QUEUE_FRAMES * HEADER_LEN`
(`:68`). The doc comment at `:67` states the design: "A control-byte charge can
fail only when the control channel is full; that condition retires the
generation." The comment at `:1266` explains why the pool is reserved: ordinary
request traffic cannot consume it, so exhaustion is a local fault rather than
load.

The consequence one layer up: `ring_reader_loop` (`:2047-2059`) calls
`dispatch` at `:2053` and then checks `retired` at `:2054`, returning if it is
set. So a `Ping` whose `Pong` could not be enqueued ends the reader loop on the
next line, but the reader does not learn why; it sees only that the generation
retired during dispatch.

The flags echoed are the host's own. `validate_inbound` (`:2073-2135`) has
already constrained a `Ping` to channel 0, epoch 0, nonzero `corr`
(`:2111-2115`) and, as a pure-header frame, to `len == 0`, binary clear, last
clear, admission `Normal` (`:2127-2134`), with priority unconstrained.
`docs/host-wire-protocol.md:263` states this is deliberate: "A conforming
`Ping` therefore never carries flags whose mandated `Pong` echo the host would
have to reject."

Existing tests. `a_ping_at_any_valid_priority_is_answered_with_an_exact_flag_echo`
(`:2804-2856`) drives `dispatch` with a `Ping` at each priority, reads the
`Pong` off `control_rx`, asserts the flag byte echoes (`:2832-2835`), and
asserts `!retired` (`:2836`). It covers the success path only.
`control_exhaustion_retires_and_releases_all_queued_bytes` (`:3237`) calls
`send_control(FrameType::Pong, ...)` 32 times (`:3239-3248`), then a 33rd
(`:3249-3256`), and asserts `error.code() == "control_capacity_exhausted"`
(`:3257`) and `retired` (`:3258`). It reaches the retiring branch, but by
calling `send_control` directly rather than through the `Ping` arm, so the
`let _ =` at `:1318` and the reader-loop consequence at `:2054` are not
exercised.

## Failure scenario

The property fails if a future edit makes a `Pong` enqueue failure
non-retiring. Two shapes are plausible.

A body is added to `Pong`. If the `Vec::new()` at `:1259` were replaced by a
caller-supplied body, the encode branch at `:1259-1265` becomes reachable, and
it returns `encode_failed` without retiring. The `let _ =` at `:1318` swallows
it, `ring_reader_loop` finds `retired` false at `:2054` and continues, and the
client is silently in breach of its only liveness obligation. The host's probe
deadline runs down and the host retires; Part 2a's
`a-timely-pong-sustains-the-generation-within-a-bounded-round` is the host-side
property that then fails.

A retire call is removed. If either `self.retire(...)` at `:1269` or `:1284`
were dropped, the same silent breach follows from that branch. The doc comment
at `:62` and `:67` would then be false.

In the current code neither shape exists, and the observable outcome of a
failed enqueue is a retired generation whose pending requests all settle with
`control_capacity_exhausted`. That is a loud local fault. What it is not is an
attributed one: nothing names the unanswered probe, which is the residual
finding routed to `client-a-a-retired-generation-forgets-why-it-retired`.

## Timing windows and dependencies

Retirement is synchronous inside the failing `send_control` call, so there is
no window for the property itself. The reader loop observes the retirement on
its very next statement (`:2053` then `:2054`).

The precondition is a full reserved pool: 32 queued control frames, or the
matching 32 header-lengths of budget. `writer_loop` (`:1986`) drains
`control_rx` with priority (`try_recv` at `:1993`, then a `biased` select at
`:1996-1999`), so under normal operation the pool empties as fast as the ring
accepts writes. Exhaustion requires the writer to be stalled or slow, which is
`client-a-pong-egress-is-not-bounded-by-any-client-side-liveness-budget`'s
territory: that record covers a `Pong` accepted and stalled; this one covers
what happens once the stall has filled the pool and the next `Pong` cannot be
accepted at all.

## What a test must construct

1. Fill the reserved pool without draining it: hold `control_rx` and call
   `send_control(FrameType::Pong, ...)` 32 times, as `:3239-3248` does, or
   consume the byte budget by the same means.
2. Deliver a `Ping` through the `Ping` arm rather than through `send_control`
   directly: build a valid `Ping` header as `:2814-2822` does and call
   `inner.dispatch(ping, Vec::new(), ByteCharge::none())` (`:2828`).
3. Assert `retired` is true afterwards, and that a pending request settled with
   code `control_capacity_exhausted`. The `control_exhaustion_*` test's
   assertions at `:3257-3258` are the model, moved behind `dispatch`.
4. Assert the negative half of the guarantee: no queued frame, no settled
   error, and no counter names `Ping` or `Pong`. This is what distinguishes
   "retired as a local fault" from "retired as a missed probe".
5. Optionally, assert the reader-loop consequence by driving
   `ring_reader_loop` with a stalled writer and observing that it returns after
   the `Ping` rather than after `eof`.

## Investigation log

### Q: Can `encode_owned_frame` reject a flag byte `validate_inbound` accepted?

- Sources examined: `client.rs:2111-2115` and `:2127-2134` (the `Ping` and
  pure-header gates), `docs/host-wire-protocol.md:263` (the design intent that a
  conforming `Ping`'s flags are always echoable), `client.rs:1259` (the encode
  call, with `Vec::new()` as body), and `wire.rs:543-572`
  (`encode_owned_frame`).
- Findings: `encode_owned_frame` never inspects `flags`. Its two `Err` returns
  are `body.len() > MAX_BODY_LEN` at `:549-553` and `u32::try_from(body_len)`
  at `:555`; both are about body length. With an empty body neither can fire.
  The first revision of this record left this open because `wire.rs` was
  another sub-part's scope; reading it closes the question.
- Missing evidence: none.
- Conclusion: resolved with answer. No. The encode branch is unreachable for a
  `Pong`, so the only real failure exits both retire, and the record's guarantee
  is `always` rather than `always-or-unreached`.

### Q: Is the discarded `Result` at `:1318` the only swallowed control send?

- Sources examined: every `send_control` call site at HEAD: `:1229` (Cancel in
  the pending-removal path, result inspected), `:1301` (inside
  `send_control_wait`, mapped to a `ClientError`), `:1318` (Pong, discarded),
  `:1414` (Cancel, discarded), `:1454` (Cancel, discarded), `:1489` (Goodbye,
  result inspected).
- Findings: three call sites discard. The two `Cancel` discards are best-effort
  by design; the caller already holds a local classification of its own
  outcome. The `Pong` discard is different in kind because no local caller is
  protected: the injured party is the host. After the reframe this is no longer
  a safety gap, because every reachable failure has already retired the
  generation before `Err` is returned. It remains the reason the retirement is
  unattributed.
- Missing evidence: none.
- Conclusion: resolved with answer. `:1318` is the only discard whose failure
  has no compensating local classification, which is why the attribution loss
  routes to `client-a-a-retired-generation-forgets-why-it-retired`.

### Q: Is escalating one unanswerable probe to full retirement intended?

- Sources examined: the comment at `client.rs:1266-1267`, the constant
  comments at `:58-68`, and the two retiring branches at `:1268-1275` and
  `:1283-1290`.
- Findings: the reserved-pool design means ordinary request traffic cannot
  exhaust the control pool, so exhaustion is a genuine local fault. The
  comments at `:62` and `:67` state the retirement explicitly. That argues the
  escalation is deliberate. It does not establish that a caller should be
  unable to tell a probe failure from any other control-admission failure,
  which is the policy question.
- Missing evidence: a design statement covering attribution of the retirement
  cause.
- Conclusion: needs human input.

### Q: Does a `Ping` arriving after `close` still count as enqueued?

- Sources examined: `client.rs:640` (`close`), `:680` (the cancel inside it),
  `writer_loop` at `:1996-1998` (`biased` select, cancelled arm breaks).
- Findings: after `close` cancels the token, `writer_loop` exits at `:1998`.
  `send_control` still checks only `retired` (`:1256`), so a `Ping` dispatched
  in that window can be enqueued successfully into `control_tx` and never
  written. `send_control` returns `Ok`. This is not a failed enqueue and so is
  outside this record's guarantee; it is a successful enqueue that produces no
  egress. The host is already tearing the generation down in that window.
- Missing evidence: none for classification. Whether it deserves its own record
  is a scoping call.
- Conclusion: resolved with answer. Out of scope for this record; noted so the
  reframe does not lose the observation.
