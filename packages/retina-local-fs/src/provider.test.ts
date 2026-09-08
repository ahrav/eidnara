import { afterEach, describe, expect, test } from "bun:test";
import { execFile, execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rename, rm, symlink, utimes, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import {
    type ProviderConfig,
    ProviderError,
    type ProviderScalar,
    runProvider,
    validateProviderConfig,
} from "./provider";

const execFileAsync = promisify(execFile);
const temporaryDirectories: string[] = [];
const originalXdgDataHome = process.env.XDG_DATA_HOME;

async function temporaryDirectory(prefix = "retina-local-fs-"): Promise<string> {
    const directory = await mkdtemp(join(tmpdir(), prefix));
    temporaryDirectories.push(directory);
    return directory;
}

afterEach(async () => {
    if (originalXdgDataHome === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = originalXdgDataHome;
    await Promise.all(
        temporaryDirectories.splice(0).map((directory) => rm(directory, { recursive: true })),
    );
});

async function poll(
    config: ProviderConfig,
    scalar: ProviderScalar | null = null,
    homeDirectory?: string,
    dataDirectory?: string,
) {
    return runProvider(
        { scalar, config },
        {
            homeDirectory,
            dataDirectory:
                dataDirectory ??
                (homeDirectory ? join(homeDirectory, ".local", "share") : undefined),
            now: () => 1_786_320_000_000,
        },
    );
}

async function git(repo: string, ...args: string[]): Promise<string> {
    const { stdout } = await execFileAsync("git", ["-C", repo, ...args], { encoding: "utf8" });
    return stdout.trim();
}

async function createRepository(): Promise<{ repo: string; firstSha: string }> {
    const repo = await temporaryDirectory("retina-local-fs-git-");
    await git(repo, "init", "--initial-branch=main");
    await git(repo, "config", "user.name", "Retina Test");
    await git(repo, "config", "user.email", "retina@example.invalid");
    await writeFile(join(repo, "state.txt"), "one\n");
    await git(repo, "add", "state.txt");
    await git(repo, "commit", "-m", "first");
    return { repo, firstSha: await git(repo, "rev-parse", "HEAD") };
}

async function commit(repo: string, content: string): Promise<string> {
    await writeFile(join(repo, "state.txt"), content);
    await git(repo, "add", "state.txt");
    await git(repo, "commit", "-m", content.trim());
    return git(repo, "rev-parse", "HEAD");
}

describe("filesystem predicates", () => {
    test("file_contains fires in the requested direction and stays quiet when unchanged", async () => {
        const directory = await temporaryDirectory();
        const path = join(directory, "status.txt");
        await writeFile(path, "ready\n");
        const config = { kind: "file_contains", path, needle: "ready" } as const;

        const first = await poll(config);
        expect(first.events).toHaveLength(1);
        expect(first.events[0]?.observed).toEqual({ contains: true });
        expect((await poll(config, first.scalar)).events).toHaveLength(0);

        await writeFile(path, "waiting\n");
        expect((await poll(config, first.scalar)).events).toHaveLength(0);

        const absent = await poll({ ...config, absent: true });
        expect(absent.events).toHaveLength(1);
        expect(absent.events[0]?.observed).toEqual({ contains: false });
    });

    test("file_contains refuses a FIFO or device node instead of blocking on open", async () => {
        const directory = await temporaryDirectory();
        const fifo = join(directory, "pipe");
        await execFileAsync("mkfifo", [fifo]);
        await expect(
            poll({ kind: "file_contains", path: fifo, needle: "ready" }),
        ).rejects.toMatchObject({ code: "unreadable_path" });
        await expect(
            poll({ kind: "file_contains", path: "/dev/zero", needle: "ready" }),
        ).rejects.toMatchObject({ code: "unreadable_path" });
    }, 3_000);

    test("file_contains scans a file larger than one read chunk, including a needle across the seam", async () => {
        const directory = await temporaryDirectory();
        const path = join(directory, "large.log");
        const chunk = 64 * 1024;
        const filler = "x".repeat(chunk * 3 - 3);
        await writeFile(path, `${filler}needle${"y".repeat(chunk)}`);
        const result = await poll({ kind: "file_contains", path, needle: "needle" });
        expect(result.events).toHaveLength(1);
        expect(result.events[0]?.observed).toEqual({ contains: true });
    });

    test("file_contains validates the target before matching an empty needle", async () => {
        const directory = await temporaryDirectory();
        const file = join(directory, "present.txt");
        await writeFile(file, "anything");
        const matched = await poll({ kind: "file_contains", path: file, needle: "" });
        expect(matched.events[0]?.observed).toEqual({ contains: true });

        await expect(
            poll({ kind: "file_contains", path: directory, needle: "" }),
        ).rejects.toMatchObject({ code: "unreadable_path" });
        await expect(
            poll({ kind: "file_contains", path: "/dev/zero", needle: "" }),
        ).rejects.toMatchObject({ code: "unreadable_path" });
    });

    test("file_contains rejects a needle larger than one scan chunk before allocating for it", () => {
        const limit = 64 * 1024;
        expect(
            validateProviderConfig({
                kind: "file_contains",
                path: "/tmp/x",
                needle: "a".repeat(limit),
            }),
        ).toMatchObject({ success: true });
        expect(
            validateProviderConfig({
                kind: "file_contains",
                path: "/tmp/x",
                needle: "a".repeat(limit + 1),
            }),
        ).toMatchObject({ success: false, reason: expect.stringContaining("needle") });
        // The bound is on UTF-8 bytes, not UTF-16 code units.
        expect(
            validateProviderConfig({
                kind: "file_contains",
                path: "/tmp/x",
                needle: "\u00e9".repeat(limit / 2 + 1),
            }),
        ).toMatchObject({ success: false });
    });

    test("path_exists treats missing as an observation and supports gone", async () => {
        const directory = await temporaryDirectory();
        const path = join(directory, "result.json");
        const existsConfig = { kind: "path_exists", path } as const;

        const missing = await poll(existsConfig);
        expect(missing.events).toHaveLength(0);
        await writeFile(path, "{}\n");
        const appeared = await poll(existsConfig, missing.scalar);
        expect(appeared.events).toHaveLength(1);
        expect((await poll(existsConfig, appeared.scalar)).events).toHaveLength(0);

        await rm(path);
        const gone = await poll({ ...existsConfig, gone: true }, appeared.scalar);
        expect(gone.events).toHaveLength(1);
        expect(gone.events[0]?.observed).toEqual({ exists: false });
    });

    test("mtime_after emits each later mtime once", async () => {
        const directory = await temporaryDirectory();
        const path = join(directory, "build.out");
        await writeFile(path, "one");
        const firstTime = new Date("2026-08-10T00:00:00.000Z");
        await utimes(path, firstTime, firstTime);

        const quiet = await poll({ kind: "mtime_after", path, since_ms: firstTime.getTime() + 1 });
        expect(quiet.events).toHaveLength(0);

        const config = { kind: "mtime_after", path, since_ms: firstTime.getTime() - 1 } as const;
        const first = await poll(config);
        expect(first.events).toHaveLength(1);
        expect((await poll(config, first.scalar)).events).toHaveLength(0);

        const secondTime = new Date(firstTime.getTime() + 10_000);
        await utimes(path, secondTime, secondTime);
        const second = await poll(config, first.scalar);
        expect(second.events).toHaveLength(1);
        expect(second.events[0]?.id).not.toBe(first.events[0]?.id);

        // An mtime that falls below the threshold and later returns to a value already reported re-fires with a new identity.
        const belowThreshold = new Date(firstTime.getTime() - 10_000);
        await utimes(path, belowThreshold, belowThreshold);
        const quietAgain = await poll(config, second.scalar);
        expect(quietAgain.events).toHaveLength(0);
        await utimes(path, secondTime, secondTime);
        const repeated = await poll(config, quietAgain.scalar);
        expect(repeated.events).toHaveLength(1);
        expect(repeated.events[0]?.id).not.toBe(second.events[0]?.id);
    });

    test("boolean occurrences have stable replay ids and distinct transition ids", async () => {
        const directory = await temporaryDirectory();
        const path = join(directory, "flag");
        await writeFile(path, "yes");
        const config = { kind: "path_exists", path } as const;

        const replayA = await poll(config);
        const replayB = await poll(config);
        expect(replayA.events[0]?.id).toBe(replayB.events[0]?.id);

        await rm(path);
        const absent = await poll(config, replayA.scalar);
        await writeFile(path, "again");
        const repeated = await poll(config, absent.scalar);
        expect(repeated.events[0]?.id).not.toBe(replayA.events[0]?.id);
    });

    test("occurrence counters saturate at the largest safe integer the scalar accepts", async () => {
        const directory = await temporaryDirectory();
        const path = join(directory, "flag");
        const config = { kind: "path_exists", path } as const;
        const absent = await poll(config);
        const [key, entry] = Object.entries(absent.scalar.predicates)[0] ?? [];
        if (!key || !entry) throw new Error("scalar has no predicate entry");
        const saturated: ProviderScalar = {
            version: 1,
            predicates: { [key]: { ...entry, occurrence: Number.MAX_SAFE_INTEGER } },
        };

        await writeFile(path, "now present");
        const fired = await poll(config, saturated);
        expect(fired.events).toHaveLength(1);
        expect(fired.scalar.predicates[key]?.occurrence).toBe(Number.MAX_SAFE_INTEGER);
        // The returned scalar must round-trip through parseScalar's safe-integer check.
        expect((await poll(config, fired.scalar)).events).toHaveLength(0);
    });
});

describe("git predicates", () => {
    test("git_commit_after fires for strict descendants and each new commit", async () => {
        const { repo, firstSha } = await createRepository();
        const equal = await poll({
            kind: "git_commit_after",
            repo_path: repo,
            sha: firstSha.slice(0, 10),
        });
        expect(equal.events).toHaveLength(0);

        const secondSha = await commit(repo, "two\n");
        const config = { kind: "git_commit_after", repo_path: repo, sha: firstSha } as const;
        const second = await poll(config, equal.scalar);
        expect(second.events).toHaveLength(1);
        expect(second.events[0]?.observed.sha).toBe(secondSha);
        expect((await poll(config, second.scalar)).events).toHaveLength(0);

        const thirdSha = await commit(repo, "three\n");
        const third = await poll(config, second.scalar);
        expect(third.events[0]?.observed.sha).toBe(thirdSha);
        expect(third.events[0]?.id).not.toBe(second.events[0]?.id);

        // Leaving the descendant and returning to it re-enters the matching state with a new event identity.
        await git(repo, "checkout", "--quiet", "--detach", firstSha);
        const left = await poll(config, third.scalar);
        expect(left.events).toHaveLength(0);
        await git(repo, "checkout", "--quiet", "main");
        const returned = await poll(config, left.scalar);
        expect(returned.events).toHaveLength(1);
        expect(returned.events[0]?.observed.sha).toBe(thirdSha);
        expect(returned.events[0]?.id).not.toBe(third.events[0]?.id);
    });

    test("git_tag_matching tracks new matching tags and semver thresholds", async () => {
        const { repo } = await createRepository();
        await git(repo, "tag", "other-1.0.0");
        const config = {
            kind: "git_tag_matching",
            repo_path: repo,
            pattern: "v*",
            above: "1.0.0",
        } as const;
        const quiet = await poll(config);
        expect(quiet.events).toHaveLength(0);

        await git(repo, "tag", "v1.0.0");
        const thresholdQuiet = await poll(config, quiet.scalar);
        expect(thresholdQuiet.events).toHaveLength(0);
        await git(repo, "tag", "v1.1.0");
        const matching = await poll(config, thresholdQuiet.scalar);
        expect(matching.events).toHaveLength(1);
        expect(matching.events[0]?.observed).toEqual({ tag: "v1.1.0" });
        expect((await poll(config, matching.scalar)).events).toHaveLength(0);

        await git(repo, "tag", "v2.0.0");
        const next = await poll(config, matching.scalar);
        expect(next.events[0]?.id).not.toBe(matching.events[0]?.id);

        // A tag deleted and re-created is observed anew and must not reuse the earlier event's identity.
        await git(repo, "tag", "--delete", "v2.0.0");
        const deleted = await poll(config, next.scalar);
        expect(deleted.events).toHaveLength(0);
        await git(repo, "tag", "v2.0.0");
        const recreated = await poll(config, deleted.scalar);
        expect(recreated.events.map((event) => event.observed)).toEqual([{ tag: "v2.0.0" }]);
        expect(recreated.events[0]?.id).not.toBe(next.events[0]?.id);
    });

    test("git_tag_matching rejects an option-like pattern instead of passing it to git", async () => {
        const { repo } = await createRepository();
        await git(repo, "tag", "other-1.0.0");
        for (const pattern of ["--format=INJECTED-%(refname)", "--", "-n"]) {
            await expect(
                poll({ kind: "git_tag_matching", repo_path: repo, pattern }),
            ).rejects.toMatchObject({ code: "invalid_config" });
        }
    });

    test("git_tag_matching orders prerelease identifiers by ASCII, not locale collation", async () => {
        // SemVer 11.4.2: alphanumeric identifiers compare in ASCII order, so
        // uppercase sorts before lowercase and 1.0.0-RC1 < 1.0.0-beta.
        const { repo } = await createRepository();
        await git(repo, "tag", "v1.0.0-RC1");
        await git(repo, "tag", "v1.0.0-beta");

        const aboveBeta = await poll({
            kind: "git_tag_matching",
            repo_path: repo,
            pattern: "v*",
            above: "1.0.0-beta",
        });
        expect(aboveBeta.events).toHaveLength(0);

        const aboveRc = await poll({
            kind: "git_tag_matching",
            repo_path: repo,
            pattern: "v*",
            above: "1.0.0-RC1",
        });
        expect(aboveRc.events.map((event) => event.observed)).toEqual([{ tag: "v1.0.0-beta" }]);
    });

    test("git_tag_matching compares numeric identifiers exactly beyond the double-precision range", async () => {
        const { repo } = await createRepository();
        // 2^53 and 2^53 + 1 are the same IEEE-754 double.
        await git(repo, "tag", "v9007199254740993.0.0");
        await git(repo, "tag", "v1.0.0-9007199254740993");
        const config = { kind: "git_tag_matching", repo_path: repo, pattern: "v*" } as const;

        const aboveCore = await poll({ ...config, above: "9007199254740992.0.0" });
        expect(aboveCore.events.map((event) => event.observed)).toEqual([
            { tag: "v9007199254740993.0.0" },
        ]);
        const abovePrerelease = await poll({ ...config, above: "1.0.0-9007199254740992" });
        expect(abovePrerelease.events.map((event) => event.observed)).toEqual([
            { tag: "v1.0.0-9007199254740993" },
            { tag: "v9007199254740993.0.0" },
        ]);
    });

    test("git_tag_matching rejects identifiers outside the SemVer grammar", async () => {
        for (const above of [
            "1.0.0-01",
            "1.0.0-alpha..1",
            "1.0.0-",
            "1.0.0-.a",
            "1.0.0+",
            "1.0.0+a..b",
        ]) {
            expect(
                validateProviderConfig({
                    kind: "git_tag_matching",
                    repo_path: "/tmp/repo",
                    pattern: "v*",
                    above,
                }),
            ).toMatchObject({ success: false, reason: expect.stringContaining(above) });
        }
        for (const above of [
            "1.0.0-0",
            "1.0.0-0a",
            "1.0.0-x.7.z.92",
            "1.0.0-alpha+001",
            "v1.0.0+build.1",
        ]) {
            expect(
                validateProviderConfig({
                    kind: "git_tag_matching",
                    repo_path: "/tmp/repo",
                    pattern: "v*",
                    above,
                }),
            ).toMatchObject({ success: true });
        }

        // An invalid tag is not a version and is never "newer" than the threshold.
        const { repo } = await createRepository();
        await git(repo, "tag", "v2.0.0-01");
        await git(repo, "tag", "v2.0.0-");
        await git(repo, "tag", "v2.0.0-1");
        const result = await poll({
            kind: "git_tag_matching",
            repo_path: repo,
            pattern: "v*",
            above: "1.0.0",
        });
        expect(result.events.map((event) => event.observed)).toEqual([{ tag: "v2.0.0-1" }]);
    });

    test("git_tag_matching lists one tag per line regardless of the repository's column setting", async () => {
        const { repo } = await createRepository();
        await git(repo, "config", "column.tag", "always");
        await git(repo, "tag", "v1.0.0");
        await git(repo, "tag", "v1.1.0");
        await git(repo, "tag", "v1.2.0");
        const result = await poll({ kind: "git_tag_matching", repo_path: repo, pattern: "v*" });
        expect(result.events.map((event) => event.observed)).toEqual([
            { tag: "v1.0.0" },
            { tag: "v1.1.0" },
            { tag: "v1.2.0" },
        ]);
    });

    test("git_tag_matching reads a tag listing larger than the runtime's default buffer", async () => {
        const { repo, firstSha } = await createRepository();
        const count = 18_000;
        const suffix = "x".repeat(40);
        const updates = Array.from(
            { length: count },
            (_, index) =>
                `create refs/tags/v1.0.${index}-${suffix}-${String(index).padStart(8, "0")} ${firstSha}\n`,
        ).join("");
        execFileSync("git", ["-C", repo, "update-ref", "--stdin"], { input: updates });
        const listing = execFileSync("git", ["-C", repo, "tag", "--list", "--no-column", "v*"], {
            encoding: "utf8",
            maxBuffer: 64 * 1024 * 1024,
        });
        expect(Buffer.byteLength(listing, "utf8")).toBeGreaterThan(1024 * 1024);

        const result = await poll({ kind: "git_tag_matching", repo_path: repo, pattern: "v*" });
        expect(result.events).toHaveLength(count);
    }, 20_000);
});

describe("compound scalar behavior", () => {
    test("OR evaluates up to four independent predicates and round-trips its scalar", async () => {
        const directory = await temporaryDirectory();
        const present = join(directory, "present");
        const missing = join(directory, "missing");
        await writeFile(present, "needle");
        const config = {
            any: [
                { kind: "path_exists", path: present },
                { kind: "path_exists", path: missing },
                { kind: "file_contains", path: present, needle: "needle" },
                { kind: "file_contains", path: present, needle: "absent" },
            ],
        } as const;

        const first = await poll(config);
        expect(first.events).toHaveLength(2);
        expect(Object.keys(first.scalar.predicates)).toHaveLength(4);
        expect((await poll(config, first.scalar)).events).toHaveLength(0);
    });

    test("accepts the authoring audit marker but still rejects unknown fields", () => {
        expect(
            validateProviderConfig({
                kind: "path_exists",
                path: "/tmp/future",
                resolved_path_exists: false,
            }),
        ).toEqual({
            success: true,
            config: {
                kind: "path_exists",
                path: "/tmp/future",
                resolved_path_exists: false,
            },
        });
        expect(
            validateProviderConfig({ kind: "path_exists", path: "/tmp/future", guessed: true }),
        ).toMatchObject({ success: false, reason: expect.stringContaining("unknown field") });
    });

    test("the authoring audit marker does not change predicate identity", async () => {
        const directory = await temporaryDirectory();
        const path = join(directory, "result.json");
        await writeFile(path, "{}");
        const first = await poll({ kind: "path_exists", path, resolved_path_exists: false });
        expect(first.events).toHaveLength(1);

        // Authoring regenerates the predicate after the path appears; the
        // stored scalar must still suppress the already-reported observation.
        const regenerated = await poll(
            { kind: "path_exists", path, resolved_path_exists: true },
            first.scalar,
        );
        expect(regenerated.events).toHaveLength(0);
        expect(Object.keys(regenerated.scalar.predicates)).toEqual(
            Object.keys(first.scalar.predicates),
        );
        const unmarked = await poll({ kind: "path_exists", path }, first.scalar);
        expect(unmarked.events).toHaveLength(0);
        expect(unmarked.events).toEqual(regenerated.events);
    });

    test("rejects compounds larger than four", async () => {
        const config = {
            any: Array.from({ length: 5 }, (_, index) => ({
                kind: "path_exists",
                path: `/tmp/${index}`,
            })),
        };
        await expect(runProvider({ scalar: null, config })).rejects.toMatchObject({
            code: "invalid_config",
        });
    });
});

describe("path fence", () => {
    test("refuses every sensitive Eidnara data root", async () => {
        const home = await temporaryDirectory("retina-local-fs-home-");
        for (const root of ["run", "context"]) {
            const path = join(home, ".local", "share", "eidnara", root, "secret.txt");
            await mkdir(join(path, ".."), { recursive: true });
            await writeFile(path, "secret");
            await expect(poll({ kind: "path_exists", path }, null, home)).rejects.toMatchObject({
                code: "fenced_path",
            });
        }
    });

    test("refuses every file under the storage root, including databases and RPC discovery files", async () => {
        const home = await temporaryDirectory("retina-local-fs-home-");
        const root = join(home, ".local", "share", "eidnara", "context");
        const paths = [
            join(root, "context.db"),
            join(root, "store.db-wal"),
            join(root, "rpc", "project", "port-123-instance.json"),
        ];
        for (const path of paths) {
            await mkdir(join(path, ".."), { recursive: true });
            await writeFile(path, "credential-bearing data");
            await expect(poll({ kind: "path_exists", path }, null, home)).rejects.toMatchObject({
                code: "fenced_path",
            });
        }
    });

    test("refuses the connection file under the runtime root", async () => {
        const home = await temporaryDirectory("retina-local-fs-home-");
        const path = join(home, ".local", "share", "eidnara", "run", "connection.json");
        await mkdir(join(path, ".."), { recursive: true });
        await writeFile(path, JSON.stringify({ key: "secret" }));

        await expect(
            poll({ kind: "file_contains", path, needle: "secret" }, null, home),
        ).rejects.toMatchObject({ code: "fenced_path" });
    });

    test("refuses an XDG-relocated connection file", async () => {
        const home = await temporaryDirectory("retina-local-fs-home-");
        const dataDirectory = await temporaryDirectory("retina-local-fs-xdg-");
        const path = join(dataDirectory, "eidnara", "run", "connection.json");
        await mkdir(join(path, ".."), { recursive: true });
        await writeFile(path, JSON.stringify({ key: "secret" }));

        process.env.XDG_DATA_HOME = dataDirectory;
        await expect(
            runProvider(
                { scalar: null, config: { kind: "path_exists", path } },
                { homeDirectory: home },
            ),
        ).rejects.toMatchObject({ code: "fenced_path" });
    });

    test("ignores a relative XDG_DATA_HOME and keeps fencing the home-derived root", async () => {
        // The daemon and the lifecycle resolver both reject a relative
        // XDG_DATA_HOME and fall back to $HOME/.local/share, so the real
        // managed tree lives under home. A fence that resolved the relative
        // value against cwd would compute its root elsewhere and admit the
        // very files it exists to refuse.
        const home = await temporaryDirectory("retina-local-fs-home-");
        const path = join(home, ".local", "share", "eidnara", "run", "connection.json");
        await mkdir(join(path, ".."), { recursive: true });
        await writeFile(path, JSON.stringify({ key: "secret" }));

        process.env.XDG_DATA_HOME = "./poisoned-relative-data";
        await expect(
            runProvider(
                { scalar: null, config: { kind: "path_exists", path } },
                { homeDirectory: home },
            ),
        ).rejects.toMatchObject({ code: "fenced_path" });
    });

    test("ignores an empty XDG_DATA_HOME and keeps fencing the home-derived root", async () => {
        const home = await temporaryDirectory("retina-local-fs-home-");
        const path = join(home, ".local", "share", "eidnara", "context", "context.db");
        await mkdir(join(path, ".."), { recursive: true });
        await writeFile(path, "credential-bearing data");

        process.env.XDG_DATA_HOME = "";
        await expect(
            runProvider(
                { scalar: null, config: { kind: "path_exists", path } },
                { homeDirectory: home },
            ),
        ).rejects.toMatchObject({ code: "fenced_path" });
    });

    test("admits a non-secret file under the Eidnara data root", async () => {
        const home = await temporaryDirectory("retina-local-fs-home-");
        const dataDirectory = await temporaryDirectory("retina-local-fs-xdg-");
        const path = join(dataDirectory, "eidnara", "docs", "notice.txt");
        await mkdir(join(path, ".."), { recursive: true });
        await writeFile(path, "public notice");

        const result = await poll(
            { kind: "file_contains", path, needle: "public" },
            null,
            home,
            dataDirectory,
        );
        expect(result.events).toHaveLength(1);
    });

    test("refuses a symlink swap after canonicalization", async () => {
        const home = await temporaryDirectory("retina-local-fs-home-");
        const path = join(home, "watched.txt");
        const original = join(home, "watched-original.txt");
        const sensitive = join(home, ".local", "share", "eidnara", "run", "connection.json");
        await writeFile(path, "safe");
        await mkdir(join(sensitive, ".."), { recursive: true });
        await writeFile(sensitive, JSON.stringify({ key: "secret" }));
        await expect(
            runProvider(
                {
                    scalar: null,
                    config: { kind: "file_contains", path, needle: "secret" },
                },
                {
                    homeDirectory: home,
                    dataDirectory: join(home, ".local", "share"),
                    beforePathUseForTests: async () => {
                        await rename(path, original);
                        await symlink(sensitive, path);
                    },
                },
            ),
        ).rejects.toMatchObject({ code: "fenced_path" });
    });

    test.skipIf(process.platform !== "linux")(
        "refuses an ancestor directory swapped for a symlink after the fence check",
        async () => {
            // O_NOFOLLOW applies only to the final component, so this swap opens
            // the fenced file; the descriptor's /proc/self/fd target exposes it.
            const home = await temporaryDirectory("retina-local-fs-home-");
            const watchedDirectory = join(home, "project");
            const path = join(watchedDirectory, "status.txt");
            const fencedDirectory = join(home, ".local", "share", "eidnara", "run");
            await mkdir(watchedDirectory);
            await writeFile(path, "safe");
            await mkdir(fencedDirectory, { recursive: true });
            await writeFile(join(fencedDirectory, "status.txt"), "secret");
            await expect(
                runProvider(
                    {
                        scalar: null,
                        config: { kind: "file_contains", path, needle: "secret" },
                    },
                    {
                        homeDirectory: home,
                        dataDirectory: join(home, ".local", "share"),
                        beforePathUseForTests: async () => {
                            await rename(watchedDirectory, `${watchedDirectory}-moved`);
                            await symlink(fencedDirectory, watchedDirectory);
                        },
                    },
                ),
            ).rejects.toMatchObject({ code: "fenced_path" });
        },
    );

    test("refuses sensitive basenames outside fenced roots", async () => {
        const home = await temporaryDirectory("retina-local-fs-home-");
        const paths = [
            join(home, "safe", "prod-binding-key-v2"),
            join(home, "safe", "operator.handle"),
            join(home, "safe", "writer.lease"),
            join(home, "project", "catalog", "dev-binding-key"),
            join(home, "project", "bin", "lease.handle"),
        ];
        for (const path of paths) {
            await mkdir(join(path, ".."), { recursive: true });
            await writeFile(path, "secret");
            await expect(poll({ kind: "path_exists", path }, null, home)).rejects.toMatchObject({
                code: "fenced_path",
            });
        }
    });

    test("resolves symlinks before refusing a fenced target", async () => {
        const home = await temporaryDirectory("retina-local-fs-home-");
        const target = join(home, ".local", "share", "eidnara", "run", "secret.txt");
        const link = join(home, "innocent-link");
        await mkdir(join(target, ".."), { recursive: true });
        await writeFile(target, "secret");
        await symlink(target, link);

        await expect(poll({ kind: "path_exists", path: link }, null, home)).rejects.toMatchObject({
            code: "fenced_path",
        });

        const missingTarget = join(home, ".local", "share", "eidnara", "context", "missing-secret");
        const danglingLink = join(home, "dangling-link");
        await mkdir(join(missingTarget, ".."), { recursive: true });
        await symlink(missingTarget, danglingLink);
        await expect(
            poll({ kind: "path_exists", path: danglingLink, gone: true }, null, home),
        ).rejects.toMatchObject({ code: "fenced_path" });
    });
});

describe("CLI exit discipline", () => {
    async function invoke(input: unknown, home: string) {
        const cli = fileURLToPath(new URL("./cli.ts", import.meta.url));
        const child = Bun.spawn({
            cmd: ["bun", cli],
            cwd: import.meta.dir,
            env: {
                ...process.env,
                HOME: home,
                XDG_DATA_HOME: join(home, ".local", "share"),
            },
            stdin: "pipe",
            stdout: "pipe",
            stderr: "pipe",
        });
        child.stdin.write(JSON.stringify(input));
        child.stdin.end();
        const [exitCode, stdout, stderr] = await Promise.all([
            child.exited,
            new Response(child.stdout).text(),
            new Response(child.stderr).text(),
        ]);
        return { exitCode, stdout, stderr };
    }

    test("returns zero and empty events for a readable unmatched file", async () => {
        const home = await temporaryDirectory("retina-local-fs-home-");
        const path = join(home, "readable.txt");
        await writeFile(path, "waiting");
        const result = await invoke(
            { scalar: null, config: { kind: "file_contains", path, needle: "ready" } },
            home,
        );
        expect(result.exitCode).toBe(0);
        expect(JSON.parse(result.stdout).events).toEqual([]);
        expect(result.stderr).toBe("");
    });

    test("returns nonzero JSON error when a path cannot be read", async () => {
        const home = await temporaryDirectory("retina-local-fs-home-");
        const directory = join(home, "not-a-file");
        await mkdir(directory);
        const result = await invoke(
            {
                scalar: null,
                config: { kind: "file_contains", path: directory, needle: "ready" },
            },
            home,
        );
        expect(result.exitCode).not.toBe(0);
        expect(result.stdout).toBe("");
        expect(JSON.parse(result.stderr)).toMatchObject({ code: "unreadable_path" });
        expect(result.stderr.trim().split("\n")).toHaveLength(1);
    });

    test("returns nonzero for invalid config and fence refusal", async () => {
        const home = await temporaryDirectory("retina-local-fs-home-");
        const invalidResult = await invoke(
            {
                scalar: null,
                config: { kind: "path_exists", path: home, token: "must-not-be-accepted" },
            },
            home,
        );
        expect(JSON.parse(invalidResult.stderr)).toMatchObject({ code: "invalid_config" });

        const fenced = join(home, ".local", "share", "eidnara", "run", "secret");
        await mkdir(join(fenced, ".."), { recursive: true });
        await writeFile(fenced, "secret");
        const fencedResult = await invoke(
            { scalar: null, config: { kind: "path_exists", path: fenced } },
            home,
        );
        expect(JSON.parse(fencedResult.stderr)).toMatchObject({ code: "fenced_path" });
    });
});

test("ProviderError remains machine distinguishable", () => {
    expect(new ProviderError("example", "message")).toMatchObject({
        name: "ProviderError",
        code: "example",
        message: "message",
    });
});
