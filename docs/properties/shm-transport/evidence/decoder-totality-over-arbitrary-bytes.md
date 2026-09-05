# decoder-totality-over-arbitrary-bytes

## Discovery trigger

Three fuzz targets exist and each hands arbitrary bytes to a decoder, but no
record states the contract those targets are testing. Reading the decoders for
what they promise on hostile input turned up the gap: `RingGrant::decode`
carries three `.expect()` calls, `harness::read_u64` slice-indexes with
hand-computed offsets, and the only bound on any length-driven allocation is a
constant checked inside `validate`. Panic-freedom is asserted today by one test
that sweeps ten lengths and two fill bytes. That is a smoke test for a totality
claim, not evidence for it.

## Evidence trail

Three decode entry points, all in `crates/shm-transport`:

- `harness.rs:28-98` `frame_descriptor` — gates on
  `bytes.len() != FRAME_DESCRIPTOR_BYTES` (`:29-31`), then reads fields at
  hand-written offsets (`:32-60`) and calls `FrameDescriptor::validate`
  (`:73`).
- `harness.rs:102-113` `provider_grant` — `RingGrant::decode_slice`
  (`backend/ring.rs:924-927`).
- `harness.rs:118-146` `provider_sample` — `SamplePrefix::snapshot`
  (`backend/sample.rs:33-63`) then `validate` (`:74-98`).

No accepted value escapes a partially checked state. `ValidatedFrame` is
constructed once, at `descriptor.rs:325-333`, after all fourteen guards
(`:268-323`). `ValidatedSample` is constructed once, at `sample.rs:94-97`,
after all nine (`:79-93`). Both have private fields and no other constructor.
`RingGrant` differs in shape but not in effect: the value is materialized at
`ring.rs:902-918` *before* `checked_layout()?` runs at `:919`, so a
geometry-invalid grant briefly exists as a value — it just cannot reach the
`Ok(grant)` at `:920`.

Panic sites reachable from a decode call:

- `ring.rs:900`, `:907`, `:912` — three `.expect()` calls, and the constant
  range indexes at `:894`, `:905`, `:910` that precede them. All operate on
  `[u8; GRANT_BYTES]` where `GRANT_BYTES` is the literal `58` at `ring.rs:47`.
  The field widths sum to `2 + 16 + 4 + 8 + 8 + 8 + 8 = 54`, plus four reserved
  bytes at `:885` and `:894`, for 58. Nothing asserts that relationship.
- `harness.rs:19-23` `read_u64` — `bytes[offset..offset + 8]`, a panicking
  index. Its only bound is the exact-length gate at `:29-31`. I computed the
  offsets: the last read is `spans_offset + 24 = 100`, ending at exactly 108,
  which equals `FRAME_DESCRIPTOR_BYTES`. The margin is zero bytes.
- `sample.rs:34-37` is the one decoder that cannot drift: it uses
  `payload.get(..SAMPLE_PREFIX_BYTES)`, which is non-panicking, and
  `SAMPLE_PREFIX_BYTES` (`:19`) is derived from `WIRE_V2_HEADER_BYTES` rather
  than written as a literal, so the `copy_from_slice` widths at `:40`, `:43`,
  `:46`, `:49`, `:52` track it.

Allocation: none of the three decoders allocates. Every intermediate is a
fixed-size array or a `Copy` struct. The first allocation driven by an
attacker-declared length is `lease.rs:331`, `vec![0u8; self.body_len]`, and
its bound is entirely the decoder's `body_len > MAX_FRAME_BYTES` rejection
(`descriptor.rs:272-274`, `sample.rs:83-85`) against `MAX_FRAME_BYTES` = 64
MiB (`arena.rs:4`).

## Failure scenario

Three shapes, none of which the current tests would catch.

1. An offset constant in `harness.rs` is edited upward without the matching
   change to `FRAME_DESCRIPTOR_BYTES`. `read_u64` then indexes past 108 and
   panics on *every* 108-byte input, including the corpus `valid` seed. This
   one is loud, because the corpus replay would fail immediately.
2. `GRANT_BYTES` is reduced — say a field is narrowed — without updating
   `decode`. The range index at `ring.rs:894` panics on every call. The failure
   is not input-dependent, so a fuzz campaign reports it as a crash on the
   first execution rather than as a malformed-input finding.
