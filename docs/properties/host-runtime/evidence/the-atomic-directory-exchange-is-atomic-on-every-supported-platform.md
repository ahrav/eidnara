# the-atomic-directory-exchange-is-atomic-on-every-supported-platform

## Discovery trigger

Gap G5 from `portfolio-evaluation.md`: "The atomic exchange has a distinct macOS
backend with no property and no observed test." The task was to verify the backend
divergence and the claim that no in-crate lifecycle or generation test executes on
macOS.

## Evidence trail

**Two kernel interfaces behind one expression.** `store_fs.rs:111-115`:

```
/// Atomically swaps the entries `a` and `b` under `dir`; both must exist.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn exchange_dirs(dir: &OwnedFd, a: &str, b: &str) -> rustix::io::Result<()> {
    rustix::fs::renameat_with(dir, a, dir, b, rustix::fs::RenameFlags::EXCHANGE)
}
```

and the fail-closed stub, `:117-120`:

```
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn exchange_dirs(_dir: &OwnedFd, _a: &str, _b: &str) -> rustix::io::Result<()> {
    Err(rustix::io::Errno::OPNOTSUPP)
}
```

Both arms return the raw errno. The caller in `generation.rs:1928-1931` maps it
through `classify_errno` (`:111-116`): `ENOSPC` and `EDQUOT` become
`InsufficientStorage`, every other errno becomes
`invalid("atomic digest-target exchange failed")`, so the stub's `EOPNOTSUPP`
surfaces as `NativePayloadInvalid`.

The divergence is in what `rustix` issues for `RenameFlags::EXCHANGE`:
`renameat2(RENAME_EXCHANGE)` on Linux and `renameatx_np(RENAME_SWAP)` on macOS
are separate syscalls with independent semantics,
error sets, and filesystem support, reached through one line of Rust and covered by
one cfg predicate.

**The call site is a repair, followed immediately by a deletion.**
`generation.rs:1350-1404`, inside `promote_temp`:

- `:1319-1328` try the no-replace rename; on success, fsync, validate, and return.
- `:1329-1343` the target is occupied: a valid occupant wins and the temp is
  discarded; `UnsupportedStateSchema` abandons the mutation.
- `:1344-1366` a protected, owner-selected, composition-named, or pinned corrupt
  target abandons the mutation.
- `:1368` `exchange_dirs(&self.generations_fd, temp_name, digest)?`
- `:1370` fsync the generations directory.
- `:1371-1372` revalidate the promoted target, then `remove_tree` the displaced
  orphan now at the temp name.

So a non-atomic or partially-applied exchange means `remove_tree` deletes the wrong
directory. `promote_temp`'s contract at `:1301-1306` depends on the swap being
all-or-nothing.

**macOS executes no test in this scope.** At HEAD this holds trivially:
`.github/workflows/ci.yml` contains fifteen `runs-on: ubuntu-latest` jobs and no
`macos` token, so no CI executor for the macOS arm exists in this tree. The
catalog entry attributes the removal of the macOS job to PR #131 (merge
`5d638e3e8`) in the source repository. The analysis that follows describes that
source-repository job as it then existed, and its `ci.yml` anchors are
source-catalog coordinates. CI's matrix job was
`the source repository `ci.yml` workflow:126-184`, `os: [ubuntu-latest, macos-latest]` at `:139`.
The host-runtime steps that ran on macOS were exactly four:

| Line | Step | What it runs |
| --- | --- | --- |
| `:160` | source build | `cargo build -p host-runtime`, no test binaries |
| `:182` | macOS contracts | `cargo nextest run -p host-runtime --test shm_soak` |
| `:183-185` | macOS contracts | `cargo test -p host-runtime --lib shm_provider::tests::platform_preflight_is_side_effect_free` |
| `:186-187` | lease non-escape | `cargo test -p host-runtime --doc` |

The Linux-only step at `:167-175` runs `--test shm_transport --test
transport_negotiation`. There is no `--test lifecycle`, no `--test client`, no
`--test activation`, and no unfiltered `--lib` run on either platform in this job.
The step comment at `:175-178` states the position in its own words: the macOS job
proves "side-effect-free omission (R15), not active parity".

One nuance worth recording, because it bounds what the omission does and does not
hide: the filtered `--lib` run at `:183-185` *compiles* the whole lib test binary
on macOS, so a macOS type error or a cfg mistake in `generation.rs`'s test module
would fail the build. What never happens is executing a `generation` or
`lifecycle` test body on macOS.

Reachability label evidence: `default-production`, with a platform qualifier. The
exchange is not behind a config option or a cfg(test) gate; it is reached whenever
`promote_temp` finds a corrupt unprotected occupant. The label describes the
configuration, and the platform scoping is stated in the record's Existing check
and here, because METHOD offers only three labels and none of them encodes
"production on one platform, unobserved on another".

## Failure scenario

