import { afterEach, describe, expect, it } from "bun:test";
import { execFileSync } from "node:child_process";
import {
    existsSync,
    mkdtempSync,
    readFileSync,
    rmSync,
    statSync,
    symlinkSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type DiagnosticReport, renderDiagnosticsMarkdown } from "./diagnostics-opencode";
import { readLogTailLines, TRUNCATED_RECORD_MARKER } from "./log-tail";
import { bundleIssueReport, sanitizeLogContent } from "./logs-opencode";

const tempDirs: string[] = [];

afterEach(() => {
    for (const path of tempDirs.splice(0)) rmSync(path, { recursive: true, force: true });
});

function makeReport(root: string, overrides: Partial<DiagnosticReport> = {}): DiagnosticReport {
    return {
        timestamp: "2026-05-11T12:00:00.000Z",
        platform: "darwin",
        arch: "arm64",
        nodeVersion: "v24.0.0",
        pluginVersion: "0.18.0",
        opencodeInstalled: true,
        opencodeVersion: "1.0.0",
        opencodeInstallKind: "cli",
        opencodeInstallations: [
            {
                path: join(root, "opencode"),
                source: "PATH",
                kind: "cli",
                version: "1.0.0",
                active: true,
            },
        ],
        configPaths: {
            configDir: join(root, ".config", "opencode"),
            opencodeConfig: join(root, ".config", "opencode", "opencode.jsonc"),
            opencodeConfigFormat: "jsonc",
            eidnaraConfig: join(root, ".config", "eidnara", "eidnara.jsonc"),
            tuiConfig: join(root, ".config", "opencode", "tui.jsonc"),
            tuiConfigFormat: "jsonc",
            omoConfig: null,
        },
        opencodeConfigHasPlugin: true,
        pluginRegisteredInLoadedLayers: true,
        tuiConfigHasPlugin: true,
        projectDirectory: root,
        projectOpencodeConfig: { paths: [], hasPlugin: false, parseErrors: [] },
        eidnaraConfig: {
            path: join(root, ".config", "eidnara", "eidnara.jsonc"),
            exists: false,
            flags: {},
        },
        projectConfig: {
            path: join(root, ".eidnara", "eidnara.jsonc"),
            exists: false,
            flags: {},
        },
        conflicts: {
            hasConflict: false,
            reasons: [],
            eidnaraEnabled: true,
            compactionEnabled: true,
            nativeCompaction: { auto: false, prune: false },
        },
        logFile: { path: join(root, "missing.log"), exists: false, sizeKb: 0 },
        recentSessions: [],
        sessionDiscovery: "ok",
        historianDumps: {
            byProject: [],
            legacyDumps: { dir: join(root, "dumps"), count: 0, recent: [] },
        },
        ...overrides,
    };
}

async function bundleInTempCwd(
    root: string,
    report: DiagnosticReport,
    sessionFilter: string | null = null,
): Promise<string> {
    const originalCwd = process.cwd();
    process.chdir(root);
    try {
        const bundled = await bundleIssueReport(report, "description", "title", sessionFilter);
        return readFileSync(bundled.path, "utf-8");
    } finally {
        process.chdir(originalCwd);
    }
}

