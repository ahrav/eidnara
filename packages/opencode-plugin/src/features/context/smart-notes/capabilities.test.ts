import { describe, expect, test } from "bun:test";
import { execFile } from "node:child_process";
import { mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { promisify } from "node:util";

import { createSmartNoteCapabilities, isSecretDeniedPath } from "./capabilities";
import { SmartNoteNetworkError } from "./types";

const execFileAsync = promisify(execFile);

async function withTempDir<T>(fn: (dir: string) => Promise<T>): Promise<T> {
    const dir = await mkdtemp(path.join(tmpdir(), "eidnara-smart-note-cap-"));
    try {
        return await fn(dir);
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
}

const COMMIT_DATE = "2020-01-01T00:00:00Z";

async function git(repo: string, ...args: string[]): Promise<string> {
    const { stdout } = await execFileAsync("git", ["-C", repo, ...args], {
        encoding: "utf8",
        env: {
            ...process.env,
            GIT_CONFIG_GLOBAL: "/dev/null",
            GIT_CONFIG_SYSTEM: "/dev/null",
            GIT_AUTHOR_DATE: COMMIT_DATE,
            GIT_COMMITTER_DATE: COMMIT_DATE,
        },
    });
    return stdout.trim();
}

async function createTaggedRepository(dir: string): Promise<void> {
    await git(dir, "init", "--initial-branch=main");
    await git(dir, "config", "user.name", "Smart Note Test");
    await git(dir, "config", "user.email", "smart-note@example.invalid");
    await writeFile(path.join(dir, "state.txt"), "one\n");
    await writeFile(path.join(dir, ".env"), "TOKEN=1\n");
    await git(dir, "add", "state.txt", ".env");
    await git(dir, "commit", "-m", "Record the first tagged state");
    await git(dir, "tag", "v1.2.3");
}

describe("smart-note readFile capability", () => {
    test("reads regular project files", async () => {
        await withTempDir(async (dir) => {
            await writeFile(path.join(dir, "README.md"), "hello", "utf8");
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });
            expect(await cap.readFile("README.md")).toBe("hello");
        });
    });

    test("keeps leading and trailing whitespace in names and rejects blank input", async () => {
        await withTempDir(async (dir) => {
            await writeFile(path.join(dir, "status.txt"), "plain", "utf8");
            await writeFile(path.join(dir, " status.txt"), "leading space", "utf8");
            await writeFile(path.join(dir, "status.txt "), "trailing space", "utf8");
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });
            expect(await cap.readFile("status.txt")).toBe("plain");
            expect(await cap.readFile(" status.txt")).toBe("leading space");
            expect(await cap.readFile("status.txt ")).toBe("trailing space");
            expect(await cap.readFile("   ")).toBeNull();
            expect(await cap.readFile("")).toBeNull();
        });
    });

    test("rejects a file limit that would disable the size check", async () => {
        for (const fileLimitBytes of [
            Number.NaN,
            Number.POSITIVE_INFINITY,
            0,
            -1,
            16 * 1024 * 1024 + 1,
        ]) {
            expect(() =>
                createSmartNoteCapabilities({
                    projectRoot: "/",
                    signal: new AbortController().signal,
                    fileLimitBytes,
                }),
            ).toThrow(RangeError);
        }
    });

    test("treats a backslash as an ordinary byte where the host does not use it as a separator", async () => {
        if (process.platform === "win32") return;
        await withTempDir(async (dir) => {
            await mkdir(path.join(dir, "a"));
            await writeFile(path.join(dir, "a", "b"), "nested", "utf8");
            await writeFile(path.join(dir, "a\\b"), "literal backslash", "utf8");
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });
            expect(await cap.readFile("a/b")).toBe("nested");
            expect(await cap.readFile("a\\b")).toBe("literal backslash");
        });
    });

    test("accepts in-tree names whose first component begins with dots", async () => {
        await withTempDir(async (dir) => {
            await mkdir(path.join(dir, "..generated"));
            await writeFile(path.join(dir, "..generated", "out.txt"), "generated", "utf8");
            await writeFile(path.join(dir, "..notes"), "dotted", "utf8");
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });
            expect(await cap.readFile("..generated/out.txt")).toBe("generated");
            expect(await cap.readFile("..notes")).toBe("dotted");
            expect(await cap.readFile("../outside.txt")).toBeNull();
            expect(await cap.readFile("..")).toBeNull();
        });
    });

    test("denies secrets by path pattern", async () => {
        await withTempDir(async (dir) => {
            await mkdir(path.join(dir, "secrets"));
            for (const file of [".env", ".env.local", ".envrc", ".env-prod", ".env_local"]) {
                await writeFile(path.join(dir, file), "TOKEN=1", "utf8");
            }
            await writeFile(path.join(dir, "env.ts"), "export const env = {};", "utf8");
            await writeFile(path.join(dir, "environment.md"), "not a secret", "utf8");
            await writeFile(path.join(dir, ".npmrc"), "//token", "utf8");
            await writeFile(path.join(dir, "id_ed25519"), "key", "utf8");
            await writeFile(path.join(dir, "cert.pem"), "pem", "utf8");
            await writeFile(path.join(dir, "secrets", "value.txt"), "secret", "utf8");
            await mkdir(path.join(dir, ".env.production"));
            await writeFile(path.join(dir, ".env.production", "token"), "secret", "utf8");
            await mkdir(path.join(dir, "config", ".env"), { recursive: true });
            await writeFile(path.join(dir, "config", ".env", "token"), "secret", "utf8");
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });
            for (const file of [
                ".env",
                ".env.local",
                ".envrc",
                ".env-prod",
                ".env_local",
                ".npmrc",
                "id_ed25519",
                "cert.pem",
                "secrets/value.txt",
                ".env.production/token",
                "config/.env/token",
            ]) {
                expect(await cap.readFile(file)).toBeNull();
            }
            expect(await cap.readFile("env.ts")).toBe("export const env = {};");
            expect(await cap.readFile("environment.md")).toBe("not a secret");
        });
    });

    test("rechecks canonical directory symlinks against the secret denylist", async () => {
        await withTempDir(async (dir) => {
            await mkdir(path.join(dir, "secrets"));
            await writeFile(path.join(dir, "secrets", "token.json"), "secret", "utf8");
            await symlink(path.join(dir, "secrets"), path.join(dir, "public"));
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });

            expect(await cap.readFile("public/token.json")).toBeNull();
        });
    });

    test("allows benign directory symlinks that remain inside the project", async () => {
        await withTempDir(async (dir) => {
            await mkdir(path.join(dir, "docs"));
            await writeFile(path.join(dir, "docs", "guide.txt"), "guide", "utf8");
            await symlink(path.join(dir, "docs"), path.join(dir, "public-docs"));
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });

            expect(await cap.readFile("public-docs/guide.txt")).toBe("guide");
        });
    });

    test("denies final symlink and parent symlink escapes", async () => {
        await withTempDir(async (dir) => {
            const outside = await mkdtemp(path.join(tmpdir(), "eidnara-smart-note-outside-"));
            try {
                await writeFile(path.join(outside, "outside.txt"), "outside", "utf8");
                await symlink(path.join(outside, "outside.txt"), path.join(dir, "link.txt"));
                await symlink(outside, path.join(dir, "linked-dir"));
                const cap = createSmartNoteCapabilities({
                    projectRoot: dir,
                    signal: new AbortController().signal,
                });
                expect(await cap.readFile("link.txt")).toBeNull();
                expect(await cap.readFile("linked-dir/outside.txt")).toBeNull();
            } finally {
                await rm(outside, { recursive: true, force: true });
            }
        });
    });
});

