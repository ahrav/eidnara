import { afterEach, describe, expect, it, setDefaultTimeout } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    collectDiagnostics,
    piSessionIdFromFileName,
    renderDiagnosticsMarkdown,
    sanitizeValue,
} from "./diagnostics-pi";

setDefaultTimeout(15_000);

const tempRoots: string[] = [];
const originalHome = process.env.HOME;
const originalPiDir = process.env.PI_CODING_AGENT_DIR;
const originalDataHome = process.env.XDG_DATA_HOME;
const originalCacheHome = process.env.XDG_CACHE_HOME;
const originalConfigHome = process.env.XDG_CONFIG_HOME;
const originalPath = process.env.PATH;

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
    if (originalPath === undefined) delete process.env.PATH;
    else process.env.PATH = originalPath;

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
});

describe("piSessionIdFromFileName", () => {
    it("strips Pi's timestamp prefix and the .jsonl suffix", () => {
        expect(
            piSessionIdFromFileName(
                "2026-08-07T06-17-24-707Z_019fdade-87e3-7657-9ce7-65bee79b08e3.jsonl",
            ),
        ).toBe("019fdade-87e3-7657-9ce7-65bee79b08e3");
    });

    it("keeps underscores inside a caller-chosen id", () => {
        expect(piSessionIdFromFileName("2026-08-07T06-17-24-707Z_my_session.jsonl")).toBe(
            "my_session",
        );
    });

    it("returns a file stem without a timestamp prefix unchanged", () => {
        expect(piSessionIdFromFileName("customsession.jsonl")).toBe("customsession");
    });
});

