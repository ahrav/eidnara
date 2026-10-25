import { afterAll, describe, expect, it } from "bun:test";
import { join } from "node:path";

import { generatedBytes, git, materialize, readManifest } from "./materialize.ts";

const { mkdtempSync, readlinkSync, rmSync } = process.getBuiltinModule("node:fs");
const { tmpdir } = process.getBuiltinModule("node:os");
const { readFileSync } = process.getBuiltinModule("node:fs");

const RUST_TESTS = join(import.meta.dir, "../../../../crates/daemon/src/source_corpus_tests.rs");
const scratch: string[] = [];

afterAll(() => {
    for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

describe("source corpus materializer", () => {
    it("commits the root the Rust materializer pins for ripgrep", () => {
        const parent = mkdtempSync(join(tmpdir(), "materialize-"));
        scratch.push(parent);
        const root = materialize(readManifest("ripgrep"), parent);
        const pinned = /RIPGREP_ROOT: &str = "([0-9a-f]{40})"/.exec(
            readFileSync(RUST_TESTS, "utf8"),
        )?.[1];
        expect(git(root, ["rev-list", "--max-parents=0", "HEAD"]).trim()).toBe(pinned ?? "");
        expect(readlinkSync(join(root, "HomebrewFormula"))).toBe("pkg/brew");
    });

    it("regenerates content of the recorded size and UTF-8 validity", () => {
        for (const size of [0, 1, 21, 22, 4096]) {
            expect(generatedBytes(size, true).length).toBe(size);
            expect(() =>
                new TextDecoder("utf-8", { fatal: true }).decode(generatedBytes(size, true)),
            ).not.toThrow();
            const binary = generatedBytes(size, false);
            expect(binary.length).toBe(size);
            if (size > 0) expect(binary[0]).toBe(0xff);
        }
    });
});
