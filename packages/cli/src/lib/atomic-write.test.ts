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
    statSync,
    symlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { writeFileAtomic } from "./atomic-write";

const roots: string[] = [];

afterEach(() => {
    for (const root of roots.splice(0)) {
        rmSync(root, { recursive: true, force: true });
    }
});

describe("writeFileAtomic", () => {
    it("writes content and leaves no .tmp sibling", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-"));
        roots.push(root);
        const target = join(root, "config.jsonc");
        writeFileAtomic(target, '{"ok":true}\n');
        expect(readFileSync(target, "utf-8")).toBe('{"ok":true}\n');
        expect(existsSync(`${target}.tmp`)).toBe(false);
    });

    it("preserves file mode on replace", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-mode-"));
        roots.push(root);
        const target = join(root, "config.jsonc");
        writeFileAtomic(target, "v1\n");
        chmodSync(target, 0o600);
        writeFileAtomic(target, "v2\n");
        expect(readFileSync(target, "utf-8")).toBe("v2\n");
        expect(statSync(target).mode & 0o777).toBe(0o600);
    });

    it("creates missing parent directories (fresh Eidnara config location)", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-mkdir-"));
        roots.push(root);
        const target = join(root, "eidnara", "nested", "eidnara.jsonc");
        expect(existsSync(join(root, "eidnara"))).toBe(false);
        writeFileAtomic(target, '{"created":true}\n');
        expect(readFileSync(target, "utf-8")).toBe('{"created":true}\n');
        expect(existsSync(`${target}.tmp`)).toBe(false);
    });

    it.if(process.platform !== "win32")(
        "writes through a symlink and keeps the link pointing at its target",
        () => {
            const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-symlink-"));
            roots.push(root);
            const dotfiles = join(root, "dotfiles", "opencode.jsonc");
            const link = join(root, "config", "opencode.jsonc");
            writeFileAtomic(dotfiles, "v1\n");
            mkdirSync(dirname(link), { recursive: true });
            symlinkSync(dotfiles, link);

            writeFileAtomic(link, "v2\n");

            expect(lstatSync(link).isSymbolicLink()).toBe(true);
            expect(readlinkSync(link)).toBe(dotfiles);
            expect(readFileSync(dotfiles, "utf-8")).toBe("v2\n");
            expect(existsSync(`${dotfiles}.tmp`)).toBe(false);
            expect(existsSync(`${link}.tmp`)).toBe(false);
        },
    );
});
