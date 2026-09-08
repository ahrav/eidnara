import { afterEach, describe, expect, it } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { writeNewFile } from "./fs-utils";

const tempDirs: string[] = [];

afterEach(() => {
    for (const dir of tempDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function makeTempDir(): string {
    const dir = mkdtempSync(join(tmpdir(), "eidnara-fs-utils-"));
    tempDirs.push(dir);
    return dir;
}

describe("writeNewFile", () => {
    it("numbers a second file at the same stem instead of replacing the first", () => {
        const dir = makeTempDir();
        const stem = join(dir, "report");

        const first = writeNewFile(stem, "first\n");
        const second = writeNewFile(stem, "second\n");

        expect(first).toBe(`${stem}.md`);
        expect(second).toBe(`${stem}-2.md`);
        expect(readFileSync(first, "utf-8")).toBe("first\n");
        expect(readFileSync(second, "utf-8")).toBe("second\n");
    });

    it("does not write through a symlink planted at the expected path", () => {
        const dir = makeTempDir();
        const target = join(dir, "victim.txt");
        writeFileSync(target, "untouched\n");
        const stem = join(dir, "report");
        symlinkSync(target, `${stem}.md`);

        const written = writeNewFile(stem, "report body\n");

        expect(written).toBe(`${stem}-2.md`);
        expect(readFileSync(target, "utf-8")).toBe("untouched\n");
        expect(readFileSync(written, "utf-8")).toBe("report body\n");
    });
});
