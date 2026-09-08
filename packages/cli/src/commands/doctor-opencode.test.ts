import { afterEach, describe, expect, it, spyOn } from "bun:test";
import {
    chmodSync,
    existsSync,
    mkdirSync,
    mkdtempSync,
    readFileSync,
    rmSync,
    writeFileSync,
} from "node:fs";
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

function captureDoctorLog(): { errors: string[]; successes: string[]; restore: () => void } {
    const errors: string[] = [];
    const successes: string[] = [];
    const errorSpy = spyOn(log, "error").mockImplementation((message: string) => {
        errors.push(message);
    });
    const successSpy = spyOn(log, "success").mockImplementation((message: string) => {
        successes.push(message);
    });
    return {
        errors,
        successes,
        restore: () => {
            errorSpy.mockRestore();
            successSpy.mockRestore();
        },
    };
}

function writeJsonc(path: string, value: unknown): void {
    writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}

const REGISTERED_TUI = { plugin: ["@eidnara/opencode"] };
// `detectConflicts` treats an absent `compaction` block as `auto: true`.
const REGISTERED_PLUGIN = {
    plugin: ["@eidnara/opencode"],
    compaction: { auto: false, prune: false },
};
const CONFLICTING_PLUGIN = { plugin: ["@eidnara/opencode"], compaction: { auto: true } };
const isRoot = typeof process.getuid === "function" && process.getuid() === 0;

describe("doctor OpenCode conflict repair", () => {
    it("reports a native compaction conflict, repairs it only under --force, and never touches the network", async () => {
        const { configDir, opencodeConfigPath } = installIsolatedHome();
        writeJsonc(opencodeConfigPath, CONFLICTING_PLUGIN);
        writeJsonc(join(configDir, "tui.jsonc"), { plugin: ["@eidnara/opencode@latest"] });
        const fetchCalls = installThrowingFetch();

        const { errors, successes, restore } = captureDoctorLog();

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
            restore();
        }
    });

    it("leaves native compaction on under --force while the server plugin is unregistered", async () => {
        const { configDir, opencodeConfigPath } = installIsolatedHome();
        writeJsonc(opencodeConfigPath, { plugin: [], compaction: { auto: true } });
        writeJsonc(join(configDir, "tui.jsonc"), REGISTERED_TUI);
        const cwd = makeTempDir("eidnara-doctor-project-");
        const { errors, successes, restore } = captureDoctorLog();

        try {
            const code = await runDoctor({ force: true, cwd });

            expect(code).toBe(1);
            expect(successes.some((message) => message.startsWith("Fixed:"))).toBe(false);
            expect(errors).toContain(
                "Plugin @eidnara/opencode is not registered in opencode.jsonc",
            );
            expect(
                errors.some((message) => message.startsWith("Leaving conflicts in place:")),
            ).toBe(true);
            const untouched = parseJsonc(readFileSync(opencodeConfigPath, "utf-8")) as {
                compaction?: { auto?: boolean };
            };
            expect(untouched.compaction?.auto).toBe(true);
        } finally {
            restore();
        }
    });

    it("returns 1 when --force repairs a conflict but another failure remains", async () => {
        const { configDir, opencodeConfigPath } = installIsolatedHome();
        writeJsonc(opencodeConfigPath, CONFLICTING_PLUGIN);
        writeJsonc(join(configDir, "tui.jsonc"), { plugin: [] });
        const cwd = makeTempDir("eidnara-doctor-project-");
        const { errors, successes, restore } = captureDoctorLog();

        try {
            const code = await runDoctor({ force: true, cwd });

            expect(code).toBe(1);
            expect(successes).toContain("Fixed: Disabled auto-compaction");
            expect(errors).toContain(
                "TUI sidebar plugin @eidnara/opencode is not registered in tui.jsonc",
            );
        } finally {
            restore();
        }
    });

    it.skipIf(isRoot)(
        "reports a failed --force repair and returns 1 instead of throwing",
        async () => {
            const { configDir, opencodeConfigPath } = installIsolatedHome();
            writeJsonc(opencodeConfigPath, CONFLICTING_PLUGIN);
            writeJsonc(join(configDir, "tui.jsonc"), REGISTERED_TUI);
            chmodSync(opencodeConfigPath, 0o444);
            const cwd = makeTempDir("eidnara-doctor-project-");
            const { errors, successes, restore } = captureDoctorLog();

            try {
                const code = await runDoctor({ force: true, cwd });

                expect(code).toBe(1);
                expect(
                    errors.some((message) => message.startsWith("Conflict repair failed:")),
                ).toBe(true);
                expect(successes.some((message) => message.startsWith("Fixed:"))).toBe(false);
                const untouched = parseJsonc(readFileSync(opencodeConfigPath, "utf-8")) as {
                    compaction?: { auto?: boolean };
                };
                expect(untouched.compaction?.auto).toBe(true);
            } finally {
                restore();
            }
        },
    );
});

