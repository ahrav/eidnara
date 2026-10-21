# scope-directory-ancestors-derive-only-from-paths-inside-project-root

## Discovery trigger

Plan KTD4 ("Directory context is the set of ancestor directories (including
`.`) of the session `cwd` relative to `project_root` and of paths parsed from
tool-call `input` JSON (`path`, `file_path`, `paths[]`, `cwd`) in
`FlatProjection` blocks") and U1 test scenario "A `cwd` outside
`project_root` contributes no ancestors and is not an error". Serves R5, R6,
KTD4, KTD7 (`cwd` request field).

## Evidence trail

- `crates/daemon/src/transform.rs:6874-6889`: `sel_item_from_flat` reads
  `wire::BlockKind::ToolCall { name, input, .. }` and carries `input` as a
  borrowed `serde_json::Value`; the live window's tool calls are therefore
  available as JSON at selection time.
- `crates/daemon/src/wire.rs:478-494`: a reduced tool call is rebuilt with
  `input: Value::Object(..)`, confirming the `input` type.
- `crates/daemon/src/kernel_routes/project.rs:22-26`: the bound root is
  stored canonicalized and compared as raw path bytes; the request
  `project_root` is checked with `project.accepts(Path::new(requested_root))`
  (`kernel_routes/mod.rs:421-425`).
- `docs/host-wire-protocol.md:953-976`: no `cwd` row at HEAD; `:951` unknown
  fields are ignored, so adding `cwd` is additive.
- `packages/pi-plugin/src/transform/pi-pass-inputs.ts:110` and
  `packages/opencode-plugin/src/hooks/context/transform-pass-fields.ts:55-75`:
  the pass field builders both plugins must extend with `cwd`.
- `crates/daemon/src/transform.rs:9166`: `sanitize_user_hint_query`, the
  lexical path the plan defers; it is not a path extractor and is not
  reused.
- No existing helper derives directory ancestors (grep for `ancestors()`,
  `parent()` chains over tool input finds none on the transform path).

## Failure scenario

Legal witness: `project_root = /w/eidnara`, `cwd = /w/eidnara/crates`, one
tool call `{"file_path": "/w/eidnara/crates/storage/src/lib.rs"}`, one tool
call `{"paths": ["packages/cli/src/index.ts", "/etc/passwd"]}`. Ancestors:
`{., crates, crates/storage, crates/storage/src, packages, packages/cli,
packages/cli/src}`. `/etc/passwd` contributes nothing.

Smallest violating traces:
1. Outside path accepted: `/etc/passwd` is made relative by string
   stripping and yields `..`-bearing or absolute components; a rule
   `dir:etc` (never written by N2, but a directive with `scope = path`
   could name any string) would match. Fix is in the derivation: a path not
   under the canonical root contributes nothing.
2. Missing root ancestor: the derivation yields `{crates, crates/storage}`
   without `.`. A nested file's rule carries `dir:crates/storage` and still
   matches, but a directive recorded with `scope = path` and `path = .`
   never matches, and the "including `.`" clause of KTD4 is violated.
3. Non-canonical form: Windows separators or a trailing slash survive
   (`crates\storage`, `crates/storage/`). `Exact` equality against
   `dir:crates/storage` fails and a touched directory's rule is hidden.
4. Tool result text parsed: a path inside a tool result or assistant text is
   extracted. A model that mentions `crates/storage` in prose can pull in
   that directory's rules; the plan's "no model in any authority path" is
   weakened because the model controls the tool-call input too, but input
   paths are at least the paths the model asked the host to touch.

## Timing windows and dependencies

- The window is the live window after the daemon's boundary resolution
  (7.10.4), so folded-away tool calls no longer contribute. A directory
  touched before the fold loses its rule on the next pass unless `cwd` or a
  live call names it again. This is design intent (live window), stated here
  so a test does not read it as a defect.
- Depends on N1 and N2 writing `dir:` values in the same canonical form
  (posix, relative, no trailing slash, `.` for root). Ownership of the form
  belongs to the representation pass; this record requires equality.

## What a test must construct

1. A pure function `derive_ancestors(project_root, cwd: Option<&str>,
   window: &[FlatBlock]) -> BTreeSet<String>` with a fixture table:
   - `cwd = root`, no tools: `{.}`.
   - `cwd = root/crates/storage`: `{., crates, crates/storage}`.
   - tool `file_path = root/crates/storage/src/lib.rs`: adds
     `crates/storage/src`.
   - tool `paths = ["a/b/c.rs", "/outside/x"]`: adds `a`, `a/b`; nothing
     for `/outside/x`.
   - tool `cwd = "./d/"` : `{., d}`.
   - tool `path = 42` (non-string): nothing, no error.
   - `cwd` absent: only tool paths.
   - a path with `..` escaping the root: nothing.
   - a path with `..` staying inside the root: normalized or dropped (pin
     one).
2. Assert the result never contains an absolute path, a `..` segment, a
   trailing slash, or a backslash.
3. Assert that a path present only in a `ToolResult` block or a `Text`
   block contributes nothing.
4. An integration pass with `cwd` outside `project_root` succeeds (no
   `invalid_params`) and `context_resolved.paths == []` or `["."]` per the
   chosen rule for an outside `cwd` (pin one; the plan says "contributes no
   ancestors").

## Investigation log

### Q: Is `cwd` canonicalized (symlinks) before the relative computation?
- Sources examined: `project.rs:22-26` (root canonicalized), plan U1 and
  KTD7 ("absolute path, same bounds as `project_root`").
- Findings: the plan bounds the field but does not state canonicalization.
  A `cwd` reached through a symlink into the root would be dropped as
  outside under a byte-prefix comparison.
- Missing evidence: the implementation's choice.
- Conclusion: resolvable at implementation; pin whichever rule and test the
  symlink case both ways.

### Q: Which tool-call members are read, and at what depth?
- Sources examined: plan KTD4 (`path`, `file_path`, `paths[]`, `cwd`).
- Findings: four top-level member names; nested objects are not named.
- Conclusion: resolved with answer - top-level members only; the fixture
  table must include a nested `{"args": {"path": ...}}` that contributes
  nothing, so that the closed list is pinned.
