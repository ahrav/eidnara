# current-profile-never-names-an-unvalidatable-generation

Scope note: "generation" here is the payload generation - a content-addressed
on-disk directory under `generations/`, named by the SHA-256 of its canonical
manifest bytes. It is not the connection generation of `connection.rs`, which is
an in-process `u64` lifecycle id. Nothing in this record involves async code.

## Discovery trigger

Reading `stage_and_promote` (`crates/host-runtime/src/generation.rs:1097-1107`) top to
bottom against its own doc comment (`:1091-1096`), which promises `current` is
unchanged on every failure. The promise is per-error-class, and the sequence has
two renames and four distinct sync points, so "unchanged" has to be checked
against a crash between them rather than against a returned `Err`.

## Evidence trail

The durability order is explicit and, read against the syscalls, correct.
Staging (`:1218-1279`) syncs bottom-up: each copied file is written by
`copy_source_into` (`:1796`) and its descriptor is kept, the manifest is written
unsynced by `write_new_file_unsynced` (`:1256`), then every kept descriptor is
fsynced in groups of `CONCURRENT_SYNCS` through `sync_group` (`:1243`, `:1266`),
then every directory the run created entries in is reopened and fsynced
deepest-first (`:1268-1274`), then the temp root itself (`:1276`). The
deepest-first claim rests on `dirs.iter().rev()` over a `BTreeSet<String>` of
relative paths: a prefix sorts before its extensions, so `a` precedes `a/b`
lexicographically and the reversed walk emits `a/b` before `a`. Children are
durable before the parent entry naming them.

Promotion (`:1287-1352`) is the first rename. `rename_no_replace`
(`store_fs.rs:53-79`) must not replace, because POSIX `renameat` succeeds onto an
existing empty directory (`generation.rs:1283`); on success `generations_fd` is
fsynced (`:1304`) and the digest is validated (`:1306`) before returning. Only
then does `replace_profile` (`:1357-1359`, through `replace_profile_at` at
`:1438-1467`) write the profile temp (`:1453`, fsynced inside `write_new_file`,
`store_fs.rs:312`), rename it over `current-profile.json` (`:1455-1456`), and
fsync the root (`:1464`). So the ordering is: temp contents durable, digest entry
durable, profile durable - never inverted.

Two forward guards keep the profile from naming something unreadable. Staging
refuses a manifest above `MAX_MANIFEST_BYTES` *before* writing it (`:1250-1253`),
whose comment states the reason: validation reads under that cap, so a larger
manifest could never be revalidated after promotion. And a quarantined profile
aborts the whole mutation up front (`:1129-1132`).

The failure paths preserve the selector. Staging failure removes the temp and
returns (`:1159-1162`). Promotion failure removes the temp and returns
(`:1165-1168`). `replace_profile` failing after a successful promote leaves an
orphan directory at the digest name that no profile references; `prune`
(`:1592`) removes it as unprotected, since only the owner selections, the
current profile target, retained composition members, and the caller's protected
set are protected (`:1593-1614`).

## Failure scenario

The fast path validates by pathname after the rename. When `rename_no_replace`
succeeds at `:1300-1302`, `promote_temp` fsyncs and calls `validate(digest)` at
`:1306` before returning, so `replace_profile` names a digest that has passed one
validation. Correctness there rests on the digest being computed from the exact
bytes just written (`:1164`, `:1248-1249`) and on that one validation; the
comment at `:1305` names the same-UID writer who can alter a staged file between
its copy and the rename as the reason the validation exists.

The exchange-then-revalidate window is the other shape. On same-digest repair the
sequence is `exchange_dirs` (`:1347`), fsync (`:1348`), `validate(digest)`
(`:1349`), then delete the orphan now at the temp name (`:1350`). Between `:1348`
and `:1350` the digest name holds the candidate and the temp name holds the
corrupt orphan, both durable. A crash there is benign: the profile still names
the same digest string, that name now holds the repaired directory, and a later
prune removes the temp by its `tmp-` shape (`:1620-1628`). The residue is that
`validate` at `:1349` re-resolves the digest pathname rather than inspecting the
descriptor the exchange acted on, so it attests to whatever occupies the name at
that instant.

