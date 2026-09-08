import { afterEach, describe, expect, it, setDefaultTimeout } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    collectDiagnostics,
    type PiDiagnosticReport,
    renderDiagnosticsMarkdown,
    sanitizeString,
    sanitizeValue,
} from "./diagnostics-pi";

setDefaultTimeout(15_000);

const tempRoots: string[] = [];
const originalHome = process.env.HOME;
const originalPiDir = process.env.PI_CODING_AGENT_DIR;
const originalDataHome = process.env.XDG_DATA_HOME;
const originalCacheHome = process.env.XDG_CACHE_HOME;
const originalConfigHome = process.env.XDG_CONFIG_HOME;
const originalLogPath = process.env.EIDNARA_LOG_PATH;

function makeTempRoot(prefix = "eidnara-pi-diagnostics-"): string {
    const root = mkdtempSync(join(tmpdir(), prefix));
    tempRoots.push(root);
    return root;
}

afterEach(() => {
    if (originalHome === undefined) delete process.env.HOME;
    else process.env.HOME = originalHome;
    if (originalPiDir === undefined) delete process.env.PI_CODING_AGENT_DIR;
    else process.env.PI_CODING_AGENT_DIR = originalPiDir;
    if (originalDataHome === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = originalDataHome;
    if (originalCacheHome === undefined) delete process.env.XDG_CACHE_HOME;
    else process.env.XDG_CACHE_HOME = originalCacheHome;
    if (originalConfigHome === undefined) delete process.env.XDG_CONFIG_HOME;
    else process.env.XDG_CONFIG_HOME = originalConfigHome;
    if (originalLogPath === undefined) delete process.env.EIDNARA_LOG_PATH;
    else process.env.EIDNARA_LOG_PATH = originalLogPath;

    for (const root of tempRoots.splice(0)) {
        rmSync(root, { recursive: true, force: true });
    }
});

describe("sanitizeValue Pi diagnostics redaction", () => {
    it("preserves numeric thresholds while redacting string secrets", () => {
        expect(
            sanitizeValue({
                execute_threshold_tokens: 200000,
                api_key: "sk-x",
            }),
        ).toEqual({
            execute_threshold_tokens: 200000,
            api_key: "<REDACTED>",
        });
    });

    it("redacts every credential-shaped key the shared vocabulary knows, plus cookies", () => {
        expect(
            sanitizeValue({
                credential: "c",
                auth: "a",
                private_key: "p",
                access_key: "k",
                cookie: "session=abc",
                injection_budget_tokens: 4000,
            }),
        ).toEqual({
            credential: "<REDACTED>",
            auth: "<REDACTED>",
            private_key: "<REDACTED>",
            access_key: "<REDACTED>",
            cookie: "<REDACTED>",
            injection_budget_tokens: 4000,
        });
    });

    it("keeps only presence and length for prompt prose", () => {
        expect(
            sanitizeValue({
                historian: { prompt: "Summarize the last sprint", model: "claude" },
                sidekick: { system_prompt: "Be terse" },
                prompt_surface: {
                    default: "light",
                    tool_descriptions: { ctx_search: "Find prior notes" },
                },
                system_prompt_injection: { skip_signatures: ["<!-- eidnara: skip -->", "secret"] },
            }),
        ).toEqual({
            historian: { prompt: "<REDACTED 25 chars>", model: "claude" },
            sidekick: { system_prompt: "<REDACTED 8 chars>" },
            prompt_surface: {
                default: "light",
                tool_descriptions: { ctx_search: "<REDACTED 16 chars>" },
            },
            system_prompt_injection: {
                skip_signatures: ["<REDACTED 22 chars>", "<REDACTED 6 chars>"],
            },
        });
    });
});

describe("sanitizeString home handling", () => {
    it("does not treat a root home directory as a redactable prefix", () => {
        process.env.HOME = "/";
        expect(sanitizeString("https://example.test/a/b")).toBe("https://example.test/a/b");
    });

    it("strips URL userinfo and matches Windows profile paths on any drive", () => {
        process.env.HOME = "/nonexistent/home";
        // Assembled at runtime so the fixture never appears as a credential in source.
        const userinfo = ["alice", "not-a-real-password"].join(":");
        expect(sanitizeString(`clone https://${userinfo}@example.test/repo.git`)).toBe(
            "clone https://<REDACTED>@example.test/repo.git",
        );
        expect(sanitizeString("https://opaque-private-token@example.test/repo")).toBe(
            "https://<REDACTED>@example.test/repo",
        );
        expect(sanitizeString("d:/users/alice/project")).toBe("C:\\Users\\<USER>/project");
        expect(sanitizeString("profile at d:/users/alice")).toBe("profile at C:\\Users\\<USER>");
        expect(sanitizeString("home /home/alice and /Users/alice end")).toBe(
            "home /home/<USER> and /Users/<USER> end",
        );
    });

    it("redacts every Authorization scheme", () => {
        process.env.HOME = "/nonexistent/home";
        // Assembled at runtime so the fixture never appears as a credential in source.
        const basic = `Basic ${Buffer.from("alice:not-a-real-password").toString("base64")}`;
        expect(sanitizeString(`Authorization: ${basic}`)).toBe("Authorization: <REDACTED>");
        expect(sanitizeString("authorization=Token abc.def")).toBe("authorization=<REDACTED>");
        expect(
            sanitizeString(
                "Authorization: AWS4-HMAC-SHA256 Credential=x, SignedHeaders=y, Signature=z",
            ),
        ).toBe("Authorization: <REDACTED>");
        expect(sanitizeString("the Authorization header is missing")).toBe(
            "the Authorization header is missing",
        );
    });

    it("redacts credential headers and colon-delimited secrets in text", () => {
        process.env.HOME = "/nonexistent/home";
        expect(sanitizeString("X-API-Key: opaque-value")).toBe("X-API-Key: <REDACTED>");
        expect(sanitizeString("Cookie: sid=opaque; theme=dark")).toBe("Cookie: <REDACTED>");
        // An unquoted value has no delimiter, so the shared redactor takes the rest of the segment.
        expect(sanitizeString("password: hunter2 and token=abc")).toBe("password: <REDACTED>");
        // Every value is gone even though the shared redactor's plain scalar spans the middle pairs.
        expect(
            sanitizeString("client_secret: live access_key=live credential: live auth: live"),
        ).toBe("client_secret: <REDACTED> <REDACTED> auth: <REDACTED>");
        // `tokens` is a secret label in the shared vocabulary, so the plain scalar after it goes.
        expect(sanitizeString("execute_threshold_tokens: 200000 max_tokens=3 enabled: true")).toBe(
            "execute_threshold_tokens: <REDACTED> true",
        );
        expect(sanitizeString("at 2026-07-07T12:00:01.000Z see https://example.test/x")).toBe(
            "at 2026-07-07T12:00:01.000Z see https://example.test/x",
        );
        expect(sanitizeString('client_secret: "correct horse battery staple" done')).toBe(
            "client_secret: <REDACTED>",
        );
        expect(sanitizeString('client_secret: "prefix\\" LIVE suffix" done')).toBe(
            "client_secret: <REDACTED>",
        );
        expect(sanitizeString("bearer opaque-live-token and BEARER x.y")).toBe(
            "Bearer <REDACTED> and Bearer <REDACTED>",
        );
        // Assembled at runtime so the fixture never appears as a key block in source.
        const pem = [
            "-----BEGIN",
            "PRIVATE KEY-----\nMIIE\nvQIB\n-----END",
            "PRIVATE KEY-----",
        ].join(" ");
        expect(sanitizeString(`${pem} tail`)).toBe("<PRIVATE_KEY_REDACTED> tail");
    });

    it("sanitizes dynamic record keys as well as values", () => {
        process.env.HOME = "/nonexistent/home";
        expect(
            sanitizeValue({
                permission: { bash: { "curl -H 'X-API-Key: live' https://h": "allow" } },
                prompt_surface: { tool_descriptions: { "X-API-Key: live": "desc" } },
            }),
        ).toEqual({
            // A key that names a credential is itself secret, so its value is dropped too.
            permission: { bash: { "curl -H 'X-API-Key: <REDACTED>": "<REDACTED>" } },
            prompt_surface: {
                tool_descriptions: { "X-API-Key: <REDACTED>": "<REDACTED 4 chars>" },
            },
        });
    });
});

describe("renderDiagnosticsMarkdown", () => {
    it("keeps warnings and parse errors on one line and reports an installed Pi without a version", () => {
        const report: PiDiagnosticReport = {
            timestamp: "2026-07-07T12:00:00.000Z",
            platform: "linux",
            arch: "x64",
            nodeVersion: "v24.0.0",
            pluginVersion: "0.1.0",
            piInstalled: true,
            piPath: "/usr/bin/pi",
            piVersion: null,
            settings: {
                path: "/x/settings.json",
                exists: false,
                hasEidnaraPackage: false,
                packages: [],
            },
            configPaths: { agentDir: "/x", userConfig: "/x/u.jsonc", projectConfig: "/x/p.jsonc" },
            userConfig: {
                path: "/x/u.jsonc",
                exists: true,
                parseError: "bad\n## Log (last",
                flags: {},
            },
            projectConfig: { path: "/x/p.jsonc", exists: false, flags: {} },
            loadedConfigPaths: [],
            loadWarnings: ["model key\n## Log (last\nunknown", "lone\r## Log (last"],
            conflicts: { knownConflicts: [], otherPiExtensions: [] },
            logFile: { path: "/x/eidnara.log", exists: false, sizeKb: 0 },
            recentSessions: [],
            historianDumps: {
                byProject: [],
                legacyDumps: { dir: "/x/legacy", count: 0, recent: [] },
            },
        };

        const markdown = renderDiagnosticsMarkdown(report);

        expect(markdown).toContain("- User config parse error: bad ## Log (last");
        expect(markdown).toContain("- model key ## Log (last unknown");
        expect(markdown).toContain("- lone ## Log (last");
        expect(markdown.match(/(^|\r)## Log \(last/gm)).toBeNull();
        expect(markdown).toContain("- Pi installed: true");
    });
});

interface IsolatedEnv {
    home: string;
    cwd: string;
    agentDir: string;
    configHome: string;
}

function isolateEnv(): IsolatedEnv {
    const root = makeTempRoot();
    const home = join(root, "home");
    const cwd = join(root, "workspace");
    const agentDir = join(root, "isolated", "agent");
    const configHome = join(home, ".config");
    process.env.HOME = home;
    process.env.PI_CODING_AGENT_DIR = agentDir;
    process.env.XDG_DATA_HOME = join(root, "data");
    process.env.XDG_CACHE_HOME = join(root, "cache");
    process.env.XDG_CONFIG_HOME = configHome;

    mkdirSync(join(cwd, ".eidnara"), { recursive: true });
    mkdirSync(agentDir, { recursive: true });
    mkdirSync(join(configHome, "eidnara"), { recursive: true });
    writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ packages: [] }));
    return { home, cwd, agentDir, configHome };
}

