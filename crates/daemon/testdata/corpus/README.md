# Source corpus

The source corpus is the population capture measurements run over: real repository
layouts and, from U2 on, frozen Pi and OpenCode session stores. Every fixture carries
`"schema": "eidnara.source-corpus/v1"`. The corpus is test data; no production build
reads it.

## Layout

| Path | Contents |
| --- | --- |
| `repos/<name>.json` | One repository manifest per upstream repository |
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
ASCII case, since materializing it would write into the repository's administrative directory.

## Harness closures

| Fixture set | Harness closure |
| --- | --- |
| `repos/*.json` | none; manifests come from upstream git trees |
| OpenCode sessions (U2) | OpenCode 1.18.22, the version CI installs |
| Pi sessions (U2) | Pi 0.80.2 |
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

## Digest

The digest is SHA-256 over the concatenation, for each fixture file under this directory
except this `README.md`, in byte order of relative path, of the `/`-separated relative path,
one `\n`, and the file bytes with every `\r\n` replaced by `\n`. `CORPUS_SHA256` in
`crates/daemon/src/source_corpus.rs` and `SOURCE_CORPUS_SHA256` in
`packages/e2e-tests/src/source-corpus/corpus.ts` hold the same value; each suite recomputes
it, and a fixture change fails both until both pins move in the same change.

## Redaction

Every string in every fixture is redaction-negative under
`context_core::redaction::detect_windowed_durable_text`, except the `text` of a unit record
carrying `"canary": true`, which must be positive. Canary units use the fake credential
shape of `packages/e2e-tests/tests/cassette-replay.test.ts`, so they keep exercising the
Kernel's exact-text refusal.

## Expected units and the oracle

The readers produce the expected-unit table: per native record, its units (text blocks and
settled tool result blocks, with tool name, call id, error flag, `metadata.exit`, and the
harness `truncated` flag) and its classified non-units. The OpenCode reader
(`packages/e2e-tests/src/source-corpus/opencode-db-reader.ts`) reads `message` and `part`
rows of `opencode.db` by SQL, orders parts by `(time_created, id)`, and leaves a completed
compaction summary (`summary: true`, `finish: "stop"`) out of the table; the Pi reader
(`read_pi_session`) reads session JSONL entries.
`scripts/forbid-oracle-adapter-imports.ts` runs in CI and fails when a reader names the
harness source adapters, the tool outcome module, or the shared identity and block types.

Non-unit reasons: `non_text_part`, `ignored_text`, `unsettled_text`, `unsettled_tool`,
`non_text_item` (a Pi `toolResult` holding a non-text item yields no unit for the whole
record), `skipped_item`, `non_message_entry`, `uncaptured_role` (a Pi message whose role is
neither `user`, `assistant`, nor `toolResult`, such as `bashExecution`).

Loss conditions, the closed set reconciliation maps every missing expected unit to:
`adapter_refused`, `unit_too_large`, `deferred`, `secret_bearing`, `re_admission_blocked`,
`refused_retryable`, `identity_conflict`, `superseded`, `window_refused`, `pass_failed`.
