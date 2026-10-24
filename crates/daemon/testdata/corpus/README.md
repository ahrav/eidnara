# Source corpus

The source corpus is the population capture measurements run over: real repository
layouts and, from U2 on, frozen Pi and OpenCode session stores. Every fixture carries
`"schema": "eidnara.source-corpus/v1"`. The corpus is test data; no production build
reads it.

## Layout

| Path | Contents |
| --- | --- |
| `repos/<name>.json` | One repository manifest per upstream repository |
| `sessions/<closure>/mix.json` | One frozen scripted session per harness closure |
| `README.md` | This file; outside the digest |

## Repository manifests

A manifest records one upstream repository at a pinned commit:

- `upstream`: repository URL, full commit oid, and license.
- `layout`: one of `rust_workspace`, `typescript_monorepo`, `python_src`, `go_module`,
  `eidnara`, `non_git`.
- `instructions`: the condition the `AGENTS.md`, `CLAUDE.md`, and `.cursorrules` documents hold:
  `none`, `root_only`, `nested_depth_3` (an `AGENTS.md` three or more path components deep), or
  `mixed` (`CLAUDE.md` and `.cursorrules` beside an `AGENTS.md`). `ARCHITECTURE.md` and
  `STRUCTURE.md` are outside the condition. The Rust test refuses a manifest whose documents
  hold a different condition.
- `documents`: every `AGENTS.md`, `CLAUDE.md`, `.cursorrules`, `ARCHITECTURE.md`, and
  `STRUCTURE.md` in the tree, with its bytes verbatim.
- `files`: every other tracked path with its byte size and whether its bytes are UTF-8,
  or a symlink with its target. Non-document bytes are never committed; the materializer
  regenerates content of the recorded size and UTF-8 validity.
- `exclusions`: each instruction-document line removed from the verbatim bytes, with the
  path, the 1-based line number, and the reason. An excluded line is blanked, so line
  numbers stay stable.

| Manifest | Upstream | Commit | License | Layout | Instructions |
| --- | --- | --- | --- | --- | --- |
| `repos/eidnara.json` | `https://github.com/ahrav/eidnara` | `a0e3f2993dceeba625c048dc8a480e1f668d17f3` | MIT | `eidnara` | `nested_depth_3` (eight `AGENTS.md`, no `ARCHITECTURE.md`) |
| `repos/ripgrep.json` | `https://github.com/BurntSushi/ripgrep` | `e89fff89ac9af12e8d4ce9d5fd07beb408ca730f` (tag `15.2.0`) | Unlicense OR MIT | `rust_workspace` | `none` |

Exclusions: none. No instruction document in either manifest holds a string the
redaction scan reports.

The materializer (`materialize` in `crates/daemon/src/source_corpus.rs`) writes a manifest
into a temporary directory, runs `git init` with the SHA-1 object format and one commit with
fixed identities and dates, and optionally adds a worktree on its own branch, so both
checkouts share one root commit. The validator refuses a path with a `.git` component in any
ASCII case, since materializing it would write into the repository's administrative directory,
and refuses non-ASCII paths and two paths that differ only in ASCII case, since a
case-insensitive filesystem would resolve them to one entry.

## Harness closures

| Fixture set | Harness closure |
| --- | --- |
| `repos/*.json` | none; manifests come from upstream git trees |
| `sessions/opencode-1.18.22/` | OpenCode 1.18.22, the version CI installs |
| `sessions/pi-0.80.2/` | Pi 0.80.2 (`@earendil-works/pi-coding-agent`) |
| `../opencode-tool-outcome-trace.json` | OpenCode 1.19.0 trace shape, recorded beside the 1.18.22 shape |

## Regenerating

Each fixture set has one regenerating command, and a change to a fixture names that command,
the harness closure, and the reconciliation diff in its PR. Repository manifests:

```sh
bun packages/e2e-tests/scripts/freeze-corpus-manifest.ts \
  --repo <checkout> --commit <sha> --name <name> --upstream <url> --license <spdx> \
  --layout <layout> --instructions <condition> [--exclude <path>:<line>:<reason>]... \
  --out crates/daemon/testdata/corpus/repos/<name>.json
```

The two manifests above were frozen with:

```sh
bun packages/e2e-tests/scripts/freeze-corpus-manifest.ts --repo . --commit a0e3f2993 \
  --name eidnara --upstream https://github.com/ahrav/eidnara --license MIT \
  --layout eidnara --instructions nested_depth_3 \
  --out crates/daemon/testdata/corpus/repos/eidnara.json
git clone --depth 1 --branch 15.2.0 https://github.com/BurntSushi/ripgrep.git /tmp/rg
bun packages/e2e-tests/scripts/freeze-corpus-manifest.ts --repo /tmp/rg --commit 15.2.0 \
  --name ripgrep --upstream https://github.com/BurntSushi/ripgrep --license "Unlicense OR MIT" \
  --layout rust_workspace --instructions none \
  --out crates/daemon/testdata/corpus/repos/ripgrep.json
```

Session fixtures (each `--from` rerun over the same stores reproduces the bytes and the
digest):

```sh
PATH=<dir holding opencode 1.18.22>:$PATH \
  bun packages/e2e-tests/scripts/freeze-source-corpus.ts --run <store-dir>
bun packages/e2e-tests/scripts/freeze-source-corpus.ts --from <store-dir>
```

`--run` drives the scripted mixes (`packages/e2e-tests/src/corpus-sessions/`) against the mock
provider and the direct-host fixture over a materialized `repos/ripgrep.json` checkout, keeps
each harness's own store (`opencode.db`, the Pi session JSONL) in `<store-dir>`, freezes it,
and rewrites both digest pins.

## Session fixtures

A session fixture holds the harness's own records, decoded: an OpenCode fixture holds every
`message` row of the session with its `part` rows in id order; a Pi fixture holds every JSONL
entry in file order. The freeze replaces the scratch checkout path with `/corpus/ripgrep` and
other scratch paths with `/corpus/opencode` or `/corpus/pi`, renames OpenCode `ses_`, `msg_`,
`prt_`, and `tool_` identifiers in sorted order (so part order is unchanged) and Pi entry ids in
file order, and marks each canary unit with `"canary": true`. Field names, nesting, value
types, timestamps, and every other byte stay as the harness wrote them.

The scripted mix runs one tool call per prompt, an aborted `bash` call, an OpenCode image
attachment, and a closing turn, so every earlier message reaches a capture pass. The working
files it reads (`canary.txt`, `data.json`, `notes.txt`, `pixel.png`, `ARCHITECTURE.md`) are
written beside the materialized checkout and left uncommitted. The OpenCode mix registers a
local stdio MCP server, `corpus`; the Pi mix registers the extension tool `corpus_probe`.

Census of the R1 cells (witness record, or the reason the closure cannot produce the cell;
`packages/e2e-tests/src/corpus-sessions/census.test.ts` checks every row):

| Cell | OpenCode 1.18.22 | Pi 0.80.2 |
| --- | --- | --- |
| `read` | `msg_corpus0002` | `e0005` |
| `bash` | `msg_corpus0005` | `e0009` |
| `edit` | `msg_corpus0014` | `e0021` |
| `write` | `msg_corpus0017` | `e0025` |
| `grep` | `msg_corpus0020` | `e0029` |
| `glob` | `msg_corpus0023` | Pi 0.80.2 has no glob tool; its `find` takes a glob pattern |
| `todowrite` | `msg_corpus0026` | Pi 0.80.2 has no to-do tool |
| MCP tool | `msg_corpus0032` (`corpus_probe`) | Pi 0.80.2 has no MCP client; the extension tool `corpus_probe` stands in |
| subagent call | `msg_corpus0029` (`task`) | Pi 0.80.2 has no subagent tool |
| aborted call | `msg_corpus0038` | `e0053` |
| text | `msg_corpus0002` | `e0005` |
| JSON text | `msg_corpus0008` | `e0013` |
| Pi image item | a Pi content item; OpenCode carries images as file parts | `e0041` |
| OpenCode non-text part | `msg_corpus0039` (`file`, `image/png`) | an OpenCode part kind |
| output over 64 KiB | OpenCode 1.18.22 truncates every tool result before it reaches the session store | `e0045` (70,000 bytes) |
| invalid UTF-8 | `msg_corpus0011` (U+FFFD) | `e0017` (U+FFFD) |
| canary | `msg_corpus0005` | `e0009` |

