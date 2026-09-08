import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { resolveOpenCodeDatabasePath } from "./database-paths";

const savedExplicit = process.env.OPENCODE_DB_PATH;
const savedXdgDataHome = process.env.XDG_DATA_HOME;

afterEach(() => {
    if (savedExplicit === undefined) {
        delete process.env.OPENCODE_DB_PATH;
    } else {
        process.env.OPENCODE_DB_PATH = savedExplicit;
    }
    if (savedXdgDataHome === undefined) {
        delete process.env.XDG_DATA_HOME;
    } else {
        process.env.XDG_DATA_HOME = savedXdgDataHome;
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

    test("prefers the most recently modified opencode*.db over a stale stable database", () => {
        const dataHome = mkdtempSync(join(tmpdir(), "eidnara-db-path-xdg-"));
        try {
            delete process.env.OPENCODE_DB_PATH;
            process.env.XDG_DATA_HOME = dataHome;
            const root = join(dataHome, "opencode");
            mkdirSync(root, { recursive: true });
            const stable = join(root, "opencode.db");
            const beta = join(root, "opencode-beta.db");
            writeFileSync(stable, "");
            writeFileSync(beta, "");
            const now = Date.now() / 1000;
            utimesSync(stable, now - 3600, now - 3600);
            utimesSync(beta, now, now);
            expect(resolveOpenCodeDatabasePath()).toBe(beta);

            utimesSync(stable, now + 60, now + 60);
            expect(resolveOpenCodeDatabasePath()).toBe(stable);
        } finally {
            rmSync(dataHome, { recursive: true, force: true });
        }
    });
});
