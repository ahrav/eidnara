import { describe, expect, test } from "bun:test";
import {
    chmodSync,
    lstatSync,
    mkdtempSync,
    readdirSync,
    readFileSync,
    rmSync,
    symlinkSync,
    writeFileSync,
} from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { writeFileAtomicSync } from "./atomic-file";

function withTempDir(run: (dir: string) => void): void {
    const dir = mkdtempSync(path.join(os.tmpdir(), "eidnara-atomic-file-"));
    try {
        run(dir);
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
}

describe("writeFileAtomicSync", () => {
    test("creates a new file and leaves no staging file behind", () => {
        withTempDir((dir) => {
            const target = path.join(dir, "config.jsonc");
            writeFileAtomicSync(target, "{}\n");
            expect(readFileSync(target, "utf8")).toBe("{}\n");
            expect(readdirSync(dir)).toEqual(["config.jsonc"]);
        });
    });

    test.skipIf(process.platform === "win32")(
        "keeps an existing file's mode even under a restrictive umask",
        () => {
            withTempDir((dir) => {
                const target = path.join(dir, "config.jsonc");
                writeFileSync(target, "old\n");
                chmodSync(target, 0o644);
                const previousUmask = process.umask(0o077);
                try {
                    writeFileAtomicSync(target, "new\n");
                } finally {
                    process.umask(previousUmask);
                }
                expect(lstatSync(target).mode & 0o777).toBe(0o644);
                expect(readFileSync(target, "utf8")).toBe("new\n");
            });
        },
    );

    test("a stale staging file left by a killed process does not block later writes", () => {
        withTempDir((dir) => {
            const target = path.join(dir, "config.jsonc");
            writeFileSync(target, "old\n");
            // A fixed `<target>.<pid>.tmp` name would collide with O_EXCL once the pid is reused.
            writeFileSync(`${target}.${process.pid}.tmp`, "abandoned\n");

            writeFileAtomicSync(target, "new\n");

            expect(readFileSync(target, "utf8")).toBe("new\n");
            expect(readdirSync(dir).sort()).toEqual(
                ["config.jsonc", `config.jsonc.${process.pid}.tmp`].sort(),
            );
        });
    });

    test.skipIf(process.platform === "win32")(
        "a symlink at the target is written through, not replaced, when the caller resolves it first",
        () => {
            withTempDir((dir) => {
                const real = path.join(dir, "real.jsonc");
                writeFileSync(real, "old\n");
                const link = path.join(dir, "link.jsonc");
                symlinkSync(real, link);

                writeFileAtomicSync(real, "new\n");

                expect(lstatSync(link).isSymbolicLink()).toBe(true);
                expect(readFileSync(link, "utf8")).toBe("new\n");
            });
        },
    );
});
