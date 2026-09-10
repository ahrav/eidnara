# log-lines-keep-sanitizer-and-file-hardening-guarantees

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The audit attributes per-pass cost to logging and proposes a level gate. A
gate decides which lines exist. The properties that must survive it are what
a written line may contain and how it reaches disk, because the logger is the
only defense on this path against log forgery and symlink redirection.

## Evidence trail

- [`sanitizeField`][sanitize] flattens `\n`, `\r`, and `\t` to spaces, drops
  code points where [`isControlChar`][sanitize] holds (`0x00-0x08`,
  `0x0b-0x1f`, `0x7f`), and truncates at `MAX_FIELD_CHARS = 2048` with a
  trailing ellipsis. It does not strip C1 controls or `U+2028`/`U+2029`.
- [`log`][sessionlog] returns early when `NODE_ENV === "test"`
  ([`isTestEnv`][testenv]), then pushes
  `` `[${timestamp}] ${sanitizeField(message)}${serializeData(data)}\n` ``;
  [`serializeData`][serialize] passes every branch through `sanitizeField`.
  [`sessionLog`][sessionlog] prefixes an `[eidnara]` tag and the bracketed
  session id, then calls `log`.
- The buffer flushes at [`BUFFER_SIZE_LIMIT = 50`][constants] lines or after
  [`FLUSH_INTERVAL_MS = 500`][constants]; [`flush`][flush] joins the buffer,
  calls [`ensureLogDir`][ensuredir] and [`appendPrivate`][appendpriv], and
  routes any error to [`recordSwallowedWrite`][swallow], which increments
  `swallowedWriteCount` and never throws.
- [`ensureLogDir`][ensuredir] creates the directory at
  [`PRIVATE_DIR_MODE = 0o700`][modes] when it lies inside the managed root
  ([`managedDirChain`][chain]) and runs [`assertPrivateDir`][assertdir] on
  each link, which rejects symlinks, non-directories, another uid, and
  group-or-other bits.
- [`appendPrivate`][appendpriv] opens with
  `O_WRONLY|O_APPEND|O_CREAT|O_NOFOLLOW|O_NONBLOCK` at
  [`PRIVATE_FILE_MODE = 0o600`][modes], rejects a non-regular file, tightens
  an existing managed file's mode, and writes through the descriptor.
- On the transform path, [`logTransformTiming`][stagelog] and the pass logs
  call `sessionLog`; [rust-mode-transform.test.ts:244][t244] spies on
  `logger.sessionLog` and asserts the `rust pass:` and `rust module stages:`
  lines per pass.
- No secret redaction runs here. [`shared/redaction.ts`][redaction] is
  imported by `packages/cli` and `packages/e2e-tests` only
  ([cli/redaction.ts:1][cliredact], [e2e spawn.ts:11][e2eredact]).
- [logger.test.ts:358][t358] covers control characters and the size bound,
  [:381][t381] the `0600`/`0700` modes and a planted symlink, [:342][t342]
  the swallowed-write counter, and [:408][t408] the exit flush.

## Failure scenario

A gate that skips `sanitizeField` for a "cheap" level writes a message
containing `\n[2026-...] forged line`, and the newline-delimited file gains an
entry the plugin did not write. A gate that replaces `appendPrivate` with a
plain `appendFileSync` follows a symlink planted at the log path. A gate at
the call sites removes the lines the per-pass test observes through its spy,
so the existing coverage passes vacuously or fails for the wrong reason.

## Timing windows and dependencies

Flushes are batched, so a write failure is attributed to a batch, not a
line; `swallowedWriteCount` counts batches. The `exit` handler flushes the
remaining buffer. The hardening runs on every flush, so a directory replaced
by a symlink between flushes is caught at the next one. The gate position
matters for observability: inside `log` before `buffer.push` keeps
`sessionLog` visible to spies; at the call sites it does not.

## What a test must construct

Untrusted text with embedded newlines, `0x00-0x1f`, and `0x7f` in both the
message and the data argument at every level the gate admits; a planted
symlink at the log path; a directory in the managed chain owned by another
uid; and an assertion that every written line matches
`^\[<ISO-8601>\] ` with no control code point and each field at most 2048
characters plus the ellipsis. The
[plugin checks](../existing-checks.md#plugin-pre-send) cover the sanitizer
and hardening once each; none asserts line counts per pass or event.

## Investigation log

### Q: Must the plugin redact before write, or is CLI export the boundary?

- Sources examined: [`sanitizeField`][sanitize] and its comment on untrusted
  provider bodies; the import sites of [`shared/redaction.ts`][redaction]; the
  `message.updated` handler's log calls in [event-handler.ts][evlog].
- Findings: The logger treats the text as untrusted for forgery but applies
  no secret vocabulary. Provider error bodies and model output reach the file
  as written. Redaction exists in the shared module and is applied by the CLI
  on export and by the e2e runner, not by the plugin's logger.
- Missing evidence: A stated policy on which layer owns redaction.
- Conclusion: needs human input.

[sanitize]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L14-L34
[constants]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L10-L12
[testenv]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L6
[swallow]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L46-L54
[modes]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L56-L58
[chain]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L64-L76
[assertdir]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L82-L96
[ensuredir]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L98-L109
[appendpriv]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L117-L134
[flush]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L136-L162
[serialize]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L169-L181
[sessionlog]: ../../../../../packages/opencode-plugin/src/shared/logger.ts#L183-L200
[stagelog]: ../../../../../packages/opencode-plugin/src/hooks/context/transform-stage-logger.ts#L3-L12
[evlog]: ../../../../../packages/opencode-plugin/src/hooks/context/event-handler.ts#L121-L240
[redaction]: ../../../../../packages/opencode-plugin/src/shared/redaction.ts#L1-L20
[cliredact]: ../../../../../packages/cli/src/lib/redaction.ts#L1
[e2eredact]: ../../../../../packages/e2e-tests/src/opencode-runner/spawn.ts#L11
[t244]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L244
[t358]: ../../../../../packages/opencode-plugin/src/shared/logger.test.ts#L358
[t381]: ../../../../../packages/opencode-plugin/src/shared/logger.test.ts#L381
[t342]: ../../../../../packages/opencode-plugin/src/shared/logger.test.ts#L342
[t408]: ../../../../../packages/opencode-plugin/src/shared/logger.test.ts#L408