describe("readLogTailLines", () => {
    it.if(process.platform !== "win32")("rejects a FIFO instead of blocking on it", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-log-tail-"));
        tempDirs.push(root);
        const fifo = join(root, "eidnara.log");
        execFileSync("mkfifo", [fifo]);
        expect(() => readLogTailLines(fifo)).toThrow("not a regular file");
    });

    it.if(process.platform !== "win32")("refuses a symlink instead of reading its target", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-log-tail-"));
        tempDirs.push(root);
        const target = join(root, "private.txt");
        writeFileSync(target, "secret line\n");
        const link = join(root, "eidnara.log");
        symlinkSync(target, link);
        expect(() => readLogTailLines(link, 1024)).toThrow();
    });

    it("rejects a directory as not a regular file", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-log-tail-"));
        tempDirs.push(root);
        expect(() => readLogTailLines(root, 1024)).toThrow("not a regular file");
    });

    it("returns every line of a file smaller than the byte cap", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-log-tail-"));
        tempDirs.push(root);
        const path = join(root, "eidnara.log");
        writeFileSync(path, "one\ntwo\nthree\n");
        expect(readLogTailLines(path, 1024)).toEqual(["one", "two", "three", ""]);
    });

    it("reads only the final bytes of a large file and drops the partial first line", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-log-tail-"));
        tempDirs.push(root);
        const path = join(root, "eidnara.log");
        const lines = Array.from(
            { length: 10_000 },
            (_, i) => `[2026-05-11T12:00:00.000Z] line ${i}`,
        );
        writeFileSync(path, `${lines.join("\n")}\n`);
        const tail = readLogTailLines(path, 2048);
        expect(tail.length).toBeLessThan(100);
        expect(tail.at(-2)).toBe("[2026-05-11T12:00:00.000Z] line 9999");
        // A cut inside a record must not surface as a truncated line.
        for (const line of tail.slice(0, -1)) expect(line).toMatch(/^\[2026-/);
    });

    it("does not split a multi-byte character at the cut", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-log-tail-"));
        tempDirs.push(root);
        const path = join(root, "eidnara.log");
        const line = "[2026-05-11T12:00:00.000Z] ééééééééééééééééééééé";
        writeFileSync(path, `${Array.from({ length: 50 }, () => line).join("\n")}\n`);
        const tail = readLogTailLines(path, 101);
        expect(tail.join("\n")).not.toContain("\uFFFD");
        for (const entry of tail.slice(0, -1)) expect(entry).toBe(line);
    });

    it("keeps a record that starts exactly at the window boundary", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-log-tail-"));
        tempDirs.push(root);
        const path = join(root, "eidnara.log");
        writeFileSync(path, "one\ntwo\nthree\n");
        // The final 10 bytes are "two\nthree\n", which opens on a record boundary.
        expect(readLogTailLines(path, 10)).toEqual(["two", "three", ""]);
        // The final 9 bytes are "wo\nthree\n": the partial "wo" is dropped.
        expect(readLogTailLines(path, 9)).toEqual(["three", ""]);
    });

    it("keeps a marked suffix when the newest record alone exceeds the window", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-log-tail-"));
        tempDirs.push(root);
        const path = join(root, "eidnara.log");
        const huge = `[2026-05-11T12:00:00.000Z] payload ${"é".repeat(5000)} END`;
        writeFileSync(path, `[2026-05-11T11:00:00.000Z] earlier\n${huge}\n`);
        const tail = readLogTailLines(path, 512);
        expect(tail[0]).toStartWith(TRUNCATED_RECORD_MARKER);
        expect(tail[0]).not.toContain("\uFFFD");
        expect(tail[0]).toEndWith(" END");
        expect(tail).toHaveLength(2);
    });
});