describe("collectDiagnostics Pi path resolution", () => {
    it("reads recent sessions from PI_CODING_AGENT_DIR instead of HOME/.pi/agent", async () => {
        const root = makeTempRoot();
        const home = join(root, "home");
        const cwd = join(root, "workspace");
        const agentDir = join(root, "isolated", "agent");
        process.env.HOME = home;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_DATA_HOME = join(root, "data");
        process.env.XDG_CACHE_HOME = join(root, "cache");
        process.env.XDG_CONFIG_HOME = join(root, "config");

        mkdirSync(cwd, { recursive: true });
        mkdirSync(join(cwd, ".eidnara"), { recursive: true });
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(join(process.env.XDG_CONFIG_HOME, "eidnara"), { recursive: true });

        writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ packages: [] }));
        writeFileSync(
            join(process.env.XDG_CONFIG_HOME, "eidnara", "eidnara.jsonc"),
            JSON.stringify({}),
        );
        writeFileSync(join(cwd, ".eidnara", "eidnara.jsonc"), JSON.stringify({}));

        const customSessionId = "customsession";
        const customSlugDir = join(agentDir, "sessions", "--tmp-eidnaradiagnosticproject--");
        mkdirSync(customSlugDir, { recursive: true });
        // The slug for `/tmp/eidnaradiagnosticproject` is exact; the header's
        // `cwd` names a hyphenated directory the slug cannot round-trip.
        writeFileSync(
            join(customSlugDir, `2026-07-07T12-00-00-000Z_${customSessionId}.jsonl`),
            `${JSON.stringify({ type: "session", version: 3, id: customSessionId, cwd: "/tmp/my-hyphenated-project" })}\n`,
        );

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
                directory: "/tmp/my-hyphenated-project",
                lastActiveAt: report.recentSessions[0]?.lastActiveAt,
            },
        ]);
    });

    it("falls back to the slug directory when a session file has no header", async () => {
        const root = makeTempRoot();
        const home = join(root, "home");
        const cwd = join(root, "workspace");
        const agentDir = join(root, "agent");
        process.env.HOME = home;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_DATA_HOME = join(root, "data");
        process.env.XDG_CACHE_HOME = join(root, "cache");
        process.env.XDG_CONFIG_HOME = join(root, "config");
        mkdirSync(join(cwd, ".eidnara"), { recursive: true });
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(join(process.env.XDG_CONFIG_HOME, "eidnara"), { recursive: true });
        writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ packages: [] }));
        const slugDir = join(agentDir, "sessions", "--tmp-plainproject--");
        mkdirSync(slugDir, { recursive: true });
        writeFileSync(join(slugDir, "2026-07-07T12-00-00-000Z_headerless.jsonl"), "not json\n");

        const report = await collectDiagnostics(cwd);

        expect(report.recentSessions).toEqual([
            {
                sessionId: "headerless",
                directory: "/tmp/plainproject",
                lastActiveAt: report.recentSessions[0]?.lastActiveAt,
            },
        ]);
    });

    it("reports a local checkout as the registered package and normalizes the Pi version", async () => {
        const root = makeTempRoot();
        const home = join(root, "home");
        const cwd = join(root, "workspace");
        const agentDir = join(root, "agent");
        process.env.HOME = home;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_DATA_HOME = join(root, "data");
        process.env.XDG_CACHE_HOME = join(root, "cache");
        process.env.XDG_CONFIG_HOME = join(root, "config");
        mkdirSync(join(cwd, ".eidnara"), { recursive: true });
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(join(process.env.XDG_CONFIG_HOME, "eidnara"), { recursive: true });
        const checkout = join(root, "eidnara-pi-checkout");
        mkdirSync(checkout, { recursive: true });
        writeFileSync(join(checkout, "package.json"), JSON.stringify({ name: "@eidnara/pi" }));
        writeFileSync(
            join(agentDir, "settings.json"),
            JSON.stringify({ packages: ["./../eidnara-pi-checkout", "npm:other-pi-extension"] }),
        );
        const binDir = join(root, "bin");
        mkdirSync(binDir, { recursive: true });
        const pi = join(binDir, "pi");
        writeFileSync(
            pi,
            `#!/bin/sh\necho "warning: config at ${home}/.pi/agent token=abc123"\necho 0.74.0\n`,
        );
        chmodSync(pi, 0o755);
        process.env.PATH = binDir;

        const report = await collectDiagnostics(cwd);

        expect(report.settings.hasEidnaraPackage).toBe(true);
        expect(report.conflicts.otherPiExtensions).toEqual(["npm:other-pi-extension"]);
        expect(report.piVersion).toBe("0.74.0");
        expect(renderDiagnosticsMarkdown(report)).not.toContain("abc123");
    });

    it("reports discovery as unavailable when every session directory is unreadable", async () => {
        if (typeof process.getuid === "function" && process.getuid() === 0) return;
        const root = makeTempRoot();
        const home = join(root, "home");
        const cwd = join(root, "workspace");
        const agentDir = join(root, "agent");
        process.env.HOME = home;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_DATA_HOME = join(root, "data");
        process.env.XDG_CACHE_HOME = join(root, "cache");
        process.env.XDG_CONFIG_HOME = join(root, "config");
        mkdirSync(join(cwd, ".eidnara"), { recursive: true });
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(join(process.env.XDG_CONFIG_HOME, "eidnara"), { recursive: true });
        writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ packages: [] }));
        const slugDir = join(agentDir, "sessions", "--tmp-lockedproject--");
        mkdirSync(slugDir, { recursive: true });
        writeFileSync(join(slugDir, "2026-07-07T12-00-00-000Z_hidden.jsonl"), "{}\n");
        chmodSync(slugDir, 0o000);

        try {
            const report = await collectDiagnostics(cwd);
            expect(report.recentSessions).toEqual([]);
            expect(report.sessionDiscovery).toBe("unavailable");
        } finally {
            chmodSync(slugDir, 0o755);
        }
    });

    it("sanitizes config parse errors so the issue body does not carry local paths", async () => {
        const root = makeTempRoot();
        const home = join(root, "home");
        const cwd = join(home, "workspace");
        const agentDir = join(root, "agent");
        process.env.HOME = home;
        process.env.PI_CODING_AGENT_DIR = agentDir;
        process.env.XDG_DATA_HOME = join(root, "data");
        process.env.XDG_CACHE_HOME = join(root, "cache");
        process.env.XDG_CONFIG_HOME = join(home, ".config");

        mkdirSync(join(cwd, ".eidnara"), { recursive: true });
        mkdirSync(agentDir, { recursive: true });
        mkdirSync(join(process.env.XDG_CONFIG_HOME, "eidnara"), { recursive: true });
        writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ packages: [] }));
        writeFileSync(join(process.env.XDG_CONFIG_HOME, "eidnara", "eidnara.jsonc"), "{ nope");
        writeFileSync(join(cwd, ".eidnara", "eidnara.jsonc"), "[1]");

        const report = await collectDiagnostics(cwd);

        expect(report.userConfig.parseError).toContain("<HOME>/.config/eidnara/eidnara.jsonc");
        expect(report.projectConfig.parseError).toContain(
            "<HOME>/workspace/.eidnara/eidnara.jsonc",
        );

        const markdown = renderDiagnosticsMarkdown(report);
        expect(markdown).toContain("User config parse error: Refusing to overwrite");
        expect(markdown).not.toContain(home);
    });
});
