import { describe, expect, it } from "bun:test";
import { resolve } from "node:path";
import { corpusDigest, corpusFiles, SOURCE_CORPUS_SHA256, verifyCorpusDigest } from "./corpus.ts";

const { readFileSync } = process.getBuiltinModule("node:fs");

const RUST_OWNER = resolve(import.meta.dir, "../../../../crates/daemon/src/source_corpus.rs");

describe("source corpus digest", () => {
    it("recomputes the pin the Rust owner holds over the committed fixtures", () => {
        const rust = readFileSync(RUST_OWNER, "utf8");
        const pin = /CORPUS_SHA256: &str =\s*"([0-9a-f]{64})"/.exec(rust)?.[1];
        expect(pin).toBe(SOURCE_CORPUS_SHA256);
        const files = corpusFiles();
        expect(files.map((f) => f.path)).toContain("repos/eidnara.json");
        expect(files.map((f) => f.path)).not.toContain("README.md");
        expect(() => verifyCorpusDigest(files)).not.toThrow();
    });

    it("fails when one manifest byte changes without a pin move", () => {
        const files = corpusFiles().map((file) => {
            if (file.path !== "repos/ripgrep.json") return file;
            const bytes = Buffer.from(file.bytes);
            bytes[bytes.indexOf("5")] = "6".charCodeAt(0);
            return { path: file.path, bytes };
        });
        expect(() => verifyCorpusDigest(files)).toThrow(/source corpus digest/);
    });

    it("sorts paths by bytes and normalizes CRLF to LF", () => {
        const enc = (s: string) => Buffer.from(s);
        const lf = [
            { path: "b", bytes: enc("x\ny\n") },
            { path: "B", bytes: enc("z") },
        ];
        const crlf = [
            { path: "B", bytes: enc("z") },
            { path: "b", bytes: enc("x\r\ny\r\n") },
        ];
        expect(corpusDigest(lf)).toBe(corpusDigest(crlf));
        const expected = new Bun.CryptoHasher("sha256").update("B\nzb\nx\ny\n").digest("hex");
        expect(corpusDigest(lf)).toBe(expected);
        expect(corpusDigest([{ path: "b", bytes: enc("x\ry") }])).not.toBe(
            corpusDigest([{ path: "b", bytes: enc("x\ny") }]),
        );
    });
});
