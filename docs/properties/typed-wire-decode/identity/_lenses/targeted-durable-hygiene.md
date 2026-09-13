# Targeted discovery: durable hygiene identity

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
Trigger: supplied fresh evaluation `ses_f6756093fffeVjNp36S3E8pKrM`.

Competing explanations: block-byte hashes live only in the memo, or excluded
zero-token measurements also enter a durable baseline. The second is verified:
`crates/memory-store/src/lib.rs:1369-1394,1651` stores parts/signature;
`crates/daemon/src/tail_hygiene.rs:599-625,884-885,931-934,961-990`
hashes excluded bytes and compares their identities. Lines 1010-1063 invalidate
a nonmatching non-bust baseline even when aggregate token counts are equal.

Disposition: add `durable-hygiene-baseline-preserves-content-identity`, separate
from durable served receipts. The oracle compares full measurements and refresh
outcomes from preserved old state with fixed context and cold/warm memos.

Narrow limit: no claim is made that every daemon-built block reaches this path.
Any such byte-induced invalidation needs a concrete input and owner resolution;
the accepted byte exceptions do not authorize semantic changes. New marker
`typed_wire_identity_durable_hygiene` is independently `sometimes` and observes
the loaded evaluable baseline plus excluded input, not an invalidation result.

Source facts are verified at HEAD; proposed checks remain unexercised. This
targeted pass disposes a supplied finding, not a new independent evaluation.
