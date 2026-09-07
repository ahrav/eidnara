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

beforeEach(() => {
    delete process.env.XDG_DATA_HOME;
});

afterEach(async () => {
    if (originalXdgDataHome === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = originalXdgDataHome;
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
            connectionFile: hostRelease.layout.connection_file,
            storageSubdirectory: hostRelease.layout.storage_subdirectory,
        });
    });
});

describe("isFencedPath", () => {
    const home = "/home/user";
    const share = join(home, ".local", "share");

    test("fences the runtime and storage roots under the managed subtree", () => {
        expect(isFencedPath(join(share, "eidnara", "run", "connection.json"), home, share)).toBe(
            true,
        );
        expect(isFencedPath(join(share, "eidnara", "context", "notes.db"), home, share)).toBe(true);
        expect(isFencedPath(join(share, "eidnara", "run"), home, share)).toBe(true);
    });

    test("fences key-material basenames at any location", () => {
        expect(isFencedPath(join(home, "projects", "binding-key"), home, share)).toBe(true);
        expect(isFencedPath(join(home, "projects", "x-binding-key.txt"), home, share)).toBe(true);
        expect(isFencedPath(join("/tmp", "lease.handle"), home, share)).toBe(true);
    });

    test("admits the managed subtree's other children and everything outside it", () => {
        expect(isFencedPath(join(share, "eidnara", "docs", "notice.txt"), home, share)).toBe(false);
        expect(isFencedPath(join(share, "eidnara"), home, share)).toBe(false);
        expect(isFencedPath(join(share, "eidnara", "runner", "x"), home, share)).toBe(false);
        expect(isFencedPath(join(share, "eidnara", "contexts", "x"), home, share)).toBe(false);
        expect(isFencedPath(join(share, "eidnara-other", "run", "x"), home, share)).toBe(false);
        expect(isFencedPath(join(home, "workspace", "result.json"), home, share)).toBe(false);
        expect(isFencedPath(join(share, "run", "connection.json"), home, share)).toBe(false);
    });

    test("honors an explicit data directory over XDG_DATA_HOME", () => {
        const data = "/srv/data";
        expect(isFencedPath(join(data, "eidnara", "run", "x"), home, data)).toBe(true);
        expect(isFencedPath(join(share, "eidnara", "run", "x"), home, data)).toBe(false);
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
