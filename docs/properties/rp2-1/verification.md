# RP2.1 specification verification receipt

Date: 2026-09-10. Inspected product HEAD:
`913234433ae36a80a6e22c6aac14c7f9aab74386`.

## Independent verdict

Fresh verifier `ses_f7603277bffekpr5oHztRBu2cj` returned **PASS**, with zero
unresolved BLOCK or WARN findings, after an initial full pass and scoped
rechecks of corrected files. This is a documentation/source-contract verdict,
not proof that the proposed product behavior is implemented or tested.

The initial pass fully read 48 target files, including all 30 evidence files,
and checked 491 local links/anchors and 27 recorded hashes. It verified source
preservation, milestone/dependency ordering, the owner's landing amendment,
conditional remediation, finite recovery, and the binding witness matrix.
Peripheral existing-test adequacy remains unaudited.

## Findings resolved

1. Pending versus missing dense coverage lacked exact set semantics. The
   specification, property, evidence, and testing strategy now define missing
   coverage as required minus valid and pending coverage as its subset backed
   by current non-obsolete durable work. Pending-job capacity remains separate.
2. Editing that property invalidated its recorded hash. The source register
   now matches the final catalog bytes; the last verifier pass checked all
   twelve local artifact hashes successfully.
3. The Further Notes traceability row described removed preview-process wording.
   It now matches the final approval boundary: specification approval does not
   certify implementation, approve numeric limits, or authorize release.

The verifier also confirmed the RP2.9 manifest fields and rule against
retroactively passing a candidate by changing its limits. Analyst commissioning
history was not independently certified or used as technical evidence. The
coordinator, not the user, supplied the unchanged-file assertion for scoped
rechecks.

## Mechanical receipt

- Thirty catalog records equal thirty index entries and thirty evidence files.
- Types: 25 safety, four bounded liveness, one reachability.
- Semantics: 29 `always`, one `sometimes`; all records are active, proposed,
  `test-only`, and unexercised at the product boundary.
- All thirty slugs appear in both the testing strategy and spec traceability.
- The three maps define 23, 24, and 16 unique markers. Definitions are not
  witnesses, and the aggregate marker is excluded from its own required matrix.
- Required METHOD fields occur in order. The coordinator's whitespace/schema
  checks found no errors in the catalog package.
- Exact work-item marker searches returned zero hits before the approval
  preview. These searches were collision checks, not evidence about a creation
  outcome. The publication receipt below records the subsequent approved action.

## Limits and next boundary

No product test, build, benchmark, crash campaign, or release gate ran. The
missing release script, source mapping and at-rest policy, retention/export,
SQLite publication/durability, recovery authorization, tokenizer fixture,
shared-budget interfaces, and numeric RP2.9 approvals remain explicit
implementation acceptance prerequisites.

## Publication receipt

The owner approved the complete body, testing seams, title, and work marker
through the final combined confirmation. One creation command returned
[specification 347](https://github.com/ahrav/eidnara/issues/347).

- Repository: `ahrav/eidnara`.
- Title: `RP2.1: Rebuildable search projection and vector coverage`.
- Marker: `eidnara-rp21-projection-coverage-spec-d5b8d6af-7084-4532-9b9e-ba17bb992825`.
- A readback of the issue body compared byte-for-byte equal to the approved
  temporary body with `diff` exit status zero.
- The temporary body file is removed. No creation retry or issue update ran.
- No implementation tickets, commits, or PRs were created. The catalog remains
  local, uncommitted reusable input, not a claim of remote artifact publication.

## Landing note

The statements above describe the specification task at the time it ran.
Ticket P1 ([#352](https://github.com/ahrav/eidnara/issues/352)) carries this
directory into the repository unchanged apart from the status wording in
[README](README.md) and [specification traceability](spec-traceability.md), and
adds [construction contracts](construction-contracts.md), the
[witness matrix](witness-matrix.md), and [prerequisite receipts](prerequisite-receipts.md).
This note does not alter the verifier's verdict or its evidence limits.
