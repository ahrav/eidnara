# Eidnara CLI

The `@eidnara/cli` CLI configures Eidnara, checks installed
harnesses, and controls the shared `mc-host` process.

## Daemon lifecycle

```bash
npx @eidnara/cli@latest daemon start
npx @eidnara/cli@latest daemon status
npx @eidnara/cli@latest daemon doctor
npx @eidnara/cli@latest daemon restart
npx @eidnara/cli@latest daemon stop
```

Add `--json` after an action to emit one `eidnara.daemon/v1` object:

```bash
npx @eidnara/cli@latest daemon status --json
```

`status` and `doctor` are read-only. They do not start, stage, repair, or stop
the daemon. `restart` is one serialized lifecycle transaction, not separate
CLI stop and start calls. `stop` uses authenticated lifecycle control and does
not signal a publication PID.

Exit code `0` means the v1 result has `ok: true`. Exit code `1` means an
operational lifecycle failure. Exit code `2` means invalid CLI arguments and
does not invoke lifecycle policy.

Run `npx @eidnara/cli@latest --help` for setup, doctor, migration,
and daemon command help.