OQ9 observations:

- OpenCode 1.18.22 publishes an MCP tool as `<server>_<tool>` (`corpus_probe` for server
  `corpus`, tool `probe`). The vocabulary's leading-`mcp_` strip never applies to that shape,
  so `corpus_probe` classifies as `Unknown`; a server whose tool is named `edit` or `write`
  publishes `<server>_edit`, which stays `Unknown` and collides with no tabled name.
- `AGENTS.md` at the pinned commits: `repos/ripgrep.json` holds none; `repos/eidnara.json`
  holds eight.

Invalid UTF-8 reaches both stores as U+FFFD: both harnesses decode tool output before storing
it. An MCP result holding a lone UTF-16 surrogate reaches `opencode.db` as a `\ud800` JSON
escape, and the transform pass whose window holds it ends in an error, so the scripted mix
leaves that result out; the measurement follow-up owns the case.

Reconciliation of the frozen run (live counters and descriptors, both scenarios in
`packages/e2e-tests/tests/source-capture-*.test.ts`):

| Harness | Expected units | Admitted | `unit_too_large` | `secret_bearing` | Unclassified |
| --- | --- | --- | --- | --- | --- |
| OpenCode 1.18.22 | 41 | 40 | 0 | 1 | 0 |
| Pi 0.80.2 | 38 | 36 | 1 | 1 | 0 |

Expected units exclude each session's final assistant reply, which no capture pass offers.
The Pi image-bearing `toolResult` is `non_text_item` in the reader and `adapter_refused` in
the counters.

## Digest

The digest is SHA-256 over the concatenation, for each fixture file under this directory
except this `README.md`, in byte order of relative path, of the `/`-separated relative path,
one `\n`, and the file bytes with every `\r\n` replaced by `\n`. `CORPUS_SHA256` in
`crates/daemon/src/source_corpus.rs` and `SOURCE_CORPUS_SHA256` in
`packages/e2e-tests/src/source-corpus/corpus.ts` hold the same value; each suite recomputes
it, and a fixture change fails both until both pins move in the same change.

## Redaction

Every string in every fixture is redaction-negative under
`context_core::redaction::detect_windowed_durable_text`, except inside an object carrying
`"canary": true`. That object's unit content (its `text`, or an OpenCode tool part's
`state.output` or `state.error`) must be positive. A string inside the object that contains
that content or lies within it, such as OpenCode's `state.metadata.output` preview of the same
output, is exempt; every other string inside it must still be negative. Canary units use the fake credential
shape of `packages/e2e-tests/tests/cassette-replay.test.ts`, so they keep exercising the
Kernel's exact-text refusal.

## Expected units and the oracle

The readers produce the expected-unit table: per native record, its units (text blocks and
settled tool result blocks, with tool name, call id, error flag, `metadata.exit`, and the
harness `truncated` flag) and its classified non-units. The OpenCode reader
(`packages/e2e-tests/src/source-corpus/opencode-db-reader.ts`) reads `message` and `part`
rows of `opencode.db` by SQL, orders parts by `(time_created, id)`, and leaves a completed
compaction summary (`summary: true`, `finish: "stop"`) out of the table; the Pi readers
(`read_pi_session` in Rust, `packages/e2e-tests/src/source-corpus/pi-jsonl-reader.ts` in
TypeScript) read session JSONL entries.
`scripts/forbid-oracle-adapter-imports.ts` runs in CI and fails when a reader names the
harness source adapters, the tool outcome module, or the shared identity and block types.

Non-unit reasons: `non_text_part`, `ignored_text`, `unsettled_text`, `unsettled_tool`,
`non_text_item` (a Pi `toolResult` holding a non-text item yields no unit for the whole
record), `skipped_item`, `non_message_entry`, `uncaptured_role` (a Pi message whose role is
neither `user`, `assistant`, nor `toolResult`, such as `bashExecution`).

Loss conditions, the closed set reconciliation maps every missing expected unit to:
`adapter_refused`, `unit_too_large`, `deferred`, `secret_bearing`, `re_admission_blocked`,
`refused_retryable`, `identity_conflict`, `superseded`, `window_refused`, `pass_failed`.
