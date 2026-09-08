import { afterEach, describe, expect, it } from "bun:test";
import {
    chmodSync,
    existsSync,
    lstatSync,
    mkdirSync,
    mkdtempSync,
    readFileSync,
    readlinkSync,
    rmSync,
    symlinkSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { restoreFiles, snapshotFiles } from "./file-snapshot";

const roots: string[] = [];
afterEach(() => {
    for (const root of roots.splice(0)) {
        try {
            chmodSync(root, 0o755);
        } catch {}
        rmSync(root, { recursive: true, force: true });
    }
});

function tempRoot(): string {
    const root = mkdtempSync(join(tmpdir(), "eidnara-file-snapshot-"));
    roots.push(root);
    return root;
}

describe("snapshotFiles / restoreFiles", () => {
    it("restores modified content and removes files that did not exist", () => {
        const root = tempRoot();
        const existing = join(root, "a.jsonc");
        const created = join(root, "nested", "b.jsonc");
        writeFileSync(existing, "original\n");

        const snapshot = snapshotFiles([existing, created, existing]);
        expect(snapshot).toHaveLength(2);

        writeFileSync(existing, "changed\n");
        mkdirSync(join(root, "nested"));
        writeFileSync(created, "new\n");

        restoreFiles(snapshot);
        expect(readFileSync(existing, "utf-8")).toBe("original\n");
        expect(existsSync(created)).toBe(false);
    });

    it.if(process.platform !== "win32")(
        "keeps a dangling symlink and removes the target a write created through it",
        () => {
            const root = tempRoot();
            const target = join(root, "dotfiles", "eidnara.jsonc");
            const link = join(root, "eidnara.jsonc");
            symlinkSync(target, link);
            expect(existsSync(link)).toBe(false);

            const snapshot = snapshotFiles([link]);
            mkdirSync(join(root, "dotfiles"));
            writeFileSync(target, "{}\n");
            expect(readFileSync(link, "utf-8")).toBe("{}\n");

            restoreFiles(snapshot);
            expect(lstatSync(link).isSymbolicLink()).toBe(true);
            expect(readlinkSync(link)).toBe(target);
            expect(existsSync(target)).toBe(false);
        },
    );

    it.if(process.platform !== "win32")(
        "writes a restored value through a resolving symlink",
        () => {
            const root = tempRoot();
            const target = join(root, "real.jsonc");
            const link = join(root, "link.jsonc");
            writeFileSync(target, "before\n");
            symlinkSync(target, link);

            const snapshot = snapshotFiles([link]);
            writeFileSync(link, "after\n");
            restoreFiles(snapshot);
            expect(lstatSync(link).isSymbolicLink()).toBe(true);
            expect(readFileSync(target, "utf-8")).toBe("before\n");
        },
    );

    it.if(process.platform !== "win32" && process.getuid?.() !== 0)(
        "refuses to snapshot an existing file it cannot read",
        () => {
            const root = tempRoot();
            const locked = join(root, "locked.json");
            writeFileSync(locked, "{}\n");
            chmodSync(locked, 0o000);
            try {
                expect(() => snapshotFiles([locked])).toThrow(`Cannot read ${locked}`);
            } finally {
                chmodSync(locked, 0o644);
            }
        },
    );

    it("restores a file that was deleted after the snapshot", () => {
        const root = tempRoot();
        const path = join(root, "sub", "c.json");
        mkdirSync(join(root, "sub"));
        writeFileSync(path, "{}\n");
        const snapshot = snapshotFiles([path]);
        rmSync(join(root, "sub"), { recursive: true });

        restoreFiles(snapshot);
        expect(readFileSync(path, "utf-8")).toBe("{}\n");
    });

    it.if(process.platform !== "win32" && process.getuid?.() !== 0)(
        "keeps restoring after one path fails and names the failures",
        () => {
            const root = tempRoot();
            const locked = join(root, "locked", "d.json");
            const free = join(root, "e.json");
            mkdirSync(join(root, "locked"));
            writeFileSync(locked, "keep\n");
            writeFileSync(free, "keep\n");
            const snapshot = snapshotFiles([free, locked]);
            writeFileSync(locked, "changed\n");
            writeFileSync(free, "changed\n");
            chmodSync(join(root, "locked"), 0o555);

            try {
                expect(() => restoreFiles(snapshot)).toThrow(`Could not restore: ${locked}`);
            } finally {
                chmodSync(join(root, "locked"), 0o755);
            }
            expect(readFileSync(free, "utf-8")).toBe("keep\n");
        },
    );
});