describe("bundleIssueReport code fences", () => {
    it("chooses a fence longer than any backtick run in the log so content cannot close it", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-fence-"));
        tempDirs.push(root);
        const logPath = join(root, "eidnara.log");
        writeFileSync(
            logPath,
            [
                "[2026-05-11T12:00:00.000Z] error payload follows",
                "```",
                "# not a heading, still log content",
                "```",
                "[2026-05-11T12:00:01.000Z] done",
                "",
            ].join("\n"),
        );
        const body = await bundleInTempCwd(
            root,
            makeReport(root, { logFile: { path: logPath, exists: true, sizeKb: 1 } }),
        );
        const logSection = body.slice(body.indexOf("## Log (last"));
        const fenceLines = logSection.split("\n").filter((line) => /^`{3,}$/.test(line));
        // Opener and closer are the same four-backtick run; the three-backtick lines are log content.
        expect(fenceLines).toEqual(["````", "```", "```", "````"]);
        expect(logSection).toContain("# not a heading, still log content");
    });
});

describe("bundleIssueReport configuration section", () => {
    it("renders each config tier's flags once, under Diagnostics", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-config-once-"));
        tempDirs.push(root);
        const body = await bundleInTempCwd(
            root,
            makeReport(root, {
                eidnaraConfig: {
                    path: join(root, ".config", "eidnara", "eidnara.jsonc"),
                    exists: true,
                    flags: { historian: { disable: true }, marker_value_once: "unique-marker" },
                },
            }),
        );
        expect(body.split("unique-marker").length - 1).toBe(1);
        expect(body).toContain("### User config flags");
        expect(body).toContain("Sanitized flags for both tiers are listed under Diagnostics.");
    });
});

describe("bundleIssueReport output path", () => {
    it.if(process.platform !== "win32")("creates the bundle owner-only", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-mode-"));
        tempDirs.push(root);
        const originalCwd = process.cwd();
        process.chdir(root);
        try {
            const { path } = await bundleIssueReport(makeReport(root), "description", "title");
            expect(statSync(path).mode & 0o777).toBe(0o600);
        } finally {
            process.chdir(originalCwd);
        }
    });

    it("does not overwrite a bundle written in the same second", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-collide-"));
        tempDirs.push(root);
        const originalCwd = process.cwd();
        process.chdir(root);
        try {
            const first = await bundleIssueReport(makeReport(root), "first", "title");
            const second = await bundleIssueReport(makeReport(root), "second", "title");
            expect(second.path).not.toBe(first.path);
            expect(readFileSync(first.path, "utf-8")).toContain("first");
            expect(readFileSync(second.path, "utf-8")).toContain("second");
            expect(second.path).toMatch(/eidnara-issue-\d{8}-\d{6}(-\d+)?\.md$/);
        } finally {
            process.chdir(originalCwd);
        }
    });
});

describe("bundleIssueReport log read failures", () => {
    it("still writes the bundle and records a sanitized read error when the log vanished", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-logread-"));
        tempDirs.push(root);
        const missing = join(root, "home", "alice", "eidnara.log");
        const body = await bundleInTempCwd(
            root,
            makeReport(root, { logFile: { path: missing, exists: true, sizeKb: 12 } }),
        );
        expect(body).toContain("## Diagnostics");
        expect(body).toContain("_Log could not be read: ENOENT");
        expect(body).toContain("<no log output>");
        expect(body).toContain("/home/<USER>/eidnara.log");
        expect(body).not.toContain("alice");
    });
});

describe("bundleIssueReport environment line", () => {
    it("reports a Desktop-only install as installed with an unknown version", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-desktop-"));
        tempDirs.push(root);
        const body = await bundleInTempCwd(
            root,
            makeReport(root, {
                opencodeInstalled: true,
                opencodeInstallKind: "desktop",
                opencodeVersion: null,
            }),
        );
        expect(body).toContain("- OpenCode: unknown version [desktop]");
        expect(body).not.toContain("- OpenCode: not installed");
    });

    it("reports a missing install as not installed", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-none-"));
        tempDirs.push(root);
        const body = await bundleInTempCwd(
            root,
            makeReport(root, {
                opencodeInstalled: false,
                opencodeInstallKind: "none",
                opencodeVersion: null,
                opencodeInstallations: [],
            }),
        );
        expect(body).toContain("- OpenCode: not installed");
    });
});

describe("bundleIssueReport session filter", () => {
    it("drops the stack frames that follow another session's Error record", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-session-"));
        tempDirs.push(root);
        const logPath = join(root, "eidnara.log");
        writeFileSync(
            logPath,
            [
                "[2026-05-11T12:00:00.000Z] [eidnara][ses_keepme0001] historian ran",
                "[2026-05-11T12:00:01.000Z] [eidnara][ses_other00002] historian failure: boom",
                "Error: boom",
                "    at otherFrame (/srv/app/other.ts:1:1)",
                "[2026-05-11T12:00:02.000Z] [eidnara][ses_keepme0001] Error: mine",
                "    at mineFrame (/srv/app/mine.ts:2:2)",
                "",
            ].join("\n"),
        );
        const body = await bundleInTempCwd(
            root,
            makeReport(root, { logFile: { path: logPath, exists: true, sizeKb: 1 } }),
            "ses_keepme0001",
        );
        expect(body).toContain("ses_keepme0001");
        expect(body).toContain("mineFrame");
        expect(body).not.toContain("ses_other00002");
        expect(body).not.toContain("otherFrame");
        expect(body).not.toContain("Error: boom");
    });

    it("drops continuation lines that precede the first record start", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-orphan-"));
        tempDirs.push(root);
        const logPath = join(root, "eidnara.log");
        writeFileSync(
            logPath,
            [
                "    at orphanFrame (/srv/app/orphan.ts:9:9)",
                "    at orphanFrame2 (/srv/app/orphan.ts:10:10)",
                "[2026-05-11T12:00:02.000Z] [eidnara][ses_keepme0001] kept line",
                "",
            ].join("\n"),
        );
        const body = await bundleInTempCwd(
            root,
            makeReport(root, { logFile: { path: logPath, exists: true, sizeKb: 1 } }),
            "ses_keepme0001",
        );
        expect(body).toContain("kept line");
        expect(body).not.toContain("orphanFrame");
    });

    it("excludes untagged records from a session-scoped bundle", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-untagged-"));
        tempDirs.push(root);
        const logPath = join(root, "eidnara.log");
        writeFileSync(
            logPath,
            [
                "[2026-05-11T12:00:00.000Z] [eidnara] plugin loaded from /srv/other-project",
                "[2026-05-11T12:00:01.000Z] [eidnara][ses_keepme0001] kept line",
                "[2026-05-11T12:00:02.000Z] [eidnara] daemon connect failed: ECONNREFUSED",
                "    at untaggedFrame (/srv/app/global.ts:3:3)",
                "",
            ].join("\n"),
        );
        const body = await bundleInTempCwd(
            root,
            makeReport(root, { logFile: { path: logPath, exists: true, sizeKb: 1 } }),
            "ses_keepme0001",
        );
        expect(body).toContain("kept line");
        expect(body).not.toContain("other-project");
        expect(body).not.toContain("ECONNREFUSED");
        expect(body).not.toContain("untaggedFrame");
    });

    it("keeps leading untagged lines when no session filter is set", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-nofilter-"));
        tempDirs.push(root);
        const logPath = join(root, "eidnara.log");
        writeFileSync(
            logPath,
            "    at leadingFrame (/srv/app/x.ts:1:1)\n[2026-05-11T12:00:02.000Z] line\n",
        );
        const body = await bundleInTempCwd(
            root,
            makeReport(root, { logFile: { path: logPath, exists: true, sizeKb: 1 } }),
        );
        expect(body).toContain("leadingFrame");
    });

    it("scopes recent sessions and historian buckets to the selected session", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-scope-"));
        tempDirs.push(root);
        const body = await bundleInTempCwd(
            root,
            makeReport(root, {
                recentSessions: [
                    {
                        sessionId: "ses_keepme0001",
                        title: "Selected session title",
                        directory: join(root, "project-a"),
                        lastActiveAt: "2026-05-11T12:00:00.000Z",
                    },
                    {
                        sessionId: "ses_other00002",
                        title: "Unrelated session title",
                        directory: join(root, "project-b"),
                        lastActiveAt: "2026-05-11T11:00:00.000Z",
                    },
                ],
                historianDumps: {
                    byProject: [
                        {
                            directory: join(root, "project-a"),
                            primarySessionId: "ses_other00003",
                            sessionIds: ["ses_other00003", "ses_keepme0001"],
                            count: 1,
                            recent: [],
                        },
                        {
                            directory: join(root, "project-b"),
                            primarySessionId: "ses_other00002",
                            sessionIds: ["ses_other00002"],
                            count: 1,
                            recent: [],
                        },
                    ],
                    legacyDumps: { dir: join(root, "dumps"), count: 0, recent: [] },
                },
            }),
            "ses_keepme0001",
        );
        expect(body).toContain("<REDACTED 22 chars>");
        expect(body).toContain("project-a");
        expect(body).not.toContain("Unrelated session title");
        expect(body).not.toContain("Selected session title");
        expect(body).not.toContain("project-b");
        expect(body).not.toContain("ses_other00002");
        expect(body).not.toContain("ses_other00003");
    });

    it("leaves the report unscoped when no session filter is set", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-unscoped-"));
        tempDirs.push(root);
        const body = await bundleInTempCwd(
            root,
            makeReport(root, {
                recentSessions: [
                    {
                        sessionId: "ses_other00002",
                        title: "Unrelated session title",
                        directory: join(root, "project-b"),
                        lastActiveAt: "2026-05-11T11:00:00.000Z",
                    },
                ],
            }),
        );
        expect(body).toContain("<REDACTED 23 chars>");
        expect(body).not.toContain("Unrelated session title");
    });
});

describe("bundleIssueReport session filter fail-closed records", () => {
    it("drops an untagged record and a leading fragment when a session is selected", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-session-records-"));
        tempDirs.push(root);
        const logPath = join(root, "eidnara.log");
        writeFileSync(
            logPath,
            [
                "    at leading-fragment-frame (/work/x.ts:1:1)",
                "[2026-05-11T12:00:00.000Z] [eidnara][ses_other000] other failed: boom",
                "Error: boom",
                "    at other-session-frame (/work/b.ts:1:1)",
                "[2026-05-11T12:00:01.000Z] [eidnara][ses_selected0] selected failed: mine",
                "Error: mine",
                "    at selected-session-frame (/work/a.ts:1:1)",
                "[2026-05-11T12:00:02.000Z] untagged record",
            ].join("\n"),
        );
        const report = makeReport(root, { logFile: { path: logPath, exists: true, sizeKb: 1 } });

        const body = await bundleInTempCwd(root, report, "ses_selected0");

        expect(body).not.toContain("leading-fragment-frame");
        expect(body).not.toContain("boom");
        expect(body).not.toContain("other-session-frame");
        expect(body).toContain("Error: mine");
        expect(body).toContain("selected-session-frame");
        // An untagged record cannot be attributed, so it fails closed.
        expect(body).not.toContain("untagged record");
    });
});

describe("bundleIssueReport URL redaction in config values", () => {
    it("redacts URL userinfo and query strings in config values", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-url-"));
        tempDirs.push(root);
        const report = makeReport(root, {
            eidnaraConfig: {
                path: join(root, ".config", "eidnara", "eidnara.jsonc"),
                exists: true,
                flags: {
                    embedding: {
                        endpoint: "https://svc-user:s3cr3t-pass@embed.example.com/v1?access=abc123",
                    },
                },
            },
        });

        const body = await bundleInTempCwd(root, report);

        // The account name stays to identify the endpoint; the password and the query go.
        expect(body).toContain(
            "https://svc-user:<REDACTED:password>@embed.example.com/v1?<REDACTED:query>",
        );
        expect(body).not.toContain("s3cr3t-pass");
        expect(body).not.toContain("abc123");
    });
});

describe("renderDiagnosticsMarkdown conflict detection failure", () => {
    it("reports a detection error alongside the default no-conflict verdict", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-render-conflicts-"));
        tempDirs.push(root);
        const markdown = renderDiagnosticsMarkdown(
            makeReport(root, {
                conflicts: {
                    hasConflict: false,
                    reasons: [],
                    eidnaraEnabled: true,
                    compactionEnabled: true,
                    nativeCompaction: { auto: false, prune: false },
                    detectionError: "uv_os_homedir returned ENOENT at /home/alice/.omo",
                },
            }),
        );
        expect(markdown).toContain(
            "- Conflicts detected: none (detection failed: uv_os_homedir returned ENOENT at /home/<USER>/.omo)",
        );
    });
});

describe("renderDiagnosticsMarkdown sanitization", () => {
    it("sanitizes version-probe output in the summary line and installation table", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-render-version-"));
        tempDirs.push(root);
        const noisyVersion =
            "1.2.3\nwarning: shim at /home/alice/.local/bin/opencode token=abc123\r\n";
        const markdown = renderDiagnosticsMarkdown(
            makeReport(root, {
                opencodeVersion: noisyVersion,
                opencodeInstallations: [
                    {
                        path: "/home/alice/a",
                        source: "PATH",
                        kind: "cli",
                        version: noisyVersion,
                        active: true,
                    },
                    {
                        path: "/home/alice/b",
                        source: "app",
                        kind: "desktop",
                        version: "unknown",
                        active: false,
                    },
                ],
            }),
        );
        expect(markdown).toContain(
            "(1.2.3 warning: shim at /home/<USER>/.local/bin/opencode token=<REDACTED:token>)",
        );
        expect(markdown).toContain(
            "| 1.2.3 warning: shim at /home/<USER>/.local/bin/opencode token=<REDACTED:token> | PATH |",
        );
        expect(markdown).not.toContain("alice");
        expect(markdown).not.toContain("abc123");
    });

    it("sanitizes historian dump parse errors and host-config parse errors", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-render-"));
        tempDirs.push(root);
        const markdown = renderDiagnosticsMarkdown(
            makeReport(root, {
                opencodeConfigParseError:
                    "EACCES: permission denied, open '/home/alice/.config/opencode/opencode.jsonc'",
                tuiConfigParseError: "Unexpected token } in JSON at position 12",
                historianDumps: {
                    byProject: [],
                    legacyDumps: {
                        dir: join(root, "dumps"),
                        count: 1,
                        recent: [
                            {
                                name: "historian-1.xml",
                                ageMinutes: 5,
                                sizeKb: 3,
                                parseError:
                                    "EACCES: permission denied, open '/home/alice/.eidnara/context/historian/historian-1.xml'",
                            },
                        ],
                    },
                },
            }),
        );
        expect(markdown).toContain(
            "- opencode config parse error: EACCES: permission denied, open '/home/<USER>/.config/opencode/opencode.jsonc'",
        );
        expect(markdown).toContain(
            "- tui config parse error: Unexpected token } in JSON at position 12",
        );
        expect(markdown).toContain("/home/<USER>/.eidnara/context/historian/historian-1.xml");
        expect(markdown).not.toContain("alice");
    });
});

describe("sanitizeLogContent — secret token redaction (council finding #9)", () => {
    it("redacts provider token shapes the shared redaction fixture does not enumerate", () => {
        const githubTokens = [
            "ghp_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789",
            "gho_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789",
            "ghs_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789",
            "ghu_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789",
            "ghr_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789",
        ];
        for (const token of githubTokens) {
            // The keyed `Token:` rule claims the value; a bare token falls to the provider rule.
            expect(sanitizeLogContent(`Token: ${token}`)).toBe("Token: <REDACTED:token>");
            const bare = sanitizeLogContent(`saw ${token} in output`);
            expect(bare, token).toContain("<GITHUB_TOKEN_REDACTED>");
            expect(bare, token).not.toContain(token.slice(0, 12));
        }
        expect(sanitizeLogContent("Using temp creds ASIAIOSFODNN7EXAMPLE for STS")).toContain(
            "<AWS_ACCESS_KEY_ID_REDACTED>",
        );
        for (const prefix of ["xoxp", "xoxr", "xoxs", "xoxa"]) {
            const sanitized = sanitizeLogContent(
                `using ${prefix}-1234567890-abcdefghij-ABCDEFG12345 for slack`,
            );
            expect(sanitized, prefix).toContain("<SLACK_TOKEN_REDACTED>");
            expect(sanitized, prefix).not.toContain(`${prefix}-1234`);
        }
        expect(sanitizeLogContent("authorization: bearer abcdefghij1234567890")).toContain(
            "<REDACTED:bearer>",
        );
    });

    it("derives the redaction label from the assigned env-var name", () => {
        const cases: [string, string][] = [
            ["MY_CUSTOM_API_KEY=abcdef12345", "MY_CUSTOM_API_KEY=<REDACTED:api_key>"],
            ["DATABASE_TOKEN=tokenvaluehere", "DATABASE_TOKEN=<REDACTED:database_token>"],
            ["MY_SECRET=mysecretvalue", "MY_SECRET=<REDACTED:secret>"],
            ["DB_PASSWORD=hunter2", "DB_PASSWORD=<REDACTED:db_password>"],
            ["AUTH_CREDENTIAL=abcdef", "AUTH_CREDENTIAL=<REDACTED:auth_credential>"],
            ["MY_PRIVATE_KEY=mykey-data", "MY_PRIVATE_KEY=<REDACTED:private_key>"],
        ];
        for (const [input, expected] of cases) {
            expect(sanitizeLogContent(input), input).toBe(expected);
        }
        const syntheticKey = "wJalrXUtnFEMI/" + "K7MDENG/bPxRfiCYEXAMPLEKEY"; // gitleaks:allow redaction-test fixture
        const sanitized = sanitizeLogContent(`aws_secret_access_key=${syntheticKey}`);
        expect(sanitized).toContain("aws_secret_access_key=<REDACTED:aws_secret_access_key>");
        expect(sanitized).not.toContain("wJalrXUtnFEMI");
    });

    it("keeps non-secret env vars, model names, and dotted digests without a JWT prefix", () => {
        const sanitized = sanitizeLogContent("OPENCODE_VERSION=1.4.0\nNODE_ENV=production");
        expect(sanitized).toContain("OPENCODE_VERSION=1.4.0");
        expect(sanitized).toContain("NODE_ENV=production");
        expect(
            sanitizeLogContent("Using model anthropic/claude-haiku-4-5 for historian"),
        ).toContain("anthropic/claude-haiku-4-5");
        const digest = sanitizeLogContent("computed digest: abcdefgh.ijklmnop.qrstuvwx");
        expect(digest).toContain("abcdefgh.ijklmnop.qrstuvwx");
        expect(digest).not.toContain("<JWT_REDACTED>");
    });

    it("handles a realistic log line with both path and token", () => {
        const log =
            "[2026-04-28] /Users/alice/.config/opencode using sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYzZ12345678901234567890";
        const sanitized = sanitizeLogContent(log);
        expect(sanitized).toContain("/Users/<USER>/");
        expect(sanitized).toContain("<ANTHROPIC_API_KEY_REDACTED>");
        expect(sanitized).not.toContain("alice");
        expect(sanitized).not.toContain("sk-ant-api03-AbCd");
    });

    it("handles multiline log content", () => {
        const log = [
            "Loading config from /Users/alice/.config/eidnara/eidnara.jsonc",
            "ANTHROPIC_API_KEY=sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYzZ12345678901234567890",
            'Spawning subagent with {"api_key":"superdupersecret"}',
            "Done.",
        ].join("\n");
        const sanitized = sanitizeLogContent(log);
        expect(sanitized).toContain("/Users/<USER>/");
        expect(sanitized).toContain("ANTHROPIC_API_KEY=<REDACTED:anthropic_api_key>");
        expect(sanitized).toContain('"api_key":"<REDACTED:api_key>"');
        expect(sanitized).toContain("Done.");
    });
});

describe("bundleIssueReport secret redaction", () => {
    it("redacts secret-looking config keys before writing the issue bundle", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-redaction-"));
        tempDirs.push(root);
        const originalCwd = process.cwd();
        process.chdir(root);
        try {
            const report: DiagnosticReport = {
                timestamp: "2026-05-11T12:00:00.000Z",
                platform: "darwin",
                arch: "arm64",
                nodeVersion: "v24.0.0",
                pluginVersion: "0.18.0",
                opencodeInstalled: true,
                opencodeVersion: "1.0.0",
                opencodeInstallKind: "cli",
                opencodeInstallations: [
                    {
                        path: join(root, "opencode"),
                        source: "PATH",
                        kind: "cli",
                        version: "1.0.0",
                        active: true,
                    },
                ],
                configPaths: {
                    configDir: join(root, ".config", "opencode"),
                    opencodeConfig: join(root, ".config", "opencode", "opencode.jsonc"),
                    opencodeConfigFormat: "jsonc",
                    eidnaraConfig: join(root, ".config", "eidnara", "eidnara.jsonc"),
                    tuiConfig: join(root, ".config", "opencode", "tui.jsonc"),
                    tuiConfigFormat: "jsonc",
                    omoConfig: null,
                },
                opencodeConfigHasPlugin: true,
                tuiConfigHasPlugin: true,
                projectDirectory: root,
                projectOpencodeConfig: { paths: [], hasPlugin: false, parseErrors: [] },
                eidnaraConfig: {
                    path: join(root, ".config", "eidnara", "eidnara.jsonc"),
                    exists: true,
                    flags: {
                        embedding: {
                            provider: "openai-compatible",
                            api_key: "emb-secret-value",
                            headers: {
                                Authorization: "Bearer header-secret-value",
                                "X-Api-Key": "custom-header-secret",
                            },
                        },
                        historian: { api_key: "historian-secret-value" },
                    },
                },
                projectConfig: {
                    path: join(root, ".eidnara", "eidnara.jsonc"),
                    exists: false,
                    flags: {},
                },
                conflicts: {
                    hasConflict: false,
                    reasons: [],
                    eidnaraEnabled: true,
                    compactionEnabled: true,
                    nativeCompaction: { auto: false, prune: false },
                },
                logFile: { path: join(root, "missing.log"), exists: false, sizeKb: 0 },
                recentSessions: [],
                sessionDiscovery: "ok",
                historianDumps: {
                    byProject: [],
                    legacyDumps: { dir: join(root, "dumps"), count: 0, recent: [] },
                },
            };

            const bundled = await bundleIssueReport(report, "description", "title");
            expect(existsSync(bundled.path)).toBe(true);
            const body = readFileSync(bundled.path, "utf-8");

            expect(body).toContain('"api_key": "<REDACTED:api_key>"');
            expect(body).toContain('"Authorization": "<REDACTED:authorization>"');
            expect(body).toContain('"X-Api-Key": "<REDACTED:x_api_key>"');
            expect(body).not.toContain("emb-secret-value");
            expect(body).not.toContain("historian-secret-value");
            expect(body).not.toContain("header-secret-value");
            expect(body).not.toContain("custom-header-secret");
            expect(body).not.toContain("### OpenCode installations");
            expect(body).toContain("- OpenCode installed: true [cli] (1.0.0)");
        } finally {
            process.chdir(originalCwd);
        }
    });

    it("sanitizes title, description, config paths, and recent session titles", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-issue-sanitize-"));
        tempDirs.push(root);
        const originalCwd = process.cwd();
        process.chdir(root);
        try {
            const report: DiagnosticReport = {
                timestamp: "2026-05-11T12:00:00.000Z",
                platform: "darwin",
                arch: "arm64",
                nodeVersion: "v24.0.0",
                pluginVersion: "0.18.0",
                opencodeInstalled: true,
                opencodeVersion: "1.0.0",
                opencodeInstallKind: "cli",
                opencodeInstallations: [
                    {
                        path: "/Users/alice/.opencode/bin/opencode",
                        source: "PATH",
                        kind: "cli",
                        version: "1.18.0",
                        active: true,
                    },
                    {
                        path: "/Users/alice/Applications/OpenCode.app",
                        source: "app",
                        kind: "desktop",
                        version: "unknown",
                        active: false,
                    },
                ],
                configPaths: {
                    configDir: "/Users/alice/.config/opencode",
                    opencodeConfig: "/Users/alice/.config/opencode/opencode.jsonc",
                    opencodeConfigFormat: "jsonc",
                    eidnaraConfig: "/Users/alice/.config/eidnara/eidnara.jsonc",
                    tuiConfig: "/Users/alice/.config/opencode/tui.jsonc",
                    tuiConfigFormat: "jsonc",
                    omoConfig: null,
                },
                opencodeConfigHasPlugin: true,
                tuiConfigHasPlugin: true,
                projectDirectory: root,
                projectOpencodeConfig: { paths: [], hasPlugin: false, parseErrors: [] },
                eidnaraConfig: {
                    path: "/Users/alice/.config/eidnara/eidnara.jsonc",
                    exists: true,
                    flags: {},
                },
                projectConfig: {
                    path: "/Users/alice/project/.eidnara/eidnara.json",
                    exists: true,
                    parseError:
                        "EACCES: permission denied, open '/Users/alice/project/.eidnara/eidnara.json'",
                    flags: {},
                },
                conflicts: {
                    hasConflict: false,
                    reasons: [],
                    eidnaraEnabled: true,
                    compactionEnabled: true,
                    nativeCompaction: { auto: false, prune: false },
                },
                logFile: { path: join(root, "missing.log"), exists: false, sizeKb: 0 },
                recentSessions: [
                    {
                        sessionId: "ses_1",
                        title: "Problem at /Users/alice/private token=abc123",
                        directory: "/Users/alice/project",
                        lastActiveAt: "2026-05-11T12:00:00.000Z",
                    },
                ],
                historianDumps: {
                    byProject: [],
                    legacyDumps: { dir: join(root, "dumps"), count: 0, recent: [] },
                },
            };

            const bundled = await bundleIssueReport(
                report,
                "Description with /Users/alice/private and token=abc123",
                "Title with /Users/alice/private and token=abc123",
            );
            const body = readFileSync(bundled.path, "utf-8");

            expect(body).toContain("## Title");
            expect(body).toContain("Title with /Users/<USER>/private and token=<REDACTED:token>");
            expect(body).toContain(
                "Description with /Users/<USER>/private and token=<REDACTED:token>",
            );
            expect(body).toContain(
                "User config from `/Users/<USER>/.config/eidnara/eidnara.jsonc`",
            );
            expect(body).toContain(
                "Project config from `/Users/<USER>/project/.eidnara/eidnara.json`",
            );
            expect(body).toContain(
                "- Project config parse error: EACCES: permission denied, open '/Users/<USER>/project/.eidnara/eidnara.json'",
            );
            // Session titles are user prose; only their length is shared.
            expect(body).toContain('"title": "<REDACTED 44 chars>"');
            expect(body).not.toContain("Problem at");
            expect(body).toContain("### OpenCode installations");
            expect(body).toContain(
                "| [active] | `/Users/<USER>/.opencode/bin/opencode` | 1.18.0 | PATH |",
            );
            expect(body).toContain(
                "|  | `/Users/<USER>/Applications/OpenCode.app` | unknown | app |",
            );
            expect(body).not.toContain("alice");
            expect(body).not.toContain("abc123");
        } finally {
            process.chdir(originalCwd);
        }
    });
});
