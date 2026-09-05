# credential-fingerprint-derives-from-the-product-domain

## Discovery trigger

The key-derivation domain separator was renamed at U3 to
`eidnara-broca-credential-v1`, so the committed fingerprint vector was
regenerated once. A fingerprint is the client's proof that it holds the same
provider credential row the host captured at startup, without sending the
credential. Two failures matter: a fingerprint that is a function of the row
alone would match across products and connections and could be replayed, and
a fingerprint that embedded the raw value would leak it into route-open
bodies. The audit traced the derivation from the constants to the HMAC, to
the verifier that consumes it, and to the vector tests.

## Evidence trail

All references are at `572315a`.

Constants. `CREDENTIAL_VALUE_CAP_BYTES` is 16 KiB
(`broca/subprocess.rs:48`), `CREDENTIAL_ROW_CAP_BYTES` is 64 KiB (`:51`),
`CREDENTIAL_FINGERPRINT_DOMAIN` is `eidnara-broca-credential-v1` (`:53`),
and `CREDENTIAL_FINGERPRINT_CANONICALIZATION` is
`harness-provider-name-length-value/1` (`:56`).

Row selection. `canonical_provider` (`:76-88`) maps the Pi aliases
`google-antigravity` and `openai-codex` to `google` and `openai` and accepts
`anthropic`, `google`, and `openai` for both harnesses; anything else is
`ProviderUnsupported`. `EnvSnapshot::provider_row` (`:140-165`) selects one
variable per canonical provider (`:145-150`), returns `CredentialMissing` for
an absent or empty value (`:151-160`), and `CredentialValueTooLarge` above
`CREDENTIAL_VALUE_CAP_BYTES` (`:162-164`). The row is one `(name, value)`
pair.

Derivation. `EnvSnapshot::credential_fingerprint` (`:167-201`) resolves the
canonical provider (`:173`), fetches the row (`:174`), and builds a message
from length-prefixed fields `"{len}:{field}"` (`:175`): the canonicalization
id, the harness as passed, the canonical provider, and for each row entry the
name, the decimal value length, and the value (`:176-185`). The derived key
is `HMAC-SHA256(connection_key, domain)` (`:186-189`); the fingerprint is
`HMAC-SHA256(derived_key, message)` rendered as 64 lowercase hex
(`:190-199`). The audit reproduced the committed vector in Python from this
layout: with key `00..1f`, harness `opencode`, provider `anthropic`, and row
`ANTHROPIC_API_KEY=secret`, the message is
`36:harness-provider-name-length-value/18:opencode9:anthropic17:ANTHROPIC_API_KEY1:66:secret`
and the digest is `ecac831b...7e80`.

Consumer. `CredentialVerifier::verify` (`broca/mod.rs:44-69`) recomputes the
fingerprint with the connection key installed at `:130-132` and compares it
to the presented value in constant time (`:64`). `BrocaComponent::handle`
runs it for `session.send` after the harness-availability check and before
`supervisor.send` (`:223-236`), returning `harness_unavailable` with the
error's subreason on mismatch. The verifier exists only when the component is
built with `new_with_credentials` (`:82-91`); `new` (`:73-79`) leaves it
`None`. The protocol document describes the derivation at
`docs/host-wire-protocol.md:363` and the field bounds at `:351`.

Existing checks, verified:

- `credential_fingerprint_matches_the_committed_vector`
  (`broca/subprocess.rs:1660-1680`) asserts the crate's output over the
  documented inputs equals the literal (`:1667-1672`) and that a zero key
  over the same row does not produce it (`:1673-1679`). It runs under
  `cargo test --workspace --all-targets` (`.github/workflows/ci.yml:118`).
- `provider_rows_exclude_ambient_credentials_and_enforce_caps`
  (`tests/broca_subprocess.rs:2840-2893`) asserts that ambient `AWS_*`,
  proxy, `PATH`, and `LD_PRELOAD` variables are excluded from the row
  (`:2851-2856`), that the Pi alias selects the canonical row (`:2857-2862`),
  that a custom provider is `provider_unsupported` (`:2863-2869`), that a
  value one byte over 16 KiB is `credential_value_too_large` (`:2871-2882`),
  and that the committed vector holds (`:2884-2892`). The binary is
  `harness = false` (`Cargo.toml:36-38`); its runner registers the function
  by name at `:194-195`.
- `credential_snapshot_must_match_before_backend_spawn`
  (`tests/broca_protocol.rs:435-496`) drives a real host built with
  `new_with_credentials`, sends with a wrong fingerprint and asserts
  `harness_unavailable` with zero backend starts (`:455-457`), then computes
  the correct fingerprint from the host's key and asserts the send is
  accepted and the backend starts once (`:459-493`). The record does not
  list this test.

