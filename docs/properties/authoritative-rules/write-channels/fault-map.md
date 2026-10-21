# Fault map: proposed write channels N1 and N2

Faults and enabling states each proposed obligation needs, and whether a seam
exists at HEAD `08de9051a` to construct them. "Seam" names the existing
mechanism; "proposed" means the implementation must add one.

## Fault classes

| Fault class | Availability at HEAD | Notes |
| --- | --- | --- |
| F-A model-mediated call on a user row | available - `kernel.commit` through the test harness with `source_kind: assistant` (`crates/daemon/tests/kernel_routes.rs` patterns) | the directive row itself must be seeded through the store API until U3 exists (`kernel_routes.rs:4355-4386`) |
| F-B asserted class over-declaration | available - `resolve_classes` and its test (`kernel_routes.rs:1145`) | the directive route body is proposed |
| F-C replayed operation_key, same digest | available - receipt ledger (`envelope.rs:1279-1309`), tests `kernel_routes.rs:931, 4096` | directive family proposed |
| F-D reused operation_key, different digest | available - `kernel_routes.rs:3681` pattern | |
| F-E lost response after COMMIT | constructible - kill the daemon between commit and reply in the e2e harness; or assert the replay path directly | |
| F-F dependent row citing a directive as approval | available - store API seeding as in `kernel_admission.rs:469` | |
| F-G barred predecessor (Rejected, Quarantined, Contradicted) | available - `Disposition` through `kernel.commit` (`kernel_routes.rs:4392`) | |
| F-H process crash between Kernel commits of one pass | proposed - needs a hook after each file's envelope, or a process kill between files; artifact-side hooks exist (`ingest.rs:307-360`, `test-support`) | |
| F-I concurrent passes for one project with HEAD moving between oid reads | proposed - needs an injectable oid source or a fixture repository advanced under a barrier | existence of overlapping passes on the m0 path is unverified |
| F-J detached HEAD | constructible - `git checkout <oid>` in a fixture | behavior is an open question |
| F-K unborn HEAD | constructible - `git init` without commit | |
| F-L no repository | constructible - temp directory | |
| F-M gix read error | constructible - truncate a loose object or corrupt a pack entry in a fixture | |
| F-N worktree differs from HEAD | constructible - edit without commit | |
| F-O oversized file, too many files, too many units, oversized unit | constructible - fixtures | bounds are plan constants |
| F-P non-UTF-8 file | constructible - fixture bytes | ingest classifies Sensitive (`ingest.rs:197-199`) |
| F-Q secret-shaped token in AGENTS.md | constructible - redactor patterns used by `kernel_cas.rs:1137` | artifact becomes Secret |
| F-R identical blob at a new oid | constructible - commit touching another file | |
| F-S duplicate units in one file | constructible - fixture | behavior is an open question |
| F-T observation on a skewed revision or missing trigger | available - Kernel-level tests `kernel_admission.rs:1323, 1339`; daemon-level needs a test hook to omit the trigger | |
| F-U compound scope (Project plus Entity) on the subject | constructible - `insert_scope` with two terms via store API | `names_project` yields Uncertain at HEAD |

## Per-obligation requirements

| Slug | Faults and enabling state | Constructible at existing seams |
| --- | --- | --- |
| directive-record-self-approves-at-hardcoded-class-pair | bound project; route body; F-B for the negative arm | yes (Kernel half), route proposed |
| directive-explicit-user-unreachable-from-model-paths | F-A, F-B; a mixed campaign writing through every producer | yes |
| directive-operation-key-replay-is-idempotent | F-C, F-D, F-E | yes |
| directive-revise-supersedes-same-lineage-and-reapproves | F-F, F-G; a revise request | yes (Kernel half) |
| directive-revoke-cascades-and-stops-serving | F-F; a read after the commit; F-C for replay | yes (Kernel half) |
| directive-confirmed-from-promotes-by-fresh-record | a model candidate; a confirm request; a stale candidate (F-G) | yes (Kernel half) |
| directive-explicit-user-subject-guard-refuses-model-commit | F-A on root-scoped directive; control model row; F-U for the compound-scope arm; multi-op commit | yes; the guard is proposed |
| repo-policy-rules-derive-from-head-tree-or-nothing | F-J, F-K, F-L, F-M, F-N | yes |
| repo-policy-bounds-fail-closed-per-file | F-O, F-P, F-Q | yes |
| repo-policy-segmentation-is-deterministic | fixtures incl. F-S, CRLF, fences | yes once the function exists |
| repo-policy-unit-reconciliation-by-digest | two-commit fixture (AE5); F-R; plan-outside-closure race (F-I) | fixture yes; race proposed |
| repo-policy-rule-verified-only-with-provenance-witness | F-T, F-Q; the normal path | Kernel half yes; daemon hook proposed |
| repo-policy-per-file-commit-atomic-and-pass-converges | F-H (between ingest and envelope; between files; before cache write) | proposed hook or process kill |
| repo-policy-cached-head-oid-is-advisory-and-reconverges | F-I with inverted commit and cache order | proposed |

## Coverage checks to add (preconditions, never the violation)

- `directive_rows_present`: at least one live `explicit_user` directive
  existed during the campaign (`sometimes`).
- `model_commit_targeted_directive`: a `kernel.commit` with a model class
  named a directive object (`sometimes`); paired with the `always` ledger
  check in the guard record.
- `reconcile_pass_interrupted`: a pass ended with `0 < k < n` files committed
  (`sometimes`).
- `overlapping_reconcile_passes`: two passes for one binding were in flight
  with different resolved oids (`sometimes`).
- `head_detached_during_pass`, `head_unborn_during_pass`,
  `over_bound_file_present`, `secret_bearing_agents_md_present`
  (`sometimes` each).

## Oracle ranking (cheapest valid oracle first)

1. Ledger joins over `admission_decisions`, `commit_log`, `object_registry`:
   one SQL per invariant; covers the trust boundary, the guard, the witness,
   and revision consistency after any campaign.
2. Golden fixtures for segmentation and bounds: pure functions, no store.
3. Kernel-level tests already present for self-approval, trigger lineage,
   revocation cascade, receipt replay: reuse by citation; add route-level
   twins when U3 and U5 land.
4. Two-commit fixture repository for reconciliation (AE5) and the identical-
   blob case.
5. Crash seam after each file commit: one hook, covers F-H in all three
   positions.
6. Overlapping-pass race: most expensive; needs an injectable oid source and
   a barrier; justified by F6 being otherwise unobservable.
