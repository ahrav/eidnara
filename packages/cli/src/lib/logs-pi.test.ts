import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { PiDiagnosticReport } from "./diagnostics-pi";
import { bundleIssueReport } from "./logs-pi";

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

describe("bundleIssueReport session filtering", () => {
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

    it("reports an unreadable log instead of aborting the bundle", async () => {
        const root = makeTempRoot();
        const logDir = join(root, "eidnara.log");
        mkdirSync(logDir);

        const bundled = await bundleIssueReport(reportWithLog(logDir), "desc", "title", {
            cwd: root,
            now: new Date("2026-07-07T12:00:00Z"),
        });

        expect(bundled.bodyMarkdown).toContain("<log unreadable: ");
        expect(bundled.bodyMarkdown).toContain("not a regular file");
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
    it("does not overwrite a bundle written in the same second and creates each bundle owner-only", async () => {
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
        if (process.platform !== "win32") {
            expect(statSync(first.path).mode & 0o777).toBe(0o600);
            expect(statSync(second.path).mode & 0o777).toBe(0o600);
        }
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
        "[2026-05-11T12:00:04.000Z] [eidnara][pi-session-1a2b3c4d] pi-session label line",
        "[2026-05-11T12:00:05.000Z] [eidnara][global] global label line",
        "[2026-05-11T12:00:06.000Z] [eidnara][pi] plugin label line",
        "[2026-05-11T12:00:07.000Z] plugin startup line without a session tag",
        "[2026-05-11T12:00:08.000Z] loaded | harness=pi | project=other-project | dir=/srv/other",
    ];

    it("keeps only the selected session and the global label; drops every other tag and untagged records", async () => {
        // The filter may name the session by its on-disk file stem, timestamp prefix included.
        const body = await bundleWithLog(
            `${logLines.join("\n")}\n`,
            `2026-05-11T12-00-00-000Z_${SELECTED}`,
        );

        expect(body).toContain("selected session line");
        expect(body).toContain("global label line");
        // Untagged records cannot be attributed, and `pi` / `pi-status` carry
        // per-session command output, so all of them fail closed.
        expect(body).not.toContain("plugin startup line without a session tag");
        expect(body).not.toContain("/srv/other");
        expect(body).not.toContain("status label line");
        expect(body).not.toContain("pi-session label line");
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
        expect(body).toContain("status label line");
        expect(body).toContain("pi-session label line");
        expect(body).toContain("plugin label line");
        expect(body).toContain("plugin startup line without a session tag");
        expect(body).toContain("dir=/srv/other");
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

describe("bundleIssueReport session scoping of diagnostics", () => {
    it("keeps only the selected session's dump bucket and session entry", async () => {
        const root = makeTempRoot();
        const logPath = join(root, "eidnara.log");
        writeFileSync(logPath, "");
        const report: PiDiagnosticReport = {
            ...reportWithLog(logPath),
            recentSessions: [
                {
                    sessionId: "sel",
                    directory: "/work/selected",
                    lastActiveAt: "2026-05-11T12:00:00.000Z",
                },
                {
                    sessionId: "oth",
                    directory: "/work/other-private",
                    lastActiveAt: "2026-05-11T11:00:00.000Z",
                },
            ],
            historianDumps: {
                byProject: [
                    {
                        directory: "/work/selected",
                        primarySessionId: "sel",
                        sessionIds: ["sel", "oth"],
                        count: 1,
                        recent: [{ name: "keep.xml", ageMinutes: 1, sizeKb: 1 }],
                    },
                    {
                        directory: "/work/other-private",
                        primarySessionId: "oth",
                        sessionIds: ["oth"],
                        count: 1,
                        recent: [{ name: "drop.xml", ageMinutes: 2, sizeKb: 1 }],
                    },
                ],
                legacyDumps: { dir: "/x/legacy", count: 0, recent: [] },
            },
        };

        const bundled = await bundleIssueReport(report, "desc", "title", {
            cwd: root,
            sessionFilter: "sel",
        });
        const body = readFileSync(bundled.path, "utf-8");

        expect(body).toContain("keep.xml");
        expect(body).not.toContain("drop.xml");
        expect(body).not.toContain("other-private");
        expect(body).not.toContain('"oth"');
    });
});
