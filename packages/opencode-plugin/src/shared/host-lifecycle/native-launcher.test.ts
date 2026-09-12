import { describe, expect, test } from "bun:test";
import {
    chmodSync,
    closeSync,
    constants,
    existsSync,
    mkdtempSync,
    openSync,
    rmSync,
    writeFileSync,
} from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { NativeLaunchError, runNativeLifecycle } from "./native-launcher";

const SECRET = "hunter2-credential-canary";

let counter = 0;

function scriptBinary(dir: string, body: string): string {
    counter += 1;
    const file = path.join(dir, `fake-eidnara-host-${counter}.sh`);
    writeFileSync(file, `#!/bin/sh\n${body}\n`);
    chmodSync(file, 0o700);
    return file;
}

function probeResultJson(ok: boolean): string {
    // The fixture omits `readiness` because the native binary's result does not include it.
    return JSON.stringify({
        schema: "eidnara.daemon/v1",
        // The `probe` argv is answered as `status`: that is the contracted name
        // for the read-only observation, and the command union has no `probe`.
        command: "status",
        ok,
        state: ok ? "running" : "stopped",
        reason: ok ? "healthy" : "not_running",
        remediation: ok ? null : "run_daemon_start",
        effects: null,
        checks: [],
        versions: {
            release: "0.38.0",
            proof: null,
            daemon: null,
            context: null,
            synapse: null,
            broca: null,
        },
    });
}

function successfulResultJson(command: "start" | "stop"): string {
    return JSON.stringify({
        schema: "eidnara.daemon/v1",
        command,
        ok: true,
        state: command === "start" ? "running" : "stopped",
        reason: command === "start" ? "started" : "stopped",
        remediation: null,
        effects: null,
        checks: [],
        versions: {
            release: "0.38.0",
            proof: command === "start" ? "current" : null,
            daemon: "eidnara-host/0.1.0",
            context: null,
            synapse: null,
            broca: null,
        },
    });
}

