import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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
        sessionDiscovery: "ok",
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

    it("keeps a first line that begins exactly on a line boundary", () => {
        const root = makeTempRoot();
        const logPath = join(root, "eidnara.log");
        writeFileSync(logPath, "aaaa\nbbbb\ncccc\n");

        // 10 bytes back from the end lands right after the first newline.
        expect(readLogTailLines(logPath, 10)).toEqual(["bbbb", "cccc", ""]);
    });

    it("keeps the first line when the file fits inside the bound", () => {
        const root = makeTempRoot();
        const logPath = join(root, "eidnara.log");
        writeFileSync(logPath, "first\nsecond\n");

        expect(readLogTailLines(logPath, 1024)).toEqual(["first", "second", ""]);
    });
});

describe("bundleIssueReport session filtering", () => {
    it("keeps only the picked Pi session's entries", async () => {
        const root = makeTempRoot();
        const logPath = join(root, "eidnara.log");
        const wanted = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
        const other = "0190a1b2-c3d4-7e5f-8a9b-ffffffffffff";
        writeFileSync(
            logPath,
            [
                "[2026-07-07T12:00:00.000Z] [eidnara][pi] /ctx-aug: sidekick failed (timeout): stderr",
                "[2026-07-07T12:00:00.500Z] [eidnara][pi-status] Status: rendered for a session",
                `[2026-07-07T12:00:01.000Z] [eidnara][${wanted}] /ctx-status ran`,
                `[2026-07-07T12:00:02.000Z] [eidnara][${other}] /ctx-status ran elsewhere`,
                "[2026-07-07T12:00:02.500Z] [eidnara][pi-session-1a2b3c4d] /ctx-aug: project identity",
                "[2026-07-07T12:00:03.000Z] loaded | harness=pi | project=other-project | dir=/srv/other",
            ].join("\n"),
        );

        const bundled = await bundleIssueReport(reportWithLog(logPath), "desc", "title", {
            cwd: root,
            now: new Date("2026-07-07T12:00:00Z"),
            sessionFilter: `2026-07-07T12-00-00-000Z_${wanted}`,
        });

        expect(bundled.bodyMarkdown).toContain(`[eidnara][${wanted}] /ctx-status ran`);
        expect(bundled.bodyMarkdown).not.toContain(other);
        expect(bundled.bodyMarkdown).not.toContain("sidekick failed");
        expect(bundled.bodyMarkdown).not.toContain("rendered for a session");
        expect(bundled.bodyMarkdown).not.toContain("pi-session-1a2b3c4d");
        expect(bundled.bodyMarkdown).not.toContain("/srv/other");
    });

    it("keeps every entry when no session is picked", async () => {
        const root = makeTempRoot();
        const logPath = join(root, "eidnara.log");
        writeFileSync(
            logPath,
            [
                "[2026-07-07T12:00:00.000Z] [eidnara][pi] extension loaded",
                "[2026-07-07T12:00:03.000Z] loaded | harness=pi | project=p | dir=/srv/p",
            ].join("\n"),
        );

        const bundled = await bundleIssueReport(reportWithLog(logPath), "desc", "title", {
            cwd: root,
            now: new Date("2026-07-07T12:00:00Z"),
        });

        expect(bundled.bodyMarkdown).toContain("extension loaded");
        expect(bundled.bodyMarkdown).toContain("dir=/srv/p");
    });

    it("escapes a log line that would close the Markdown fence", async () => {
        const root = makeTempRoot();
        const logPath = join(root, "eidnara.log");
        writeFileSync(
            logPath,
            [
                "[2026-07-07T12:00:00.000Z] sidekick stderr follows",
                "```",
                "  ```json",
                "inside",
                "progress 10%\r```",
                "[2026-07-07T12:00:01.000Z] newest line",
            ].join("\n"),
        );

        const bundled = await bundleIssueReport(reportWithLog(logPath), "desc", "title", {
            cwd: root,
            now: new Date("2026-07-07T12:00:00Z"),
        });

        const logSection = bundled.bodyMarkdown.slice(bundled.bodyMarkdown.indexOf("## Log ("));
        expect(logSection).toContain("\n\\```\n");
        expect(logSection).toContain("\n  \\```json\n");
        expect(logSection).toContain("progress 10%\r\\```");
        expect(logSection.match(/(^|\r)```$/gm)).toHaveLength(2);
        expect(logSection).toContain("newest line");
    });

    it("drops another session's error stack along with its tagged first line", async () => {
        const root = makeTempRoot();
        const logPath = join(root, "eidnara.log");
        const wanted = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
        const other = "0190a1b2-c3d4-7e5f-8a9b-ffffffffffff";
        writeFileSync(
            logPath,
            [
                `[2026-07-07T12:00:01.000Z] [eidnara][${other}] rust session.status failed: boom`,
                "Error: boom-other",
                "    at otherFrame (file:///other.ts:1:1)",
                `[2026-07-07T12:00:02.000Z] [eidnara][${wanted}] rust session.status failed: bang`,
                "Error: bang-wanted",
                "    at wantedFrame (file:///wanted.ts:2:2)",
            ].join("\n"),
        );

        const bundled = await bundleIssueReport(reportWithLog(logPath), "desc", "title", {
            cwd: root,
            now: new Date("2026-07-07T12:00:00Z"),
            sessionFilter: wanted,
        });

        expect(bundled.bodyMarkdown).toContain("Error: bang-wanted");
        expect(bundled.bodyMarkdown).toContain("wantedFrame");
        expect(bundled.bodyMarkdown).not.toContain("boom-other");
        expect(bundled.bodyMarkdown).not.toContain("otherFrame");
    });

    it("reports an unreadable log instead of aborting the bundle", async () => {
        const root = makeTempRoot();
        const logDir = join(root, "eidnara.log");
        mkdirSync(logDir);

        const bundled = await bundleIssueReport(reportWithLog(logDir), "desc", "title", {
            cwd: root,
            now: new Date("2026-07-07T12:00:00Z"),
        });

        expect(bundled.bodyMarkdown).toContain("<log unreadable: ");
        expect(bundled.bodyMarkdown).toContain("EISDIR");
        expect(bundled.bodyMarkdown).toContain("## Diagnostics");
    });

    it("distinguishes an installed Pi without a version from an absent Pi", async () => {
        const root = makeTempRoot();
        const logPath = join(root, "eidnara.log");
        writeFileSync(logPath, "");
        const report = { ...reportWithLog(logPath), piInstalled: true, piPath: "/usr/bin/pi" };

        const bundled = await bundleIssueReport(report, "desc", "title", {
            cwd: root,
            now: new Date("2026-07-07T12:00:00Z"),
        });

        expect(bundled.bodyMarkdown).toContain("- Pi: installed, version unavailable");
    });
});

describe("bundleIssueReport file naming", () => {
    it("does not overwrite a bundle written in the same second", async () => {
        const root = makeTempRoot();
        const logPath = join(root, "eidnara.log");
        writeFileSync(logPath, "[2026-07-07T12:00:00.000Z] one\n");
        const now = new Date(2026, 6, 7, 12, 0, 0);

        const first = await bundleIssueReport(reportWithLog(logPath), "first", "t", {
            cwd: root,
            now,
        });
        const second = await bundleIssueReport(reportWithLog(logPath), "second", "t", {
            cwd: root,
            now,
        });

        expect(first.path).toBe(join(root, "eidnara-pi-issue-20260707-120000.md"));
        expect(second.path).toBe(join(root, "eidnara-pi-issue-20260707-120000-2.md"));
        expect(readFileSync(first.path, "utf-8")).toContain("first");
        expect(readFileSync(second.path, "utf-8")).toContain("second");
    });
});

const SELECTED = "019fdade-87e3-7657-9ce7-65bee79b08e3";
const OTHER_UUID = "019fdade-0000-7657-9ce7-65bee79b08e3";
const OTHER_CUSTOM = "my_session";

async function bundleWithLog(log: string, sessionFilter: string | null): Promise<string> {
    const root = makeTempRoot();
    const logPath = join(root, "eidnara.log");
    writeFileSync(logPath, log);
    const report: PiDiagnosticReport = {
        ...reportWithLog(logPath),
        recentSessions: [
            { sessionId: SELECTED, directory: "/work/a", lastActiveAt: "2026-05-11T12:00:00.000Z" },
        ],
    };
    const bundled = await bundleIssueReport(report, "desc", "title", { cwd: root, sessionFilter });
    return readFileSync(bundled.path, "utf-8");
}

describe("bundleIssueReport session filtering by tag class", () => {
    const logLines = [
        `[2026-05-11T12:00:00.000Z] [eidnara][${SELECTED}] selected session line`,
        `[2026-05-11T12:00:01.000Z] [eidnara][${OTHER_UUID}] other uuid session line`,
        `[2026-05-11T12:00:02.000Z] [eidnara][${OTHER_CUSTOM}] other custom session line`,
        "[2026-05-11T12:00:03.000Z] [eidnara][pi-status] status label line",
        "[2026-05-11T12:00:04.000Z] [eidnara][global] global label line",
        "[2026-05-11T12:00:05.000Z] [eidnara][pi] plugin label line",
        "[2026-05-11T12:00:06.000Z] plugin startup line without a session tag",
    ];

    it("keeps only the selected session and the global label; drops every other tag and untagged records", async () => {
        const body = await bundleWithLog(`${logLines.join("\n")}\n`, SELECTED);

        expect(body).toContain("selected session line");
        expect(body).toContain("global label line");
        // Untagged records cannot be attributed, and `pi` / `pi-status` carry
        // per-session command output, so all of them fail closed.
        expect(body).not.toContain("plugin startup line without a session tag");
        expect(body).not.toContain("status label line");
        expect(body).not.toContain("plugin label line");
        expect(body).not.toContain("other uuid session line");
        expect(body).not.toContain("other custom session line");
    });

    it("drops the untagged continuation lines of another session's multi-line record", async () => {
        const log = [
            `[2026-05-11T12:00:00.000Z] [eidnara][${OTHER_UUID}] rust session.status failed: boom`,
            "Error: boom",
            "    at other-session-frame (/work/b/file.ts:1:1)",
            `[2026-05-11T12:00:01.000Z] [eidnara][${SELECTED}] selected failed: mine`,
            "Error: mine",
            "    at selected-session-frame (/work/a/file.ts:1:1)",
            "[2026-05-11T12:00:02.000Z] untagged record",
            "    continuation of the untagged record",
        ].join("\n");
        const body = await bundleWithLog(`${log}\n`, SELECTED);

        expect(body).not.toContain("boom");
        expect(body).not.toContain("other-session-frame");
        expect(body).toContain("Error: mine");
        expect(body).toContain("selected-session-frame");
        expect(body).not.toContain("continuation of the untagged record");
    });

    it("drops untagged lines that precede the first record when a session is selected", async () => {
        const log = [
            "    at leading-fragment-frame (/work/b/file.ts:1:1)",
            "    at another-leading-frame (/work/b/file.ts:2:2)",
            `[2026-05-11T12:00:01.000Z] [eidnara][${SELECTED}] selected line`,
        ].join("\n");
        const body = await bundleWithLog(`${log}\n`, SELECTED);

        expect(body).not.toContain("leading-fragment-frame");
        expect(body).not.toContain("another-leading-frame");
        expect(body).toContain("selected line");
    });

    it("keeps every line when no session is selected", async () => {
        const body = await bundleWithLog(`${logLines.join("\n")}\n`, null);

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
            `early only line\n${filler.join("\n")}\nfinal tail line\n`,
            null,
        );

        expect(body).toContain("final tail line");
        expect(body).not.toContain("early only line");
        expect(body.length).toBeLessThan(70_000);
    });
});
