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
                    '  estimateTokens() { throw new native.NativeStartupError("tokenizer_export_unavailable"); },',
                    "}));",
                ].join("\n"),
            );
            const systemPromptUrl = pathToFileURL(join(import.meta.dir, "system-prompt.ts")).href;
            const statusUrl = pathToFileURL(join(import.meta.dir, "dialogs/status-dialog.ts")).href;
            const testUtilsUrl = pathToFileURL(
                join(import.meta.dir, "__tests__/test-utils.ts"),
            ).href;
            writeFileSync(
                join(directory, "driver.ts"),
                [
                    `import { processSystemPromptForCache, piSystemPromptStateFor } from ${JSON.stringify(systemPromptUrl)};`,
                    `import { buildPiStatusDetail, showStatusDialog } from ${JSON.stringify(statusUrl)};`,
                    `import { fakeContext, fakeKernelResolver } from ${JSON.stringify(testUtilsUrl)};`,
                    'const sessionId = "native-unavailable";',
                    'const result = processSystemPromptForCache({ sessionId, systemPrompt: "Base\\nToday\'s date: Mon Jan 01 2024", isCacheBusting: false });',
                    "const state = piSystemPromptStateFor(sessionId);",
                    'const frozen = processSystemPromptForCache({ sessionId, systemPrompt: "Base\\nToday\'s date: Tue Jan 02 2024", isCacheBusting: false });',
                    'const changed = processSystemPromptForCache({ sessionId, systemPrompt: "Changed\\nToday\'s date: Tue Jan 02 2024", isCacheBusting: false });',
                    "const kernel = fakeKernelResolver();",
                    'const pi = { getAllTools: () => [{ name: "read", description: "Read a file", parameters: {} }] };',
                    'const deps = { kernelClient: kernel.kernelClient, projectIdentity: "test-project" };',
                    "let rendered = [];",
                    'const ctx = { ...fakeContext(sessionId), getSystemPrompt: () => "system prompt", getContextUsage: () => ({ tokens: 1000, contextWindow: 10000 }),',
                    "  ui: { async custom(factory) { const component = factory({ requestRender() {} }, { fg: (_name, text) => text, bold: (text) => text }, undefined, () => {});",
                    "    try { rendered = component.render(90); } finally { component.dispose(); } } } };",
                    'const detail = buildPiStatusDetail(pi, ctx, deps, sessionId, kernel.kernel.snapshot("explicit_search"));',
                    "await showStatusDialog(pi, ctx, deps);",
                    "console.log(JSON.stringify({ result, state, frozen, changed, latestState: piSystemPromptStateFor(sessionId), detail, rendered }));",
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
                timeout: 10_000,
            });
            expect(run.exitCode, run.stderr.toString()).toBe(0);
            const report = JSON.parse(run.stdout.toString()) as {
                result: { systemPrompt: string; currentHash: string };
                frozen: { systemPrompt: string; hashChanged: boolean };
                changed: { systemPrompt: string; hashChanged: boolean; currentHash: string };
                latestState: { systemPromptHash: string; systemPromptTokens: number | null };
                detail: { systemPromptTokens: number | null; toolDefinitionTokens: number | null };
                rendered: string[];
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
            expect(report.detail.systemPromptTokens).toBeNull();
            expect(report.detail.toolDefinitionTokens).toBeNull();
            const rendered = report.rendered.join("\n");
            expect(rendered).toContain("Token counts unavailable");
            expect(rendered).not.toContain("Conversation*");
            expect(rendered).not.toContain("Tool Defs");
            expect(rendered).not.toContain("█");
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });
});
