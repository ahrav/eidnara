# partial-setup-reclaims-only-unexposed-resources

## Discovery trigger

Setup failure refunds only resources proved unexposed; a quarantine-accounting failure leaves the backing charge counted forever (KTD5, KTD7). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/profile.rs:622`
- `crates/host-runtime/src/ring_transport.rs:521`

Witness status: partial - `crates/shm-transport/tests/profile.rs:215` covers worker/backing settlement, quarantine, and uncertain retention; `crates/host-runtime/src/ring_transport.rs:473` refunds on a pre-exposure failure, and `crates/host-runtime/src/ring_transport.rs:3542` shows a refused admission charges nothing and the released charge admits the next connection. A failure injected between descriptor duplication and grant transfer is not yet exercised.

## Failure scenario

Refunding storage a peer may have mapped lets a later connection map over it.

## Timing windows and dependencies

Failure after ring creation before the grant is sent; quarantine accounting failure.

## What a test must construct

A `DuplexRing::create` failure; a poisoned accounting lock at quarantine time.

Situation markers that must fire independently of the safety check:

- `admission.setup_failed_before_exposure`
- `admission.quarantine_accounting_failed`

Check semantics: `always` - after a setup failure before the grant is sent, `snapshot().active` returns to its prior value; after a quarantine failure, `BackingAdmission::is_active()` is false and no refund occurs on drop.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: partial at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: the last implementation task injects a descriptor-duplication failure in the combined matrix.
- Conclusion: unresolved, needs the named handoff.
