import { afterEach, describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { PiDiagnosticReport } from "./diagnostics-pi";
import { bundleIssueReport, readLogTailLines } from "./logs-pi";

const tempRoots: string[] = [];

function makeTempRoot(): string {
    const root = mkdtempSync(join(tmpdir(), "eidnara-pi-logs-"));
    tempRoots.push(root);
    return root;
}

afterEach(() => {
    for (const root of tempRoots.splice(0)) {
        rmSync(root, { recursive: true, force: true });
    }
});

function reportWithLog(logPath: string): PiDiagnosticReport {
    return {
        timestamp: "2026-07-07T12:00:00.000Z",
        platform: "linux",
        arch: "x64",
        nodeVersion: "v24.0.0",
        pluginVersion: "0.1.0",
        piInstalled: false,
        piPath: null,
        piVersion: null,
        settings: {
            path: "/x/settings.json",
            exists: false,
            hasEidnaraPackage: false,
            packages: [],
        },
        configPaths: {
            agentDir: "/x/agent",
            userConfig: "/x/user.jsonc",
            projectConfig: "/x/p.jsonc",
        },
        userConfig: { path: "/x/user.jsonc", exists: false, flags: {} },
        projectConfig: { path: "/x/p.jsonc", exists: false, flags: {} },
        loadedConfigPaths: [],
        loadWarnings: [],
        conflicts: { knownConflicts: [], otherPiExtensions: [] },
        logFile: { path: logPath, exists: true, sizeKb: 0 },
        recentSessions: [],
        historianDumps: { byProject: [], legacyDumps: { dir: "/x/legacy", count: 0, recent: [] } },
    };
}

describe("readLogTailLines", () => {
    it("returns only complete lines from the bounded tail", () => {
        const root = makeTempRoot();
        const logPath = join(root, "eidnara.log");
        const lines = Array.from(
            { length: 50 },
            (_, index) => `line-${String(index).padStart(3, "0")}`,
        );
        writeFileSync(logPath, `${lines.join("\n")}\n`);

        const tail = readLogTailLines(logPath, 25);

        expect(tail).toEqual(["line-048", "line-049", ""]);
    });

    it("keeps the first line when the file fits inside the bound", () => {
        const root = makeTempRoot();
        const logPath = join(root, "eidnara.log");
        writeFileSync(logPath, "first\nsecond\n");

        expect(readLogTailLines(logPath, 1024)).toEqual(["first", "second", ""]);
    });
});

describe("bundleIssueReport session filtering", () => {
    it("keeps the picked Pi session's tagged lines and untagged lines, and drops other sessions", async () => {
        const root = makeTempRoot();
        const logPath = join(root, "eidnara.log");
        const wanted = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
        const other = "0190a1b2-c3d4-7e5f-8a9b-ffffffffffff";
        writeFileSync(
            logPath,
            [
                "[2026-07-07T12:00:00.000Z] [eidnara][pi] extension loaded",
                `[2026-07-07T12:00:01.000Z] [eidnara][${wanted}] /ctx-status ran`,
                `[2026-07-07T12:00:02.000Z] [eidnara][${other}] /ctx-status ran elsewhere`,
                "[2026-07-07T12:00:03.000Z] untagged line",
            ].join("\n"),
        );

        const bundled = await bundleIssueReport(reportWithLog(logPath), "desc", "title", {
            cwd: root,
            now: new Date("2026-07-07T12:00:00Z"),
            sessionFilter: `2026-07-07T12-00-00-000Z_${wanted}`,
        });

        expect(bundled.bodyMarkdown).toContain(`[eidnara][${wanted}] /ctx-status ran`);
        expect(bundled.bodyMarkdown).toContain("[eidnara][pi] extension loaded");
        expect(bundled.bodyMarkdown).toContain("untagged line");
        expect(bundled.bodyMarkdown).not.toContain(other);
    });
});