describe("smart-note git capabilities", () => {
    test("gitTag returns the bare tag in a dirty worktree", async () => {
        await withTempDir(async (dir) => {
            await createTaggedRepository(dir);
            await writeFile(path.join(dir, "state.txt"), "two\n");
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });
            expect(await cap.gitTag()).toBe("v1.2.3");
        });
    });

    test("gitTag is null in a repository with commits but no tags", async () => {
        await withTempDir(async (dir) => {
            await createTaggedRepository(dir);
            await git(dir, "tag", "-d", "v1.2.3");
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });
            expect(await cap.gitTag()).toBeNull();
            expect(await cap.gitHeadSha()).toMatch(/^[0-9a-f]{40}$/);
        });
    });

    test("ordinary git failures resolve to an empty result", async () => {
        await withTempDir(async (dir) => {
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });
            expect(await cap.gitHeadSha()).toBeNull();
            expect(await cap.gitTag()).toBeNull();
            expect(await cap.gitLog()).toEqual([]);
        });
    });

    test("a repository with no commits yields empty answers", async () => {
        await withTempDir(async (dir) => {
            await git(dir, "init", "--initial-branch=main");
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });
            expect(await cap.gitHeadSha()).toBeNull();
            expect(await cap.gitTag()).toBeNull();
            expect(await cap.gitLog()).toEqual([]);
        });
    });

    test("a repository git cannot read rejects instead of answering empty", async () => {
        await withTempDir(async (dir) => {
            await createTaggedRepository(dir);
            await writeFile(path.join(dir, ".git", "HEAD"), "garbage\n", "utf8");
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });
            await expect(cap.gitHeadSha()).rejects.toBeInstanceOf(SmartNoteNetworkError);
            await expect(cap.gitTag()).rejects.toBeInstanceOf(SmartNoteNetworkError);
            await expect(cap.gitLog()).rejects.toBeInstanceOf(SmartNoteNetworkError);
        });
    });

    test("an unparseable since filter selects no commits instead of widening the log", async () => {
        await withTempDir(async (dir) => {
            await createTaggedRepository(dir);
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });
            expect(await cap.gitLog({ since: "2019/01/01" })).toHaveLength(1);
            expect(await cap.gitLog({ since: "Jan 1, 2019" })).toHaveLength(1);
            expect(await cap.gitLog({ since: "2999/01/01" })).toEqual([]);
            expect(await cap.gitLog({ since: "not a date" })).toEqual([]);
            expect(await cap.gitLog({ since: "2019-01-01\n" })).toEqual([]);
            expect(await cap.gitLog({ since: "" })).toEqual([]);
        });
    });

    test("pathspec magic cannot reach a denied path or widen the requested history", async () => {
        await withTempDir(async (dir) => {
            await createTaggedRepository(dir);
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });
            expect(await cap.gitLog({ path: "state.txt" })).toHaveLength(1);
            expect(await cap.gitLog({ path: ".env" })).toEqual([]);
            expect(await cap.gitLog({ path: ":(top).env" })).toEqual([]);
            expect(await cap.gitLog({ path: ":/.env" })).toEqual([]);
            expect(await cap.gitLog({ path: "*" })).toEqual([]);
        });
    });

    test("repository-local git environment does not redirect queries away from the project", async () => {
        await withTempDir(async (project) => {
            await withTempDir(async (other) => {
                await createTaggedRepository(project);
                await createTaggedRepository(other);
                await writeFile(path.join(other, "state.txt"), "two\n");
                await git(
                    other,
                    "commit",
                    "-am",
                    "Record the second state in the other repository",
                );
                const projectHead = await git(project, "rev-parse", "HEAD");
                const otherHead = await git(other, "rev-parse", "HEAD");
                expect(projectHead).not.toBe(otherHead);

                const saved = {
                    GIT_DIR: process.env.GIT_DIR,
                    GIT_WORK_TREE: process.env.GIT_WORK_TREE,
                };
                process.env.GIT_DIR = path.join(other, ".git");
                process.env.GIT_WORK_TREE = other;
                try {
                    const cap = createSmartNoteCapabilities({
                        projectRoot: project,
                        signal: new AbortController().signal,
                    });
                    expect(await cap.gitHeadSha()).toBe(projectHead);
                    expect((await cap.gitLog())[0]?.sha).toBe(projectHead);
                } finally {
                    for (const [name, value] of Object.entries(saved)) {
                        if (value === undefined) delete process.env[name];
                        else process.env[name] = value;
                    }
                }
            });
        });
    });

    test("a commit subject keeps every delimiter character git passes through", async () => {
        await withTempDir(async (dir) => {
            await createTaggedRepository(dir);
            const subject = "Record a subject\u001fwith an embedded separator\u001ftwice";
            await git(dir, "commit", "--allow-empty", "-m", subject);
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: new AbortController().signal,
            });
            const [latest] = await cap.gitLog({ maxCount: 1 });
            expect(latest?.subject).toBe(subject);
            // `%aI` renders UTC as `Z` on newer git and `+00:00` on older git; compare the instant.
            expect(Date.parse(latest?.authorDate ?? "")).toBe(Date.parse(COMMIT_DATE));
            expect(latest?.sha).toMatch(/^[0-9a-f]{40}$/);
        });
    });

    test("a git binary that cannot be spawned rejects instead of reporting empty history", async () => {
        await withTempDir(async (dir) => {
            await createTaggedRepository(dir);
            const savedPath = process.env.PATH;
            process.env.PATH = path.join(dir, "no-such-bin");
            try {
                const cap = createSmartNoteCapabilities({
                    projectRoot: dir,
                    signal: new AbortController().signal,
                });
                await expect(cap.gitHeadSha()).rejects.toBeInstanceOf(SmartNoteNetworkError);
                await expect(cap.gitLog()).rejects.toBeInstanceOf(SmartNoteNetworkError);
            } finally {
                if (savedPath === undefined) delete process.env.PATH;
                else process.env.PATH = savedPath;
            }
        });
    });

    test("aborted git calls reject instead of masquerading as empty results", async () => {
        await withTempDir(async (dir) => {
            await createTaggedRepository(dir);
            const controller = new AbortController();
            controller.abort(new Error("sweep deadline"));
            const cap = createSmartNoteCapabilities({
                projectRoot: dir,
                signal: controller.signal,
            });
            await expect(cap.gitHeadSha()).rejects.toBeInstanceOf(SmartNoteNetworkError);
            await expect(cap.gitTag()).rejects.toBeInstanceOf(SmartNoteNetworkError);
            await expect(cap.gitLog()).rejects.toBeInstanceOf(SmartNoteNetworkError);
        });
    });
});

describe("smart-note secret path denylist", () => {
    test("blocks common credential file names and key material", () => {
        for (const file of [
            ".aws/credentials",
            ".pgpass",
            ".netrc",
            "certs/client.p12",
            "certs/client.pfx",
            "certs/server.crt",
            "certs/server.key",
            "certs/server.pem",
            "prod-service-account.json",
            "prod_service_account.json",
            ".config/gcloud/application_default_credentials.json",
            "gcloud/legacy_credentials/user/adc.json",
        ]) {
            expect(isSecretDeniedPath(file)).toBe(true);
        }
    });

    test("does not block ordinary similarly named project files", () => {
        for (const file of [
            "docs/certificate-notes.md",
            "src/keymap.ts",
            "service-accounting/report.json",
            "docs/gcloud-setup.md",
        ]) {
            expect(isSecretDeniedPath(file)).toBe(false);
        }
    });
});
