# measured-transfer-is-witnessed-by-the-data

Status: invalidated. The hardware-envelope benchmark and its manifest are
removed. The former partial exercise status and current-check claim are
withdrawn.

## Discovery trigger

The former benchmark reported a checksum for each transfer arm. Discovery
asked whether the reported value depended on delivered bytes rather than
only on requested iteration and payload counts.

## Evidence trail

The cleanup removed `crates/shm-transport/benches/hardware_envelope.rs` and
its manifest, including the consumer fold and expected-checksum comparison.
No current measurement arm or mismatch check remains for this record.

The retained `LeaseSpan::checksum` helper is a wrapping byte sum
(`crates/shm-transport/src/lease.rs:97-125`). Its existence does not establish
that a removed benchmark receives, reports, or compares those bytes.

The [pre-cleanup investigation](https://github.com/ahrav/eidnara/blob/705899ad28341043c163d2a137b076acbbc707e2/docs/properties/shm-transport/evidence/measured-transfer-is-witnessed-by-the-data.md)
is preserved in version history. Its source offsets and "At HEAD" notes
refer to historical trees, not this checkout.

## Failure scenario

A future benchmark could report its requested payload size as a checksum and
hide corruption. This record does not assert a defect in retained transport
behavior or withdraw its separate correctness guarantees.

## Timing windows and dependencies

Reactivation requires a measured transfer, its reporting path, and an
independent expected value. No current campaign is pending.

## What a test must construct

A replacement must corrupt a delivered byte without preventing delivery and
observe a failing comparison. Its stated corruption model must account for
a wrapping sum's inability to detect reordering or compensating byte changes.

## Investigation log

### Q: Does the former partial witness still execute?

- Sources examined: The cleanup diff, tracked-file inventory, and retained
  checksum helper.
- Findings: The benchmark comparison is removed; the helper remains.
- Missing evidence: No replacement measurement witness is retained.
- Conclusion: invalidated. A helper alone is not a benchmark witness.
