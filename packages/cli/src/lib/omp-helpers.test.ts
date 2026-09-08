import { afterEach, describe, expect, it } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import {
    detectOmpBinary,
    getOmpCommandInvocation,
    getOmpFallbackCandidates,
    getOmpSetting,
    getOmpVersion,
    listOmpPlugins,
    parseOmpModelsOutput,
    runOmpCommand,
} from "./omp-helpers";

const originalPath = process.env.PATH;
const originalPackageDir = process.env.PI_PACKAGE_DIR;
const originalHome = process.env.HOME;
const roots: string[] = [];

afterEach(() => {
    if (originalPath === undefined) delete process.env.PATH;
    else process.env.PATH = originalPath;
    if (originalPackageDir === undefined) delete process.env.PI_PACKAGE_DIR;
    else process.env.PI_PACKAGE_DIR = originalPackageDir;
    if (originalHome === undefined) delete process.env.HOME;
    else process.env.HOME = originalHome;
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe("OMP binary discovery", () => {
    /* */
    function makePackageRoot(): { root: string; binDir: string } {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-package-"));
        roots.push(root);
        const binDir = join(root, "bin");
        mkdirSync(join(root, "pkg", "dist"), { recursive: true });
        mkdirSync(binDir, { recursive: true });
        writeFileSync(
            join(root, "pkg", "package.json"),
            JSON.stringify({ name: "@oh-my-pi/pi-coding-agent" }),
        );
        writeFileSync(join(root, "pkg", "dist", "cli.js"), "");
        writeFileSync(join(binDir, "bun"), "#!/bin/sh\n");
        chmodSync(join(binDir, "bun"), 0o755);
        process.env.HOME = join(root, "home");
        process.env.PI_PACKAGE_DIR = join(root, "pkg");
        return { root, binDir };
    }

    it("honors a validated PI_PACKAGE_DIR install root when Bun can run it", () => {
        const { root, binDir } = makePackageRoot();
        process.env.PATH = binDir;

        expect(detectOmpBinary()).toEqual({
            path: join(root, "pkg", "dist", "cli.js"),
            source: "package",
        });
    });

    it("ignores the package root when no Bun runtime can execute the CLI script", () => {
        const { root } = makePackageRoot();
        process.env.PATH = join(root, "empty-bin");

        expect(detectOmpBinary()).toBeNull();
    });

    it("finds Bun under ~/.bun/bin when it is absent from PATH", () => {
        const { root } = makePackageRoot();
        const bunBin = join(root, "home", ".bun", "bin");
        mkdirSync(bunBin, { recursive: true });
        writeFileSync(join(bunBin, "bun"), "#!/bin/sh\n");
        chmodSync(join(bunBin, "bun"), 0o755);
        process.env.PATH = join(root, "empty-bin");
        const cli = join(root, "pkg", "dist", "cli.js");

        expect(detectOmpBinary()).toEqual({ path: cli, source: "package" });
        expect(getOmpCommandInvocation(cli, ["--version"])).toEqual({
            command: join(bunBin, "bun"),
            args: [cli, "--version"],
            env: { PATH: `${bunBin}${delimiter}${process.env.PATH}` },
        });
    });

    it("routes a package CLI script through Bun instead of spawning it directly", () => {
        const { root, binDir } = makePackageRoot();
        process.env.PATH = binDir;
        const cli = join(root, "pkg", "dist", "cli.js");

        expect(getOmpCommandInvocation(cli, ["--version"])).toEqual({
            command: join(binDir, "bun"),
            args: [cli, "--version"],
            env: { PATH: `${binDir}${delimiter}${process.env.PATH}` },
        });
    });

    it("runs a native OMP binary directly with its directory on the child PATH", () => {
        const { binDir } = makePackageRoot();
        process.env.PATH = binDir;

        expect(getOmpCommandInvocation("/usr/bin/omp", ["config", "path"])).toEqual({
            command: "/usr/bin/omp",
            args: ["config", "path"],
            env: { PATH: `/usr/bin${delimiter}${binDir}` },
        });
    });
});

describe.if(process.platform !== "win32")("OMP fallback launchers", () => {
    it("runs an env-shebang launcher whose Bun runtime sits beside it, outside the parent PATH", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-bun-"));
        roots.push(root);
        const bunBin = join(root, ".bun", "bin");
        mkdirSync(bunBin, { recursive: true });
        // The stand-in "bun" runtime echoes the script's arguments so the test sees it ran.
        writeFileSync(join(bunBin, "bun"), '#!/bin/sh\nshift\necho "omp/1.2.3 $*"\n');
        chmodSync(join(bunBin, "bun"), 0o755);
        writeFileSync(join(bunBin, "omp"), "#!/usr/bin/env bun\n");
        chmodSync(join(bunBin, "omp"), 0o755);
        process.env.PATH = join(root, "empty-bin");

        expect(getOmpVersion(join(bunBin, "omp"))).toBe("1.2.3");
    });

    it("puts a Bun found under the home directory on the child PATH of a launcher elsewhere", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-bun-"));
        roots.push(root);
        const bunBin = join(root, ".bun", "bin");
        const launcherBin = join(root, "usr-local-bin");
        mkdirSync(bunBin, { recursive: true });
        mkdirSync(launcherBin, { recursive: true });
        writeFileSync(join(bunBin, "bun"), '#!/bin/sh\nshift\necho "omp/1.2.3 $*"\n');
        chmodSync(join(bunBin, "bun"), 0o755);
        writeFileSync(join(launcherBin, "omp"), "#!/usr/bin/env bun\n");
        chmodSync(join(launcherBin, "omp"), 0o755);
        process.env.HOME = root;
        process.env.PATH = join(root, "empty-bin");

        const invocation = getOmpCommandInvocation(join(launcherBin, "omp"), ["--version"]);
        expect(invocation.env?.PATH?.split(delimiter)).toEqual([
            launcherBin,
            bunBin,
            join(root, "empty-bin"),
        ]);
        expect(getOmpVersion(join(launcherBin, "omp"))).toBe("1.2.3");
    });
});

