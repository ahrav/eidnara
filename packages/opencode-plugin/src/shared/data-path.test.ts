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

    test("getCacheDir and getOpenCodeCacheDir fall back to <homedir>/.cache when XDG_CACHE_HOME is unset (all platforms)", () => {
        // OpenCode's xdg-basedir uses this fallback on every platform.
        expect(getCacheDir()).toBe(path.join(os.homedir(), ".cache"));
        expect(getOpenCodeCacheDir()).toBe(path.join(os.homedir(), ".cache", "opencode"));
    });

    test("getCacheDir and getOpenCodeCacheDir honor XDG_CACHE_HOME when set", () => {
        process.env.XDG_CACHE_HOME = "/tmp/custom-cache";
        expect(getCacheDir()).toBe("/tmp/custom-cache");
        expect(getOpenCodeCacheDir()).toBe(path.join("/tmp/custom-cache", "opencode"));
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

    test("getDataDir falls back to <homedir>/.local/share for an unset, empty, or relative XDG_DATA_HOME like the daemon does", () => {
        // The daemon's `default_data_root` treats an empty or relative value as absent; a
        // verbatim "" would join into a cwd-relative tree the daemon never writes.
        for (const value of [undefined, "", "relative/data"]) {
            if (value === undefined) delete process.env.XDG_DATA_HOME;
            else process.env.XDG_DATA_HOME = value;
            expect(getDataDir(), `XDG_DATA_HOME=${JSON.stringify(value)}`).toBe(
                path.join(os.homedir(), ".local", "share"),
            );
        }
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

    test("getEidnaraStorageDir honors EIDNARA_TEST_DATA_DIR when XDG_DATA_HOME is unset or relative", () => {
        // When XDG_DATA_HOME is unset, test fallbacks must not resolve to shared storage.
        // Both the guard and the fallback must classify XDG_DATA_HOME the same way,
        // otherwise a relative value escapes the guard and produces the relative path "eidnara/context".
        // Whitespace inside a non-blank value is part of the path.
        const cases: Array<[string | undefined, string]> = [
            [undefined, "/tmp/eidnara-test-isolation"],
            ["relative/data", "/tmp/eidnara-test-isolation"],
            [undefined, "/tmp/eidnara-test "],
        ];
        for (const [xdgDataHome, testDataDir] of cases) {
            if (xdgDataHome === undefined) delete process.env.XDG_DATA_HOME;
            else process.env.XDG_DATA_HOME = xdgDataHome;
            process.env.EIDNARA_TEST_DATA_DIR = testDataDir;
            expect(getEidnaraStorageDir(), `XDG_DATA_HOME=${JSON.stringify(xdgDataHome)}`).toBe(
                path.join(testDataDir, "eidnara", "context"),
            );
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

    test("getEidnaraStorageDir resolves the production eidnara/context layout for an unset or empty XDG_DATA_HOME", () => {
        // Deleting EIDNARA_TEST_DATA_DIR and NODE_ENV makes this assertion exercise the production path.
        const savedTestDir = process.env.EIDNARA_TEST_DATA_DIR;
        const savedNodeEnv = process.env.NODE_ENV;
        delete process.env.EIDNARA_TEST_DATA_DIR;
        delete process.env.NODE_ENV;
        try {
            for (const value of [undefined, ""]) {
                if (value === undefined) delete process.env.XDG_DATA_HOME;
                else process.env.XDG_DATA_HOME = value;
                const resolved = getEidnaraStorageDir();
                expect(path.isAbsolute(resolved), `XDG_DATA_HOME=${JSON.stringify(value)}`).toBe(
                    true,
                );
                expect(resolved).toBe(
                    path.join(os.homedir(), ".local", "share", "eidnara", "context"),
                );
            }
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

    test("getProjectEidnaraDir composes <project>/.eidnara/context regardless of XDG_DATA_HOME or a trailing slash", () => {
        // Project-local artifacts must remain inside the project so OpenCode's external_directory permission system permits access.
        // OpenCode treats artifacts under the project directory as project-internal, avoiding historian Read permission prompts.
        // XDG_DATA_HOME affects shared storage only.
        process.env.XDG_DATA_HOME = "/tmp/custom-data";
        for (const project of ["/Users/me/Work/proj", "/some/project/"]) {
            expect(getProjectEidnaraDir(project)).toBe(path.join(project, ".eidnara", "context"));
        }
    });

    test("getProjectEidnaraHistorianDir appends historian/", () => {
        expect(getProjectEidnaraHistorianDir("/Users/me/Work/proj")).toBe(
            path.join("/Users/me/Work/proj", ".eidnara", "context", "historian"),
        );
    });

    test("getEidnaraLogPath falls back to a per-user harness temp dir when the env override is unset or blank", () => {
        const userRoot = `eidnara-${process.getuid?.() ?? os.userInfo().username}`;
        for (const override of [undefined, "   "]) {
            if (override === undefined) delete process.env.EIDNARA_LOG_PATH;
            else process.env.EIDNARA_LOG_PATH = override;
            expect(
                getEidnaraLogPath("opencode"),
                `EIDNARA_LOG_PATH=${JSON.stringify(override)}`,
            ).toBe(path.join(os.tmpdir(), userRoot, "opencode", "eidnara.log"));
            expect(getEidnaraLogPath("pi")).toBe(
                path.join(os.tmpdir(), userRoot, "pi", "eidnara.log"),
            );
        }
    });

    test("getEidnaraLogPath honors a non-blank EIDNARA_LOG_PATH verbatim, including inner whitespace", () => {
        for (const override of ["/tmp/custom/eidnara.log", "/var/log/eidnara.log "]) {
            process.env.EIDNARA_LOG_PATH = override;
            expect(getEidnaraLogPath("pi")).toBe(override);
        }
    });
});

describe("ensureEidnaraArtifactGitignore", () => {
    test("creates .eidnara/.gitignore with one fenced block that ignores only the artifact dir, even after a second call", () => {
        const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-gi-"));
        try {
            ensureEidnaraArtifactGitignore(dir);
            const gi = readFileSync(path.join(dir, ".eidnara", ".gitignore"), "utf8");
            expect(gi).toContain("# >>> eidnara");
            expect(gi).toContain("context/");
            expect(gi).toContain("# <<< eidnara");
            // `.eidnara/.gitignore` remains tracked; only `context/` is ignored.
            expect(gi).not.toContain("eidnara.jsonc");
            expect(gi).not.toContain("*.jsonc");

            ensureEidnaraArtifactGitignore(dir);
            const again = readFileSync(path.join(dir, ".eidnara", ".gitignore"), "utf8");
            expect(again.split("# >>> eidnara").length - 1).toBe(1);
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

    test("repairs a block that lost its rule or its closing marker", () => {
        const cases: Array<[string, string]> = [
            ["node_modules/\n# >>> eidnara\n# <<< eidnara\n", "node_modules/\n"],
            ["node_modules/\n# >>> eidnara\n", "node_modules/\n"],
            ["# >>> eidnara\nsomething-else/\n# <<< eidnara\nafter/\n", "after/\n"],
        ];
        for (const [broken, keptOutside] of cases) {
            const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-gi-"));
            try {
                const ckDir = path.join(dir, ".eidnara");
                mkdirSync(ckDir, { recursive: true });
                writeFileSync(path.join(ckDir, ".gitignore"), broken);
                ensureEidnaraArtifactGitignore(dir);
                const gi = readFileSync(path.join(ckDir, ".gitignore"), "utf8");
                expect(gi, broken).toContain(keptOutside);
                expect(gi, broken).toContain("# >>> eidnara\ncontext/\n# <<< eidnara\n");
                expect(gi.split("# >>> eidnara").length - 1, broken).toBe(1);
                expect(gi, broken).not.toContain("something-else/");
            } finally {
                rmSync(dir, { recursive: true, force: true });
            }
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
