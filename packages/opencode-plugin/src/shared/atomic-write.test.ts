/// <reference types="bun-types" />

import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import {
    chmodSync,
    existsSync,
    mkdtempSync,
    readFileSync,
    rmSync,
    statSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { writeFileAtomic } from "./atomic-write";

describe("writeFileAtomic", () => {
    let root: string;

    beforeEach(() => {
        root = mkdtempSync(join(tmpdir(), "eidnara-atomic-write-"));
    });

    afterEach(() => {
        rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    });

    it("writes the body and leaves no temporary file behind", () => {
        const target = join(root, "config.json");
        writeFileAtomic(target, "{}\n");
        expect(readFileSync(target, "utf-8")).toBe("{}\n");
        expect(existsSync(`${target}.tmp`)).toBe(false);
    });

    it("preserves the permission bits of an existing destination", () => {
        const target = join(root, "config.json");
        writeFileSync(target, "old");
        chmodSync(target, 0o600);
        writeFileAtomic(target, "new");
        expect(readFileSync(target, "utf-8")).toBe("new");
        expect(statSync(target).mode & 0o777).toBe(0o600);
    });

    it("leaves the destination untouched when the temporary write fails", () => {
        const target = join(root, "missing-dir", "config.json");
        expect(() => writeFileAtomic(target, "new")).toThrow();
        expect(existsSync(target)).toBe(false);
    });
});
