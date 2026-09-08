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

    it("creates the target of a dangling symlink and keeps the link", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-dangling-"));
        roots.push(root);
        const target = join(root, "dotfiles", "opencode.jsonc");
        const link = join(root, "opencode.jsonc");
        symlinkSync(target, link);
        expect(existsSync(target)).toBe(false);

        writeFileAtomic(link, "v1\n");

        expect(lstatSync(link).isSymbolicLink()).toBe(true);
        expect(readFileSync(target, "utf-8")).toBe("v1\n");
        expect(readFileSync(link, "utf-8")).toBe("v1\n");
    });

    it("follows a multi-hop dangling chain and creates only the final target", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-chain-"));
        roots.push(root);
        const missing = join(root, "dotfiles", "opencode.jsonc");
        const managed = join(root, "managed-link.jsonc");
        const link = join(root, "opencode.jsonc");
        symlinkSync(missing, managed);
        symlinkSync(managed, link);

        writeFileAtomic(link, "v1\n");

        expect(lstatSync(link).isSymbolicLink()).toBe(true);
        expect(lstatSync(managed).isSymbolicLink()).toBe(true);
        expect(readFileSync(missing, "utf-8")).toBe("v1\n");
        expect(readFileSync(link, "utf-8")).toBe("v1\n");
    });

    it("refuses a symlink cycle instead of looping", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-cycle-"));
        roots.push(root);
        const a = join(root, "a.jsonc");
        const b = join(root, "b.jsonc");
        symlinkSync(b, a);
        symlinkSync(a, b);

        expect(() => writeFileAtomic(a, "v1\n")).toThrow(/symbolic links/);
    });

    it("resolves a relative dangling link text against the link's directory", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-relative-"));
        roots.push(root);
        mkdirSync(join(root, "cfg"));
        const link = join(root, "cfg", "opencode.jsonc");
        symlinkSync(join("..", "dotfiles", "opencode.jsonc"), link);

        writeFileAtomic(link, "v1\n");

        expect(lstatSync(link).isSymbolicLink()).toBe(true);
        expect(readFileSync(join(root, "dotfiles", "opencode.jsonc"), "utf-8")).toBe("v1\n");
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
