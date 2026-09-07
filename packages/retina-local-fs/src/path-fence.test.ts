import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import hostRelease from "../../../release/host-release.json";
import {
    isFencedPath,
    managedLayout,
    resolveAndFenceProviderPath,
    revalidateProviderPath,
} from "./path-fence";

const tempRoots: string[] = [];
const originalXdgDataHome = process.env.XDG_DATA_HOME;
const originalHome = process.env.HOME;

function restoreEnv(name: string, value: string | undefined): void {
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
}

beforeEach(() => {
    delete process.env.XDG_DATA_HOME;
});

afterEach(async () => {
    restoreEnv("XDG_DATA_HOME", originalXdgDataHome);
    restoreEnv("HOME", originalHome);
    await Promise.all(
        tempRoots.splice(0).map((root) => rm(root, { recursive: true, force: true })),
    );
});

async function makeHome(): Promise<string> {
    const home = await realpath(await mkdtemp(join(tmpdir(), "retina-fence-")));
    tempRoots.push(home);
    return home;
}

async function writeFileAt(path: string, contents: string): Promise<void> {
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, contents);
}

describe("managed layout", () => {
    test("fence names equal the release contract's layout values", () => {
        expect(managedLayout).toEqual({
            managedSubtree: hostRelease.layout.managed_subtree,
            runtimeDirectory: hostRelease.layout.runtime_directory,
            storageSubdirectory: hostRelease.layout.storage_subdirectory,
        });
    });
});

describe("isFencedPath", () => {
    const home = "/home/user";
    const share = join(home, ".local", "share");

    test("fences the runtime and storage roots under the managed subtree", () => {
        expect(isFencedPath(join(share, "eidnara", "run", "connection.json"), share)).toBe(true);
        expect(isFencedPath(join(share, "eidnara", "context", "notes.db"), share)).toBe(true);
        expect(isFencedPath(join(share, "eidnara", "run"), share)).toBe(true);
    });

    test("fences key-material basenames at any location", () => {
        expect(isFencedPath(join(home, "projects", "binding-key"), share)).toBe(true);
        expect(isFencedPath(join(home, "projects", "x-binding-key.txt"), share)).toBe(true);
        expect(isFencedPath(join("/tmp", "lease.handle"), share)).toBe(true);
    });

    test("admits the managed subtree's other children and everything outside it", () => {
        expect(isFencedPath(join(share, "eidnara", "docs", "notice.txt"), share)).toBe(false);
        expect(isFencedPath(join(share, "eidnara"), share)).toBe(false);
        expect(isFencedPath(join(share, "eidnara", "runner", "x"), share)).toBe(false);
        expect(isFencedPath(join(share, "eidnara", "contexts", "x"), share)).toBe(false);
        expect(isFencedPath(join(share, "eidnara-other", "run", "x"), share)).toBe(false);
        expect(isFencedPath(join(home, "workspace", "result.json"), share)).toBe(false);
        expect(isFencedPath(join(share, "run", "connection.json"), share)).toBe(false);
    });

    test("honors an explicit data directory over XDG_DATA_HOME", () => {
        const data = "/srv/data";
        expect(isFencedPath(join(data, "eidnara", "run", "x"), data)).toBe(true);
        expect(isFencedPath(join(share, "eidnara", "run", "x"), data)).toBe(false);
    });
});