describe("native launcher output handling (U3 scenario 17)", () => {
    const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-native-launcher-"));
    process.on("exit", () => rmSync(dir, { recursive: true, force: true }));

    test("a retained executable descriptor runs through the inherited child fd", async () => {
        const binary = scriptBinary(dir, `echo '${probeResultJson(false)}'\nexit 1`);
        const fd = openSync(binary, constants.O_RDONLY | constants.O_NOFOLLOW);
        try {
            const result = await runNativeLifecycle(
                { kind: "retained-fd", fd },
                { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
            );
            expect(result.reason).toBe("not_running");
        } finally {
            closeSync(fd);
        }
    });

    test("a conforming single JSON object with agreeing exit parses", async () => {
        const binary = scriptBinary(dir, `echo '${probeResultJson(false)}'\nexit 1`);
        const result = await runNativeLifecycle(
            { kind: "test-binary", path: binary },
            { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
        );
        expect(result.state).toBe("stopped");
        expect(result.reason).toBe("not_running");
        expect(result.readiness).toBeNull();
    });

    test("a result for a different command is rejected even when exit and JSON agree", async () => {
        // A `start` that receives a contract-valid successful `stop` result would
        // otherwise be reported as a started daemon.
        const binary = scriptBinary(dir, `echo '${successfulResultJson("stop")}'\nexit 0`);
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "start", dataRoot: dir, deadlineMs: 10_000 },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error?.code).toBe("command_mismatch");
        // The same object answers a `stop` invocation.
        const stopped = await runNativeLifecycle(
            { kind: "test-binary", path: binary },
            { command: "stop", dataRoot: dir, deadlineMs: 10_000 },
        );
        expect(stopped.command).toBe("stop");
    });

    test("a successful start result binds to the start invocation", async () => {
        const binary = scriptBinary(dir, `echo '${successfulResultJson("start")}'\nexit 0`);
        const started = await runNativeLifecycle(
            { kind: "test-binary", path: binary },
            { command: "start", dataRoot: dir, deadlineMs: 10_000 },
        );
        expect(started.versions.proof).toBe("current");
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "restart", dataRoot: dir, deadlineMs: 10_000 },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error?.code).toBe("command_mismatch");
    });

    test("extra stdout bytes after the object fail closed", async () => {
        const binary = scriptBinary(
            dir,
            `echo '${probeResultJson(false)}'\necho '{"second":1}'\nexit 1`,
        );
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error?.code).toBe("malformed_output");
    });

    test("unknown fields, empty output, and non-JSON output fail closed", async () => {
        const bodies = [
            `echo '{"schema":"eidnara.daemon/v1","surprise":1}'\nexit 1`,
            `exit 1`,
            `echo 'plain text failure'\nexit 1`,
        ];
        for (const body of bodies) {
            const binary = scriptBinary(dir, body);
            let error: NativeLaunchError | null = null;
            try {
                await runNativeLifecycle(
                    { kind: "test-binary", path: binary },
                    { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
                );
            } catch (caught) {
                error = caught as NativeLaunchError;
            }
            expect(error?.code).toBe("malformed_output");
        }
    });

    test("exit/result disagreement is rejected even when the JSON is valid", async () => {
        const binary = scriptBinary(dir, `echo '${probeResultJson(false)}'\nexit 0`);
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error?.code).toBe("exit_disagreement");
    });

    test("stderr is tainted: its bytes never appear in the typed failure", async () => {
        const binary = scriptBinary(dir, `echo "${SECRET}" >&2\necho 'not json'\nexit 1`);
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error).toBeInstanceOf(NativeLaunchError);
        expect(error?.message).not.toContain(SECRET);
        expect(error?.stack ?? "").not.toContain(SECRET);
    });

    test("a signal exit is typed, never parsed", async () => {
        const binary = scriptBinary(dir, `kill -KILL $$`);
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error?.code).toBe("signal_exit");
    });

    test("a hung child is killed at the deadline (KTD22 bound)", async () => {
        // A backgrounded grandchild keeps stdout open after the child exits, so
        // `close` waits for the stdio grace timer instead of EOF.
        const binary = scriptBinary(dir, `sleep 30 &\nsleep 30`);
        const deadlineMs = 500;
        const started = performance.now();
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "probe", dataRoot: dir, deadlineMs },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error?.code).toBe("timeout");
        expect(error?.childMayHaveActed).toBe(true);
        // An uncapped 250ms stdio grace after the kill would land near 750ms.
        expect(performance.now() - started).toBeLessThan(deadlineMs + 150);
    }, 10_000);

    test("the deadline clock starts at entry, so slow pre-spawn work spawns no child", async () => {
        // `toJSON` consumes the deadline budget before child spawning.
        const sentinel = path.join(dir, "late-spawn-ran");
        const binary = scriptBinary(dir, `touch ${sentinel}`);
        const busyWaitMs = 150;
        const envelope = {
            toJSON() {
                const until = performance.now() + busyWaitMs;
                while (performance.now() < until) {
                    // spin
                }
                return { probe: true };
            },
        };
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "start", dataRoot: dir, deadlineMs: 50, envelope },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error?.code).toBe("timeout");
        expect(error?.message).toContain("before the child was spawned");
        // No process existed, so the failure must not read as one that ran.
        expect(error?.childMayHaveActed).toBe(false);
        expect(existsSync(sentinel)).toBe(false);
    }, 10_000);

    test("the child receives only the budget remaining after pre-spawn work", async () => {
        const busyWaitMs = 700;
        const deadlineMs = 1_000;
        const envelope = {
            toJSON() {
                const until = performance.now() + busyWaitMs;
                while (performance.now() < until) {
                    // spin
                }
                return { probe: true };
            },
        };
        const binary = scriptBinary(dir, `sleep 30`);
        const started = performance.now();
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "probe", dataRoot: dir, deadlineMs, envelope },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error?.code).toBe("timeout");
        expect(performance.now() - started).toBeLessThan(busyWaitMs + deadlineMs);
    }, 10_000);

    test("an exhausted deadline is rejected before any child is spawned", async () => {
        // setTimeout coerces a nonpositive or non-finite delay to 1ms, so
        // without a pre-spawn check a mutating transaction would start and be
        // SIGKILLed a millisecond later, leaving effects behind for a call that
        // had no budget. The sentinel proves no child ran.
        const sentinel = path.join(dir, "exhausted-deadline-ran");
        const binary = scriptBinary(dir, `touch ${sentinel}`);
        for (const deadlineMs of [
            0,
            -1,
            Number.NaN,
            Number.POSITIVE_INFINITY,
            2_147_483_648,
            Number.MAX_SAFE_INTEGER,
        ]) {
            let error: NativeLaunchError | null = null;
            try {
                await runNativeLifecycle(
                    { kind: "test-binary", path: binary },
                    { command: "start", dataRoot: dir, deadlineMs },
                );
            } catch (caught) {
                error = caught as NativeLaunchError;
            }
            expect(error?.code).toBe("usage_error");
        }
        expect(existsSync(sentinel)).toBe(false);
    }, 10_000);

    test("relative launch paths are rejected before a child is spawned", async () => {
        // The child runs with cwd: "/", so a relative value silently changes
        // meaning instead of failing. A first segment colliding with a real root
        // entry is the dangerous case: "bin/echo" resolves to /bin/echo and
        // executes the WRONG binary, surfacing later as malformed output.
        const sentinel = path.join(dir, "relative-path-ran");
        const absolute = scriptBinary(dir, `touch ${sentinel}\necho '${probeResultJson(false)}'`);
        const relative = path.relative(process.cwd(), absolute);
        expect(path.isAbsolute(relative)).toBe(false);

        let targetError: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: relative },
                { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
            );
        } catch (caught) {
            targetError = caught as NativeLaunchError;
        }
        expect(targetError?.code).toBe("usage_error");
        expect(targetError?.message).toContain("not absolute");

        let payloadError: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: absolute },
                { command: "start", dataRoot: dir, deadlineMs: 10_000, payloadDir: "./dist" },
            );
        } catch (caught) {
            payloadError = caught as NativeLaunchError;
        }
        expect(payloadError?.code).toBe("usage_error");
        expect(payloadError?.message).toContain("payload directory is not absolute");

        // Neither call reached a child.
        expect(existsSync(sentinel)).toBe(false);
    }, 10_000);

    test("byte-invalid stdout fails closed instead of decoding to U+FFFD", async () => {
        // Buffer.toString("utf8") substitutes U+FFFD for an invalid byte, so a
        // corrupt byte inside an otherwise well-formed JSON string would parse,
        // validate, and be accepted as a conforming result carrying a silently
        // mangled value. The payload here is contract-valid except that one byte
        // of `versions.release` is 0xFF, which is not legal UTF-8.
        const valid = probeResultJson(false);
        const marker = '"release":"0.38.0"';
        expect(valid).toContain(marker);
        const [head, tail] = valid.split(marker) as [string, string];
        const payload = Buffer.concat([
            Buffer.from(head, "utf8"),
            Buffer.from('"release":"0.3', "utf8"),
            Buffer.from([0xff]),
            Buffer.from('8.0"', "utf8"),
            Buffer.from(tail, "utf8"),
        ]);
        // Sanity: lossy decoding really would accept this, which is the bug.
        expect(() => JSON.parse(payload.toString("utf8"))).not.toThrow();

        const payloadFile = path.join(dir, "corrupt-stdout.bin");
        writeFileSync(payloadFile, payload);
        const binary = scriptBinary(dir, `cat ${payloadFile}\nexit 1`);
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error).toBeInstanceOf(NativeLaunchError);
        expect(error?.code).toBe("malformed_output");
        expect(error?.message).toContain("not valid UTF-8");
    }, 10_000);

    test("usage exits (2) are a typed contract failure with no lifecycle result", async () => {
        const binary = scriptBinary(dir, `exit 2`);
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error?.code).toBe("usage_error");
        // Argument parsing fails before command dispatch, so no command can act.
        expect(error?.childMayHaveActed).toBe(false);
    });

    test("the child receives the envelope on stdin and only the data root in its environment", async () => {
        // `eidnara-host` reads `XDG_DATA_HOME` and `HOME` when resolving its data directory.
        const binary = scriptBinary(
            dir,
            `input=$(cat)\nif [ "$input" = '{"probe":true}' ] && [ "$XDG_DATA_HOME" = '${dir}' ] && [ -z "$HOME" ] && [ -z "$LD_PRELOAD" ]; then\n  echo '${probeResultJson(false)}'\n  exit 1\nfi\nexit 2`,
        );
        const result = await runNativeLifecycle(
            { kind: "test-binary", path: binary },
            { command: "probe", dataRoot: dir, deadlineMs: 10_000, envelope: { probe: true } },
        );
        expect(result.reason).toBe("not_running");
    });

    test("a relative data root or an env override of it is rejected before a child is spawned", async () => {
        const sentinel = path.join(dir, "data-root-ran");
        const binary = scriptBinary(dir, `touch ${sentinel}`);
        let relativeError: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "start", dataRoot: "share", deadlineMs: 10_000 },
            );
        } catch (caught) {
            relativeError = caught as NativeLaunchError;
        }
        expect(relativeError?.code).toBe("usage_error");
        expect(relativeError?.message).toContain("data root is not absolute");

        let overrideError: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                {
                    command: "start",
                    dataRoot: dir,
                    deadlineMs: 10_000,
                    env: { XDG_DATA_HOME: "/elsewhere" },
                },
            );
        } catch (caught) {
            overrideError = caught as NativeLaunchError;
        }
        expect(overrideError?.code).toBe("usage_error");
        expect(overrideError?.message).toContain("may not override the data root");
        expect(existsSync(sentinel)).toBe(false);
    });

    test("an envelope that cannot be serialized fails before a child exists", async () => {
        const sentinel = path.join(dir, "unserializable-envelope-child-ran");
        const binary = scriptBinary(dir, `: > "${sentinel}"\nsleep 2`);
        const envelope: Record<string, unknown> = {};
        envelope.self = envelope;
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "probe", dataRoot: dir, deadlineMs: 250, envelope },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error).toBeInstanceOf(NativeLaunchError);
        expect(error?.code).toBe("usage_error");
        // Serializing after the spawn would let the raw serialization throw
        // escape with a live child that nothing collects or kills; the absent
        // marker proves no child ever ran.
        await new Promise((resolve) => setTimeout(resolve, 500));
        expect(existsSync(sentinel)).toBe(false);
    }, 10_000);

    test("an envelope with no JSON form is typed, not a silently empty stdin", async () => {
        const binary = scriptBinary(dir, `echo '${probeResultJson(false)}'\nexit 1`);
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                {
                    command: "probe",
                    dataRoot: dir,
                    deadlineMs: 10_000,
                    envelope: () => "no json form",
                },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error).toBeInstanceOf(NativeLaunchError);
        expect(error?.code).toBe("usage_error");
    });

    test("an envelope over the native 64 KiB cap is rejected before a child is spawned", async () => {
        const sentinel = path.join(dir, "oversized-envelope-ran");
        const binary = scriptBinary(dir, `touch ${sentinel}\ncat > /dev/null`);
        const cap = 64 * 1024;
        // `{"pad":"…"}` wraps the payload in 10 bytes, so the string length sets
        // the serialized size exactly.
        const atCap = { pad: "x".repeat(cap - 10) };
        const overCap = { pad: "x".repeat(cap - 10 + 1) };
        expect(Buffer.byteLength(JSON.stringify(atCap), "utf8")).toBe(cap);
        expect(Buffer.byteLength(JSON.stringify(overCap), "utf8")).toBe(cap + 1);

        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "start", dataRoot: dir, deadlineMs: 10_000, envelope: overCap },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error?.code).toBe("usage_error");
        expect(error?.message).toContain("byte cap");
        expect(existsSync(sentinel)).toBe(false);

        // Exactly at the cap is accepted and reaches the child.
        let atCapError: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "start", dataRoot: dir, deadlineMs: 10_000, envelope: atCap },
            );
        } catch (caught) {
            atCapError = caught as NativeLaunchError;
        }
        expect(atCapError?.code).not.toBe("usage_error");
        expect(existsSync(sentinel)).toBe(true);
    }, 10_000);

    test("stderr past the cap is discarded without killing a healthy child", async () => {
        // Stderr must stay drained, not closed: this child writes far past the
        // cap and then its conforming object, so a closed read end would take
        // the write side down with EPIPE/SIGPIPE before stdout ever arrives.
        const binary = scriptBinary(
            dir,
            [
                "chunk=xxxxxxxxxxxxxxxx",
                "chunk=$chunk$chunk$chunk$chunk",
                "chunk=$chunk$chunk$chunk$chunk",
                "chunk=$chunk$chunk$chunk$chunk",
                "i=0",
                "while [ $i -lt 512 ]; do",
                '  echo "$chunk" >&2',
                "  i=$((i + 1))",
                "done",
                `echo '${probeResultJson(false)}'`,
                "exit 1",
            ].join("\n"),
        );
        const result = await runNativeLifecycle(
            { kind: "test-binary", path: binary },
            { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
        );
        expect(result.reason).toBe("not_running");
    }, 15_000);

    test("spawn failure for a missing binary is typed", async () => {
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: path.join(dir, "does-not-exist") },
                { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error?.code).toBe("spawn_failed");
    });

    test("a platform with no descriptor exec path is a typed platform failure, not a spawn error", async () => {
        let error: NativeLaunchError | null = null;
        try {
            await runNativeLifecycle(
                { kind: "retained-fd", fd: 0 },
                { command: "probe", dataRoot: dir, deadlineMs: 10_000, platform: "win32" },
            );
        } catch (caught) {
            error = caught as NativeLaunchError;
        }
        expect(error).toBeInstanceOf(NativeLaunchError);
        expect(error?.code).toBe("unsupported_platform");
    });
});
