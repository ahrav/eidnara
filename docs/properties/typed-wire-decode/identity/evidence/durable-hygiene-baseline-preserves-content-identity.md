# durable-hygiene-baseline-preserves-content-identity

## Discovery trigger

Fresh evaluator `ses_f6756093fffeVjNp36S3E8pKrM` identifies a durable hash
domain omitted from the process-cache and served-receipt analysis.
Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: user-supplied independent findings, plan R4/R7 and its consumer
table, and latency-audit B4. No new implementation or exercise is implied.

## Evidence trail

1. `crates/memory-store/src/lib.rs:1369-1394` serializes each hygiene part's
   key, content hash, kind, tokens, U tokens, tag metadata, and protection.
   `TailHygieneBaseline` stores those parts and a content signature.
2. `crates/memory-store/src/lib.rs:1649-1651` stores that baseline in
   `ModuleMeta`; it is not the in-process `TailHygieneMemo`.
3. `crates/daemon/src/tail_hygiene.rs:599-625` hashes kind name, NUL, and
   content. Excluded parts use `excluded` and zero tokens, but still hash.
4. Lines 884-885 and 931-934 pass complete `block.bytes` to excluded-part
   measurement. Other active part kinds use extracted content at 887-928.
5. Lines 943-970 preserve full part tuples and hash ordered
   `key:content_hash\0` entries into the content signature.
   The `hex_digest` at lines 424-425 is lowercase SHA-256 of the raw bytes.
6. Lines 975-1004 compare part key, hash, kind, tokens, tags, and protection.
   Equal U/T totals alone do not make a prefix equal.
7. Lines 1010-1063 preserve an invalidated generation or mark a nonmatching
   non-bust baseline unevaluable and invalidated. Busts replace its generation.
8. `crates/daemon/src/transform.rs:4710-4738` measures ordinary transform
   input, stores the bust baseline, and refreshes a loaded baseline on defer.
   Lines 4749-4762 feed that result into reminder decisions.

## Failure scenario

An excluded tool or opaque block has zero measured tokens. Canonicalization
changes its bytes, so its kind-prefixed hash changes while U/T remain equal.
A loaded non-bust baseline then loses evaluability. Treating this as a harmless
memo miss misses the durable transition and its reminder-channel effect.

Plugin-domain preservation requires identical old/new measurements with
identical non-byte inputs. No accepted false omission grants permission to
invalidate a durable baseline or change reminder decisions for daemon-only
or legacy inputs. Those cases require an owner decision.

## Timing windows and dependencies

The boundary is old serialized metadata followed by candidate decode with an
empty memo. Compare the same previous row, fixed time, tags, coverage, core,
protection set, and bust flag. A fresh empty metadata row is not a witness.
Reachability is default-production: ordinary transform calls the measurement
and refresh path without enabling the memo differential or a test hook.

## What a test must construct

- A persisted nonempty evaluable baseline containing a block-byte-derived
  excluded P part, plus ordinary active parts with fixed tag/protection state.
- Unchanged replay and append-only input through cold and warm memos.
- A real single-part mutation with unchanged key and zero-token classification.
- Literal old part hashes/signature and independent expected refresh fields,
  including evaluability, invalidation, generation, deltas, and computed time.
- Daemon-built and unknown-envelope legacy inputs as separate unresolved
  compatibility controls, not silently rebaselined successful cases.
- `typed_wire_identity_durable_hygiene` as an independent `sometimes` marker.

## Investigation log

### Q: Are zero-token parts irrelevant to persistent hygiene identity?

- Sources examined: hashing, part assembly, signature, and prefix refresh.
- Findings: no. Excluded parts hash complete bytes and participate in prefix
  comparison and the durable signature even when tokens and U tokens are zero.
- Missing evidence: an old/new persisted-baseline campaign.
- Conclusion: resolved as source fact; the preservation check is unexercised.

### Q: May normalization invalidate a daemon-only or old unknown-field baseline?

- Sources examined: plan accepted byte exceptions and baseline refresh.
- Findings: the plan grants byte changes, not new baseline or nudge semantics.
- Missing evidence: demonstrated affected input path and an owner-approved
  resolution of the frozen-behavior conflict.
- Conclusion: needs human input before implementation. Hygiene owner and
  `/testing:test-strategy` receive the oracle; test and guard reviewers receive
  existing checks as unaudited. No fresh portfolio rerun is claimed.
