# System lens: claimed liveness guarantees

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External lead: plan R6 and latency-audit W5.

## Observations

`crates/daemon/src/history_summarizer.rs:326-334` rejects a mismatched chunk
fingerprint. `crates/daemon/src/lib.rs:16443-16461` skips unreadable raw rows.
Both can prevent useful recovery, but neither defines a new bounded progress
promise for this decode replacement.

## Disposition

Historical compatibility is a safety check on a supplied old row and on a
specified fingerprint input. Situation coverage must construct those inputs.
Do not translate successful deserialization into an unbounded promise that a
history_summarizer firing eventually publishes.

## Narrow nonapplicability and missing evidence

No liveness record is added because this slice changes synchronous
serialization and identity, not retry scheduling or completion deadlines.
HistorySummarizer scheduling, failure backoff, and producer completion remain outside
this identity portfolio. No recovery-duration evidence is supplied.
