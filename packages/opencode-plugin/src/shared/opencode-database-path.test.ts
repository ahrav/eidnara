import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { resolveOpenCodeDatabasePath } from "./opencode-database-path";

const tempRoots: string[] = [];
const originalExplicit = process.env.OPENCODE_DB_PATH;

afterEach(() => {
    if (originalExplicit === undefined) delete process.env.OPENCODE_DB_PATH;
    else process.env.OPENCODE_DB_PATH = originalExplicit;
    for (const root of tempRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function dataDir(): string {
    const root = mkdtempSync(join(tmpdir(), "eidnara-opencode-db-"));
    tempRoots.push(root);
    mkdirSync(join(root, "opencode", "storage"), { recursive: true });
    delete process.env.OPENCODE_DB_PATH;
    return root;
}

describe("resolveOpenCodeDatabasePath", () => {
    test("prefers an existing OPENCODE_DB_PATH", () => {
        const root = dataDir();
        const explicit = join(root, "elsewhere.db");
        writeFileSync(explicit, "");
        writeFileSync(join(root, "opencode", "opencode.db"), "");
        process.env.OPENCODE_DB_PATH = explicit;
        expect(resolveOpenCodeDatabasePath(root)).toBe(explicit);
    });

    test("ignores OPENCODE_DB_PATH when the file is missing", () => {
        const root = dataDir();
        writeFileSync(join(root, "opencode", "opencode.db"), "");
        process.env.OPENCODE_DB_PATH = join(root, "missing.db");
        expect(resolveOpenCodeDatabasePath(root)).toBe(join(root, "opencode", "opencode.db"));
    });

    test("falls back to the newest channel database", () => {
        const root = dataDir();
        const older = join(root, "opencode", "opencode-beta.db");
        const newer = join(root, "opencode", "opencode-dev.db");
        writeFileSync(older, "");
        writeFileSync(newer, "");
        utimesSync(older, new Date(1_000_000), new Date(1_000_000));
        utimesSync(newer, new Date(2_000_000), new Date(2_000_000));
        expect(resolveOpenCodeDatabasePath(root)).toBe(newer);
    });

    test("skips a candidate that vanishes before it is statted", () => {
        const root = dataDir();
        const valid = join(root, "opencode", "opencode-dev.db");
        writeFileSync(valid, "");
        // A dangling symlink is listed by readdir but fails stat, like a database rotated mid-walk.
        symlinkSync(join(root, "gone.db"), join(root, "opencode", "opencode-beta.db"));
        expect(resolveOpenCodeDatabasePath(root)).toBe(valid);
    });

    test("falls back to a storage database and otherwise throws", () => {
        const root = dataDir();
        expect(() => resolveOpenCodeDatabasePath(root)).toThrow(/Unable to locate OpenCode DB/);
        const storage = join(root, "opencode", "storage", "sessions.db");
        writeFileSync(storage, "");
        expect(resolveOpenCodeDatabasePath(root)).toBe(storage);
    });
});
