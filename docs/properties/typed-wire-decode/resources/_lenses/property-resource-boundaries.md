# Property lens: resource boundaries

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope and P notation: [source register](../source-register.md).

P:L88, P:L198, and P:L213 imply three different measurements: decoder peak,
retained ownership, and decode-plus-projection peak. One does not prove the
others. The decoder counter's requested-size floor
(`crates/daemon/tests/parse_charge_covers_typed_decode.rs:28-29`) does not
establish allocator residency.

Candidates: combined decode footprint; ownership-led retained accounting;
continuous pool coverage; isolated messages allocation gate. The latter uses
`events <= 16 * message_count` and `peak < 3 * messages_json_bytes`; integer
division and whole-body counters can both change its meaning. Whole-body
native Values and projection remain measured in the separate full ledger.
