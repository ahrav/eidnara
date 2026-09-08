import { afterEach, describe, expect, it, mock } from "bun:test";
import type { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
    chmodSync,
    existsSync,
    mkdirSync,
    mkdtempSync,
    realpathSync,
    rmSync,
    symlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path, { join } from "node:path";
import {
    __clearProjectIdentityResolutionCacheForTests,
    __clearProjectIdentityTransientCooldownForTests,
    __resetProjectIdentityForTests,
    __setProjectIdentityTestHooks,
    ProjectIdentityError,
    resolveProjectIdentity,
    resolveProjectIdentityStrict,
    takeDubiousOwnershipProjectIdentityWarning,
} from "./project-identity";

const tempDirs: string[] = [];
const FIRST_ROOT_COMMIT = "abcdef1234567890abcdef1234567890abcdef12";
const SECOND_ROOT_COMMIT = "1234567890abcdef1234567890abcdef12345678";

afterEach(() => {
    __resetProjectIdentityForTests();
    for (const dir of tempDirs) {
        try {
            chmodSync(dir, 0o755);
        } catch {}
        rmSync(dir, { recursive: true, force: true });
    }
    tempDirs.length = 0;
});

function makeTempDir(prefix: string): string {
    const dir = mkdtempSync(join(tmpdir(), prefix));
    tempDirs.push(dir);
    return dir;
}

function makeRepoWithGitMetadata(prefix: string): string {
    const dir = makeTempDir(prefix);
    mkdirSync(join(dir, ".git"));
    return dir;
}

function returningRootCommit(rootCommit: string): typeof execFileSync {
    // The test hook supplies deterministic git output while `.git` remains on disk.
    return (() => `${rootCommit}\n`) as typeof execFileSync;
}

function expectedDirIdentity(directory: string): string {
    const resolved = path.resolve(directory);
    let canonical: string;
    try {
        canonical = realpathSync.native(resolved);
    } catch {
        canonical = resolved;
    }
    return `dir:${createHash("md5").update(canonical, "utf8").digest("hex").slice(0, 12)}`;
}

function expectProjectIdentityError(fn: () => void): ProjectIdentityError {
    let caught: unknown;
    try {
        fn();
    } catch (error) {
        caught = error;
    }

    expect(caught).toBeInstanceOf(ProjectIdentityError);
    if (!(caught instanceof ProjectIdentityError)) {
        throw new Error("Expected ProjectIdentityError");
    }
    return caught;
}

function makeGitFailure(fields: {
    stderr?: string;
    code?: string;
    signal?: string;
    killed?: boolean;
}) {
    const error = new Error("git failed") as Error & {
        stderr?: Buffer;
        code?: string;
        signal?: string;
        killed?: boolean;
    };
    if (fields.stderr !== undefined) error.stderr = Buffer.from(fields.stderr, "utf8");
    if (fields.code !== undefined) error.code = fields.code;
    if (fields.signal !== undefined) error.signal = fields.signal;
    if (fields.killed !== undefined) error.killed = fields.killed;
    return error;
}

describe("project identity", () => {
    it("resolveProjectIdentityStrict returns the git root commit identity", () => {
        const repo = makeRepoWithGitMetadata("project-identity-git-");
        __setProjectIdentityTestHooks({ execFileSync: returningRootCommit(FIRST_ROOT_COMMIT) });

        const identity = resolveProjectIdentityStrict(repo);

        expect(identity).toBe(`git:${FIRST_ROOT_COMMIT}`);
        expect(identity.slice("git:".length).length).toBeGreaterThanOrEqual(7);
    });

    it("resolveProjectIdentityStrict throws not_git_repo for non-git directories", () => {
        const directory = makeTempDir("project-identity-non-git-");

        const error = expectProjectIdentityError(() => resolveProjectIdentityStrict(directory));

        expect(error.errorClass).toBe("not_git_repo");
        expect(error.rawDirectory).toBe(directory);
    });

    it("resolveProjectIdentityStrict classifies missing directories without falling back", () => {
        const directory = makeTempDir("project-identity-missing-");
        rmSync(directory, { recursive: true, force: true });

        const error = expectProjectIdentityError(() => resolveProjectIdentityStrict(directory));

        expect(["permission_denied", "unknown"]).toContain(error.errorClass);
        expect(error.rawDirectory).toBe(directory);
    });

    it("resolveProjectIdentityStrict caches git identities across calls", () => {
        const repo = makeRepoWithGitMetadata("project-identity-cache-");
        __setProjectIdentityTestHooks({ execFileSync: returningRootCommit(FIRST_ROOT_COMMIT) });
        const first = resolveProjectIdentityStrict(repo);

        chmodSync(repo, 0o000);
        try {
            expect(resolveProjectIdentityStrict(repo)).toBe(first);
        } finally {
            chmodSync(repo, 0o755);
        }
    });

    it("drops a cached git identity when an accessible directory loses its metadata", () => {
        const repo = makeRepoWithGitMetadata("project-identity-metadata-removed-");
        const execMock = mock(returningRootCommit(FIRST_ROOT_COMMIT));
        __setProjectIdentityTestHooks({ execFileSync: execMock as unknown as typeof execFileSync });

        expect(resolveProjectIdentity(repo)).toBe(`git:${FIRST_ROOT_COMMIT}`);
        rmSync(join(repo, ".git"), { recursive: true, force: true });

        expect(resolveProjectIdentity(repo)).toBe(expectedDirIdentity(repo));
        expect(execMock).toHaveBeenCalledTimes(1);
    });

    it("retains a cached git identity while its directory is missing", () => {
        const repo = makeRepoWithGitMetadata("project-identity-cached-missing-");
        __setProjectIdentityTestHooks({ execFileSync: returningRootCommit(FIRST_ROOT_COMMIT) });
        const identity = resolveProjectIdentity(repo);

        rmSync(repo, { recursive: true, force: true });

        expect(resolveProjectIdentity(repo)).toBe(identity);
    });

    it("resolveProjectIdentity falls back to dir identity for non-git directories", () => {
        const directory = makeTempDir("project-identity-wrapper-");

        expect(resolveProjectIdentity(directory)).toBe(expectedDirIdentity(directory));
    });

    it("uses the no-git fast path without invoking git", () => {
        const directory = makeTempDir("project-identity-no-git-fast-");
        const execMock = mock(() => {
            throw new Error("git should not be launched for a directory with no .git ancestor");
        });
        __setProjectIdentityTestHooks({ execFileSync: execMock as unknown as typeof execFileSync });

        expect(resolveProjectIdentity(directory)).toBe(expectedDirIdentity(directory));
        expect(execMock).not.toHaveBeenCalled();
    });

    it("detects git metadata in ancestors for subdirectory sessions", () => {
        const repo = makeRepoWithGitMetadata("project-identity-ancestor-");
        const subdir = join(repo, "nested", "session");
        mkdirSync(subdir, { recursive: true });
        __setProjectIdentityTestHooks({ execFileSync: returningRootCommit(FIRST_ROOT_COMMIT) });

        expect(resolveProjectIdentity(subdir)).toBe(`git:${FIRST_ROOT_COMMIT}`);
    });

    it("detects git metadata through symlinked checkout paths", () => {
        const repo = makeRepoWithGitMetadata("project-identity-symlink-repo-");
        const subdir = join(repo, "nested", "session");
        mkdirSync(subdir, { recursive: true });
        const linkParent = makeTempDir("project-identity-symlink-parent-");
        const link = join(linkParent, "checkout");
        try {
            symlinkSync(subdir, link, "dir");
        } catch (error) {
            if ((error as { code?: unknown }).code === "EPERM") return;
            throw error;
        }
        const execMock = mock(() => `${FIRST_ROOT_COMMIT}\n`);
        __setProjectIdentityTestHooks({ execFileSync: execMock as unknown as typeof execFileSync });

        expect(resolveProjectIdentity(link)).toBe(`git:${FIRST_ROOT_COMMIT}`);
        expect(execMock).toHaveBeenCalledTimes(1);
    });

    it("gives a non-git directory one dir: identity through its symlink and its real path", () => {
        const target = makeTempDir("project-identity-dir-symlink-target-");
        const linkParent = makeTempDir("project-identity-dir-symlink-parent-");
        const link = join(linkParent, "alias");
        try {
            symlinkSync(target, link, "dir");
        } catch (error) {
            if ((error as { code?: unknown }).code === "EPERM") return;
            throw error;
        }

        expect(resolveProjectIdentity(link)).toBe(resolveProjectIdentity(target));
        expect(resolveProjectIdentity(link)).toBe(expectedDirIdentity(target));
    });

    it("re-resolves a cached subdirectory once it becomes its own repository", () => {
        const outer = makeRepoWithGitMetadata("project-identity-outer-then-nested-");
        const nested = join(outer, "packages", "child");
        mkdirSync(nested, { recursive: true });
        // Git answers for the nearest `.git` above its cwd, so the mock keys on whether the nested repository exists yet.
        const execMock = mock(() =>
            existsSync(join(nested, ".git")) ? `${SECOND_ROOT_COMMIT}\n` : `${FIRST_ROOT_COMMIT}\n`,
        );
        __setProjectIdentityTestHooks({ execFileSync: execMock as unknown as typeof execFileSync });

        expect(resolveProjectIdentity(nested)).toBe(`git:${FIRST_ROOT_COMMIT}`);
        expect(resolveProjectIdentity(nested)).toBe(`git:${FIRST_ROOT_COMMIT}`);
        expect(execMock).toHaveBeenCalledTimes(1);
        mkdirSync(join(nested, ".git"));
        expect(resolveProjectIdentity(nested)).toBe(`git:${SECOND_ROOT_COMMIT}`);
        expect(execMock).toHaveBeenCalledTimes(2);
    });

    it("does not reuse an enclosing repository's identity for a nested repository during a transient failure", () => {
        const outer = makeRepoWithGitMetadata("project-identity-outer-");
        const nested = join(outer, "vendor", "child");
        mkdirSync(join(nested, ".git"), { recursive: true });
        let failNested = false;
        const execMock = mock((_file: string, _args: string[], options: { cwd?: string }) => {
            if (options.cwd === realpathSync.native(nested) || options.cwd === nested) {
                if (failNested) throw makeGitFailure({ code: "ETIMEDOUT" });
                return `${SECOND_ROOT_COMMIT}\n`;
            }
            return `${FIRST_ROOT_COMMIT}\n`;
        });
        __setProjectIdentityTestHooks({ execFileSync: execMock as unknown as typeof execFileSync });

        expect(resolveProjectIdentity(outer)).toBe(`git:${FIRST_ROOT_COMMIT}`);
        failNested = true;

        expect(resolveProjectIdentity(nested)).not.toBe(`git:${FIRST_ROOT_COMMIT}`);
        expect(resolveProjectIdentity(nested)).toBe(expectedDirIdentity(nested));
    });

    it("reuses the last successful git identity during transient failures and cooldown", () => {
        const directory = makeRepoWithGitMetadata("project-identity-last-known-");
        let now = 1_000;
        let mode: "first" | "timeout" | "second" = "first";
        const execMock = mock(() => {
            if (mode === "first") return `${FIRST_ROOT_COMMIT}\n`;
            if (mode === "second") return `${SECOND_ROOT_COMMIT}\n`;
            throw makeGitFailure({ code: "ETIMEDOUT" });
        });
        __setProjectIdentityTestHooks({
            execFileSync: execMock as unknown as typeof execFileSync,
            nowMs: () => now,
        });

        expect(resolveProjectIdentity(directory)).toBe(`git:${FIRST_ROOT_COMMIT}`);
        __clearProjectIdentityResolutionCacheForTests(directory);
        mode = "timeout";

        expect(resolveProjectIdentity(directory)).toBe(`git:${FIRST_ROOT_COMMIT}`);
        expect(resolveProjectIdentity(directory)).toBe(`git:${FIRST_ROOT_COMMIT}`);
        expect(execMock).toHaveBeenCalledTimes(2);

        mode = "second";
        now += 5 * 60 * 1000 + 1;
        expect(resolveProjectIdentity(directory)).toBe(`git:${SECOND_ROOT_COMMIT}`);
        expect(execMock).toHaveBeenCalledTimes(3);
    });

    it("classifies dubious ownership and falls back until the cooldown expires", () => {
        const directory = makeRepoWithGitMetadata("project-identity-dubious-");
        let now = 1_000;
        let recovered = false;
        const execMock = mock(() => {
            if (recovered) return `${FIRST_ROOT_COMMIT}\n`;
            throw makeGitFailure({
                stderr:
                    "fatal: detected dubious ownership in repository at '/repo'\n" +
                    "To add an exception for this directory, call:\n",
            });
        });
        __setProjectIdentityTestHooks({
            execFileSync: execMock as unknown as typeof execFileSync,
            nowMs: () => now,
        });

        const strictError = expectProjectIdentityError(() =>
            resolveProjectIdentityStrict(directory),
        );
        expect(strictError.errorClass).toBe("dubious_ownership");

        expect(resolveProjectIdentity(directory)).toBe(expectedDirIdentity(directory));
        expect(takeDubiousOwnershipProjectIdentityWarning(directory)).toContain(
            "git config --global --add safe.directory",
        );
        expect(takeDubiousOwnershipProjectIdentityWarning(directory)).toBeNull();

        recovered = true;
        expect(resolveProjectIdentity(directory)).toBe(expectedDirIdentity(directory));
        expect(execMock).toHaveBeenCalledTimes(2);

        now += 5 * 60 * 1000 + 1;
        expect(resolveProjectIdentity(directory)).toBe(`git:${FIRST_ROOT_COMMIT}`);
        expect(execMock).toHaveBeenCalledTimes(3);
    });

    it("cools down git timeouts so immediate retries do not rerun git", () => {
        const directory = makeRepoWithGitMetadata("project-identity-timeout-");
        const execMock = mock(() => {
            throw makeGitFailure({ code: "ETIMEDOUT" });
        });
        __setProjectIdentityTestHooks({ execFileSync: execMock as unknown as typeof execFileSync });

        expect(resolveProjectIdentity(directory)).toBe(expectedDirIdentity(directory));
        expect(resolveProjectIdentity(directory)).toBe(expectedDirIdentity(directory));
        expect(execMock).toHaveBeenCalledTimes(1);
    });

    it("re-probes after a transient cooldown is cleared", () => {
        const directory = makeRepoWithGitMetadata("project-identity-clear-cooldown-");
        let recovered = false;
        const execMock = mock(() => {
            if (recovered) return `${FIRST_ROOT_COMMIT}\n`;
            throw makeGitFailure({ code: "ETIMEDOUT" });
        });
        __setProjectIdentityTestHooks({ execFileSync: execMock as unknown as typeof execFileSync });

        expect(resolveProjectIdentity(directory)).toBe(expectedDirIdentity(directory));
        recovered = true;
        __clearProjectIdentityTransientCooldownForTests(directory);

        expect(resolveProjectIdentity(directory)).toBe(`git:${FIRST_ROOT_COMMIT}`);
        expect(execMock).toHaveBeenCalledTimes(2);
    });

    it("still propagates permission_denied git failures", () => {
        const directory = makeRepoWithGitMetadata("project-identity-permission-");
        __setProjectIdentityTestHooks({
            execFileSync: mock(() => {
                throw makeGitFailure({ code: "EACCES" });
            }) as unknown as typeof execFileSync,
        });

        const error = expectProjectIdentityError(() => resolveProjectIdentity(directory));

        expect(error.errorClass).toBe("permission_denied");
    });

    it("ProjectIdentityError carries classification and raw directory fields", () => {
        const cause = new Error("inner");
        const error = new ProjectIdentityError("git_timeout", "/raw/path", "timed out", cause);

        expect(error.name).toBe("ProjectIdentityError");
        expect(error.errorClass).toBe("git_timeout");
        expect(error.rawDirectory).toBe("/raw/path");
        expect(error.cause).toBe(cause);
    });
});
