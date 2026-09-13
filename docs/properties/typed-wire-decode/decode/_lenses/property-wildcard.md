# Property lens: wildcard, run last

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
This follows the system wildcard and every other property lens.

- Preserve JSON values, not arbitrary lexical bytes, inside retained Values.
  `OpaqueBlock.arc` is `Option<Value>` at
  `crates/memory-store/src/lib.rs:431-438`; explicit null becomes absence.
  The same optionality applies to tail_delta and native_messages. A blanket
  promise to retain every explicit null would exceed the accepted type model.
- Unknown fields of typed nested meta/origin/kind/output objects are also
  discarded when their enclosing original disappears. Payload Values remain
  opaque to this discard rule. Test placement, not just sentinel spelling.
- Derived serde can have sequence-form struct behavior. Include malformed
  and positional envelope shapes in the compatibility corpus rather than
  adding an unapproved object-only policy.
- Equal typed Values need not have identical numeric spelling. The mutation
  record compares typed equality and independently expected serialization;
  it makes no equality-to-hash biconditional.
- `BodyLane::Tree` has several causes. The independent coverage record must
  witness gate acceptance, typed invalidity, and valid tree conversion jointly.

These refinements enter the seven proposed records. The initial discovery
pass on 2026-09-13 leaves independent review pending. The subsequent supplied
four-lens pass by `ses_f6756093fffeVjNp36S3E8pKrM` is complete on the same date;
see [its disposition](../portfolio-evaluation.md). This discoverer's wildcard
is not that independent pass, and no property is exercised by either review.
