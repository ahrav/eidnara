import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
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
});
