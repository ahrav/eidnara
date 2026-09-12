import { describe, expect, test } from "bun:test";
import { join, resolve } from "node:path";

const PACKAGE_ROOT = resolve(import.meta.dir, "..", "..");

/**
 * OpenCode loads the TUI half of this plugin through the package's `./tui`
 * export. `Bun.resolveSync` checks the export map without evaluating the entry
 * module.
 */
describe("./tui package export", () => {
    test("resolves to src/tui/entry.mjs", () => {
        const resolved = Bun.resolveSync("@eidnara/opencode/tui", PACKAGE_ROOT);
        expect(resolved).toBe(join(PACKAGE_ROOT, "src", "tui", "entry.mjs"));
    });
});
