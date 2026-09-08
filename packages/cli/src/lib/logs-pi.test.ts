import { afterEach, describe, expect, it } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { PiDiagnosticReport } from "./diagnostics-pi";
import { bundleIssueReport } from "./logs-pi";

const tempDirs: string[] = [];

afterEach(() => {
    for (const path of tempDirs.splice(0)) rmSync(path, { recursive: true, force: true });
});

const SELECTED = "019fdade-87e3-7657-9ce7-65bee79b08e3";
const OTHER_UUID = "019fdade-0000-7657-9ce7-65bee79b08e3";
const OTHER_CUSTOM = "my_session";

function makeReport(root: string, logPath: string): PiDiagnosticReport {
    return {
        timestamp: "2026-05-11T12:00:00.000Z",
        platform: "linux",
        arch: "x64",
        nodeVersion: "v24.0.0",
        pluginVersion: "0.1.0",
        piInstalled: true,
        piPath: join(root, "pi"),
        piVersion: "0.74.0",
        settings: {
            path: join(root, "settings.json"),
            exists: true,
            hasEidnaraPackage: true,
            packages: ["npm:@eidnara/pi"],
        },
        configPaths: {
            agentDir: join(root, "agent"),
            userConfig: join(root, "eidnara.jsonc"),
            projectConfig: join(root, "project", "eidnara.jsonc"),
        },
        userConfig: { path: join(root, "eidnara.jsonc"), exists: false, flags: {} },
        projectConfig: { path: join(root, "project", "eidnara.jsonc"), exists: false, flags: {} },
        loadedConfigPaths: [],
        loadWarnings: [],
        conflicts: { knownConflicts: [], otherPiExtensions: [] },
        logFile: { path: logPath, exists: true, sizeKb: 1 },
        recentSessions: [
            { sessionId: SELECTED, directory: "/work/a", lastActiveAt: "2026-05-11T12:00:00.000Z" },
        ],
        historianDumps: {
            byProject: [],
            legacyDumps: { dir: join(root, "dumps"), count: 0, recent: [] },
        },
    };
}

async function bundleWithLog(
    prefix: string,
    log: string,
    sessionFilter: string | null,
): Promise<string> {
    const root = mkdtempSync(join(tmpdir(), prefix));
    tempDirs.push(root);
    const logPath = join(root, "eidnara.log");
    writeFileSync(logPath, log);
    const bundled = await bundleIssueReport(makeReport(root, logPath), "desc", "title", {
        cwd: root,
        sessionFilter,
    });
    return readFileSync(bundled.path, "utf-8");
}

describe("bundleIssueReport session filtering", () => {
    const logLines = [
        `[2026-05-11T12:00:00.000Z] [eidnara][${SELECTED}] selected session line`,
        `[2026-05-11T12:00:01.000Z] [eidnara][${OTHER_UUID}] other uuid session line`,
        `[2026-05-11T12:00:02.000Z] [eidnara][${OTHER_CUSTOM}] other custom session line`,
        "[2026-05-11T12:00:03.000Z] [eidnara][pi-status] status label line",
        "[2026-05-11T12:00:04.000Z] [eidnara][global] global label line",
        "[2026-05-11T12:00:05.000Z] [eidnara][pi] plugin label line",
        "[2026-05-11T12:00:06.000Z] plugin startup line without a session tag",
    ];

    it("keeps the selected session, untagged lines, and non-session labels; drops every other tag", async () => {
        const body = await bundleWithLog(
            "eidnara-pi-issue-filter-",
            `${logLines.join("\n")}\n`,
            SELECTED,
        );

        expect(body).toContain("selected session line");
        expect(body).toContain("status label line");
        expect(body).toContain("global label line");
        expect(body).toContain("plugin label line");
        expect(body).toContain("plugin startup line without a session tag");
        expect(body).not.toContain("other uuid session line");
        expect(body).not.toContain("other custom session line");
    });

    it("keeps every line when no session is selected", async () => {
        const body = await bundleWithLog(
            "eidnara-pi-issue-nofilter-",
            `${logLines.join("\n")}\n`,
            null,
        );

        expect(body).toContain("other uuid session line");
        expect(body).toContain("other custom session line");
    });
});

describe("bundleIssueReport log reading", () => {
    it("reads only the tail of a large log and drops the leading partial line", async () => {
        // 5,000 lines of 2 KiB each is ~10 MiB, larger than the 4 MiB tail window.
        const filler = Array.from(
            { length: 5000 },
            (_, i) => `[2026-05-11T12:00:00.000Z] filler ${i} ${"x".repeat(2000)}`,
        );
        const body = await bundleWithLog(
            "eidnara-pi-issue-tail-",
            `early only line\n${filler.join("\n")}\nfinal tail line\n`,
            null,
        );

        expect(body).toContain("final tail line");
        expect(body).not.toContain("early only line");
        expect(body.length).toBeLessThan(70_000);
    });
});
