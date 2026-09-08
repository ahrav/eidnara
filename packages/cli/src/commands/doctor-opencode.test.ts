import { afterEach, describe, expect, it, spyOn } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parse as parseJsonc } from "comment-json";
import { log } from "../lib/prompts";
import { runDoctor } from "./doctor-opencode";

const ENV_KEYS = [
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "PATH",
    "OPENCODE_CONFIG_DIR",
    "OPENCODE_DISABLE_AUTOCOMPACT",
] as const;

const tempDirs: string[] = [];
const originalEnv = new Map<string, string | undefined>();
const originalFetch = globalThis.fetch;

function makeTempDir(prefix = "eidnara-doctor-"): string {
    const dir = mkdtempSync(join(tmpdir(), prefix));
    tempDirs.push(dir);
    return dir;
}

function snapshotEnv(): void {
    for (const key of ENV_KEYS) originalEnv.set(key, process.env[key]);
}

afterEach(() => {
    for (const [key, value] of originalEnv) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
    originalEnv.clear();
    globalThis.fetch = originalFetch;
    for (const dir of tempDirs.splice(0)) {
        rmSync(dir, { recursive: true, force: true });
    }
});

/**
 * The fixture isolates every path doctor touches: `HOME`, the XDG config and
 * data roots, and `PATH` (which carries only a fake `opencode` binary so the
 * host's real installation cannot leak into detection).
 */
function installIsolatedHome(): { configDir: string; opencodeConfigPath: string } {
    snapshotEnv();
    const root = makeTempDir();
    const binDir = join(root, "bin");
    const configHome = join(root, ".config");
    const configDir = join(configHome, "opencode");
    mkdirSync(binDir, { recursive: true });
    mkdirSync(configDir, { recursive: true });

    const fakeOpenCode = join(binDir, "opencode");
    writeFileSync(fakeOpenCode, "#!/bin/sh\necho 1.0.0\n");
    chmodSync(fakeOpenCode, 0o755);

    process.env.HOME = root;
    process.env.XDG_CONFIG_HOME = configHome;
    process.env.XDG_DATA_HOME = join(root, ".local", "share");
    process.env.PATH = binDir;
    delete process.env.OPENCODE_CONFIG_DIR;
    delete process.env.OPENCODE_DISABLE_AUTOCOMPACT;

    const opencodeConfigPath = join(configDir, "opencode.jsonc");
    return { configDir, opencodeConfigPath };
}

function installThrowingFetch(): unknown[] {
    const fetchCalls: unknown[] = [];
    globalThis.fetch = ((input: unknown) => {
        fetchCalls.push(input);
        throw new Error("network access is disabled in this test");
    }) as unknown as typeof fetch;
    return fetchCalls;
}

