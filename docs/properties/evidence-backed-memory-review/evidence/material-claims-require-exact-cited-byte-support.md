# material-claims-require-exact-cited-byte-support

## Discovery trigger

Specification U2 acceptance (#709, owned by #592): each finite corpus case
declares expected action, material claims, exact cited bytes, allowable
support and contradiction, shared-origin groups, answerability, and required
limitations; an irrelevant but structurally valid citation scores valid
citation and unsupported claim; same native origin counts once.

## Evidence trail

`crates/daemon/tests/support/memory_reviewer_corpus.rs` - `Case`, `Claim`,
`Span`, `Cite`, `Score`, `locate`, `cited_range`, and `score`. Spans are
written as their exact bytes and resolved to the one range they occupy.

`crates/daemon/tests/memory_reviewer_coordinator.rs` - `run_case` maps each
citation to its source by alias, checks the evidence id and disclosure, and
compares the cited ranges, the score, the origin groups, the limitations, and
the target tokens with the case's declarations. `publish_span` publishes a span
descriptor over the same commit identity and evidence as its base.

`crates/daemon/src/memory_reviewer/broker.rs` - `native_origin_key` keys an
origin by class and identity without revision, representation, or span, which
the corpus's origin function mirrors from the descriptor detail.

`crates/daemon/src/memory_reviewer/coordinator.rs` - `proposal_target` names a
native subject by its object id, revision, snapshot, and last-change commit
token.

## Failure scenario

A proposal cites the first 40 bytes of a superseding commit, which stop inside
"pre-test hook", and the corpus counts the revision as supported because the
citation is valid.

## What a test must construct

A valid prefix citation whose bytes do not contain the claim's span, expecting
one unwarranted claim; a valid citation of an unrelated source, expecting one
unwarranted claim; two spans of one commit, expecting one independent origin.
