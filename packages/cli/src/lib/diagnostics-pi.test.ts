import { afterEach, describe, expect, it, setDefaultTimeout } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { collectDiagnostics, sanitizeValue } from "./diagnostics-pi";

setDefaultTimeout(15_000);

const tempRoots: string[] = [];
const originalHome = process.env.HOME;
const originalPiDir = process.env.PI_CODING_AGENT_DIR;
const originalDataHome = process.env.XDG_DATA_HOME;
const originalCacheHome = process.env.XDG_CACHE_HOME;
const originalConfigHome = process.env.XDG_CONFIG_HOME;

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

    it("keeps only presence and length for prompt prose", () => {
        expect(
            sanitizeValue({
                historian: { prompt: "Summarize the last sprint", model: "claude" },
                sidekick: { system_prompt: "Be terse" },
                prompt_surface: {
                    default: "light",
                    tool_descriptions: { ctx_search: "Find prior notes" },
                },
            }),
        ).toEqual({
            historian: { prompt: "<REDACTED 25 chars>", model: "claude" },
            sidekick: { system_prompt: "<REDACTED 8 chars>" },
            prompt_surface: {
                default: "light",
                tool_descriptions: { ctx_search: "<REDACTED 16 chars>" },
            },
        });
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
