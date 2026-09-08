import { afterEach, describe, expect, it } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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
            compactionEnabled: true,
            nativeCompaction: { auto: false, prune: false },
        },
        logFile: { path: join(root, "missing.log"), exists: false, sizeKb: 0 },
        recentSessions: [],
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
        expect(body).toContain("Selected session title");
        expect(body).toContain("project-a");
        expect(body).not.toContain("Unrelated session title");
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
        expect(body).toContain("Unrelated session title");
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
    describe("Anthropic API keys", () => {
        it("redacts sk-ant-api03-* tokens", () => {
            const log =
                "Using key sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-AbCd to call API";
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("<ANTHROPIC_API_KEY_REDACTED>");
            expect(sanitized).not.toContain("sk-ant-api03-AbCdEf");
        });

        it("redacts sk-ant-* legacy form", () => {
            const log = "key=sk-ant-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-AbCd";
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("<REDACTED:key>");
        });
    });

    describe("OpenAI API keys", () => {
        it("redacts sk-proj-* project keys", () => {
            const log = "OPENAI_API_KEY=sk-proj-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789";
            const sanitized = sanitizeLogContent(log);
            // The environment-variable redactor runs before token redactors.
            expect(sanitized).toBe("OPENAI_API_KEY=<REDACTED:openai_api_key>");
        });

        it("redacts standalone sk-* tokens (legacy OpenAI shape)", () => {
            const log = "calling with sk-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789ABcd then continuing";
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("<OPENAI_API_KEY_REDACTED>");
            expect(sanitized).not.toContain("sk-AbCdEfGhIjKlMn");
        });
    });

    describe("GitHub tokens", () => {
        it("redacts github_pat_* fine-grained PATs", () => {
            const log = "Sending github_pat_11ABCDEFG0_supersecrettokencharactershere to API";
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("<GITHUB_PAT_REDACTED>");
            expect(sanitized).not.toContain("github_pat_11ABC");
        });

        it("redacts ghp_* (classic personal access)", () => {
            const log = "Authorization: token ghp_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789";
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("<GITHUB_TOKEN_REDACTED>");
            expect(sanitized).not.toContain("ghp_AbCd");
        });

        it("redacts gho_* (OAuth) and ghs_* (server-to-server) and ghu_* (user-to-server)", () => {
            const tokens = [
                "gho_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789",
                "ghs_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789",
                "ghu_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789",
                "ghr_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789",
            ];
            for (const token of tokens) {
                const sanitized = sanitizeLogContent(`Token: ${token}`);
                expect(sanitized).toContain("<GITHUB_TOKEN_REDACTED>");
                expect(sanitized).not.toContain(token.slice(0, 12));
            }
        });
    });

    describe("HuggingFace tokens", () => {
        it("redacts hf_* tokens", () => {
            const log = "HF_TOKEN=hf_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789";
            const sanitized = sanitizeLogContent(log);
            // env-var redactor wins (more semantic context preserved)
            expect(sanitized).toBe("HF_TOKEN=<REDACTED:token>");
        });

        it("redacts standalone hf_* tokens not in assignment form", () => {
            const log = "Using model with hf_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789 for download";
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("<HUGGINGFACE_TOKEN_REDACTED>");
            expect(sanitized).not.toContain("hf_AbCd");
        });
    });

    describe("AWS credentials", () => {
        it("redacts AKIA access key IDs", () => {
            const log = "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toBe("AWS_ACCESS_KEY_ID=<REDACTED:aws_access_key>");
        });

        it("redacts standalone AKIA in log narration", () => {
            const log = "Found credentials with id AKIAIOSFODNN7EXAMPLE in env";
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("<AWS_ACCESS_KEY_ID_REDACTED>");
            expect(sanitized).not.toContain("AKIAIOSFODNN");
        });

        it("redacts ASIA temporary credentials", () => {
            const log = "Using temp creds ASIAIOSFODNN7EXAMPLE for STS";
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("<AWS_ACCESS_KEY_ID_REDACTED>");
        });

        it("redacts AWS secret access keys in assignment context", () => {
            const syntheticKey = "wJalrXUtnFEMI/" + "K7MDENG/bPxRfiCYEXAMPLEKEY"; // gitleaks:allow redaction-test fixture
            const log = `aws_secret_access_key=${syntheticKey}`;
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("<REDACTED:aws_secret_access_key>");
            expect(sanitized).not.toContain("wJalrXUtnFEMI");
            expect(sanitized).toContain("aws_secret_access_key");
        });
    });

    describe("Slack tokens", () => {
        it("redacts xoxb (bot) tokens", () => {
            const log = "SLACK_BOT_TOKEN=xoxb-1234567890-abcdefghij-ABCDEFG12345"; // gitleaks:allow redaction-test fixture
            const sanitized = sanitizeLogContent(log);
            // env-var wins
            expect(sanitized).toBe("SLACK_BOT_TOKEN=<REDACTED:token>");
        });

        it("redacts standalone xoxp/xoxr/xoxs", () => {
            for (const prefix of ["xoxp", "xoxr", "xoxs", "xoxa"]) {
                const log = `using ${prefix}-1234567890-abcdefghij-ABCDEFG12345 for slack`;
                const sanitized = sanitizeLogContent(log);
                expect(sanitized).toContain("<SLACK_TOKEN_REDACTED>");
                expect(sanitized).not.toContain(`${prefix}-1234`);
            }
        });
    });

    describe("Google API keys", () => {
        it("redacts AIza* keys", () => {
            // 4 + 31 characters after the prefix satisfy the 35-character Google key shape.
            const fixture = `AIza${"SyD-"}${"x".repeat(31)}`;
            const log = `Calling Maps API with ${fixture} then done`;
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("<GOOGLE_API_KEY_REDACTED>");
            expect(sanitized).not.toContain("AIzaSyD-x");
        });
    });

    describe("Generic env-var assignments", () => {
        it("redacts FOO_API_KEY=value", () => {
            const sanitized = sanitizeLogContent("MY_CUSTOM_API_KEY=abcdef12345");
            expect(sanitized).toBe("MY_CUSTOM_API_KEY=<REDACTED:api_key>");
        });

        it("redacts BAR_TOKEN=value", () => {
            const sanitized = sanitizeLogContent("DATABASE_TOKEN=tokenvaluehere");
            expect(sanitized).toBe("DATABASE_TOKEN=<REDACTED:token>");
        });

        it("redacts BAZ_SECRET=value", () => {
            const sanitized = sanitizeLogContent("MY_SECRET=mysecretvalue");
            expect(sanitized).toBe("MY_SECRET=<REDACTED:secret>");
        });

        it("redacts QUX_PASSWORD=value", () => {
            const sanitized = sanitizeLogContent("DB_PASSWORD=hunter2");
            expect(sanitized).toBe("DB_PASSWORD=<REDACTED:password>");
        });

        it("redacts COMPOUND_CREDENTIAL=value", () => {
            const sanitized = sanitizeLogContent("AUTH_CREDENTIAL=abcdef");
            expect(sanitized).toBe("AUTH_CREDENTIAL=<REDACTED:auth_credential>");
        });

        it("redacts PRIVATE_KEY assignments", () => {
            const sanitized = sanitizeLogContent("MY_PRIVATE_KEY=mykey-data");
            expect(sanitized).toBe("MY_PRIVATE_KEY=<REDACTED:private_key>");
        });

        it("does NOT redact non-secret env vars", () => {
            const sanitized = sanitizeLogContent("OPENCODE_VERSION=1.4.0\nNODE_ENV=production");
            expect(sanitized).toContain("OPENCODE_VERSION=1.4.0");
            expect(sanitized).toContain("NODE_ENV=production");
        });
    });

    describe("JSON-style secret assignments", () => {
        it('redacts "api_key": "value"', () => {
            const log = '{"api_key": "abc123secret"}';
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain('"api_key": "<REDACTED:api_key>"');
            expect(sanitized).not.toContain("abc123secret");
        });

        it('redacts "access_token": "value"', () => {
            const log = '{"access_token": "supersecret"}';
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain('"access_token": "<REDACTED:access_token>"');
        });

        it('redacts "client_secret": "value"', () => {
            const log = '{"client_secret":"abc"}';
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain('"client_secret":"<REDACTED:client_secret>"');
        });

        it('redacts "password": "value" case-insensitively', () => {
            const log = '{"Password": "hunter2"}';
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain('"<REDACTED:password>"');
            expect(sanitized).not.toContain("hunter2");
        });
    });

    describe("Bearer tokens in HTTP headers", () => {
        it("redacts Authorization: Bearer * keeping the prefix", () => {
            const syntheticToken = ["eyJhbGciOiJIUzI1NiJ9", "signature"].join("."); // gitleaks:allow redaction-test fixture
            const log = `Authorization: Bearer ${syntheticToken}`;
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("Authorization:");
            expect(sanitized).toContain("Bearer");
            expect(sanitized).toContain("<REDACTED:bearer>");
            expect(sanitized).not.toContain(syntheticToken);
        });

        it("handles case-insensitive header name", () => {
            const log = "authorization: bearer abcdefghij1234567890";
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("<REDACTED:bearer>");
        });
    });

    describe("JWT tokens", () => {
        it("redacts a three-segment JWT", () => {
            const syntheticJwt = [
                "eyJhbGciOiJIUzI1NiJ9",
                "eyJzdWIiOiJ1c2VyIn0",
                "dozjgNryP4J3jVmNHl0w5N_XgL1JxXYbXvpvYTByA",
            ].join(".");
            const log = `Got JWT: ${syntheticJwt} in response`;
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("<JWT_REDACTED>");
            expect(sanitized).not.toContain("eyJhbGciOiJIUzI1NiJ9");
        });

        it("does not redact arbitrary base64 strings without JWT prefix", () => {
            const log = "computed digest: abcdefgh.ijklmnop.qrstuvwx";
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("abcdefgh.ijklmnop.qrstuvwx");
            expect(sanitized).not.toContain("<JWT_REDACTED>");
        });
    });

    describe("Path and username redaction (existing behavior preserved)", () => {
        it("still redacts /Users/<name>/ paths", () => {
            const sanitized = sanitizeLogContent("File at /Users/alice/code/file.ts");
            expect(sanitized).toContain("/Users/<USER>/");
            expect(sanitized).not.toContain("/Users/alice/");
        });

        it("still redacts /home/<name>/ paths on Linux-style logs", () => {
            const sanitized = sanitizeLogContent("File at /home/bob/code/file.ts");
            expect(sanitized).toContain("/home/<USER>/");
        });

        it("still redacts C:\\Users\\<name>\\ paths on Windows-style logs", () => {
            const sanitized = sanitizeLogContent("File at C:\\Users\\charlie\\code");
            expect(sanitized).toContain("C:\\Users\\<USER>\\");
        });
    });

    describe("Combined sanitization (paths + secrets)", () => {
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

    describe("Empty/safe inputs", () => {
        it("returns empty string unchanged", () => {
            expect(sanitizeLogContent("")).toBe("");
        });

        it("returns plain text without secrets unchanged", () => {
            const log = "This is a normal log line with no secrets in it.";
            expect(sanitizeLogContent(log)).toBe(log);
        });

        it("does not over-redact a legitimate model name", () => {
            const log = "Using model anthropic/claude-haiku-4-5 for historian";
            const sanitized = sanitizeLogContent(log);
            expect(sanitized).toContain("anthropic/claude-haiku-4-5");
        });
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
                    compactionEnabled: true,
                    nativeCompaction: { auto: false, prune: false },
                },
                logFile: { path: join(root, "missing.log"), exists: false, sizeKb: 0 },
                recentSessions: [],
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
            expect(body).toContain(
                '"title": "Problem at /Users/<USER>/private token=<REDACTED:token>"',
            );
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
