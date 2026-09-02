# Lease store density: measured finding, decision recorded

Status: PARKED BY DECISION (2026-08-14). Owner: SUBC (commons owner).

## Measurement (BROCA, 2026-08-14, re-run independently before delivery)

- 20,484 lease files; 20,933 logical bytes; 83,902,464 physical bytes (80.0 MiB)
- Amplification 4,008x on a 4 KiB-block APFS volume (portable number is the
  logical ~20 KiB; amplification varies with block size / inline-data support)
- 99.7% of runs mint a new session identity (743 files/24h by st_birthtime,
  agreeing with 741 runs / 739 distinct sessions from run_index) -> ~2.9 MiB/day
- A packed WITHOUT ROWID table holding the same 20,482 pairs measures
  466,944 bytes (22.8 bytes/key, 99.4% reduction) on the real corpus

## Why this is parked rather than fixed

The layout lives in `lease` (then `cortexkit-lease`); five repos depend on it (broca,
claustrum, synapse, broca-tagref, commons). A layout change is a cross-repo
migration with a window where an epoch must be durable in BOTH the old file
and the new table simultaneously. Getting that window wrong breaks
single-writer exclusion — the one invariant the lease exists to provide and
the reason the epoch file must outlive its actor (see lease crate docs:
advisory-lock + persisted epoch CAS; the file is never unlinked, by design,
to avoid the unlink-inode race).

The U4 `lease` (then `cortexkit-lease`) 0.2 breaking-revision trigger fired. The migration
remains parked because packed-table dual-write rollout requires consumer
coordination that is unavailable for this change. Combining that layout
migration with independent lease-state safety hardening would make both changes
harder to verify.

~1 GiB/year of block-amplified small files is real but not worth risking a
fencing invariant on this fleet's timeline. The measurement is done and
recorded so the future decision starts from evidence.

Process death during temporary-file publication can leave a stale `.tmp` orphan;
it does not replace the final lease path.

## Re-open triggers (any one), each watched by the seat that can see it

- Lease directory physical size exceeds 1 GiB — WATCHED BY BROCA (wake armed
  2026-08-14 on st_blocks*512 crossing 1 GiB; ~320 days at measured growth).
  Broca holds the deployment; the crate cannot see its consumers' disks, so
  this condition written only in this document would be one nobody is
  positioned to check.
- A consumer appears with high-frequency ephemeral identities (orders of
  magnitude above ~750 sessions/day) — watched by SUBC (visible from commons
  consumer reviews, invisible from broca's seat)
- The lease crate takes a breaking rev for an unrelated reason (piggyback the
  layout migration on an already-paid cross-repo window) — watched by SUBC
  (crate owner)

## Migration sketch for whoever picks this up

Dual-write epoch to old file + packed table during the window; readers prefer
the table and fall back to the file; cut reads over only after every consumer
repo is on the dual-write rev; retire files lazily. The dangerous step is any
reader that can see the table EMPTY while the file holds a newer epoch —
i.e. the fallback order must be newest-wins across both stores, never
table-wins.

## Provenance

Copied from `commons@89abb409b8c71b03146eedb5bf64cd964f2a92c0` `docs/lease-store-density.md`. The measurement and decision are historical (2026-08-14). The layout it describes is the `lease` crate in this workspace; the repack stays parked until the runtime lives here and the trigger can be re-measured.