## Timing windows and dependencies

Fault class H4, plus a real termination. Every window is inside one
`transaction.lock` hold: `promote_temp`'s two renames, and the gap between the
promote and `replace_profile`. Depends on
`validation-and-enumeration-address-one-directory-object` for the meaning of
`validate` at `:1349`, and on `persisted-state-quarantine-caps-agree`, since the
`:1251` pre-write refusal is stated in terms of the same cap.

## What a test must construct

Storage exhaustion at each write and sync point, which needs `fsync`-time
exhaustion on a delayed-allocation filesystem to reach the
`fsync_preserving_storage` classification (`:198-203`, through `classify_errno` at
`:100-105`), and a process kill between
the promoting rename and the profile rename. No fault injection and no crash
test exists. Four in-crate tests cover adjacent ground:
`stage_validate_and_read_current_roundtrip` (`:2096`), the same-digest repair
including the protected-target refusal
(`same_digest_corrupt_target_is_repaired_only_by_validated_exchange`, `:2916`),
post-hoc tampering (`validation_rejects_every_tampered_shape`, `:2404`), and the
quarantine abort (`unknown_schemas_are_quarantined_and_block_mutation`, `:2533`).
`interrupted_staging_leaves_the_old_profile_complete` (`:2958`) plants a leftover
temp and a leftover profile temp rather than interrupting anything, so it proves
inertness of residue, not crash recovery.

## Investigation log

### Q: Should the exchange-then-revalidate window be crash-tested? Whether any reader can observe the intermediate was not established.

- Sources examined: `generation.rs:1287-1352` (`promote_temp`), `:1357-1359` and
  `:1438-1467` (`replace_profile` and `replace_profile_at`), `:901` and
  `:1032-1076` (`validate` and `validate_in_dir`), `store_fs.rs:112-120`
  (`exchange_dirs`), `:53-79` (`rename_no_replace`); every `GenerationStore`
  construction site
  (`crates/daemon/src/bin/eidnara-host.rs:845`, `:966`, `:1001`;
  `crates/daemon/src/bin/eidnara_host/serve.rs:544`); `preflight_generation`
  (`eidnara-host.rs:813-847`), `resolve_generation` (`:952-1010`),
  `serve::run` (`serve.rs:538-547`).
- Findings: the crash outcome itself is decidable and safe, for the reason given
  above - same digest string, exchanged content, temp removed by prune. The
  observability half is not. `read_current` and `validate` take no lock
  internally, so exclusion is the caller's. Both launcher readers are under the
  held lock: `preflight_generation`'s comment at `eidnara-host.rs:830` says the
  store state "is observable under the lock we already hold", and
  `resolve_generation` is reached inside the serialized transaction (`:1268`,
  `:1510`). The one reader that takes no transaction lock is the spawned
  daemon's own revalidation, `serve::run` at `serve.rs:544-547`, in a different
  process. `GenerationStore::prune` has exactly one production caller,
  `eidnara-host.rs:975`, under the lock.
- Missing evidence: whether the launcher's lock hold extends across the child's
  revalidation, and therefore whether `serve::run` can ever run concurrently
  with another process inside `promote_temp`. The daemon takes `lifetime.lock`,
  not `transaction.lock`, so nothing in the store excludes it; the exclusion
  would come from spawn ordering, which no test asserts.
- Conclusion: unresolved, and narrower than the catalog states. I found no
  reader that can observe the intermediate, but the reason is spawn ordering
  rather than mutual exclusion, and that ordering is unasserted. The crash
  question is answerable without human input; the concurrent-observer question
  needs the launcher's lock lifetime around spawn established first.