describe("doctor OpenCode conflict repair", () => {
    it("reports a native compaction conflict, repairs it only under --force, and never touches the network", async () => {
        const { configDir, opencodeConfigPath } = installIsolatedHome();
        writeFileSync(
            opencodeConfigPath,
            `${JSON.stringify(
                { plugin: ["@eidnara/opencode"], compaction: { auto: true } },
                null,
                2,
            )}\n`,
        );
        // `ensureTuiPluginEntry` leaves this exact entry alone, so the TUI check cannot count as a fix.
        writeFileSync(
            join(configDir, "tui.jsonc"),
            `${JSON.stringify({ plugin: ["@eidnara/opencode@latest"] }, null, 2)}\n`,
        );
        const fetchCalls = installThrowingFetch();

        const errors: string[] = [];
        const successes: string[] = [];
        const errorSpy = spyOn(log, "error").mockImplementation((message: string) => {
            errors.push(message);
        });
        const successSpy = spyOn(log, "success").mockImplementation((message: string) => {
            successes.push(message);
        });

        try {
            const detectCode = await runDoctor({});

            expect(detectCode).toBe(1);
            expect(errors).toContain(
                "Conflict: OpenCode auto-compaction is enabled (compaction.auto=true)",
            );
            expect(successes.some((message) => message.startsWith("Fixed:"))).toBe(false);
            const untouched = parseJsonc(readFileSync(opencodeConfigPath, "utf-8")) as {
                compaction?: { auto?: boolean };
            };
            expect(untouched.compaction?.auto).toBe(true);

            errors.length = 0;
            successes.length = 0;

            const repairCode = await runDoctor({ force: true });

            expect(repairCode).toBe(0);
            expect(errors).toContain(
                "Conflict: OpenCode auto-compaction is enabled (compaction.auto=true)",
            );
            expect(successes).toContain("Fixed: Disabled auto-compaction");
            const repaired = parseJsonc(readFileSync(opencodeConfigPath, "utf-8")) as {
                plugin?: unknown[];
                compaction?: { auto?: boolean; prune?: boolean };
            };
            expect(repaired.compaction?.auto).toBe(false);
            expect(repaired.compaction?.prune).toBe(false);
            expect(repaired.plugin).toEqual(["@eidnara/opencode"]);

            errors.length = 0;
            const cleanCode = await runDoctor({});
            expect(cleanCode).toBe(0);
            expect(errors.some((message) => message.startsWith("Conflict:"))).toBe(false);

            expect(fetchCalls).toEqual([]);
        } finally {
            errorSpy.mockRestore();
            successSpy.mockRestore();
        }
    });

    it("reports a missing TUI sidebar entry without writing tui.jsonc, and adds it only under --force", async () => {
        const { configDir, opencodeConfigPath } = installIsolatedHome();
        writeFileSync(
            opencodeConfigPath,
            `${JSON.stringify(
                { plugin: ["@eidnara/opencode"], compaction: { auto: false, prune: false } },
                null,
                2,
            )}\n`,
        );
        const tuiConfigPath = join(configDir, "tui.jsonc");
        writeFileSync(tuiConfigPath, `${JSON.stringify({ plugin: [] }, null, 2)}\n`);
        const before = readFileSync(tuiConfigPath, "utf-8");

        const errors: string[] = [];
        const successes: string[] = [];
        const errorSpy = spyOn(log, "error").mockImplementation((message: string) => {
            errors.push(message);
        });
        const successSpy = spyOn(log, "success").mockImplementation((message: string) => {
            successes.push(message);
        });

        try {
            const detectCode = await runDoctor({});

            expect(detectCode).toBe(1);
            expect(errors).toContain("TUI sidebar plugin is not registered in tui.json");
            expect(readFileSync(tuiConfigPath, "utf-8")).toBe(before);

            errors.length = 0;
            const repairCode = await runDoctor({ force: true });

            expect(repairCode).toBe(0);
            expect(successes).toContain("Added TUI sidebar plugin to tui.json");
            const repaired = parseJsonc(readFileSync(tuiConfigPath, "utf-8")) as {
                plugin?: unknown[];
            };
            expect(repaired.plugin).toEqual(["@eidnara/opencode@latest"]);
        } finally {
            errorSpy.mockRestore();
            successSpy.mockRestore();
        }
    });

    it("exits 1 under --force when a failure remains after the conflict repair", async () => {
        const { configDir, opencodeConfigPath } = installIsolatedHome();
        // The compaction conflict is repairable; the missing plugin entry is not.
        writeFileSync(
            opencodeConfigPath,
            `${JSON.stringify({ plugin: [], compaction: { auto: true } }, null, 2)}\n`,
        );
        writeFileSync(
            join(configDir, "tui.jsonc"),
            `${JSON.stringify({ plugin: ["@eidnara/opencode@latest"] }, null, 2)}\n`,
        );

        const errors: string[] = [];
        const successes: string[] = [];
        const errorSpy = spyOn(log, "error").mockImplementation((message: string) => {
            errors.push(message);
        });
        const successSpy = spyOn(log, "success").mockImplementation((message: string) => {
            successes.push(message);
        });

        try {
            const repairCode = await runDoctor({ force: true });

            expect(repairCode).toBe(1);
            expect(successes).toContain("Fixed: Disabled auto-compaction");
            expect(
                errors.some((message) =>
                    message.startsWith("Plugin @eidnara/opencode is not registered"),
                ),
            ).toBe(true);
        } finally {
            errorSpy.mockRestore();
            successSpy.mockRestore();
        }
    });
});
