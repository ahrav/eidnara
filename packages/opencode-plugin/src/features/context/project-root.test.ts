import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { resolveProjectRootDirectory } from "./project-root";

describe("resolveProjectRootDirectory", () => {
    test("a directory containing .git resolves to itself from a nested child", () => {
        const root = mkdtempSync(join(tmpdir(), "project-root-git-"));
        try {
            mkdirSync(join(root, ".git"));
            const nested = join(root, "src", "deep");
            mkdirSync(nested, { recursive: true });
            expect(resolveProjectRootDirectory(nested)).toBe(realpathSync.native(root));
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("a directory without .git resolves to its realpath", () => {
        const root = mkdtempSync(join(tmpdir(), "project-root-plain-"));
        try {
            const nested = join(root, "child");
            mkdirSync(nested);
            expect(resolveProjectRootDirectory(nested)).toBe(realpathSync.native(nested));
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("a symlinked subtree beneath another checkout resolves to the link target's repository", () => {
        const root = mkdtempSync(join(tmpdir(), "project-root-symlink-"));
        try {
            const outer = join(root, "outer");
            const target = join(root, "target");
            mkdirSync(join(outer, ".git"), { recursive: true });
            mkdirSync(join(target, ".git"), { recursive: true });
            mkdirSync(join(target, "subdir", "deep"), { recursive: true });
            symlinkSync(join(target, "subdir"), join(outer, "link"));
            expect(resolveProjectRootDirectory(join(outer, "link", "deep"))).toBe(
                realpathSync.native(target),
            );
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("a symlinked subtree whose target has no .git resolves to the target's realpath, not the outer checkout", () => {
        const root = mkdtempSync(join(tmpdir(), "project-root-symlink-plain-"));
        try {
            const outer = join(root, "outer");
            const target = join(root, "target", "subdir", "deep");
            mkdirSync(join(outer, ".git"), { recursive: true });
            mkdirSync(target, { recursive: true });
            symlinkSync(join(root, "target", "subdir"), join(outer, "link"));
            expect(resolveProjectRootDirectory(join(outer, "link", "deep"))).toBe(
                realpathSync.native(target),
            );
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("a missing directory walks its spelled ancestors to an existing .git", () => {
        const root = mkdtempSync(join(tmpdir(), "project-root-missing-"));
        try {
            mkdirSync(join(root, ".git"));
            expect(resolveProjectRootDirectory(join(root, "not", "created"))).toBe(
                realpathSync.native(root),
            );
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});
