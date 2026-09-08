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

async function git(repo: string, ...args: string[]): Promise<string> {
    const { stdout } = await execFileAsync("git", ["-C", repo, ...args], {
        encoding: "utf8",
        env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_SYSTEM: "/dev/null" },
    });
    return stdout.trim();
}

async function createTaggedRepository(dir: string): Promise<void> {
    await git(dir, "init", "--initial-branch=main");
    await git(dir, "config", "user.name", "Smart Note Test");
    await git(dir, "config", "user.email", "smart-note@example.invalid");
    await writeFile(path.join(dir, "state.txt"), "one\n");
    await git(dir, "add", "state.txt");
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