describe("doctor OpenCode read-only checks", () => {
    it("reports a missing TUI entry without creating tui.json, and leaves a bare entry unchanged", async () => {
        const { configDir, opencodeConfigPath } = installIsolatedHome();
        writeJsonc(opencodeConfigPath, REGISTERED_PLUGIN);
        const cwd = makeTempDir("eidnara-doctor-project-");
        const tuiConfigPath = join(configDir, "tui.jsonc");
        const { errors, restore } = captureDoctorLog();

        try {
            const missingCode = await runDoctor({ cwd });

            expect(missingCode).toBe(1);
            expect(
                errors.some((message) =>
                    message.startsWith("TUI sidebar plugin @eidnara/opencode is not registered"),
                ),
            ).toBe(true);
            expect(existsSync(tuiConfigPath)).toBe(false);
            expect(existsSync(join(configDir, "tui.json"))).toBe(false);

            const bareEntry = `${JSON.stringify(REGISTERED_TUI, null, 2)}\n`;
            writeFileSync(tuiConfigPath, bareEntry);
            errors.length = 0;

            expect(await runDoctor({ cwd })).toBe(0);
            expect(await runDoctor({ force: true, cwd })).toBe(0);
            expect(readFileSync(tuiConfigPath, "utf-8")).toBe(bareEntry);
            expect(errors).toEqual([]);
        } finally {
            restore();
        }
    });

    it("fails when opencode.jsonc cannot be parsed", async () => {
        const { configDir, opencodeConfigPath } = installIsolatedHome();
        writeFileSync(opencodeConfigPath, '{ "plugin": ["@eidnara/opencode"], \n');
        writeJsonc(join(configDir, "tui.jsonc"), REGISTERED_TUI);
        const cwd = makeTempDir("eidnara-doctor-project-");
        const { errors, restore } = captureDoctorLog();

        try {
            const code = await runDoctor({ cwd });

            expect(code).toBe(1);
            expect(
                errors.some((message) =>
                    message.startsWith("Could not parse opencode.jsonc to verify the Plugin entry"),
                ),
            ).toBe(true);
        } finally {
            restore();
        }
    });

    it.each([
        "eidnara.jsonc",
        "eidnara.json",
    ])("fails on a malformed project-only %s", async (fileName) => {
        const { configDir, opencodeConfigPath } = installIsolatedHome();
        writeJsonc(opencodeConfigPath, REGISTERED_PLUGIN);
        writeJsonc(join(configDir, "tui.jsonc"), REGISTERED_TUI);
        const cwd = makeTempDir("eidnara-doctor-project-");
        mkdirSync(join(cwd, ".eidnara"), { recursive: true });
        const projectConfigPath = join(cwd, ".eidnara", fileName);
        writeFileSync(projectConfigPath, '{ "historian": { \n');
        const { errors, successes, restore } = captureDoctorLog();

        try {
            const code = await runDoctor({ cwd });

            expect(code).toBe(1);
            expect(
                errors.some((message) =>
                    message.startsWith(`Eidnara project ${fileName} parse failed:`),
                ),
            ).toBe(true);
            expect(successes).toContain(`Eidnara project config: ${projectConfigPath}`);
        } finally {
            restore();
        }
    });

    it("prints loader warnings for a project config that names a removed key", async () => {
        const { configDir, opencodeConfigPath } = installIsolatedHome();
        writeJsonc(opencodeConfigPath, REGISTERED_PLUGIN);
        writeJsonc(join(configDir, "tui.jsonc"), REGISTERED_TUI);
        const cwd = makeTempDir("eidnara-doctor-project-");
        mkdirSync(join(cwd, ".eidnara"), { recursive: true });
        writeJsonc(join(cwd, ".eidnara", "eidnara.jsonc"), { dreamer: { enabled: true } });
        const warnings: string[] = [];
        const warnSpy = spyOn(log, "warn").mockImplementation((message: string) => {
            warnings.push(message);
        });
        const { errors, restore } = captureDoctorLog();

        try {
            const code = await runDoctor({ cwd });

            expect(code).toBe(0);
            expect(errors).toEqual([]);
            expect(
                warnings.some(
                    (message) =>
                        message.startsWith("[project config]") && message.includes('"dreamer"'),
                ),
            ).toBe(true);
        } finally {
            warnSpy.mockRestore();
            restore();
        }
    });

    it("keeps native compaction on when Eidnara is disabled with enabled: false", async () => {
        const { configDir, opencodeConfigPath } = installIsolatedHome();
        writeJsonc(opencodeConfigPath, CONFLICTING_PLUGIN);
        writeJsonc(join(configDir, "tui.jsonc"), REGISTERED_TUI);
        const eidnaraDir = join(configDir, "..", "eidnara");
        mkdirSync(eidnaraDir, { recursive: true });
        writeJsonc(join(eidnaraDir, "eidnara.jsonc"), { enabled: false });
        const { errors, successes, restore } = captureDoctorLog();

        try {
            const code = await runDoctor({ force: true });

            expect(code).toBe(0);
            expect(errors.some((message) => message.startsWith("Conflict:"))).toBe(false);
            expect(successes.some((message) => message.startsWith("Fixed:"))).toBe(false);
            const untouched = parseJsonc(readFileSync(opencodeConfigPath, "utf-8")) as {
                compaction?: { auto?: boolean };
            };
            expect(untouched.compaction?.auto).toBe(true);
        } finally {
            restore();
        }
    });
});
