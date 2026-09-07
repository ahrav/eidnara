import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import {
    ensureEidnaraArtifactGitignore,
    getCacheDir,
    getDataDir,
    getEidnaraLogPath,
    getEidnaraStorageDir,
    getLegacyOpenCodeEidnaraStorageDir,
    getOpenCodeCacheDir,
    getOpenCodeStorageDir,
    getProjectEidnaraDir,
    getProjectEidnaraHistorianDir,
} from "./data-path";

const savedEnv = {
    XDG_CACHE_HOME: process.env.XDG_CACHE_HOME,
    XDG_DATA_HOME: process.env.XDG_DATA_HOME,
    LOCALAPPDATA: process.env.LOCALAPPDATA,
    EIDNARA_LOG_PATH: process.env.EIDNARA_LOG_PATH,
    EIDNARA_TEST_DATA_DIR: process.env.EIDNARA_TEST_DATA_DIR,
    NODE_ENV: process.env.NODE_ENV,
};

describe("data-path", () => {
    beforeEach(() => {
        process.env.XDG_CACHE_HOME = undefined;
        process.env.XDG_DATA_HOME = undefined;
        process.env.LOCALAPPDATA = undefined;
        process.env.EIDNARA_LOG_PATH = undefined;
        // Bun requires deleting an environment key to unset it.
        delete process.env.XDG_CACHE_HOME;
        delete process.env.XDG_DATA_HOME;
        delete process.env.LOCALAPPDATA;
        delete process.env.EIDNARA_LOG_PATH;
    });

    afterEach(() => {
        // afterEach restores or deletes every environment variable this suite touches so unset NODE_ENV and EIDNARA_TEST_DATA_DIR cannot leak into subsequent tests.
        for (const [key, value] of Object.entries(savedEnv)) {
            if (value !== undefined) process.env[key] = value;
            else delete process.env[key];
        }
    });

    test("getCacheDir falls back to <homedir>/.cache when XDG_CACHE_HOME is unset (all platforms)", () => {
        // OpenCode's xdg-basedir uses this fallback on every platform.
        expect(getCacheDir()).toBe(path.join(os.homedir(), ".cache"));
    });

    test("getCacheDir honors XDG_CACHE_HOME when set", () => {
        process.env.XDG_CACHE_HOME = "/tmp/custom-cache";
        expect(getCacheDir()).toBe("/tmp/custom-cache");
    });

    test("getCacheDir ignores LOCALAPPDATA on Windows (must match OpenCode's xdg-basedir)", () => {
        // OpenCode's xdg-basedir ignores LOCALAPPDATA when resolving the Windows cache directory.
        process.env.LOCALAPPDATA = "C:\\Users\\Test\\AppData\\Local";
        expect(getCacheDir()).toBe(path.join(os.homedir(), ".cache"));
    });

    test("getOpenCodeCacheDir appends 'opencode' to the cache base", () => {
        expect(getOpenCodeCacheDir()).toBe(path.join(os.homedir(), ".cache", "opencode"));
    });

    test("getOpenCodeCacheDir with XDG_CACHE_HOME set", () => {
        process.env.XDG_CACHE_HOME = "/tmp/custom-cache";
        expect(getOpenCodeCacheDir()).toBe(path.join("/tmp/custom-cache", "opencode"));
    });

    test("getDataDir falls back to <homedir>/.local/share when XDG_DATA_HOME is unset", () => {
        expect(getDataDir()).toBe(path.join(os.homedir(), ".local", "share"));
    });

    test("getOpenCodeStorageDir composes correctly", () => {
        expect(getOpenCodeStorageDir()).toBe(
            path.join(os.homedir(), ".local", "share", "opencode", "storage"),
        );
    });

    test("getEidnaraStorageDir uses eidnara/context layout", () => {
        // Deleting EIDNARA_TEST_DATA_DIR and NODE_ENV makes this assertion exercise the production path.
        const savedTestDir = process.env.EIDNARA_TEST_DATA_DIR;
        const savedNodeEnv = process.env.NODE_ENV;
        delete process.env.EIDNARA_TEST_DATA_DIR;
        delete process.env.NODE_ENV;
        try {
            expect(getEidnaraStorageDir()).toBe(
                path.join(os.homedir(), ".local", "share", "eidnara", "context"),
            );
        } finally {
            if (savedTestDir !== undefined) process.env.EIDNARA_TEST_DATA_DIR = savedTestDir;
            if (savedNodeEnv !== undefined) process.env.NODE_ENV = savedNodeEnv;
        }
    });

    test("getEidnaraStorageDir backstops to a temp dir under NODE_ENV=test with no guard set", () => {
        const savedTestDir = process.env.EIDNARA_TEST_DATA_DIR;
        delete process.env.EIDNARA_TEST_DATA_DIR;
        process.env.NODE_ENV = "test";
        try {
            const resolved = getEidnaraStorageDir();
            expect(resolved).not.toContain(path.join(os.homedir(), ".local", "share"));
            expect(resolved.endsWith(path.join("eidnara", "context"))).toBe(true);
            expect(getEidnaraStorageDir()).toBe(resolved);
        } finally {
            if (savedTestDir !== undefined) process.env.EIDNARA_TEST_DATA_DIR = savedTestDir;
        }
    });

    test("getEidnaraStorageDir honors EIDNARA_TEST_DATA_DIR when XDG_DATA_HOME is unset", () => {
        // When XDG_DATA_HOME is unset, test fallbacks must not resolve to shared storage.
        // CLI doctors must not run integrity checks against production data.
        process.env.EIDNARA_TEST_DATA_DIR = "/tmp/eidnara-test-isolation";
        expect(getEidnaraStorageDir()).toBe(
            path.join("/tmp/eidnara-test-isolation", "eidnara", "context"),
        );
    });

    test("getEidnaraStorageDir prefers XDG_DATA_HOME over EIDNARA_TEST_DATA_DIR", () => {
        // EIDNARA_TEST_DATA_DIR isolates the data homes required by several suites.
        process.env.EIDNARA_TEST_DATA_DIR = "/tmp/eidnara-test-isolation";
        process.env.XDG_DATA_HOME = "/tmp/custom-data";
        expect(getEidnaraStorageDir()).toBe(path.join("/tmp/custom-data", "eidnara", "context"));
    });

    test("getEidnaraStorageDir honors XDG_DATA_HOME", () => {
        process.env.XDG_DATA_HOME = "/tmp/custom-data";
        expect(getEidnaraStorageDir()).toBe(path.join("/tmp/custom-data", "eidnara", "context"));
    });

    test("getLegacyOpenCodeEidnaraStorageDir points at the pre-managed-subtree OpenCode path", () => {
        // The legacy data path must remain stable so upgrades can migrate pre-shared-storage data.
        expect(getLegacyOpenCodeEidnaraStorageDir()).toBe(
            path.join(os.homedir(), ".local", "share", "opencode", "storage", "plugin", "eidnara"),
        );
    });

    test("legacy storage dir distinct from new shared dir even with same XDG override", () => {
        // Even when XDG_DATA_HOME points to the same location, the resolvers must return different paths to prevent migration from overwriting its source.
        // self-overwrite.
        process.env.XDG_DATA_HOME = "/tmp/test-xdg";
        const legacy = getLegacyOpenCodeEidnaraStorageDir();
        const shared = getEidnaraStorageDir();
        expect(legacy).not.toBe(shared);
        expect(legacy).toContain("opencode");
        expect(shared).toContain("eidnara");
    });

    test("getProjectEidnaraDir composes <project>/.eidnara/context", () => {
        // Project-local artifacts must remain inside the project so OpenCode's external_directory permission system permits access.
        // OpenCode treats artifacts under the project directory as project-internal, avoiding historian Read permission prompts.
        // OpenCode prompts for historian Read access when artifacts are outside the project directory.
        expect(getProjectEidnaraDir("/Users/me/Work/proj")).toBe(
            path.join("/Users/me/Work/proj", ".eidnara", "context"),
        );
    });

    test("getProjectEidnaraHistorianDir appends historian/", () => {
        expect(getProjectEidnaraHistorianDir("/Users/me/Work/proj")).toBe(
            path.join("/Users/me/Work/proj", ".eidnara", "context", "historian"),
        );
    });

    test("getProjectEidnaraDir is unaffected by XDG_DATA_HOME", () => {
        // XDG_DATA_HOME affects shared storage only; project-local artifacts remain under the supplied project directory.
        // XDG_DATA_HOME affects shared storage only; project-local artifacts remain under the supplied project directory.
        // XDG_DATA_HOME affects shared storage only; project-local artifacts remain under the supplied project directory.
        // XDG_DATA_HOME affects shared storage only; project-local artifacts remain under the supplied project directory.
        process.env.XDG_DATA_HOME = "/tmp/custom-data";
        expect(getProjectEidnaraDir("/some/project")).toBe(
            path.join("/some/project", ".eidnara", "context"),
        );
    });

    test("getProjectEidnaraDir handles trailing slashes via path.join", () => {
        expect(getProjectEidnaraDir("/some/project/")).toBe(
            path.join("/some/project/", ".eidnara", "context"),
        );
    });

    test("getEidnaraLogPath falls back to the harness temp dir when the env override is unset", () => {
        expect(getEidnaraLogPath("opencode")).toBe(
            path.join(os.tmpdir(), "opencode", "eidnara", "eidnara.log"),
        );
        expect(getEidnaraLogPath("pi")).toBe(
            path.join(os.tmpdir(), "pi", "eidnara", "eidnara.log"),
        );
    });

    test("getEidnaraLogPath honors EIDNARA_LOG_PATH", () => {
        process.env.EIDNARA_LOG_PATH = "/tmp/custom/eidnara.log";
        expect(getEidnaraLogPath("pi")).toBe("/tmp/custom/eidnara.log");
    });

    test("getEidnaraLogPath ignores a blank EIDNARA_LOG_PATH", () => {
        process.env.EIDNARA_LOG_PATH = "   ";
        expect(getEidnaraLogPath("pi")).toBe(
            path.join(os.tmpdir(), "pi", "eidnara", "eidnara.log"),
        );
    });
});

