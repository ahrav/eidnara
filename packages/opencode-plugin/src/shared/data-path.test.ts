import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import {
    chmodSync,
    lstatSync,
    mkdirSync,
    mkdtempSync,
    readdirSync,
    readFileSync,
    rmSync,
    symlinkSync,
    writeFileSync,
} from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import hostRelease from "../../../../release/host-release.json";
import {
    ensureEidnaraArtifactGitignore,
    getCacheDir,
    getDataDir,
    getEidnaraLogPath,
    getEidnaraStorageDir,
    getOpenCodeCacheDir,
    getProjectEidnaraDir,
    getProjectEidnaraHistorianDir,
    storageSubtreePath,
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

    test("getCacheDir treats an empty XDG_CACHE_HOME as unset like xdg-basedir", () => {
        process.env.XDG_CACHE_HOME = "";
        expect(getCacheDir()).toBe(path.join(os.homedir(), ".cache"));
        expect(getOpenCodeCacheDir()).toBe(path.join(os.homedir(), ".cache", "opencode"));
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

    test("getDataDir ignores an empty XDG_DATA_HOME like the daemon does", () => {
        // The daemon's `default_data_root` treats an empty value as absent; a
        // verbatim "" would join into a cwd-relative tree the daemon never writes.
        process.env.XDG_DATA_HOME = "";
        expect(getDataDir()).toBe(path.join(os.homedir(), ".local", "share"));
    });

    test("getDataDir ignores a relative XDG_DATA_HOME like the daemon does", () => {
        process.env.XDG_DATA_HOME = "relative/data";
        expect(getDataDir()).toBe(path.join(os.homedir(), ".local", "share"));
    });

    test("getDataDir keeps whitespace inside an absolute XDG_DATA_HOME like the daemon does", () => {
        // The daemon converts the raw variable to a path; trimming here would name a different tree.
        process.env.XDG_DATA_HOME = "/srv/eidnara-data ";
        expect(getDataDir()).toBe("/srv/eidnara-data ");
        expect(getEidnaraStorageDir()).toBe(path.join("/srv/eidnara-data ", "eidnara", "context"));
    });

    test.skipIf(process.platform === "win32")(
        "getDataDir refuses a relative or unset HOME instead of resolving under cwd or the account database",
        async () => {
            // `os.homedir()` reads HOME at process start, so each case needs a child process.
            // Bun materializes `$HOME/.bun` on startup, so the child runs inside a disposable cwd.
            const scratch = mkdtempSync(path.join(os.tmpdir(), "eidnara-relative-home-"));
            try {
                const script = `
                    const { getDataDir } = await import(process.env.DATA_PATH_MODULE_URL);
                    try { console.log(JSON.stringify({ dir: getDataDir() })); }
                    catch (error) { console.log(JSON.stringify({ error: error.message })); }
                `;
                for (const home of ["relative-home", undefined]) {
                    const env: Record<string, string> = {};
                    for (const [key, value] of Object.entries(process.env)) {
                        if (value !== undefined && key !== "XDG_DATA_HOME" && key !== "HOME") {
                            env[key] = value;
                        }
                    }
                    if (home !== undefined) env.HOME = home;
                    env.DATA_PATH_MODULE_URL = new URL("./data-path.ts", import.meta.url).href;
                    const child = Bun.spawn({
                        cmd: ["bun", "--eval", script],
                        cwd: scratch,
                        env,
                        stdout: "pipe",
                        stderr: "pipe",
                    });
                    const [exitCode, stdout, stderr] = await Promise.all([
                        child.exited,
                        new Response(child.stdout).text(),
                        new Response(child.stderr).text(),
                    ]);
                    expect(exitCode, stderr).toBe(0);
                    const result = JSON.parse(stdout.trim()) as { dir?: string; error?: string };
                    expect(result.dir, `HOME=${home}`).toBeUndefined();
                    expect(result.error).toContain(
                        "XDG_DATA_HOME and HOME are unset, empty, or relative",
                    );
                }
            } finally {
                rmSync(scratch, { recursive: true, force: true });
            }
        },
    );

    test("getEidnaraStorageDir treats a relative XDG_DATA_HOME as unset for test isolation", () => {
        // Both the guard and the fallback must classify XDG_DATA_HOME the same way,
        // otherwise "" escapes the guard and produces the relative path "eidnara/context".
        process.env.EIDNARA_TEST_DATA_DIR = "/tmp/eidnara-test-isolation";
        process.env.XDG_DATA_HOME = "relative/data";
        expect(getEidnaraStorageDir()).toBe(
            path.join("/tmp/eidnara-test-isolation", "eidnara", "context"),
        );
    });

    test("getEidnaraStorageDir never resolves a relative path from an empty XDG_DATA_HOME", () => {
        const savedTestDir = process.env.EIDNARA_TEST_DATA_DIR;
        const savedNodeEnv = process.env.NODE_ENV;
        delete process.env.EIDNARA_TEST_DATA_DIR;
        delete process.env.NODE_ENV;
        process.env.XDG_DATA_HOME = "";
        try {
            const resolved = getEidnaraStorageDir();
            expect(path.isAbsolute(resolved)).toBe(true);
            expect(resolved).toBe(path.join(os.homedir(), ".local", "share", "eidnara", "context"));
        } finally {
            if (savedTestDir !== undefined) process.env.EIDNARA_TEST_DATA_DIR = savedTestDir;
            if (savedNodeEnv !== undefined) process.env.NODE_ENV = savedNodeEnv;
        }
    });

    test("storageSubtreePath takes its segment names from the release contract", () => {
        expect(storageSubtreePath("/data")).toBe(
            path.join(
                "/data",
                hostRelease.layout.managed_subtree,
                hostRelease.layout.storage_subdirectory,
            ),
        );
        expect(hostRelease.layout.managed_subtree).toBe("eidnara");
        expect(hostRelease.layout.storage_subdirectory).toBe("context");
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

    test("getEidnaraStorageDir treats a blank EIDNARA_TEST_DATA_DIR as unset", () => {
        // A whitespace-only value must not select `"   "/eidnara/context`; `NODE_ENV=test` uses its unset-variable fallback.
        process.env.EIDNARA_TEST_DATA_DIR = "   ";
        process.env.NODE_ENV = "test";
        const resolved = getEidnaraStorageDir();
        expect(path.isAbsolute(resolved)).toBe(true);
        expect(resolved).not.toContain(path.join("   ", "eidnara"));
        expect(resolved).not.toContain(path.join(os.homedir(), ".local", "share"));
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

    test("getEidnaraLogPath falls back to a per-user harness temp dir when the env override is unset", () => {
        const userRoot = `eidnara-${process.getuid?.() ?? os.userInfo().username}`;
        expect(getEidnaraLogPath("opencode")).toBe(
            path.join(os.tmpdir(), userRoot, "opencode", "eidnara.log"),
        );
        expect(getEidnaraLogPath("pi")).toBe(path.join(os.tmpdir(), userRoot, "pi", "eidnara.log"));
    });

    test("getEidnaraLogPath honors EIDNARA_LOG_PATH", () => {
        process.env.EIDNARA_LOG_PATH = "/tmp/custom/eidnara.log";
        expect(getEidnaraLogPath("pi")).toBe("/tmp/custom/eidnara.log");
    });

    test("getEidnaraLogPath ignores a blank EIDNARA_LOG_PATH", () => {
        process.env.EIDNARA_LOG_PATH = "   ";
        const userRoot = `eidnara-${process.getuid?.() ?? os.userInfo().username}`;
        expect(getEidnaraLogPath("pi")).toBe(path.join(os.tmpdir(), userRoot, "pi", "eidnara.log"));
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
            // The staging file is renamed into place, not left beside the result.
            expect(readdirSync(ckDir)).toEqual([".gitignore"]);
        } finally {
            rmSync(dir, { recursive: true, force: true });
        }
    });

    test.skipIf(process.platform === "win32")(
        "keeps the existing .gitignore mode across the atomic replacement",
        () => {
            const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-gi-"));
            try {
                const ckDir = path.join(dir, ".eidnara");
                mkdirSync(ckDir, { recursive: true });
                const gitignore = path.join(ckDir, ".gitignore");
                writeFileSync(gitignore, "scratch/\n");
                chmodSync(gitignore, 0o600);
                ensureEidnaraArtifactGitignore(dir);
                expect(lstatSync(gitignore).mode & 0o777).toBe(0o600);
                expect(readFileSync(gitignore, "utf8")).toContain("context/");
            } finally {
                rmSync(dir, { recursive: true, force: true });
            }
        },
    );

    test.skipIf(process.platform === "win32")(
        "restores a permissive .gitignore mode even under a restrictive umask",
        () => {
            const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-gi-"));
            const previousUmask = process.umask(0o077);
            try {
                const ckDir = path.join(dir, ".eidnara");
                mkdirSync(ckDir, { recursive: true });
                const gitignore = path.join(ckDir, ".gitignore");
                writeFileSync(gitignore, "scratch/\n");
                chmodSync(gitignore, 0o644);
                ensureEidnaraArtifactGitignore(dir);
                // The create mode is filtered through the umask; only an explicit fchmod keeps 0644.
                expect(lstatSync(gitignore).mode & 0o777).toBe(0o644);
            } finally {
                process.umask(previousUmask);
                rmSync(dir, { recursive: true, force: true });
            }
        },
    );

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

    test("a sibling guard sharing the prefix does not count as Eidnara's block", () => {
        const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-gi-"));
        try {
            const ckDir = path.join(dir, ".eidnara");
            mkdirSync(ckDir, { recursive: true });
            writeFileSync(
                path.join(ckDir, ".gitignore"),
                "# >>> eidnara-cache\ncache/\n# <<< eidnara-cache\n",
            );
            ensureEidnaraArtifactGitignore(dir);
            const gi = readFileSync(path.join(ckDir, ".gitignore"), "utf8");
            expect(gi).toContain("# >>> eidnara-cache\ncache/\n# <<< eidnara-cache\n");
            expect(gi).toContain("# >>> eidnara\ncontext/\n# <<< eidnara\n");
        } finally {
            rmSync(dir, { recursive: true, force: true });
        }
    });

    test.skipIf(process.platform === "win32")(
        "never writes through a symlinked .eidnara/.gitignore",
        () => {
            const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-gi-"));
            try {
                const victim = path.join(dir, "victim.txt");
                writeFileSync(victim, "untouched\n");
                const ckDir = path.join(dir, "project", ".eidnara");
                mkdirSync(ckDir, { recursive: true });
                symlinkSync(victim, path.join(ckDir, ".gitignore"));

                ensureEidnaraArtifactGitignore(path.join(dir, "project"));

                expect(readFileSync(victim, "utf8")).toBe("untouched\n");
                expect(lstatSync(path.join(ckDir, ".gitignore")).isSymbolicLink()).toBe(true);
            } finally {
                rmSync(dir, { recursive: true, force: true });
            }
        },
    );

    test.skipIf(process.platform === "win32")(
        "never writes through a symlinked .eidnara directory",
        () => {
            const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-gi-"));
            try {
                const outside = path.join(dir, "outside");
                mkdirSync(outside);
                const project = path.join(dir, "project");
                mkdirSync(project);
                symlinkSync(outside, path.join(project, ".eidnara"));

                ensureEidnaraArtifactGitignore(project);

                expect(readdirSync(outside)).toEqual([]);
            } finally {
                rmSync(dir, { recursive: true, force: true });
            }
        },
    );
});
