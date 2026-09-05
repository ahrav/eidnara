# setup-descriptors-name-distinct-open-files

## Discovery trigger

Round 17 review of the PR: `reject_aliased_descriptors` enforces pairwise
distinctness of the six transferred descriptors and the setup socket, three unit
tests exercise it, and no catalog record or fault-map row owned the requirement.

## Evidence trail

- `packages/shm-native/src/setup.rs:382-389`: `reject_aliased_descriptors`
  chains the six `OwnedFd`s with the setup stream and calls
  `reject_aliased_files`; the grant receive calls it at `:357`, after the
  descriptor count check at `:354-356` and before the grant message is read.
- `setup.rs:397-408`: `reject_aliased_files` compares every pair with
  `same_open_file`; `Some(true)` refuses, `None` (kernel or sandbox refused
  `kcmp`) falls back to `reject_aliased_inodes` (`:410-418`), which refuses on a
  repeated `(st_dev, st_ino)`.
- `setup.rs:379-381`: the doc comment records why the comparison is on the open
  file description: `SCM_RIGHTS` installs a fresh descriptor number per slot.
- `setup.rs:392-396`: the doc comment records the fallback's weakness, that every
  anonymous-inode descriptor shares one inode identity, so the fallback refuses
  distinct eventfds; the check fails closed under it.
- `packages/shm-native/src/lib.rs:725-731`: the raw `attach` entry point applies
  `reject_aliased_files` to the duplicated descriptors before any mapping or
  registry insertion, mapping `InvalidData` to `descriptor_error()`.
- `crates/host-runtime/src/ring_transport.rs:855`: the bridge's
  `attach_with_descriptors` has no equivalent check.
- Tests: `distinct_descriptors_are_accepted_and_a_dup_is_rejected`
  (`setup.rs:671-691`), `inode_fallback_rejects_the_same_aliases` (`:694`),
  `kcmp_separates_eventfds_that_share_an_inode` (`:723`). All call the
  functions directly.

## Failure scenario

A host sends the same doorbell in two slots. Both ring directions wake on one
socket; a wake meant for the capacity ring is drained as a data wake or vice
versa, and the client parks on a doorbell that will never be signalled for the
direction it is waiting on. A host that places the setup socket in a doorbell
slot has the client drain setup bytes as wake tokens.

## Timing windows and dependencies

None; the check runs once per setup on a fixed bundle. The fallback arm depends
on whether the sandbox permits `kcmp(2)`.

## What a test must construct

A real setup socket carrying an aliased bundle into `begin_connect`, and an
aliased bundle handed to the raw `attach` entry point, each asserting refusal
with no channel, external reference, or pending entry created. Present: the three
direct-call tests (`setup.rs:671-691`, `:694`, `:723`), which call
`reject_aliased_descriptors`, `reject_aliased_files`, and `reject_aliased_inodes`
directly and not through the setup or attach paths that invoke them. Missing: the
setup-socket test through `begin_connect` and the raw-attach
test through `attach`; neither is written, so whether the production call at
`setup.rs:357` and the entry-point call at `lib.rs:725-731` refuse an aliased
bundle without leaving state behind is asserted by no test.

## Investigation log

### Q: Is the check applied on every path that attaches transferred descriptors?

- Sources examined: `setup.rs:340-360`, `lib.rs:700-745`,
  `ring_transport.rs:855-880`.
- Findings: both addon entry points apply it; the in-crate bridge endpoint does
  not.
- Missing evidence: whether the bridge is meant to trust its sender.
- Conclusion: recorded as the open question; Reachability names all three paths.
