# p2-git-open-is-isolated-and-allocation-bounded

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S5, touchpoints C5 and AC7. U2 routes the capture HEAD read through this open.

## Evidence trail

`isolated_options` (`crates/daemon/src/git_sources.rs:131`) starts from `gix::open::Options::isolated()` and overrides `gitoxide.objects.allocLimit` with the bound after the repository's own configuration. `open` (`:138`) calls `open_with_slots` (`:148`) with gix's default slot sizing; `open_with_slots` sets the store's pack index slot count, maps an `extensions.objectFormat` refusal to `UnsupportedObjectFormat`, and sets `objects.ignore_replacements`. At the U1 branch the only caller of `open` was `GitReconciler` (`crates/daemon/src/git_reconcile.rs:244`), which only tests construct. U2 ([#1134](https://github.com/ahrav/eidnara/issues/1134)) adds `observe_head` (`crates/daemon/src/memory_capture/head.rs:32`), which every native capture submit reaches with the shipped configuration, so the record is relabelled `default-production` per METHOD rule 4.

`observe_head` opens with 128 index slots. gix's default sizing resolves the alternates chain and lists the `pack` directory of the repository and of every alternate when the store opens, and a resolve error or a listing error other than `NotFound` refuses the open; for a given slot count `open_with_slots` runs the same reads in `sizing_inputs_readable` (`:174`) and refuses those repositories. `observe_head` then reads the header of HEAD's object before it consults the store: `file_object_kind` (`:205`) reads the loose object through gix's loose store, whose header read inflates at most 64 bytes into a fixed buffer, and then the repository's own pack indices (`packed_kind`, `:223`; `index_kind`, `:280`), which look the id up in an index and decode the pack entry header, following delta bases inside that pack. Neither path materializes an object, and both look up the id HEAD names directly, so no replacement applies. When both miss, the store opened by `open_with_slots` answers (`store_object_kind`, `:329`); when that lookup exhausts the 128 slots, the lookup repeats in a store opened by `open`.

## Failure scenario

A repository configuration that raised the allocation limit would let one packed delta allocate past the bound.

## Timing windows and dependencies

None.

## What a test must construct

A delta-packed commit against an oversized base; a repository declaring an unsupported object format; a `refs/replace` entry.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/daemon/tests/git_sources.rs:549`, `:827`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Which reachability label holds after U2?

- Sources examined: callers of `git_sources::open` and
  `git_sources::open_with_slots` at the U2 branch.
- Findings: `observe_head` calls `open_with_slots` from
  `accept_native_output` on every submit whose output parses; memory capture
  is on by default.
- Missing evidence: none.
- Conclusion: resolved with answer: `default-production`.

### Q: Do the header reads outside the store keep the bounds?

- Sources examined: `file_object_kind`, `packed_kind`, and `index_kind` at
  the U2 branch; gix-odb's loose `try_header` and gix-pack's `decode_header`.
- Findings: the loose read inflates into a 64-byte header buffer; the pack
  read decodes entry headers and, for a delta, the first bytes of the delta
  stream that state the result size. No object body is inflated, so the
  allocation limit has nothing to bound on these paths. A store with more
  indices than the first 128 slots answers after the retry with default
  sizing (`crates/daemon/src/memory_capture/head.rs:690`).
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does a given slot count refuse the roots the default sizing refuses?

- Sources examined: gix-odb 0.84.0 `Store::at_opts` and
  `collect_indices_and_mtime_sorted_by_size`; `sizing_inputs_readable`.
- Findings: the default sizing refuses a root whose alternates chain does
  not resolve or whose own or alternate `pack` directory fails to list with
  an error other than `NotFound`; `sizing_inputs_readable` performs both
  reads. `unborn_unsupported_non_commit_absent_and_non_repository_roots_are_unavailable`
  (`crates/daemon/src/memory_capture/head.rs:416`) constructs a cyclic
  alternates chain, a `pack` that is a regular file, and an alternate whose
  `pack` is a regular file. The default sizing also refuses more than 32767
  indices, a pack entry without a modification time, and an objects path
  whose symlink chain exceeds gix's realpath limit when no alternates file
  exists; the check does not repeat those three refusals.
- Missing evidence: none.
- Conclusion: resolved with answer.