describe("OMP fallback discovery", () => {
    it("covers standard Windows npm and Bun install directories", () => {
        const home = "C:\\Users\\fox";
        const appData = "C:\\Users\\fox\\AppData\\Roaming";
        expect(getOmpFallbackCandidates("win32", home, appData)).toEqual([
            join(appData, "npm", "omp.cmd"),
            join(appData, "npm", "omp.exe"),
            join(home, ".bun", "bin", "omp.exe"),
            join(home, ".bun", "bin", "omp.cmd"),
        ]);
    });
});

describe("OMP model discovery", () => {
    it("parses model selectors without flattening scoped or nested IDs", () => {
        const output = JSON.stringify({
            models: [
                {
                    provider: "anthropic",
                    id: "claude-opus",
                    selector: "anthropic/claude-opus",
                },
                {
                    provider: "modal",
                    id: "@modal/qwen/model-v1",
                    selector: "modal/@modal/qwen/model-v1",
                },
                { provider: "openai", id: "fallback/model" },
            ],
        });

        expect(parseOmpModelsOutput(output)).toEqual([
            "anthropic/claude-opus",
            "modal/@modal/qwen/model-v1",
            "openai/fallback/model",
        ]);
    });
});

describe("OMP command execution", () => {
    it("preserves spawn timeout errors when stderr is empty", () => {
        const result = runOmpCommand(process.execPath, ["-e", "while (true) {}"], 10);
        expect(result.ok).toBe(false);
        expect(result.stderr.length).toBeGreaterThan(0);
    });

    it("captures JSON-sized output above Node's default buffer", () => {
        const result = runOmpCommand(process.execPath, [
            "-e",
            "process.stdout.write('x'.repeat(2 * 1024 * 1024))",
        ]);
        expect(result.ok).toBe(true);
        expect(result.stdout.length).toBe(2 * 1024 * 1024);
    });
});

describe("OMP plugin listing", () => {
    it("returns null for an unknown payload shape instead of an empty list", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-list-"));
        try {
            for (const [index, payload] of ["{}", '{"npm":{}}', "[]", "null"].entries()) {
                const fake = join(root, `omp-${index}`);
                writeFileSync(fake, `#!/bin/sh\nprintf '%s' '${payload}'\n`);
                chmodSync(fake, 0o755);
                expect(listOmpPlugins(fake)).toBeNull();
            }
            const malformedRow = join(root, "omp-malformed-row");
            writeFileSync(
                malformedRow,
                `#!/bin/sh\nprintf '%s' '{"npm":[{"name":"@eidnara/pi","enabled":true}]}'\n`,
            );
            chmodSync(malformedRow, 0o755);
            expect(listOmpPlugins(malformedRow)).toBeNull();
            const empty = join(root, "omp-empty");
            writeFileSync(empty, `#!/bin/sh\nprintf '%s' '{"npm":[],"marketplace":[]}'\n`);
            chmodSync(empty, 0o755);
            expect(listOmpPlugins(empty)).toEqual([]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});

describe.if(process.platform !== "win32")("getOmpSetting", () => {
    function fakeOmp(valuesByKey: Record<string, string>): string {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-settings-"));
        roots.push(root);
        const omp = join(root, "omp");
        const cases = Object.entries(valuesByKey)
            .map(([key, json]) => `  ${key}) printf '%s' '${json}' ;;`)
            .join("\n");
        writeFileSync(omp, `#!/bin/sh\ncase "$3" in\n${cases}\nesac\n`);
        chmodSync(omp, 0o755);
        return omp;
    }

    it("returns only the type each key declares", () => {
        const omp = fakeOmp({
            "compaction.enabled": '{"value":false}',
            "memory.backend": '{"value":"sqlite"}',
        });
        expect(getOmpSetting(omp, "compaction.enabled")).toBe(false);
        expect(getOmpSetting(omp, "memory.backend")).toBe("sqlite");
    });

    it("rejects a value of the other primitive type", () => {
        const omp = fakeOmp({
            "compaction.enabled": '{"value":"false"}',
            "memory.backend": '{"value":true}',
        });
        expect(getOmpSetting(omp, "compaction.enabled")).toBeNull();
        expect(getOmpSetting(omp, "memory.backend")).toBeNull();
    });
});