describe("ensureEidnaraArtifactGitignore", () => {
    test("creates .eidnara/.gitignore with a fenced context block", () => {
        const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-gi-"));
        try {
            ensureEidnaraArtifactGitignore(dir);
            const gi = readFileSync(path.join(dir, ".eidnara", ".gitignore"), "utf8");
            expect(gi).toContain("# >>> eidnara");
            expect(gi).toContain("context/");
            expect(gi).toContain("# <<< eidnara");
        } finally {
            rmSync(dir, { recursive: true, force: true });
        }
    });

    test("is idempotent — a second call does not duplicate the block", () => {
        const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-gi-"));
        try {
            ensureEidnaraArtifactGitignore(dir);
            ensureEidnaraArtifactGitignore(dir);
            const gi = readFileSync(path.join(dir, ".eidnara", ".gitignore"), "utf8");
            const occurrences = gi.split("# >>> eidnara").length - 1;
            expect(occurrences).toBe(1);
        } finally {
            rmSync(dir, { recursive: true, force: true });
        }
    });

    test("preserves a sibling module's existing entries (appends, never clobbers)", () => {
        const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-gi-"));
        try {
            const ckDir = path.join(dir, ".eidnara");
            mkdirSync(ckDir, { recursive: true });
            // .gitignore must not contain another `eidnara` block when one already exists.
            writeFileSync(
                path.join(ckDir, ".gitignore"),
                "# >>> other:aft\naft/scratch/\n# <<< other:aft\n",
            );
            ensureEidnaraArtifactGitignore(dir);
            const gi = readFileSync(path.join(ckDir, ".gitignore"), "utf8");
            expect(gi).toContain("# >>> other:aft");
            expect(gi).toContain("aft/scratch/");
            expect(gi).toContain("# >>> eidnara");
            expect(gi).toContain("context/");
        } finally {
            rmSync(dir, { recursive: true, force: true });
        }
    });

    test("does not ignore the project config — only the artifact dir", () => {
        const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-gi-"));
        try {
            ensureEidnaraArtifactGitignore(dir);
            const gi = readFileSync(path.join(dir, ".eidnara", ".gitignore"), "utf8");
            // `.eidnara/.gitignore` remains tracked; only `context/` is ignored.
            expect(gi).not.toContain("eidnara.jsonc");
            expect(gi).not.toContain("*.jsonc");
        } finally {
            rmSync(dir, { recursive: true, force: true });
        }
    });
});