describe("collectDiagnostics Pi path resolution", () => {
    it("reads recent sessions from PI_CODING_AGENT_DIR instead of HOME/.pi/agent", async () => {
        const { home, cwd, agentDir, configHome } = isolateEnv();
        writeFileSync(join(configHome, "eidnara", "eidnara.jsonc"), JSON.stringify({}));
        writeFileSync(join(cwd, ".eidnara", "eidnara.jsonc"), JSON.stringify({}));

        const customProject = "/tmp/eidnaradiagnosticproject";
        const customSessionId = "2026-07-07T12-00-00-000Z_customsession";
        const customSlugDir = join(agentDir, "sessions", "--tmp-eidnaradiagnosticproject--");
        mkdirSync(customSlugDir, { recursive: true });
        writeFileSync(join(customSlugDir, `${customSessionId}.jsonl`), '{"type":"session"}\n');

        const homeFallbackSlugDir = join(
            home,
            ".pi",
            "agent",
            "sessions",
            "--tmp-homefallbackproject--",
        );
        mkdirSync(homeFallbackSlugDir, { recursive: true });
        writeFileSync(
            join(homeFallbackSlugDir, "2026-07-07T12-00-00-000Z_homesession.jsonl"),
            '{"type":"session"}\n',
        );

        const report = await collectDiagnostics(cwd);

        expect(report.configPaths.agentDir).toBe(agentDir);
        expect(report.recentSessions).toEqual([
            {
                sessionId: customSessionId,
                directory: customProject,
                lastActiveAt: report.recentSessions[0]?.lastActiveAt,
            },
        ]);
    });

    it("takes the project directory from the session header instead of the lossy slug", async () => {
        const { cwd, agentDir } = isolateEnv();
        const slugDir = join(agentDir, "sessions", "--tmp-my-project--");
        mkdirSync(slugDir, { recursive: true });
        writeFileSync(
            join(slugDir, "2026-07-07T12-00-00-000Z_hyphenated.jsonl"),
            `${JSON.stringify({ type: "session", cwd: "/tmp/my-project" })}\n{"type":"message"}\n`,
        );

        const report = await collectDiagnostics(cwd);

        expect(report.recentSessions.map((session) => session.directory)).toEqual([
            "/tmp/my-project",
        ]);
    });

    it("keeps a session launched from the filesystem root and ignores a non-regular log", async () => {
        const { cwd, agentDir } = isolateEnv();
        const rootSlugDir = join(agentDir, "sessions", "----");
        mkdirSync(rootSlugDir, { recursive: true });
        writeFileSync(
            join(rootSlugDir, "2026-07-07T12-00-00-000Z_root.jsonl"),
            '{"type":"session"}\n',
        );
        writeFileSync(join(rootSlugDir, ".jsonl"), '{"type":"session"}\n');
        const logDir = join(cwd, "log-as-directory");
        mkdirSync(logDir);
        process.env.EIDNARA_LOG_PATH = logDir;

        const report = await collectDiagnostics(cwd);

        expect(report.recentSessions.map((session) => session.sessionId)).toEqual([
            "2026-07-07T12-00-00-000Z_root",
        ]);
        expect(report.recentSessions.map((session) => session.directory)).toEqual(["/"]);
        expect(report.logFile).toEqual({ path: logDir, exists: false, sizeKb: 0 });
    });

    it("diagnoses the .json config the Pi loader selects and sanitizes its parse error", async () => {
        const { home, cwd, configHome } = isolateEnv();
        const userJson = join(configHome, "eidnara", "eidnara.json");
        writeFileSync(userJson, "{ not json");

        const report = await collectDiagnostics(cwd);

        expect(report.userConfig.path).toBe(userJson);
        expect(report.userConfig.exists).toBe(true);
        expect(report.userConfig.parseError).toContain("<HOME>/.config/eidnara/eidnara.json");
        expect(report.userConfig.parseError).not.toContain(home);
        expect(report.projectConfig.path).toBe(join(cwd, ".eidnara", "eidnara.jsonc"));
        expect(report.projectConfig.exists).toBe(false);
    });
});
