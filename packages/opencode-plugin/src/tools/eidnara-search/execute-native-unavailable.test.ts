import { describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

type Capture = { ok: true; value: { status: string; text: string } } | { ok: false; error: string };

/**
 * Bun module mocks are process-global, so the throwing `@eidnara/shm-native` mock lives in a
 * subprocess preload where it cannot leak into other test files.
 */
describe("eidnara_search without native token counting", () => {
    it("answers a designed tool error instead of escaping the native failure", async () => {
        const directory = mkdtempSync(join(tmpdir(), "eidnara-search-native-unavailable-"));
        try {
            const url = (relative: string) =>
                JSON.stringify(pathToFileURL(join(import.meta.dir, relative)).href);
            writeFileSync(
                join(directory, "preload.ts"),
                [
                    'import { mock } from "bun:test";',
                    `import * as native from ${url("../../../../shm-native/index.ts")};`,
                    'mock.module("@eidnara/shm-native", () => ({ ...native,',
                    "  estimateTokens(text) {",
                    "    const policy = globalThis.__tokenCounting;",
                    '    if (policy === "throw" || (policy === "throw-after-query" && text !== "alpha")) {',
                    '      throw new native.NativeStartupError("tokenizer_export_unavailable");',
                    "    }",
                    "    return text.length;",
                    "  },",
                    "}));",
                ].join("\n"),
            );
            writeFileSync(
                join(directory, "driver.ts"),
                [
                    `import { executeEidnaraSearch } from ${url("execute.ts")};`,
                    `import { KernelClient } from ${url("../../shared/kernel-client/index.ts")};`,
                    `import { FakeKernel, FakeKernelTransport } from ${url("../../shared/kernel-client-testing/fake-kernel.ts")};`,
                    "const kernel = new FakeKernel();",
                    `kernel.seedDecision({ object_id: "mem_${"a".repeat(32)}", decision_kind: "ARCHITECTURE", summary: "alpha reaches the packer" });`,
                    "const transport = new FakeKernelTransport(kernel);",
                    "const deps = {",
                    "  kernelClient: ({ sessionId, projectRoot }) => new KernelClient({ transport, enabled: true, sessionId, projectRoot }),",
                    '  resolveProjectPath: () => "git:repo-project",',
                    "};",
                    'const ctx = { sessionID: "ses-search", directory: "/tmp/eidnara-search" };',
                    "async function capture(run) {",
                    "  try { return { ok: true, value: await run() }; }",
                    "  catch (error) { return { ok: false, error: String(error) }; }",
                    "}",
                    'globalThis.__tokenCounting = "throw";',
                    'const unavailable = await capture(() => executeEidnaraSearch(deps, { query: "alpha" }, ctx));',
                    "const missingQuery = await capture(() => executeEidnaraSearch(deps, {}, ctx));",
                    'globalThis.__tokenCounting = "throw-after-query";',
                    'const packing = await capture(() => executeEidnaraSearch(deps, { query: "alpha" }, ctx));',
                    "console.log(JSON.stringify({ unavailable, missingQuery, packing }));",
                ].join("\n"),
            );
            const run = Bun.spawnSync({
                cmd: [
                    process.execPath,
                    "--preload",
                    join(directory, "preload.ts"),
                    join(directory, "driver.ts"),
                ],
                cwd: directory,
                stdout: "pipe",
                stderr: "pipe",
                timeout: 20_000,
            });
            expect(run.exitCode, run.stderr.toString()).toBe(0);
            const report = JSON.parse(run.stdout.toString()) as {
                unavailable: Capture;
                missingQuery: Capture;
                packing: Capture;
            };

            // The query bound fails before any daemon read.
            expect(report.unavailable).toMatchObject({ ok: true, value: { status: "invalid" } });
            if (report.unavailable.ok) {
                expect(report.unavailable.value.text).toStartWith("Error:");
                expect(report.unavailable.value.text).toContain("token counting");
                expect(report.unavailable.value.text).toContain("tokenizer_export_unavailable");
            }

            // Query presence is validated without a token count.
            expect(report.missingQuery).toEqual({
                ok: true,
                value: { status: "invalid", text: "Error: 'query' is required." },
            });

            // The output budget cannot be enforced, so the results are refused rather than delivered unbounded.
            expect(report.packing).toMatchObject({ ok: true, value: { status: "invalid" } });
            if (report.packing.ok) {
                expect(report.packing.value.text).toStartWith("Error:");
                expect(report.packing.value.text).toContain("token counting");
            }
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });
});
