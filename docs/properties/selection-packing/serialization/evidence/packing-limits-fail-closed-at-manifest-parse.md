# packing-limits-fail-closed-at-manifest-parse

## Discovery trigger

RP2.8 AC8 and the U4b ticket require the packing bounds to join the fail-closed
runtime limit manifest, with missing, unknown-name, version-mismatched, or
declared-but-unapproved values failing at parse, and the manifest as the one
budget authority reachable from the packing path.

## Evidence trail

- `crates/daemon/src/projection_gates.rs` `RuntimeManifest::parse` reads the
  `PACKING_LIMITS` names by name into `PackingManifest` when any is present,
  refuses a missing or non-numeric member, counts the names in its
  unknown-limit check, reads the `search_projection.packing.approved` flag
  beside the hooks, and refuses a present group without the flag as
  `ManifestRefusal::PackingUnapproved`.
- `crates/daemon/src/packing/serialize.rs` `PackingLimits::from_manifest`
  converts the group into typed bounds, refusing a zero value as
  `PackingLimitRefusal::Zero` except for the adjustment pass cap, and a size
  the target cannot hold or a body bound past `MAX_WIRE_BODY_BYTES` as
  `OutOfRange`; the bound constructors carry no default.
- `crates/daemon/tests/packing_serialize.rs` builds manifests with the group
  and one malformation each, compares the derived bounds whole against literal
  structs, and scans the packing sources for the legacy budget helpers.

## Failure scenario

A partially declared group that parses lets the packer run under a bound
nobody approved; a default inside a bound constructor does the same when the
manifest omits the group.

## Timing windows and dependencies

None.

## What a test must construct

- A manifest with the full group and the flag enabled, and variants with the
  flag absent or disabled, one member missing, an unknown name, a non-numeric
  value, a version disagreement with the identity, each limit at zero, and a
  body bound at and one past the transport maximum.
- A read of the packing sources that finds none of the legacy budget helpers.

## Investigation log

### Q: Is the record `default-production` or `explicit-config-only`?

- Sources examined: `crates/daemon/src/projection_gates.rs`
  `RuntimeManifest::parse` (the packing branch is entered only when
  `PACKING_LIMITS.iter().any(|name| limits_object.contains_key(*name))`, and
  a manifest without the group leaves `packing` as `None`);
  `crates/daemon/src/projection_admission.rs`, which reads
  `runtime-manifest.json` from `<home>/search-admission/` at every refresh
  and leaves the gate closed when the record is missing or refused;
  `../../../METHOD.md` rule 4.
- Findings: the parse runs whenever an operator publishes the record, but the
  approval, partial-group, and malformation refusals need a manifest that
  carries at least one `packing_*` limit. The unknown-limit check that counts
  the packing names runs for every manifest, and a manifest without the
  group parses and yields no limits; those two clauses are reached by
  default, the refusals are not.
- Missing evidence: none.
- Conclusion: resolved with answer - `explicit-config-only`, with the
  enabling configuration named on the record's `Reachability:` line.

### Q: Does a build without the packing group refuse a manifest that carries it?

- Sources examined: `RuntimeManifest::parse`'s unknown-limit and unknown-hook
  checks; `crates/daemon/src/projection_admission.rs` `AdmissionInputs::read`;
  `../../../search-projection/construction-contracts.md` CC9.
- Findings: a build whose `PACKING_LIMITS` and `PACKING_APPROVAL_ID` are absent
  refuses the keys as an unknown limit or hook, and that refusal closes the
  gate for every hook, so the manifest gains the keys only after the daemon
  that knows them is deployed, and a rollback must revert the manifest.
- Missing evidence: none; the order is recorded in CC9 and the catalog's Q4
  rulings.
- Conclusion: resolved with answer - the rollout order is a deployment rule
  the parser enforces by refusing unknown names.
