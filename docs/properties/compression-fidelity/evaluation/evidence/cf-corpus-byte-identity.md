# cf-corpus-byte-identity

## Discovery trigger

KTD1 in the [settled contract](https://github.com/ahrav/eidnara/issues/707),
"Implementation Decisions" (historical plan line 153), requires Rust and
TypeScript to bind to the SHA-256 digest of the same complete corpus file bytes.
KTD7 in that section (historical plan line 159) rejects reuse of the
incident-specific fingerprint protocol for corpus or capture bytes.
These are claims under test.

Inspected repository HEAD: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`,
2026-09-19. The original plan records inspection at revision `1555f00c`;
[catalog provenance](../catalog.md#scope-and-evidence-boundary) records its hash
and the HEAD drift. This record is test-only: the proposed corpus serves replay,
not production loading. No digest comparison was exercised.

## Evidence trail

- Plan lines 189-190 place the shared corpus under daemon testdata and separate
  immutable obligations from candidate outputs. Line 239 requires rejection of
  mismatched corpus digests; line 271 records the run identity once.
- [Daemon Cargo manifest](../../../../../crates/daemon/Cargo.toml#L58) already
  includes `sha2`. This establishes library availability, not a corpus check.
- [Fixture build selection](../../../../../packages/e2e-tests/src/rust-runner/hermetic-host.ts#L306-L330)
  accepts an existing `EIDNARA_E2E_DIRECT_HOST_FIXTURE_BIN` and caches the chosen
  build promise. A path's existence does not bind its compiled test data.
- [Incident semantic fingerprint](../../../../../packages/e2e-tests/src/incident-pool/registry.ts#L136-L148)
  hashes a structured object containing its own contract domain and fixtures.
- [Canonical JSON hashing](../../../../../packages/e2e-tests/src/incident-pool/history.ts#L29-L39)
  serializes structured data before SHA-256. Formatting differences disappear.
- [Implementation bundle hashing](../../../../../packages/e2e-tests/src/incident-pool/registry.ts#L152-L169)
  includes a domain prefix, sorted filenames, lengths, and bytes. It is also not
  SHA-256 of the single corpus file.
- [Fingerprint tests](../../../../../packages/e2e-tests/src/incident-pool/runner.test.ts#L248-L284)
  explicitly assert formatting/key-order equivalence and implementation-byte
  sensitivity. Status: `unaudited`; no run is claimed.
- At the inspected revision neither `compression-fidelity.json` nor its Rust
  module existed. U1 (#718) adds both, with matching Rust and TypeScript
  digest pins.
  [Package scripts](../../../../../packages/e2e-tests/package.json#L6-L26) contain
  no compression-fidelity evaluator.

## Failure scenario

A prebuilt Rust fixture retains corpus A while TypeScript reads corpus B. Both
use the same case IDs. A comparison reports matched cases even though a source
revision or scenario changed. A canonical-JSON hash can also accept differently
formatted files contrary to the explicit byte-identity requirement.

This is a constructed failure target, not an observed evaluator defect. The
counterexplanation is that existing incident hashing already supplies the needed
identity. Its input framing and formatting-equivalence test discriminate against
that explanation. Reuse SHA-256 libraries, not those fingerprint domains.

## Timing windows and dependencies

The vulnerable windows are corpus editing after compilation, selecting a stale
prebuilt binary, and loading different bytes on the two comparison sides.
Identity must be checked before admitting a run's observations. Equal prompt
hashes are not required: the plan permits prompt changes as treatment.
Missing or unreadable corpus input rejects admission. A cached manifest digest
cannot substitute for reading the complete runtime bytes, and parsing or
canonicalizing JSON before hashing violates the byte-identity requirement.

The corpus owner supplies valid cases/sources/scenarios. This record does not
duplicate source-span validation or define another schema owner. A matching
digest proves shared bytes, not semantic truth or executed coverage.

## What a test must construct

1. Load the same complete byte sequence in Rust and TypeScript and record the
   manifest identity without parsing/reserializing it first.
2. Keep a compiled digest while changing whitespace, source text at equal byte
   length, or scenario content in the runtime file. Each mismatch rejects.
3. Keep bytes unchanged while varying the candidate prompt hash. Identity remains
   valid; prompt identity is recorded separately.
4. Observe `cf-eval-corpus-bytes-differ` before rejection. This marker must not
   require the incorrect run to be admitted.
5. Supply a missing path and a genuine read failure. Observe
   `cf-eval-corpus-missing` and `cf-eval-corpus-unreadable` separately, and require
   rejection rather than an empty corpus or a cached-digest fallback.

## Investigation log

### Q: Can incident fingerprints implement the corpus identity unchanged?
- Sources examined: `registry.ts:136-169`, `history.ts:29-39`, and
  `runner.test.ts:248-284` at the inspected HEAD.
- Findings: Both fingerprint paths add structure beyond the corpus bytes;
  semantic fingerprinting intentionally ignores formatting.
- Missing evidence: None for the distinction between these hash inputs.
- Conclusion: Resolved with answer. Neither existing protocol is the required
  plain corpus-byte SHA-256. Existing related checks remain `unaudited`.

### Q: How will Rust expose its compiled corpus digest to the evaluator?
- Sources examined: Plan lines 153, 189, 218-219, 233; fixture build selection;
  repository filename and filesystem inventory.
- Findings: The plan requires mismatch rejection. At the inspected revision
  the corpus was absent; U1 (#718) adds it with Rust and TypeScript pins.
  Script selection, the runtime digest exchange, and the evaluator do not
  exist.
- Missing evidence: The implemented digest exchange and its stale-build test.
- Conclusion: Unresolved, needs the U1/U3/U4 implementation. No new exchange
  protocol is designed by this catalog.
