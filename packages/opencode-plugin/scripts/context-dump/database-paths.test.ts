import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { resolveOpenCodeDatabasePath } from "./database-paths";

const savedExplicit = process.env.OPENCODE_DB_PATH;

afterEach(() => {
    if (savedExplicit === undefined) {
        delete process.env.OPENCODE_DB_PATH;
    } else {
        process.env.OPENCODE_DB_PATH = savedExplicit;
    }
});

describe("resolveOpenCodeDatabasePath", () => {
    test("returns an explicit OPENCODE_DB_PATH that exists", () => {
        const dir = mkdtempSync(join(tmpdir(), "eidnara-db-path-"));
        try {
            const dbPath = join(dir, "custom.db");
            writeFileSync(dbPath, "");
            process.env.OPENCODE_DB_PATH = dbPath;
            expect(resolveOpenCodeDatabasePath()).toBe(dbPath);
        } finally {
            rmSync(dir, { recursive: true, force: true });
        }
    });

    test("throws when OPENCODE_DB_PATH is set but does not exist", () => {
        const missing = join(tmpdir(), "eidnara-db-path-missing", "typo.db");
        process.env.OPENCODE_DB_PATH = missing;
        expect(() => resolveOpenCodeDatabasePath()).toThrow(
            `OPENCODE_DB_PATH is set to ${missing}, which does not exist`,
        );
    });
});
