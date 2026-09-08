import { afterEach, describe, expect, it, spyOn } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import os, { homedir, tmpdir } from "node:os";
import { isAbsolute, join } from "node:path";
import { envFirstHomeDir, hasHomeDir, hasPiAgentDir, resolveOmpPaths } from "./paths";

const ENV_KEYS = [
    "HOME",
    "XDG_DATA_HOME",
    "OMP_PROFILE",
    "PI_PROFILE",
    "PI_CONFIG_DIR",
    "PI_CODING_AGENT_DIR",
] as const;

const saved = new Map<string, string | undefined>();
const roots: string[] = [];
let originalCwd: string | undefined;

function setEnv(key: (typeof ENV_KEYS)[number], value: string | undefined): void {
    if (!saved.has(key)) saved.set(key, process.env[key]);
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
}

afterEach(() => {
    for (const [key, value] of saved) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
    saved.clear();
    if (originalCwd !== undefined) {
        process.chdir(originalCwd);
        originalCwd = undefined;
    }
    for (const root of roots.splice(0)) {
        rmSync(root, { recursive: true, force: true });
    }
});

const xdgPlatform = process.platform === "linux" || process.platform === "darwin";

describe("envFirstHomeDir", () => {
    it.if(process.platform !== "win32")("prefers HOME over the passwd entry on POSIX", () => {
        setEnv("HOME", "/virt/other-home");
        expect(envFirstHomeDir()).toBe("/virt/other-home");
    });

    it.if(process.platform === "win32")("ignores HOME and follows os.homedir() on Windows", () => {
        setEnv("HOME", "C:\\msys64\\home\\fox");
        expect(envFirstHomeDir()).toBe(homedir());
    });

    it.if(process.platform !== "win32")(
        "rejects a relative HOME instead of resolving under the working directory",
        () => {
            // A relative HOME is ignored in favor of the passwd entry; a relative passwd entry
            // (Bun echoes HOME) is refused outright, and the doctors' predicates report no home.
            setEnv("HOME", "rel/home");
            setEnv("PI_CODING_AGENT_DIR", undefined);
            const resolved = envFirstHomeDir();
            expect(isAbsolute(resolved)).toBe(true);
            expect(resolved).not.toBe("rel/home");
            const userInfoSpy = spyOn(os, "userInfo").mockImplementation(() => {
                throw new Error("no passwd entry");
            });
            const homedirSpy = spyOn(os, "homedir").mockImplementation(() => "rel/home");
            try {
                expect(() => envFirstHomeDir()).toThrow("Relative home directory");
                expect(hasHomeDir()).toBe(false);
                expect(hasPiAgentDir()).toBe(false);
            } finally {
                homedirSpy.mockRestore();
                userInfoSpy.mockRestore();
            }
        },
    );

    it.if(process.platform !== "win32")(
        "throws instead of yielding cwd-relative paths without a home",
        () => {
            setEnv("HOME", undefined);
            const userInfoSpy = spyOn(os, "userInfo").mockImplementation(() => {
                throw new Error("no passwd entry");
            });
            const homedirSpy = spyOn(os, "homedir").mockImplementation(() => {
                throw Object.assign(new Error("uv_os_homedir returned ENOENT"), { code: "ENOENT" });
            });
            try {
                expect(() => envFirstHomeDir()).toThrow("No home directory");
            } finally {
                homedirSpy.mockRestore();
                userInfoSpy.mockRestore();
            }
        },
    );
});

describe("resolveOmpPaths", () => {
    it.skipIf(!xdgPlatform)("uses an absolute XDG_DATA_HOME whose omp directory exists", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-paths-"));
        roots.push(root);
        const home = join(root, "home");
        const dataHome = join(root, "data");
        mkdirSync(join(dataHome, "omp"), { recursive: true });
        for (const key of ENV_KEYS) setEnv(key, undefined);
        setEnv("HOME", home);
        setEnv("XDG_DATA_HOME", dataHome);

        const paths = resolveOmpPaths();

        expect(paths.configRoot).toBe(join(home, ".omp"));
        expect(paths.dataRoot).toBe(join(dataHome, "omp"));
        expect(paths.sessionsRoot).toBe(join(dataHome, "omp", "sessions"));
    });

    it.skipIf(!xdgPlatform)("ignores a relative XDG_DATA_HOME that resolves under cwd", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-paths-rel-"));
        roots.push(root);
        const home = join(root, "home");
        const cwd = join(root, "project");
        mkdirSync(join(cwd, "data", "omp"), { recursive: true });
        for (const key of ENV_KEYS) setEnv(key, undefined);
        setEnv("HOME", home);
        setEnv("XDG_DATA_HOME", "./data");
        originalCwd = process.cwd();
        process.chdir(cwd);

        const paths = resolveOmpPaths();

        expect(paths.dataRoot).toBe(join(home, ".omp"));
        expect(paths.pluginsDir).toBe(join(home, ".omp", "plugins"));
        expect(paths.sessionsRoot).toBe(join(home, ".omp", "agent", "sessions"));
    });
});
