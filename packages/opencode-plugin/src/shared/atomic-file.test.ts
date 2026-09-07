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

    test.skipIf(process.platform === "win32")(
        "refuses a symlink planted at the staging name instead of writing through it",
        () => {
            withTempDir((dir) => {
                const target = path.join(dir, "config.jsonc");
                writeFileSync(target, "old\n");
                const victim = path.join(dir, "victim.txt");
                writeFileSync(victim, "untouched\n");
                symlinkSync(victim, `${target}.${process.pid}.tmp`);

                expect(() => writeFileAtomicSync(target, "new\n")).toThrow();

                expect(readFileSync(victim, "utf8")).toBe("untouched\n");
                expect(readFileSync(target, "utf8")).toBe("old\n");
            });
        },
    );
});
