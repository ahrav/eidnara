import { describe, expect, it } from "bun:test";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

/**
 *
 */

const PLUGIN_SRC = join(import.meta.dir, "..");
const SCAN_ROOTS = [
    PLUGIN_SRC,
    join(PLUGIN_SRC, "../../pi-plugin/src"),
    join(PLUGIN_SRC, "../../cli/src"),
].filter((dir) => existsSync(dir));
const ALLOWED = new Set(["shared/sqlite.ts", "shared/sqlite-bind-style.test.ts"]);
const BIND_PATTERN = /\.(run|get|all)\(\s*\[/g;

function collectTsFiles(dir: string, acc: string[] = []): string[] {
    for (const entry of readdirSync(dir)) {
        const full = join(dir, entry);
        const st = statSync(full);
        if (st.isDirectory()) {
            collectTsFiles(full, acc);
        } else if (entry.endsWith(".ts") && !entry.endsWith(".test.ts")) {
            acc.push(full);
        }
    }
    return acc;
}

describe("sqlite bind style", () => {
    it("uses spread positional binds, never the array form", () => {
        const violations: string[] = [];
        for (const root of SCAN_ROOTS) {
            for (const file of collectTsFiles(root)) {
                const rel = file.slice(root.length + 1);
                if (ALLOWED.has(rel)) continue;
                const source = readFileSync(file, "utf8");
                const lines = source.split("\n");
                for (const match of source.matchAll(BIND_PATTERN)) {
                    const i = source.slice(0, match.index).split("\n").length - 1;
                    const line = lines[i] ?? "";
                    // Promise.all([...]) is not a SQLite statement bind.
                    if (line.includes("Promise.all(")) continue;
                    const trimmed = line.trim();
                    if (trimmed.startsWith("//") || trimmed.startsWith("*")) continue;
                    const pkg = root.includes("pi-plugin")
                        ? "pi-plugin"
                        : root.includes("cli")
                          ? "cli"
                          : "plugin";
                    violations.push(`${pkg}/${rel}:${i + 1}  ${trimmed}`);
                }
            }
        }
        expect(
            violations,
            `Array-form SQLite binds found — use spread positional .run(a, b) ` +
                `instead of .run([a, b]) (breaks under node:sqlite on Pi/Desktop):\n` +
                violations.join("\n"),
        ).toEqual([]);
    });
});
