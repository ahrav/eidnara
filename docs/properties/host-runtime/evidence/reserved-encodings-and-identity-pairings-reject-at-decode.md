# reserved-encodings-and-identity-pairings-reject-at-decode

Carried into this sub-part from the superseded `part-2b-wire-and-channels`,
where it was record 3 of `_lenses/lens-a-wire-format.md` (`L291-334`). This is
the record the earlier triage flagged as needing an `Existing check:` refresh
because `tests/protocol_vectors.rs` changed. The refresh was done at carry time
and is recorded below: **three citations repaired, and the property itself
unchanged.**

## Discovery trigger

The header's forward-compatibility budget is its reserved regions: flag bits 6-7,
the reserved priority encoding, the reserved admission encoding `0b11`, and the
unassigned type bytes. A decoder that *masks* a reserved value instead of
refusing it spends that budget silently, and there is no way to notice from the
outside. Two of `Flags`' six accessors return `Option` and four return `bool`,
which is a live invitation to write the next one in the wrong style.

## Evidence trail

**The six gates that carry this property, all re-read at carry time.**

| Rule | Line | Rejection |
| --- | --- | --- |
| Unassigned type byte | `:321` | `DecodeError::UnknownFrameType { byte }` |
| Reserved flag bit 6 or 7 | `:323-325` | `DecodeError::ReservedFlagBits { flags }` |
| Reserved priority encoding | `:326-328` | `DecodeError::ReservedPriorityBits { flags }` |
| Reserved admission encoding | `:329-331` | `DecodeError::ReservedAdmissionClass { flags }` |
| Sheddable on a delivery-required type | `:332-339` | `DecodeError::SheddableIllegalFrameType { ty, flags }` |
| Channel-and-epoch mismatch | `:345-347`, `:352-354` | `NonzeroEpochOnControlChannel`, `ZeroEpochOnRoutedChannel` |

Every one returns `Err`. None clears a bit, defaults a value, or normalizes.

**The `Option`-versus-`bool` asymmetry, which is the record's real subject.**
`Flags` is `pub struct Flags(pub u8)` at `wire.rs:142`. Its accessors:

- `is_binary` at `:159-161` - `bool`, `self.0 & FLAG_BINARY != 0`
- `is_last` at `:164-166` - `bool`, `self.0 & FLAG_LAST != 0`
- `priority` at `:169-171` - `Option<Priority>` via `Priority::from_bits`
- `admission_class` at `:174-176` - `Option<AdmissionClass>` via
  `AdmissionClass::from_bits`
- `has_reserved_bits` at `:179-181` - `bool`, `self.0 & FLAG_RESERVED_MASK != 0`

The two `Option` accessors are the only reason a reserved priority or admission
encoding is refused rather than silently coerced, and the refusal happens because
`decode_header` propagates the `None`: `:326` tests `priority().is_none()` and
`:329-331` chains `.ok_or(...)?` on `admission_class()`. `Priority::from_bits`
(`:101`) and `AdmissionClass::from_bits` (`:121`) are what return `None`. A
future flag written in the `bool` style over a widened field would mask, and
nothing outside these two call sites would notice.

**The channel-and-epoch pairing is a true biconditional.** `:345` rejects
`channel == 0 && epoch != 0` and `:352` rejects `channel != 0 && epoch == 0`, so
the accepted set is exactly `(channel == 0) == (epoch == 0)`. That matches the
normative table at `docs/host-wire-protocol.md:232-233`: "0 is control; routed
channels are nonzero" and "0 on channel 0; routed epochs are nonzero". `:352`
carries a five-line comment (`:348-351`) explaining that a routed channel with no
epoch names no bindable route, which is design intent rather than an accident.

**Existing coverage, and the three repairs.**

In-file, three tests:

- `reject_unknown_frame_type_and_reserved_flag_encodings` (`:745-774`) covers
  four inputs: type byte 99 (`:748`), flags `0b1000_0000` (`:757`), flags
  `0b0000_0110` (`:763`), flags `0b0011_0000` (`:769`). **Repair 1:** the lens
  wrote `:745-773`; `:773` is the `);` of the last assertion and the closing
  brace is `:774`.
- `sheddable_rejected_on_every_illegal_frame_type` (`:836-862`) builds Sheddable
  flags at `:838` and sweeps the illegal types, then confirms `Push` and
  `StreamData` are accepted at `:858-861`.
- `epoch_boundaries_round_trip_and_control_channel_epoch_is_reserved`
  (`:795-833`) covers both halves of the pairing, ending with
  `ZeroEpochOnRoutedChannel { channel: 7 }` at `:831`.

End-to-end, two tests in `tests/protocol_vectors.rs`, and both moved:

- **Repair 2.** `structural_corruption_closes_silently` at `:512` no longer
  exists. It was **renamed** to `structural_corruption_is_rejected_before_dispatch`
  and now sits at `:351` (`#[tokio::test]` at `:350`). This was verified as a
  rename rather than a rewrite by printing both versions: the doc comment above
  it is byte-identical ("Each structurally illegal frame retires the generation
  with no `Error` frame and no resynchronization (protocol §6.3, AE2, V13-V15,
  V17, V42)"), and so is the `struct Case { name, bytes }` declaration and the
  first `Case` of the table, `"unsupported version"`. So the check the lens cited
  is the check that still exists, under a new name.
- **Repair 3.** `pure_header_frames_accept_any_valid_priority` kept its name and
  moved from `:656` to `:504` (`#[tokio::test]` at `:503`).

The cause is one commit: `63c4d277` ("refactor(shm): enforce ring-only
architecture") took `tests/protocol_vectors.rs` from 976 lines at `1c193ae0` to
762 at `e447c927`. Blob hashes confirm the file is unchanged between `1c193ae0`
and `793a973e` (`21f03055`) and changed at `e447c927` (`0cbd259e`). Neither
subject file moved: `crates/host-runtime/src/wire.rs` is blob `fd0bb178` at all three
commits.

None of the five checks runs in CI, under this sub-part's `R0`.

**What the coverage adds up to.** Four flag or type bytes out of 512 possible
`(flags, type)` byte values, the Sheddable cross-product in full, and both
pairing halves. Missing: the exhaustive 256×256 sweep, and any assertion that a
rejected encoding is never normalized. The second gap is the interesting one,
because the existing tests all assert a specific `DecodeError` variant, which
means they would catch a *changed* rejection but not a rejection replaced by
acceptance-with-masking - that would simply stop matching `Err(..)` and fail, so
in fact they would catch it for these four inputs and only these four.

**Reachability.** The three production `decode_header` call sites named in
`catalog.md`'s group preamble: `ring_transport.rs:503`, `ring_transport.rs:730`,
`client.rs:1978`. All six gates are unconditional statements in the function
body, with no `cfg`, feature, or config branch between any call site and any
gate. `wire.rs`'s only two `#[cfg]` attributes are `:541` and `:646`.

## Failure scenario

Version 3 of the protocol assigns flag bits 6-7 to a new field. A version-2 peer
that *masks* rather than rejects receives such a frame, clears the bits, and
processes the frame as though the new field were absent. That is exactly the
failure the reserved region exists to prevent: the whole point of reserving bits
is that an old peer must close the generation rather than half-understand the
frame.

The concrete regression path is narrow and plausible. Someone adds a sixth flag
accessor for a newly assigned bit, writes it in the `bool` style like
`is_binary` and `is_last` because those are the majority, and widens
`FLAG_RESERVED_MASK` to free the bit. The reserved-bit gate at `:323` then no
longer covers it, and no `Option` propagates, so the bit is read when set and
ignored when the field's encoding is invalid.

A second, cheaper failure: someone "fixes" a spurious rejection by changing
`:326` from `if flags.priority().is_none() { return Err(..) }` to a default,
because a peer sent a reserved priority and the connection dropped. That is a
one-line change, it looks like robustness, and it silently converts a
forward-compatibility refusal into a coercion.

For the pairing, the failure is different in kind. Dropping either half of the
biconditional does not lose forward compatibility; it produces a frame that
names no bindable route and has to be dropped further up as unmatched, which the
comment at `:348-351` says was the alternative the author rejected. The
observable consequence would be an accepted frame with no destination.

## Timing windows and dependencies

None. All six gates are straight-line statements in a pure function over one
immutable slice.

One dependency worth stating: the Sheddable gate at `:332-339` reads
`ty.is_pure_header`'s sibling classification, the `matches!(ty, Push |
StreamData)` set. If the set of delivery-required types changes, the gate's
meaning changes with it and no test outside
`sheddable_rejected_on_every_illegal_frame_type` (`:836-862`) would notice.

This record's `Check:` sweeps flags crossed with types, which overlaps the
totality record's arbitrary-bytes sweep. They are not the same oracle: totality
asserts the call returns and the eleven postconditions hold; this one asserts a
*specific* `DecodeError` variant per invalid combination, which is strictly
stronger and catches a rejection moved to the wrong gate.

## What a test must construct

One in-crate sweep and one negative assertion, both in `wire.rs`'s test module.

1. **The 256×256 sweep.** For every `(flags_byte, type_byte)` pair, build a
   21-byte header with a legal channel-and-epoch pairing and assert the outcome
   against an independently computed expectation: reject with
   `UnknownFrameType` if the type byte is above 11, else `ReservedFlagBits` if
   bits 6-7 are set, else `ReservedPriorityBits` if the priority field is
   `0b11`, else `ReservedAdmissionClass` if the admission field is `0b11`, else
   `SheddableIllegalFrameType` if Sheddable and the type is outside
   `{Push, StreamData}`, else `PureHeaderFrameWithBody` if the type is pure-header
   and `len != 0`, else accept. 65,536 cases, each a pure call. The expectation
   must be computed from the protocol table at
   `docs/host-wire-protocol.md:226-234` (the offset table), `:240-246` (the
   flag bit fields, with `:246` reading "| 6-7 | reserved | MUST be zero |") and
   `:248` (the Sheddable restriction), not from `Flags`' own accessors,
   or the oracle is circular - per METHOD's rule against a circular expected
   value.
2. **Both pairing classes.** Cross the above with `(channel, epoch)` in
   `{(0, 0), (0, 1), (1, 0), (1, 1)}` and assert `NonzeroEpochOnControlChannel`
   for `(0, 1)` and `ZeroEpochOnRoutedChannel` for `(1, 0)`. Note the gate order:
   the flag and type gates precede the pairing gates, so a frame that violates
   both reports the flag error, and the expectation must model that ordering.
3. **No-normalization assertion.** For every accepted result, assert
   `!header.flags.has_reserved_bits()`, `header.flags.priority().is_some()` and
   `header.flags.admission_class().is_some()`. This is the assertion that would
   survive a masking regression, because it is stated over the *accepted* value
   rather than over the rejection.

No fault injection. The 65,536-case sweep is a few milliseconds of pure
arithmetic. The cost is placement: it lands in the lib target, which no CI job
builds.

## Investigation log

### Q: Do the two `tests/protocol_vectors.rs` checks the lens cited still exist, and at which lines?
- Sources examined: `tests/protocol_vectors.rs` at `1c193ae0`, `793a973e`
  and `e447c927`; the blob hashes of that file and of
  `crates/host-runtime/src/wire.rs` at all three commits; commit `63c4d277`
  ("refactor(shm): enforce ring-only architecture"); the doc comment, the
  `struct Case { name, bytes }` declaration and the first `Case` of the table
  printed from both versions of the renamed test. The lens itself recorded
  `Open questions: None.` for this record, so this entry records a verification
  the carry performed, not a question the lens left open.
- Findings: `structural_corruption_closes_silently` at `:512` no longer
  exists. It is renamed to `structural_corruption_is_rejected_before_dispatch`
  at `:351` (`#[tokio::test]` at `:350`). The doc comment above it, the `Case`
  struct and the `"unsupported version"` case are byte-identical across the
  rename, so it is the same check under a new name.
  `pure_header_frames_accept_any_valid_priority` keeps its name and moves from
  `:656` to `:504` (`#[tokio::test]` at `:503`). One commit causes both moves:
  `63c4d277` takes the file from 976 lines at `1c193ae0` to 762 at `e447c927`.
  The file's blob is `21f03055` at `1c193ae0` and `793a973e` and `0cbd259e`
  at `e447c927`. `wire.rs` is blob `fd0bb178` at all three commits, so the
  subject did not move.
- Missing evidence: none at carry time.
- Conclusion: resolved with answer. Both checks exist at the new positions
  recorded in the evidence trail. Neither the property, the guarantee, the
  check semantics nor the impact changes.

### Q: Is the lens's in-file span for `reject_unknown_frame_type_and_reserved_flag_encodings` correct?
- Sources examined: `crates/host-runtime/src/wire.rs` at `e447c927`, lines
  `:745-774`.
- Findings: the lens wrote `:745-773`. `:773` is the `);` of the last
  assertion and the closing brace of the function is `:774`. The span is short
  by one line.
- Missing evidence: none.
- Conclusion: resolved with answer. The span is corrected to `:745-774` in
  the evidence trail. The test's four inputs and its assertions are unchanged.

### Q: Is the gap in existing coverage the oracle's strength or its input set?
- Sources examined: the three in-file tests (`:745-774`, `:795-833`,
  `:836-862`) and the `DecodeError` variants each one asserts.
- Findings: every existing test asserts a specific `DecodeError` variant, not
  just `Err(..)`. That pins which gate fired, which is stronger than "rejects".
  A masking regression on any of the four covered inputs fails today, because
  acceptance stops matching `Err(..)`. The uncovered set is the other 65,532
  `(flags, type)` pairs, not the shape of the assertion.
- Missing evidence: the exhaustive 256x256 sweep and the no-normalization
  assertion described under "What a test must construct".
- Conclusion: resolved with answer. The gap is input coverage. The sweep must
  keep the variant-pinning shape of the existing tests rather than weaken to
  `is_err()`.

### Q: Do this record's citations still resolve against the current checkout?
- Sources examined: `git cat-file -t` for `e447c927`, `63c4d277` and
  `1c193ae0`; `git rev-parse HEAD`; `git hash-object` and `grep -n` over
  `crates/host-runtime/src/wire.rs` and
  `crates/host-runtime/tests/protocol_vectors.rs` at `HEAD`.
- Findings: `HEAD` is `e6b944f`. None of `e447c927`, `63c4d277` or
  `1c193ae0` is a valid object in this checkout, so the carry-time history
  cannot be replayed here. `wire.rs` is blob `ee247eb3`, not `fd0bb178`. Its
  decode gates start at `:325` (`UnknownFrameType`) and end at `:354-356`
  (`ZeroEpochOnRoutedChannel`), and every gate still returns `Err`. The three
  in-file tests are at `:712`, `:762` and `:801`. The file's two `#[cfg]`
  attributes are at `:513` and `:613`. `tests/protocol_vectors.rs` is 765
  lines; `structural_corruption_is_rejected_before_dispatch` is at `:362` and
  `pure_header_frames_accept_any_valid_priority` at `:515`.
- Missing evidence: the commit path from `e447c927` to `e6b944f`, which this
  checkout does not contain. Without it the drift cannot be attributed to a
  specific change.
- Conclusion: unresolved, needs a citation refresh of the sections above
  against `e6b944f`. The line numbers elsewhere in this record are carry-time
  numbers and this log keeps them as written so the record stays internally
  consistent until that refresh is done.
