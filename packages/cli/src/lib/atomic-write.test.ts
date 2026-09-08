import { afterEach, describe, expect, it } from "bun:test";
import {
    chmodSync,
    existsSync,
    lstatSync,
    mkdirSync,
    mkdtempSync,
    readdirSync,
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

function stagedSiblings(dir: string): string[] {
    return readdirSync(dir).filter((name) => name.endsWith(".tmp"));
}

describe("writeFileAtomic", () => {
    it("writes content and leaves no staged sibling", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-"));
        roots.push(root);
        const target = join(root, "config.jsonc");
        writeFileAtomic(target, '{"ok":true}\n');
        expect(readFileSync(target, "utf-8")).toBe('{"ok":true}\n');
        expect(stagedSiblings(root)).toEqual([]);
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
        expect(stagedSiblings(join(root, "eidnara", "nested"))).toEqual([]);
    });

    it("replaces the file behind a symlinked config and keeps the link", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-link-"));
        roots.push(root);
        const real = join(root, "dotfiles", "opencode.jsonc");
        mkdirSync(join(root, "dotfiles"));
        writeFileAtomic(real, "v1\n");
        const link = join(root, "config", "opencode.jsonc");
        mkdirSync(join(root, "config"));
        symlinkSync(real, link);

        writeFileAtomic(link, "v2\n");

        expect(lstatSync(link).isSymbolicLink()).toBe(true);
        expect(readFileSync(real, "utf-8")).toBe("v2\n");
        expect(readFileSync(link, "utf-8")).toBe("v2\n");
        expect(stagedSiblings(join(root, "dotfiles"))).toEqual([]);
        expect(stagedSiblings(join(root, "config"))).toEqual([]);
    });

    it("creates the missing target behind a dangling symlink and keeps the link", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-dangling-"));
        roots.push(root);
        const real = join(root, "dotfiles", "opencode.jsonc");
        const link = join(root, "config", "opencode.jsonc");
        mkdirSync(join(root, "config"));
        symlinkSync(join("..", "dotfiles", "opencode.jsonc"), link);
        expect(existsSync(link)).toBe(false);

        writeFileAtomic(link, "v1\n");

        expect(lstatSync(link).isSymbolicLink()).toBe(true);
        expect(readFileSync(real, "utf-8")).toBe("v1\n");
        expect(readFileSync(link, "utf-8")).toBe("v1\n");
        expect(stagedSiblings(join(root, "dotfiles"))).toEqual([]);
        expect(stagedSiblings(join(root, "config"))).toEqual([]);
    });

    it("removes the staged sibling when the rename fails", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-fail-"));
        roots.push(root);
        const target = join(root, "config.jsonc");
        mkdirSync(target);

        expect(() => writeFileAtomic(target, "v1\n")).toThrow();

        expect(statSync(target).isDirectory()).toBe(true);
        expect(stagedSiblings(root)).toEqual([]);
    });

    it("refuses a symlink cycle instead of replacing a link with a file", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-cycle-"));
        roots.push(root);
        const a = join(root, "a.jsonc");
        const b = join(root, "b.jsonc");
        symlinkSync("b.jsonc", a);
        symlinkSync("a.jsonc", b);

        expect(() => writeFileAtomic(a, "v1\n")).toThrow(/symlink cycle/);

        expect(lstatSync(a).isSymbolicLink()).toBe(true);
        expect(lstatSync(b).isSymbolicLink()).toBe(true);
        expect(stagedSiblings(root)).toEqual([]);
    });

    it("refuses a dangling chain longer than the hop limit", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-atomic-long-chain-"));
        roots.push(root);
        // link-0 -> link-1 -> ... -> link-40 -> missing; the chain never reaches a non-link within 32 hops.
        for (let i = 0; i <= 40; i++) {
            symlinkSync(`link-${i + 1}.jsonc`, join(root, `link-${i}.jsonc`));
        }
        const head = join(root, "link-0.jsonc");

        expect(() => writeFileAtomic(head, "v1\n")).toThrow(/exceeds 32 hops/);

        expect(lstatSync(head).isSymbolicLink()).toBe(true);
        expect(stagedSiblings(root)).toEqual([]);
    });
});
