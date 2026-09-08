import { afterAll, describe, expect, mock, test } from "bun:test";
import * as realChildProcess from "node:child_process";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { NativeLaunchError, runNativeLifecycle } from "./native-launcher";

// Stdio pipe read errors emit on the stream, not the `ChildProcess` `error` event.
// Stream `error` events without listeners cause an uncaught host exception under Node.
const realSpawn = realChildProcess.spawn;
type Injection =
    | { stream: "stdout" | "stderr"; when: "immediately" }
    | { stream: "stdout"; when: "after-first-chunk" };
let injection: Injection | null = null;
let lastChild: realChildProcess.ChildProcess | null = null;
let errorListenersAtInjection: { stdout: number; stderr: number } | null = null;

mock.module("node:child_process", () => ({
    ...realChildProcess,
    spawn: (...args: Parameters<typeof realSpawn>) => {
        const child = realSpawn(...args);
        lastChild = child;
        const planned = injection;
        if (planned === null) return child;
        const inject = () => {
            errorListenersAtInjection = {
                stdout: child.stdout?.listenerCount("error") ?? 0,
                stderr: child.stderr?.listenerCount("error") ?? 0,
            };
            child[planned.stream]?.destroy(new Error("injected EIO"));
        };
        if (planned.when === "immediately") {
            queueMicrotask(inject);
        } else {
            child.stdout?.once("data", inject);
        }
        return child;
    },
}));

afterAll(() => {
    mock.module("node:child_process", () => realChildProcess);
});

const completeStoppedResult = JSON.stringify({
    schema: "eidnara.daemon/v1",
    command: "status",
    ok: false,
    state: "stopped",
    reason: "not_running",
    remediation: "run_daemon_start",
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

describe("native launcher stdio read errors", () => {
    const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-native-launcher-stream-"));
    afterAll(() => rmSync(dir, { recursive: true, force: true }));

    function scriptBinary(name: string, body: string): string {
        const file = path.join(dir, `${name}.sh`);
        writeFileSync(file, `#!/bin/sh\n${body}\n`);
        chmodSync(file, 0o700);
        return file;
    }

    async function run(binary: string, planned: Injection): Promise<unknown> {
        injection = planned;
        lastChild = null;
        errorListenersAtInjection = null;
        try {
            await runNativeLifecycle(
                { kind: "test-binary", path: binary },
                { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
            );
            return null;
        } catch (caught) {
            return caught;
        } finally {
            injection = null;
        }
    }

    for (const stream of ["stdout", "stderr"] as const) {
        test(`a ${stream} read error is handled on the stream and settles as a typed failure`, async () => {
            const binary = scriptBinary(stream, `sleep 0.3\necho 'late output'\nexit 0`);
            const error = await run(binary, { stream, when: "immediately" });
            expect(lastChild).not.toBeNull();
            const listeners = errorListenersAtInjection as {
                stdout: number;
                stderr: number;
            } | null;
            expect(listeners).not.toBeNull();
            expect(listeners?.stdout).toBeGreaterThanOrEqual(1);
            expect(listeners?.stderr).toBeGreaterThanOrEqual(1);
            expect(error).toBeInstanceOf(NativeLaunchError);
            expect((error as NativeLaunchError).message).not.toContain("injected EIO");
        }, 10_000);
    }

    test("a complete result buffered before a stdout read error is rejected, not accepted", async () => {
        const binary = scriptBinary(
            "valid-prefix",
            `echo '${completeStoppedResult}'\nsleep 0.3\nexit 1`,
        );
        const error = await run(binary, { stream: "stdout", when: "after-first-chunk" });
        expect(error).toBeInstanceOf(NativeLaunchError);
        expect((error as NativeLaunchError).code).toBe("malformed_output");
        expect((error as NativeLaunchError).message).toContain("not fully read");
    }, 10_000);
});
