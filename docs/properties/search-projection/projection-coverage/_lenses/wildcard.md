# RP2.1 wildcard passes

System and external-source provenance are in [model.md](model.md).
Revision: `913234433ae36a80a6e22c6aac14c7f9aab74386`; date: 2026-09-10.
Both wildcard passes run after all named model and property passes.

## System-model wildcard

The plan calls `pending_outbox` existing, but
`crates/kernel/src/outbox.rs:450-453` filters on global `published_at`, not
the registered consumer checkpoint. A second consumer or publisher can make
rows disappear from that API without this projection consuming them. This is
an unresolved adapter seam, not proof of an RP2.1 bug in absent code. The
consumer must name how it obtains its complete durable prefix.

`outbox.rs:89-92` can initialize a consumer above zero. The local checkpoint
and acknowledgement comparison needs the same source incarnation and an
established bootstrap baseline. Comparing a fresh empty file to an old
consumer's checkpoint is not a valid oracle setup.

## Property wildcard

`canonical_memory.rs:43-66` hashes rendered lines, while its rows omit source
revision. A revision whose change is outside the rendering cap can leave the
rendered digest unchanged. Reusing that digest as occurrence revision or
vector freshness would violate the RP2.1 identity contract. This adds a
same-rendered-bytes/different-source-revision case to the identity and coverage
records. It does not allege a bug in the rendering digest's own purpose.

`codec/mod.rs:82-92,207-217` checks `serde_json::Value` equality. JSON envelope
whitespace and escape spelling are outside that oracle. Selected raw tool
bytes need an independently captured source buffer and byte offsets; a
decode/re-encode round trip cannot silently define what "exact" means.

A full lexical count beside an old vector for each row can look complete.
Per-occurrence revision/model validation and per-class denominators prevent
that cancellation. Raw tools excluded from dense policy are a distinct state
from dense-required rows whose tokenizer or vector is missing.

## Retained owner questions

- Which per-consumer source read preserves already-published required rows?
- What establishes the source-incarnation/bootstrap baseline for local progress?
- Which byte boundary is authoritative for multipart tool results and spans?
- Who fixes the finite catch-up/sweep bound before enablement is assessed?

These questions appear in the catalog/evidence or ownership handoff. None is
resolved by inventing a new API, wire method or numeric threshold.
