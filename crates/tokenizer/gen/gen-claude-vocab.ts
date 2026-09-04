/**
 * Regenerates `assets/claude.tiktoken` and `assets/claude.pat` from the `ai-tokenizer` claude
 * encoding.
 *
 *   bun crates/tokenizer/gen/gen-claude-vocab.ts
 *
 * `claude.pat` is the upstream `pat_str` verbatim. `src/lib.rs` derives its runtime pattern
 * from it under test, so an upstream pattern change surfaces as a failing Rust test even when
 * the vocabulary itself is unchanged.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";

type StringEncoder = Record<string, number>;
type BinaryEncoder = Array<[Uint8Array, number]>;

// `ai-tokenizer` is a root dev dependency; resolving from the repository root makes the
// generator independent of the current working directory.
const repoRoot = join(import.meta.dir, "..", "..", "..");
const claudeEntry = Bun.resolveSync("ai-tokenizer/encoding/claude", repoRoot);

async function main(): Promise<void> {
    const enc = (await import(claudeEntry)) as {
        pat_str: unknown;
        stringEncoder: unknown;
        binaryEncoder: unknown;
    };
    if (typeof enc.pat_str !== "string" || enc.pat_str.length === 0) {
        throw new Error("ai-tokenizer claude encoding has no pat_str");
    }
    const patStr = enc.pat_str;
    const stringEncoder = enc.stringEncoder as StringEncoder;
    const binaryEncoder = enc.binaryEncoder as BinaryEncoder;

    const rows: Array<[string, number]> = [];

    for (const [str, rank] of Object.entries(stringEncoder)) {
        rows.push([Buffer.from(str, "utf8").toString("base64"), rank]);
    }
    for (const [bytes, rank] of binaryEncoder) {
        rows.push([Buffer.from(bytes).toString("base64"), rank]);
    }

    const ranks = rows.map((r) => r[1]);
    const rankSet = new Set(ranks);
    if (rankSet.size !== ranks.length) {
        throw new Error(`duplicate ranks: ${ranks.length - rankSet.size}`);
    }
    const tokenSet = new Set(rows.map((r) => r[0]));
    if (tokenSet.size !== rows.length) {
        throw new Error(`duplicate token byte sequences: ${rows.length - tokenSet.size}`);
    }
    const singleByteCovered = new Set<number>();
    for (const [b64] of rows) {
        const b = Buffer.from(b64, "base64");
        if (b.length === 1) singleByteCovered.add(b[0] ?? 0);
    }
    if (singleByteCovered.size !== 256) {
        throw new Error(`missing base bytes: ${256 - singleByteCovered.size}`);
    }

    // Sorting by rank keeps regenerated assets stable and diffable.
    rows.sort((a, b) => a[1] - b[1]);

    const assetsDir = join(import.meta.dir, "..", "assets");
    const body = rows.map(([b64, rank]) => `${b64} ${rank}`).join("\n");
    const vocabPath = join(assetsDir, "claude.tiktoken");
    writeFileSync(vocabPath, `${body}\n`, "utf8");
    const patPath = join(assetsDir, "claude.pat");
    writeFileSync(patPath, `${patStr}\n`, "utf8");

    // eslint-disable-next-line no-console
    console.log(
        `wrote ${rows.length} vocab entries (ranks ${ranks.length ? Math.min(...ranks) : 0}..${
            ranks.length ? Math.max(...ranks) : 0
        }) -> ${vocabPath}\nwrote pat_str (${patStr.length} chars) -> ${patPath}`,
    );
}

main();