A generation directory at the digest name is corrupt, for example a partially
written manifest after a power loss, and is not in the protected set. A restage of
the same digest runs. `rename_no_replace` returns `Ok(false)`, `validate` fails,
the protection check passes, and `exchange_dirs` runs.

On macOS, if `renameatx_np(RENAME_SWAP)` behaves differently on APFS from
`RENAME_EXCHANGE` on ext4, for example by not being atomic across a crash, by
failing on a directory rather than a file, or by returning a different errno that
the `classify_errno` call at `generation.rs:1930` flattens into one message, then
either the
mutation is abandoned with a message that names the wrong cause, or the swap
partially applies and `:1372` deletes a retained generation.

The classifier matters here on its own: apart from `ENOSPC` and `EDQUOT`, every
error from either syscall
becomes `invalid("atomic digest-target exchange failed")`, so a platform-specific
`ENOTSUP` from a filesystem that does not support the swap is indistinguishable
from an `EXDEV` or a permission failure. The record's check should assert the
error is returned, not what it says, but the diagnosis loss is worth noting.

## Timing windows and dependencies

The exchange must be atomic with respect to a crash, because `:1368-1372` is
exchange, fsync, revalidate, delete. A crash between `:1368` and `:1372` leaves the
corrupt orphan at the temp name, which the catalog's
`current-profile-never-names-an-unvalidatable-generation` covers as an orphan a
later prune removes. A non-atomic exchange breaks that reasoning, because the
intermediate state is then not one of the two states the recovery argument assumes.

Dependency: same-filesystem. Both syscalls operate within one directory descriptor
(`dir` is passed as both the old and new dirfd at `store_fs.rs:114`), so
cross-device is not
reachable here.

## What a test must construct

The fixture already exists. `generation.rs:2986`
`same_digest_corrupt_target_is_repaired_only_by_validated_exchange` drives
`promote_temp` into the exchange branch: `:2970-2976` covers the protected corrupt
target being refused and `:2978-2993` covers the unprotected case exchanging
atomically and the orphan temp being removed.

What is missing is executing it on macOS. Concretely:

1. Add a macOS job to `.github/workflows/ci.yml` that builds `host-runtime` and
   runs the `generation::tests` filter of `--lib`; at HEAD the workflow has no
   macOS job at all, so this is a new job rather than a selection change to the
   source repository's former macOS branch (`ci.yml` workflow:175-181`).
2. For the stub platforms, a compile-time assertion is the only available check:
   call `exchange_dirs` under `#[cfg(not(any(target_os = "linux", target_os =
   "macos")))]` and assert it returns `Err`. That branch has no CI target in this
   repository, so the check documents intent rather than proving behaviour.
3. Strengthen the existing Linux test with an explicit no-intermediate assertion:
   after the exchange, assert both names are occupied, the digest name validates,
   and the temp name holds the corrupt content.

Note that this record's obligation is mostly a CI change rather than a new test,
which places it in the same enforcement class as
`the-largest-lifecycle-proof-runs-in-ci` and is one of the three biases the
portfolio evaluation asked a human to separate.

## Investigation log

### Q: Is macOS a supported deployment target for the lifecycle store, or only a development platform?

- Sources examined: `store_fs.rs:112-120` (both cfg arms),
  `.github/workflows/ci.yml` at HEAD (fifteen `runs-on: ubuntu-latest` jobs, no
  `macos` token), the source repository's former `ci.yml` workflow:126-184` in
  full (the matrix, both platform branches, and the omission comment at
  `:171-174`; source-catalog coordinates), `src/generation.rs` and
  `src/lifecycle.rs` cfg inventory (`generation.rs:2421` and `lifecycle.rs:1798`,
  `:1920`, `:1942`, `:1981` are the only `target_os` gates in either file; the
  production gates moved to `store_fs.rs:58`, `:112`, and `:117`).
- Findings: the evidence points both ways and cannot be resolved from the code.
  For support: macOS gets a real syscall mapping rather than the stub, host-runtime
  was built on macOS in the source repository's CI, and `lifecycle.rs`'s Linux-only
  gates are all inside test code rather than production paths, so the production
  lifecycle path compiles and is intended to work on macOS. Against support: that
  macOS job ran no lifecycle or generation test, its own comment framed macOS as
  proving omission rather than parity, which is a statement about the shared-memory
  provider rather than about the store, and at HEAD `.github/workflows/ci.yml` has
  no macOS job, so nothing builds or tests host-runtime on macOS in this tree.
- Missing evidence: no product document, target list, or release manifest was
  examined in this pass, and none is cited in either source file.
- Conclusion: needs human input. If macOS is supported, this is a real coverage gap
  and the fix is a new macOS CI job. If it is not, the honest shape is to move
  macOS to the fail-closed stub at `store_fs.rs:117-120`, which would turn a silent
  divergence into an explicit refusal.
