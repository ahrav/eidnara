import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { scanPredecessorTokens } from "./smoke-tarball-install";

const TOKENIZER = "host-linux-x64-gnu/package/payload/model/x/tokenizer.json";
// `TOKEN` is split so this source file passes the CI `Predecessor tokens` gate.
const TOKEN = ["m", "c"].join("");
const VOCABULARY = `{"model": {"vocab": {"Ġ${TOKEN}": 3044, "${TOKEN.toUpperCase()}": 7722}}}\n`;

function sha256(text: string): string {
    return createHash("sha256").update(text).digest("hex");
}

describe("predecessor-token scan", () => {
    let root: string;

    beforeEach(() => {
        root = mkdtempSync(join(tmpdir(), "eidnara-token-scan-"));
    });

    afterEach(() => {
        rmSync(root, { recursive: true, force: true });
    });

    function write(rel: string, text: string): void {
        mkdirSync(dirname(join(root, rel)), { recursive: true });
        writeFileSync(join(root, rel), text);
    }

    test("a locked third-party input at its locked bytes is not scanned", () => {
        write(TOKENIZER, VOCABULARY);
        expect(scanPredecessorTokens(root, new Map([[TOKENIZER, sha256(VOCABULARY)]]))).toEqual([]);
    });

    test("bytes at a locked path that differ from the lock are scanned", () => {
        write(TOKENIZER, VOCABULARY);
        const hits = scanPredecessorTokens(root, new Map([[TOKENIZER, "0".repeat(64)]]));
        expect(hits.length).toBeGreaterThan(0);
        expect(hits[0]).toStartWith(`${TOKENIZER}:1:`);
    });

    test("project-authored files are scanned", () => {
        write(TOKENIZER, VOCABULARY);
        write("cli/package/dist/index.js", `// ${TOKEN} bridge\n`);
        expect(scanPredecessorTokens(root, new Map([[TOKENIZER, sha256(VOCABULARY)]]))).toEqual([
            `cli/package/dist/index.js:1: // ${TOKEN} bridge`,
        ]);
    });
});
