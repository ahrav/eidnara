# Property lens: failure recovery

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope: [source register](../source-register.md).

Malformed JSON, a typed-shape error, and transient admission failure are
different outcomes at `crates/daemon/src/lib.rs:12923-12946,8115-8123`.
The whole tree parse must enter a peak window before conversion; a failed
typed attempt must remain in that same window before fallback.

Candidate: `decode-footprint-covers-both-lanes-combined-peak`, including late
failure after large text and dense prefixes. Retry-after-release is bounded
to an explicit second attempt, not an eventual-recovery claim. Process-crash
and durable restart are N/A to these temporary allocations.