describe("resolveAndFenceProviderPath", () => {
    test("refuses fenced paths under $XDG_DATA_HOME and returns canonical admitted paths", async () => {
        const home = await makeHome();
        const dataDirectory = join(home, "xdg");
        process.env.XDG_DATA_HOME = dataDirectory;
        const fenced = join(dataDirectory, "eidnara", "run", "connection.json");
        await writeFileAt(fenced, "{}");
        await expect(
            resolveAndFenceProviderPath(fenced, { allowMissing: false, homeDirectory: home }),
        ).rejects.toMatchObject({ code: "fenced_path" });

        const admitted = join(home, "workspace", "result.json");
        await writeFileAt(admitted, "{}");
        const alias = join(home, "alias.json");
        await symlink(admitted, alias);
        await expect(
            resolveAndFenceProviderPath(alias, { allowMissing: false, homeDirectory: home }),
        ).resolves.toBe(admitted);
        await expect(
            resolveAndFenceProviderPath("~/workspace/result.json", {
                allowMissing: false,
                homeDirectory: home,
            }),
        ).resolves.toBe(admitted);
    });

    test("refuses key-material basenames outside the managed subtree", async () => {
        const home = await makeHome();
        const handle = join(home, "workspace", "lease.handle");
        await writeFileAt(handle, "");
        await expect(
            resolveAndFenceProviderPath(handle, { allowMissing: false, homeDirectory: home }),
        ).rejects.toMatchObject({ code: "fenced_path" });
    });

    test("ignores a relative or empty XDG_DATA_HOME and fences the home-derived root", async () => {
        const home = await makeHome();
        const fenced = join(home, ".local", "share", "eidnara", "context", "notes.db");
        await writeFileAt(fenced, "");
        for (const value of ["./relative-data", ""]) {
            process.env.XDG_DATA_HOME = value;
            await expect(
                resolveAndFenceProviderPath(fenced, { allowMissing: false, homeDirectory: home }),
            ).rejects.toMatchObject({ code: "fenced_path" });
        }
    });

    test("treats an empty explicit data directory as absent", async () => {
        const home = await makeHome();
        const fenced = join(home, ".local", "share", "eidnara", "run", "connection.json");
        await writeFileAt(fenced, "{}");
        await expect(
            resolveAndFenceProviderPath(fenced, {
                allowMissing: false,
                homeDirectory: home,
                dataDirectory: "",
            }),
        ).rejects.toMatchObject({ code: "fenced_path" });
    });

    test("treats an empty explicit home directory as absent", async () => {
        const home = await makeHome();
        process.env.HOME = home;
        const fenced = join(home, ".local", "share", "eidnara", "run", "connection.json");
        await writeFileAt(fenced, "{}");
        await expect(
            resolveAndFenceProviderPath(fenced, { allowMissing: false, homeDirectory: "" }),
        ).rejects.toMatchObject({ code: "fenced_path" });
    });

    test("refuses a relative explicit home or data directory instead of rooting it at cwd", async () => {
        const home = await makeHome();
        const fenced = join(home, ".local", "share", "eidnara", "run", "connection.json");
        await writeFileAt(fenced, "{}");
        await expect(
            resolveAndFenceProviderPath(fenced, {
                allowMissing: false,
                homeDirectory: "relative-home",
            }),
        ).rejects.toMatchObject({ code: "invalid_option" });
        await expect(
            resolveAndFenceProviderPath(fenced, {
                allowMissing: false,
                homeDirectory: home,
                dataDirectory: "relative-data",
            }),
        ).rejects.toMatchObject({ code: "invalid_option" });
    });

    test("checks a symlinked data directory by its real path", async () => {
        const home = await makeHome();
        const realShare = join(home, "real-share");
        const fenced = join(realShare, "eidnara", "run", "connection.json");
        await writeFileAt(fenced, "{}");
        const linkedShare = join(home, "linked-share");
        await symlink(realShare, linkedShare);
        process.env.XDG_DATA_HOME = linkedShare;
        await expect(
            resolveAndFenceProviderPath(fenced, { allowMissing: false, homeDirectory: home }),
        ).rejects.toMatchObject({ code: "fenced_path" });
    });

    test("resolves symlinks before fencing and tolerates a missing leaf when allowed", async () => {
        const home = await makeHome();
        const target = join(home, ".local", "share", "eidnara", "run", "connection.json");
        await writeFileAt(target, "{}");
        const link = join(home, "innocent.json");
        await symlink(target, link);
        await expect(
            resolveAndFenceProviderPath(link, { allowMissing: false, homeDirectory: home }),
        ).rejects.toMatchObject({ code: "fenced_path" });

        const missing = join(home, "workspace", "not-yet.json");
        await expect(
            resolveAndFenceProviderPath(missing, { allowMissing: true, homeDirectory: home }),
        ).resolves.toBe(missing);
        await expect(
            resolveAndFenceProviderPath(missing, { allowMissing: false, homeDirectory: home }),
        ).rejects.toMatchObject({ code: "unreadable_path" });
    });

    test("refuses a dangling symlink whose target normalizes back to itself", async () => {
        const home = await makeHome();
        const loopDir = join(home, "loop");
        await mkdir(loopDir, { recursive: true });
        const link = join(loopDir, "y");
        await symlink("nope/../y", link);
        await expect(
            resolveAndFenceProviderPath(link, { allowMissing: true, homeDirectory: home }),
        ).rejects.toMatchObject({ code: "unreadable_path" });

        process.env.XDG_DATA_HOME = link;
        const anywhere = join(home, "workspace", "result.json");
        await writeFileAt(anywhere, "{}");
        await expect(
            resolveAndFenceProviderPath(anywhere, { allowMissing: false, homeDirectory: home }),
        ).rejects.toMatchObject({ code: "unreadable_path" });
    });

    test("resolves a missing leaf's relative link target against the real parent", async () => {
        const home = await makeHome();
        const run = join(home, ".local", "share", "eidnara", "run");
        await mkdir(run, { recursive: true });
        const linkDir = join(home, "a", "b", "link");
        await mkdir(dirname(linkDir), { recursive: true });
        await symlink(run, linkDir);
        const leaf = join(linkDir, "leaf.json");
        await symlink("../run/secret.json", leaf);
        await expect(
            resolveAndFenceProviderPath(leaf, { allowMissing: true, homeDirectory: home }),
        ).rejects.toMatchObject({ code: "fenced_path" });

        const deep = join(home, "deep", "dir");
        await mkdir(deep, { recursive: true });
        const plainLinkDir = join(home, "c", "d", "link");
        await mkdir(dirname(plainLinkDir), { recursive: true });
        await symlink(deep, plainLinkDir);
        const plainLeaf = join(plainLinkDir, "leaf.json");
        await symlink("../plain.json", plainLeaf);
        const expected = join(home, "deep", "plain.json");
        await expect(
            resolveAndFenceProviderPath(plainLeaf, { allowMissing: true, homeDirectory: home }),
        ).resolves.toBe(expected);
        await writeFile(expected, "{}");
        expect(await realpath(plainLeaf)).toBe(expected);
    });
});

describe("revalidateProviderPath", () => {
    test("returns the canonical path unchanged and refuses a path that moved", async () => {
        const home = await makeHome();
        const admitted = join(home, "workspace", "result.json");
        await writeFileAt(admitted, "{}");
        await expect(
            revalidateProviderPath(admitted, { allowMissing: false, homeDirectory: home }),
        ).resolves.toBe(admitted);

        const target = join(home, "elsewhere", "result.json");
        await writeFileAt(target, "{}");
        const link = join(home, "workspace", "moved.json");
        await symlink(target, link);
        await expect(
            revalidateProviderPath(link, { allowMissing: false, homeDirectory: home }),
        ).rejects.toMatchObject({
            code: "fenced_path",
            message: `Refusing path changed after fence check: ${link}`,
        });
    });
});