3. The `body_len <= MAX_FRAME_BYTES` guard is relaxed or reordered below the
   point where `body_len` is used. `lease.rs:331` then allocates whatever the
   peer declared, up to `u64::MAX` truncated to `usize`. Nothing between the
   descriptor field and the `vec!` re-checks the bound.

## Timing windows and dependencies

No timing window: all three decoders are pure functions over one immutable byte
slice, which is what `harness.rs:1-6` claims and what I confirmed — no file
descriptor, mapping, or thread effect in any of them. The dependencies are
compile-time constants: `WIRE_V2_HEADER_BYTES` and `MAX_SPANS`
(`descriptor.rs:19`, `:23`), `GRANT_BYTES` (`ring.rs:47`), `MAX_FRAME_BYTES`
(`arena.rs:4`). Two of the three width constants are derived from those and one,
`GRANT_BYTES`, is an independent literal. This record is the precondition for
`accepted-decode-consumes-its-declared-width`: exact-consumption reasoning is
only meaningful once every input is known to terminate in accept or reject.

## What a test must construct

No fault injection. Three additions to what exists. First, a length sweep over
`0..=2 * width` for each decoder. The source tree had a ten-length, two-fill
sweep at `tests/contract.rs:743-768`; this tree does not (the file ends at line
735 and no decoder sweep exists anywhere in it), so the record's statement that
arbitrary-length coverage is absent here is the baseline. The new sweep needs
several fill patterns and structured mutation of an accepted seed — a `0x00` and
`0xff` fill alone reaches none of the arithmetic guards. Second, static assertions binding
each width constant to the sum of its field widths, so a narrowed field is a
compile error rather than a runtime index panic. Third, an allocation oracle:
assert that no accepted decode causes an allocation larger than
`MAX_FRAME_BYTES`, which requires observing `lease.rs:331` rather than the
decoder. Coverage check to emit: `shm_decode_reached_accept_path` per decoder,
so a campaign that only ever rejects is visible as such.

## Investigation log

### Q: Is any panic site reachable from arbitrary bytes at HEAD?

- Sources examined: `harness.rs` in full; `backend/ring.rs:874-962`;
  `backend/sample.rs:33-98`; `descriptor.rs:217-335`; `arena.rs:4`, `:40-67`;
  `lease.rs:331`; `tests/contract.rs:743-768` (source tree; not at HEAD); `tests/fuzz_corpus.rs` in
  full.
- Findings: no, not at HEAD. I computed the harness offsets by hand and by
  program: schema `0..2`, wire header `2..23`, incarnation `23..39`, lane
  `39..43`, sequence `43..51`, body length `51..59`, allocation start `59..67`,
  allocation length `67..75`, span count `75`, spans `76..108`. The final byte
  read is index 107 and the gate admits exactly 108, so every `read_u64` is in
  bounds. The three `.expect()` calls in `ring.rs::decode` are on constant
  eight-, sixteen-, and four-byte ranges of a 58-byte array and are infallible.
  The accept-path `.expect()` calls in `harness.rs:78-84` are guarded by
  `validate`: `span(index)` is `Some` for `index < span_count`
  (`descriptor.rs:386-388`), and the two `checked_add`s cannot overflow because
  `validate` already bounded `spans[0].offset + spans[0].len <= arena_bytes`
  (`descriptor.rs:289-295`) and forced `spans[1].offset == 0` with
  `spans[1].len <= arena_bytes` (`:310-319`).
- Missing evidence: nothing for the reachability question. What is missing is
  any statement of the invariants that keep it true. `GRANT_BYTES` is a literal
  with no static tie to its field widths, and the harness offsets have zero
  slack against their length gate, so both are one edit away from an
  unconditional panic that no property currently forbids.
- Conclusion: resolved with answer. Totality holds at this commit and rests on
  three hand-maintained constants and one zero-margin offset computation. The
  record exists because the reasoning lives nowhere in the tree, and because
  the existing evidence — a ten-length two-fill sweep — is far weaker than the
  claim it is taken to support.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 111, `tests/contract.rs:743-768` (the ten-length two-fill decoder sweep): The file ends at line 735 and contains no decoder length sweep, which the body of this record already states.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
