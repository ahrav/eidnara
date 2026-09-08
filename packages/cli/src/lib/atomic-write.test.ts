import { afterEach, describe, expect, it } from "bun:test";
import {
    chmodSync,
    existsSync,
    lstatSync,
    mkdirSync,
    mkdtempSync,
    readFileSync,
    rmSync,
    statSync,
    symlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
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

    it("writes through a symlink and keeps the link in place", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-symlink-"));
        roots.push(root);
        const dotfiles = join(root, "dotfiles");
        mkdirSync(dotfiles);
        const backing = join(dotfiles, "opencode.jsonc");
        writeFileAtomic(backing, "v1\n");
        const link = join(root, "opencode.jsonc");
        symlinkSync(backing, link);

        writeFileAtomic(link, "v2\n");

        expect(lstatSync(link).isSymbolicLink()).toBe(true);
        expect(readFileSync(backing, "utf-8")).toBe("v2\n");
        expect(readFileSync(link, "utf-8")).toBe("v2\n");
        expect(existsSync(`${backing}.tmp`)).toBe(false);
        expect(existsSync(`${link}.tmp`)).toBe(false);
    });

    it("replaces a dangling symlink with a regular file", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-dangling-"));
        roots.push(root);
        const link = join(root, "opencode.jsonc");
        symlinkSync(join(root, "missing-target.jsonc"), link);

        writeFileAtomic(link, "v1\n");

        expect(lstatSync(link).isSymbolicLink()).toBe(false);
        expect(readFileSync(link, "utf-8")).toBe("v1\n");
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
});
