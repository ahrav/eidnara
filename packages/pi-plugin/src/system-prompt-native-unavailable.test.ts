import { describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

describe("Pi system prompt without native token counting", () => {
    it("keeps prompt processing and records an unavailable count", () => {
        const directory = mkdtempSync(join(tmpdir(), "eidnara-pi-native-unavailable-"));
        try {
            const nativeUrl = pathToFileURL(
                join(import.meta.dir, "../../shm-native/index.ts"),
            ).href;
            writeFileSync(
                join(directory, "preload.ts"),
                [
                    'import { mock } from "bun:test";',
                    `import * as native from ${JSON.stringify(nativeUrl)};`,
                    'mock.module("@eidnara/shm-native", () => ({ ...native,',
                    '  estimateTokens() { throw new Error("native tokenizer unavailable"); },',
                    "}));",
                ].join("\n"),
            );
            const systemPromptUrl = pathToFileURL(join(import.meta.dir, "system-prompt.ts")).href;
            writeFileSync(
                join(directory, "driver.ts"),
                [
                    `import { processSystemPromptForCache, piSystemPromptStateFor } from ${JSON.stringify(systemPromptUrl)};`,
                    'const sessionId = "native-unavailable";',
                    'const result = processSystemPromptForCache({ sessionId, systemPrompt: "Base\\nToday\'s date: Mon Jan 01 2024", isCacheBusting: false });',
                    "const state = piSystemPromptStateFor(sessionId);",
                    'const frozen = processSystemPromptForCache({ sessionId, systemPrompt: "Base\\nToday\'s date: Tue Jan 02 2024", isCacheBusting: false });',
                    'const changed = processSystemPromptForCache({ sessionId, systemPrompt: "Changed\\nToday\'s date: Tue Jan 02 2024", isCacheBusting: false });',
                    "console.log(JSON.stringify({ result, state, frozen, changed, latestState: piSystemPromptStateFor(sessionId) }));",
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
            });
            expect(run.exitCode, run.stderr.toString()).toBe(0);
            const report = JSON.parse(run.stdout.toString()) as {
                result: { systemPrompt: string; currentHash: string };
                frozen: { systemPrompt: string; hashChanged: boolean };
                changed: { systemPrompt: string; hashChanged: boolean; currentHash: string };
                latestState: { systemPromptHash: string; systemPromptTokens: number | null };
                state: {
                    systemPromptHash: string;
                    systemPromptTokens: number | null;
                    stickyDate?: string;
                };
            };
            expect(report.result.systemPrompt).toContain("Today's date: Mon Jan 01 2024");
            expect(report.state.systemPromptHash).toBe(report.result.currentHash);
            expect(report.state.stickyDate).toBe("Today's date: Mon Jan 01 2024");
            expect(report.state.systemPromptTokens).toBeNull();
            expect(report.frozen.systemPrompt).toBe(report.result.systemPrompt);
            expect(report.frozen.hashChanged).toBe(false);
            expect(report.changed.hashChanged).toBe(true);
            expect(report.changed.systemPrompt).toContain("Today's date: Tue Jan 02 2024");
            expect(report.latestState.systemPromptHash).toBe(report.changed.currentHash);
            expect(report.latestState.systemPromptTokens).toBeNull();
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });
});
