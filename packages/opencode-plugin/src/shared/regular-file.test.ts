import { afterEach, describe, expect, it } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readRegularFileSync } from "./regular-file";

const roots: string[] = [];
const root = (): string => {
    const dir = mkdtempSync(join(tmpdir(), "eidnara-regular-file-"));
    roots.push(dir);
    return dir;
};

afterEach(() => {
    for (const dir of roots.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe("readRegularFileSync", () => {
    it("reads a file at or under its byte cap", () => {
        const file = join(root(), "small");
        writeFileSync(file, "abcd");
        expect(readRegularFileSync(file)).toBe("abcd");
        expect(readRegularFileSync(file, 4)).toBe("abcd");
    });

    it("refuses a file over its byte cap", () => {
        const file = join(root(), "large");
        writeFileSync(file, "abcde");
        expect(() => readRegularFileSync(file, 4)).toThrow("exceeds 4 bytes");
    });

    it("refuses a FIFO without blocking on a writer", () => {
        const fifo = join(root(), "fifo");
        execFileSync("mkfifo", [fifo]);
        expect(() => readRegularFileSync(fifo, 4)).toThrow("not a regular file");
    });
});