## Failure scenario

1. A managed client captures a `route.open` body from another product's host
   whose fingerprint scheme is `HMAC(connection_key, row)` without a product
   domain, and replays the fingerprint here under the same credential.
2. If the derivation here used the same scheme, the replay would verify.
3. As written, the derived key folds `eidnara-broca-credential-v1`, so the
   same connection key and row yield a different fingerprint per product.
   Within one product, the connection key is per-incarnation
   (`instance.rs:257-258`), so a captured fingerprint is bound to one
   host's key.

The leak direction: the value enters the pre-image only as HMAC input, and
the output is a fixed 32-byte digest, so the fingerprint carries no bytes of
the credential.

## Timing windows and dependencies

None on the derivation. The verifier's key is a `OnceLock` set through
`install_connection_key` (`broca/mod.rs:130-132`); a `session.send` before
the key is installed fails with `credential_snapshot_mismatch` (`:50`)
rather than skipping the check. The row is read from the startup
`EnvSnapshot`, so a credential rotated in the process environment after
start does not change the expected fingerprint.

## What a test must construct

The record's check is covered by the two vector tests, the host-level test,
and `credential_fingerprint_matches_the_documented_derivation_across_rows`
(the test module of `broca/subprocess.rs`). That campaign writes the
documented derivation independently of `credential_fingerprint` and compares
the two over three connection keys, eight harness-and-provider pairs
including both Pi aliases (`openai-codex`, `google-antigravity`) that
canonicalize onto shared provider names, and
nine value shapes: five chosen to collide under naive concatenation (a
colon that mimics the field separator, a digit run that mimics a length
prefix, one byte), the longest admitted value, a multibyte value
(`sécrét`, 8 bytes, 6 characters) whose length prefix differs between byte
and character counting, and two non-UTF-8 values (`0x80`, `0x81`) that a
lossy conversion maps to the same replacement character. The transcript
carries the raw OS bytes of the name and value, so the two raw values
fingerprint apart. It asserts every distinct
`(key, harness, canonical provider, variable, value)` row yields a distinct
fingerprint, that an empty value is refused as `CredentialMissing`, and that
a value over `CREDENTIAL_VALUE_CAP_BYTES` is refused as
`CredentialValueTooLarge`, both before fingerprinting.

The remaining gaps:

1. A cross-product negative: the same key and row under a different domain
   string produces a different digest. The campaign varies every input the
   function takes; the domain is a constant the function does not expose.
2. A row-cap check. `CREDENTIAL_ROW_CAP_BYTES` (`:51`) has no reader in
   `crates/host-runtime/src`; only the per-value cap is enforced
   (`:162-164`). The record's open question already names this.

## Investigation log

### Q: Was the committed vector produced independently of the crate?

- Sources examined: `broca/subprocess.rs:167-201`, `:1660-1683`;
  `tests/broca_subprocess.rs:2884-2892`; a Python HMAC over the layout
  described above.
- Findings: the evidence summary at U3 states the vector came from a Python
  implementation of the documented derivation. The audit re-derived it from
  the code's layout and obtained the same `ecac831b...7e80`. The protocol
  document at `docs/host-wire-protocol.md:363` names the domain, the
  two-stage HMAC, and the canonicalization id but does not spell the
  `"{len}:{field}"` encoding or the field order, so an implementer working
  from the document alone could not reproduce the vector without reading
  `subprocess.rs`.
- Missing evidence: the U3 Python script is not in the tree.
- Conclusion: resolved for agreement between an external HMAC and the crate.
  The field layout is specified only by the code and the catalog record.

### Q: Is the fingerprint check reachable in default production?

- Sources examined: `broca/mod.rs:73-91`, `:223-236`; a grep for
  `BrocaComponent::new_with_credentials` across `crates`.
- Findings: the only callers of `new_with_credentials` are in
  `tests/broca_protocol.rs:443`. Every other construction uses `new`, which
  sets `credential_verifier` to `None`, and `handle` then skips the
  fingerprint check. `provider_row` itself runs on every spawn
  (`broca/opencode.rs:116`, `broca/pi.rs:215`), so the row selection and
  caps are default-production; the fingerprint comparison is not exercised
  by any non-test constructor in this tree.
- Missing evidence: the daemon that composes `BrocaComponent` with a
  captured `EnvSnapshot`, scheduled for U4 (`docs/properties/README.md:52`).
- Conclusion: resolved with a correction to the record. The record's
  reachability line says every credential row is fingerprinted before a
  harness spawns. In this tree the row is selected before every spawn, but
  the fingerprint is computed only when a caller constructs the component
  with credentials, which no production code does yet.
