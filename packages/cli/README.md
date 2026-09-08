# Eidnara CLI

`@eidnara/cli` installs as the `eidnara` command. It configures the Eidnara
plugin for OpenCode, Pi, and Oh My Pi (OMP), checks those configurations, and
controls the shared `eidnara-host` daemon. It requires Node.js 24.15 or newer,
the same floor as the plugins it bundles; `doctor --issue` reads the OpenCode
session database through `node:sqlite`, which older Node releases lack or gate
behind a flag.

## Commands

```bash
eidnara setup                # interactive setup; add --dry-run to preview
eidnara doctor               # check configuration
eidnara doctor --force       # repair configuration conflicts
eidnara doctor --issue       # write a redacted diagnostics bundle
eidnara daemon <action>      # start | stop | restart | status | doctor
eidnara --version
eidnara --help
```

`doctor` targets every installed harness by default. `setup` configures one
harness per run: it uses the only installed harness, or asks which one when
several are installed. Add `--harness opencode`, `--harness pi`, or
`--harness omp` to either command to name the harness yourself.

## Setup

`setup` detects the harness, asks for the historian and Sidekick models, and
writes:

- OpenCode: the `@eidnara/opencode` plugin entry in `opencode.jsonc` and
  `tui.jsonc`. When Eidnara compaction is on (the default), setup also turns
  off OpenCode's native `compaction.auto` and `compaction.prune`. When
  `compaction.enabled` is `false` in `eidnara.jsonc`, setup leaves those
  native fields as they are.
- Pi: the `npm:@eidnara/pi` package entry in `settings.json`.
- OMP: the plugin enabled through `omp`, with `compaction.enabled` and
  `memory.backend` turned off so two context managers do not run at once.
- All three: the user configuration at
  `$XDG_CONFIG_HOME/eidnara/eidnara.jsonc` with its `$schema` URL.

Setup reads only local files and the harness binaries. It makes no network
requests.

## Doctor

`doctor` reports the harness installation, the plugin entry, the user and
project configuration, configuration conflicts, the log file, and historian
dumps. It writes nothing. `doctor --force` repairs configuration only, and what
it repairs depends on the harness:

- OpenCode: applies the conflict fixes it reports (native compaction, DCP,
  OMO hooks). A missing plugin entry in `opencode.jsonc` or `tui.jsonc` and a
  missing `eidnara.jsonc` are reported; run `setup` to write them.
- Pi: adds the missing `npm:@eidnara/pi` package entry and writes a missing
  default `eidnara.jsonc`.
- OMP: enables the plugin when OMP has it installed but disabled, writes a
  missing default `eidnara.jsonc`, and turns off `compaction.enabled` and
  `memory.backend`. A plugin that is not installed is reported; install it
  with `omp plugin install @eidnara/pi`.

`doctor --issue` writes `eidnara-issue-*.md`,
`eidnara-pi-issue-*.md`, or `eidnara-omp-issue-*.md` in the current directory
with secrets and personal paths redacted, and offers to open a GitHub issue
through `gh` when it is installed and authenticated.

## Daemon lifecycle

```bash
eidnara daemon start
eidnara daemon status
eidnara daemon doctor
eidnara daemon restart
eidnara daemon stop
```

Add `--json` after an action to emit one `eidnara.daemon/v1` object:

```bash
eidnara daemon status --json
```

`status` and `doctor` are read-only. They do not start, stage, repair, or stop
the daemon. `restart` is one serialized lifecycle transaction, not separate
CLI stop and start calls. `stop` uses authenticated lifecycle control and does
not signal a publication PID.

Exit code `0` means the v1 result has `ok: true`. Exit code `1` means an
operational lifecycle failure. Exit code `2` means invalid CLI arguments and
does not invoke lifecycle policy.
