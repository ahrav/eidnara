import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { resolveWriteTarget } from "./resolve-write-target";

function withTempDir(run: (dir: string) => void): void {
    const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-resolve-target-"));
    try {
        run(dir);
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
}

describe("resolveWriteTarget", () => {
    test("returns an absent path or a regular file unchanged", () => {
        withTempDir((dir) => {
            const absent = path.join(dir, "nope.jsonc");
            expect(resolveWriteTarget(absent)).toBe(absent);
            const file = path.join(dir, "tui.jsonc");
            writeFileSync(file, "{}\n");
            expect(resolveWriteTarget(file)).toBe(file);
        });
    });

    test.skipIf(process.platform === "win32")("follows a dangling symlink to its target", () => {
        withTempDir((dir) => {
            const target = path.join(dir, "dotfiles", "tui.jsonc");
            mkdirSync(path.dirname(target));
            const link = path.join(dir, "tui.jsonc");
            symlinkSync(target, link);
            expect(resolveWriteTarget(link)).toBe(target);
        });
    });

    test.skipIf(process.platform === "win32")(
        "resolves a relative link against its physical parent, not the lexical one",
        () => {
            withTempDir((dir) => {
                // config -> real/config ; real/config/tui.jsonc -> ../target.jsonc
                // The link's parent is physically real/config, so the target is real/target.jsonc.
                const real = path.join(dir, "real");
                mkdirSync(path.join(real, "config"), { recursive: true });
                symlinkSync(path.join(real, "config"), path.join(dir, "config"));
                symlinkSync("../target.jsonc", path.join(real, "config", "tui.jsonc"));

                const throughLink = path.join(dir, "config", "tui.jsonc");
                expect(resolveWriteTarget(throughLink)).toBe(path.join(real, "target.jsonc"));
            });
        },
    );

    test.skipIf(process.platform === "win32")(
        "throws on a symlink cycle instead of returning a link",
        () => {
            withTempDir((dir) => {
                const a = path.join(dir, "a.jsonc");
                const b = path.join(dir, "b.jsonc");
                symlinkSync(b, a);
                symlinkSync(a, b);
                expect(() => resolveWriteTarget(a)).toThrow("too many levels of symbolic links");
            });
        },
    );
});
