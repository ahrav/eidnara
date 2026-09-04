/**
 * Differential corpus for `guard.sh`: random strings from every benchmark arm distribution plus
 * mutation-fuzzed golden cases, each encoded by `ai-tokenizer@1.0.6` the way
 * `gen-token-golden.ts` does (null-prototype encoder). `tests/differential.rs` reads the JSONL
 * this writes and requires identical ids.
 *
 *   bun crates/tokenizer/gen/gen-diff-corpus.ts <out.jsonl> [seed]
 *
 * Reference ids come from encoding each regex piece separately and concatenating. That is what
 * `encodeOrdinary` does internally; calling it piecewise only bypasses its whole-text shortcut
 * for inputs under 10 characters, which is equivalent because no vocabulary entry spans a piece
 * boundary (checked below, fails loudly if the vocabulary ever changes that).
 *
 * Documented divergence filtered by exact rule: the reference decodes candidate byte slices
 * with a BOM-stripping `TextDecoder`, so any slice beginning with `EF BB BF` is scored as if the
 * BOM were absent. That can only happen when U+FEFF shares a pre-token piece with another
 * character, so such texts are dropped and counted; a text whose U+FEFF is a piece by itself
 * is compared exactly.
 */
import { writeFileSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { ARM_GENERATORS, rng, shortString, type Rng } from "./gen-bench-corpus.ts";

const repoRoot = join(import.meta.dir, "..", "..", "..");
const claudeEntry = Bun.resolveSync("ai-tokenizer/encoding/claude", repoRoot);
const tokenizerEntry = Bun.resolveSync("ai-tokenizer", repoRoot);

const MAX_PIECE_BYTES = 4096;

const int = (r: Rng, lo: number, hi: number): number => lo + Math.floor(r() * (hi - lo + 1));
const pick = <T>(r: Rng, xs: readonly T[]): T => xs[Math.floor(r() * xs.length)] as T;

const MUTATION_POOL = [
    " ", "  ", "\n", "\r\n", "\t", "\u00a0", "\u3000", "\u0085", "\ufeff", "\u200b", "\u2028",
    "a", "Z", "é", "ß", "你", "の", "한", "🚀", "👨‍👩‍👧‍👦", "\u0301", "'s", "'t", "'re", "'ll", "'d",
    "0", "42", "3.14", ".", ",", "!", "?", "-", "_", "/", "\\", "<", ">", "{", "}", "\"", "`",
    "valueOf", "__proto__", "<EOT>", "\u{10ffff}", "\u{1d573}", "\u0000", "\u007f",
];

function mutate(r: Rng, text: string, rounds: number): string {
    let cps = Array.from(text);
    for (let k = 0; k < rounds; k++) {
        const at = cps.length ? int(r, 0, cps.length) : 0;
        switch (int(r, 0, 4)) {
            case 0:
                cps.splice(at, 0, ...Array.from(pick(r, MUTATION_POOL)));
                break;
            case 1:
                if (cps.length) cps.splice(Math.min(at, cps.length - 1), 1);
                break;
            case 2: {
                const len = int(r, 1, 8);
                const chunk = cps.slice(at, at + len);
                cps.splice(int(r, 0, cps.length), 0, ...chunk);
                break;
            }
            case 3:
                if (cps.length > 1) {
                    const j = int(r, 0, cps.length - 1);
                    const i = Math.min(at, cps.length - 1);
                    [cps[i], cps[j]] = [cps[j]!, cps[i]!];
                }
                break;
            default: {
                const len = int(r, 1, 6);
                cps.splice(at, len, ...Array.from(pick(r, MUTATION_POOL)).concat(cps.slice(at, at + len)));
            }
        }
    }
    return cps.join("");
}

interface Case {
    text: string;
    ids: number[];
}

async function main(): Promise<void> {
    const outPath = process.argv[2];
    if (!outPath) throw new Error("usage: gen-diff-corpus.ts <out.jsonl> [seed]");
    const seed = Number(process.argv[3] ?? 0xd1ff);

    const enc = (await import(claudeEntry)) as {
        pat_str: string;
        stringEncoder: Record<string, number>;
    };
    const { default: Tokenizer } = await import(tokenizerEntry);
    const ownKeyEncoding = {
        ...enc,
        stringEncoder: Object.assign(Object.create(null) as Record<string, number>, enc.stringEncoder),
    };
    const tk = new Tokenizer(ownKeyEncoding);
    const pieceRe = new RegExp(enc.pat_str, "ug");

    for (const tokenStr of Object.keys(enc.stringEncoder)) {
        if ((tokenStr.match(pieceRe) ?? []).length > 1) {
            throw new Error(`vocabulary entry spans piece boundary: ${JSON.stringify(tokenStr)}`);
        }
    }

    const pieces = (text: string): string[] => text.match(pieceRe) ?? [];
    const bomDivergent = (text: string): boolean =>
        pieces(text).some((p) => p.includes("\ufeff") && Array.from(p).length > 1);
    const encode = (text: string): number[] => {
        const out: number[] = [];
        for (const p of pieces(text)) {
            for (const id of tk.encode(p, "all") as unknown[]) {
                if (typeof id !== "number" || !Number.isInteger(id) || id < 0) {
                    throw new Error(`non-integer id for ${JSON.stringify(p)}: ${String(id)}`);
                }
                out.push(id);
            }
        }
        return out;
    };

    const r = rng(seed);
    const texts: string[] = [];
    const arms = Object.entries(ARM_GENERATORS);

    // 14k arm-distributed strings of 1..600 bytes.
    for (let i = 0; i < 14_000; i++) {
        const [, gen] = pick(r, arms);
        texts.push(gen(r, int(r, 1, 600)));
    }
    // 4k short strings, same distribution as the short_strings arm.
    for (let i = 0; i < 4_000; i++) texts.push(shortString(r));
    // Mutation-fuzzed golden cases, 1..12 rounds each.
    const golden = JSON.parse(readFileSync(join(import.meta.dir, "..", "testdata", "token-golden.json"), "utf8")) as Array<{ text: string }>;
    for (let i = 0; i < 4_000; i++) {
        const base = pick(r, golden).text;
        texts.push(mutate(r, base, int(r, 1, 12)));
    }
    // Splices of two golden cases at random points.
    for (let i = 0; i < 1_000; i++) {
        const a = Array.from(pick(r, golden).text);
        const b = Array.from(pick(r, golden).text);
        texts.push(a.slice(0, int(r, 0, a.length)).join("") + b.slice(int(r, 0, b.length)).join(""));
    }
    // 2k long strings up to 8 KiB where every piece is at or under MAX_PIECE_BYTES, so the
    // chunking bypass is exercised on inputs longer than the cap.
    let long = 0;
    while (long < 2_000) {
        const [, gen] = pick(r, arms);
        const t = gen(r, int(r, MAX_PIECE_BYTES + 1, 8192));
        if (!bomDivergent(t) && pieces(t).every((p) => Buffer.byteLength(p) <= MAX_PIECE_BYTES)) {
            texts.push(t);
            long++;
        }
    }

    let dropped = 0;
    let longCount = 0;
    const cases: Case[] = [];
    for (const text of texts) {
        if (bomDivergent(text)) {
            dropped++;
            continue;
        }
        if (Buffer.byteLength(text) > MAX_PIECE_BYTES) longCount++;
        cases.push({ text, ids: encode(text) });
    }
    if (cases.length < 20_000) throw new Error(`only ${cases.length} cases after filtering`);
    writeFileSync(outPath, cases.map((c) => JSON.stringify(c)).join("\n") + "\n", "utf8");
    console.log(
        `wrote ${cases.length} cases (${longCount} longer than MAX_PIECE_BYTES, ${dropped} dropped by BOM rule) -> ${outPath}`,
    );
}

await main();
