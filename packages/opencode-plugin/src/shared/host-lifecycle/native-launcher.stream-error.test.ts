import { afterAll, describe, expect, mock, test } from "bun:test";
import * as realChildProcess from "node:child_process";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { NativeLaunchError, runNativeLifecycle } from "./native-launcher";

// Stdio pipe read errors emit on the stream, not the `ChildProcess` `error` event.
// Stream `error` events without listeners cause an uncaught host exception under Node.
const realSpawn = realChildProcess.spawn;
let failStream: "stdout" | "stderr" | null = null;
let lastChild: realChildProcess.ChildProcess | null = null;
let errorListenersAtInjection: { stdout: number; stderr: number } | null = null;

mock.module("node:child_process", () => ({
    ...realChildProcess,
    spawn: (...args: Parameters<typeof realSpawn>) => {
        const child = realSpawn(...args);
        lastChild = child;
        const target = failStream;
        if (target !== null) {
            queueMicrotask(() => {
                errorListenersAtInjection = {
                    stdout: child.stdout?.listenerCount("error") ?? 0,
                    stderr: child.stderr?.listenerCount("error") ?? 0,
                };
                child[target]?.destroy(new Error("injected EIO"));
            });
        }
        return child;
    },
}));

afterAll(() => {
    mock.module("node:child_process", () => realChildProcess);
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

    for (const stream of ["stdout", "stderr"] as const) {
        test(`a ${stream} read error is handled on the stream and settles as a typed failure`, async () => {
            const binary = scriptBinary(stream, `sleep 0.3\necho 'late output'\nexit 0`);
            failStream = stream;
            lastChild = null;
            errorListenersAtInjection = null;
            let error: unknown = null;
            try {
                await runNativeLifecycle(
                    { kind: "test-binary", path: binary },
                    { command: "probe", dataRoot: dir, deadlineMs: 10_000 },
                );
            } catch (caught) {
                error = caught;
            } finally {
                failStream = null;
            }
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
});
