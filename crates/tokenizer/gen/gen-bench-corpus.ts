/**
 * Seeded generator for the benchmark corpus under `benches/corpus/` and for the differential
 * corpus `guard.sh` builds at check time. Fixtures are committed; rerunning this script must
 * reproduce them byte for byte, so the PRNG is fixed (mulberry32) and every arm draws from its
 * own seeded stream.
 *
 *   bun crates/tokenizer/gen/gen-bench-corpus.ts            # rewrite benches/corpus/*
 *
 * `gen-diff-corpus.ts` imports the arm generators to draw random strings from the same
 * distributions with a different seed.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export type Rng = () => number;

/** mulberry32: 32-bit state, returns floats in [0, 1). Good enough for corpus shaping. */
export function rng(seed: number): Rng {
    let a = seed >>> 0;
    return () => {
        a = (a + 0x6d2b79f5) >>> 0;
        let t = a;
        t = Math.imul(t ^ (t >>> 15), t | 1);
        t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
}

const int = (r: Rng, lo: number, hi: number): number => lo + Math.floor(r() * (hi - lo + 1));
const pick = <T>(r: Rng, xs: readonly T[]): T => xs[Math.floor(r() * xs.length)] as T;
const chance = (r: Rng, p: number): boolean => r() < p;

const WORDS = (
    "the of and to in a is that for it as was with be by on not he i this are or his from at " +
    "which but have an had they you were their one all we can her has there been if more when " +
    "will would who so no she other its may these than also any some into only them those such " +
    "over new two time first out our even most made after many must through before years where " +
    "much your way well down should because each just those people how too little state good " +
    "very make world still own see men work long here get both between life being under never " +
    "day same another know while last might us great old year off come since against go came " +
    "right used take three states himself few house use during without again place american " +
    "around however home small found mrs thought went say part once general high upon school " +
    "every don't does got united left number course war until always away something fact though " +
    "water less public put think almost hand enough far took head yet government system better " +
    "set told nothing night end why called didn't eyes find going look asked later knew point " +
    "next program city business give group toward young days let room president side social " +
    "given present several order national possible rather second face per among form important " +
    "often things looked early white john case become large big need four within felt along " +
    "children saw best church ever least power development light thing seemed family interest " +
    "want members mind country area others turned although open god service certain kind problem " +
    "began different door thus help means sense whole matter perhaps itself york times human " +
    "law line above name example action company hands local show whether five history gave " +
    "today either act feet across quite taken anything seen having death experience body half " +
    "really week word car field already themselves information tell together shall college " +
    "money period held keep sure real probably free seems political question behind cannot " +
    "office brought whose special heard major ago moment study known result available street " +
    "economic boy position reason change south board individual job society west close turn " +
    "love true community full force court air seem necessary wife future age voice center " +
    "woman common control cost policy front six top girl clear further land run provide feel " +
    "party material minutes strong third table religious mother music tokenizer bytes merge " +
    "rank vocabulary latency corpus benchmark scanner boundary whitespace lookahead"
).split(" ");

const CONTRACTIONS = ["I'm", "it's", "you've", "they'll", "we're", "don't", "she'd", "can't", "won't", "isn't"];

export function prose(r: Rng, bytes: number): string {
    const out: string[] = [];
    let n = 0;
    while (n < bytes) {
        const words = int(r, 5, 22);
        const parts: string[] = [];
        for (let i = 0; i < words; i++) {
            let w = chance(r, 0.06) ? pick(r, CONTRACTIONS) : pick(r, WORDS);
            if (i === 0) w = w[0]!.toUpperCase() + w.slice(1);
            if (i > 0 && chance(r, 0.08)) w = "," + " " + w;
            else if (i > 0) w = " " + w;
            parts.push(w);
        }
        let s = parts.join("") + pick(r, [".", ".", ".", "!", "?", ";", ":"]);
        if (chance(r, 0.15)) s = `"${s}"`;
        if (chance(r, 0.1)) s += ` (${pick(r, WORDS)} ${int(r, 1, 999)})`;
        s += chance(r, 0.12) ? "\n\n" : " ";
        out.push(s);
        n += s.length;
    }
    return out.join("");
}

const IDENTS = ["buf", "len", "offset", "piece", "rank", "table", "ctx", "state", "index", "value", "result", "count", "next", "prev", "start", "end", "bytes", "text", "config", "handle"];
const TYPES = ["u32", "usize", "&str", "Vec<u8>", "Option<Rank>", "bool", "string", "number", "Uint8Array", "Promise<void>"];

function rustSnippet(r: Rng): string {
    const f = pick(r, IDENTS);
    const g = pick(r, IDENTS);
    const t = pick(r, TYPES);
    return [
        `/// ${pick(r, WORDS)} ${pick(r, WORDS)} ${pick(r, WORDS)}.`,
        `pub fn ${f}_${g}(${g}: &[u8], ${f}: ${t}) -> Result<${pick(r, TYPES)}, Error> {`,
        `    let mut ${pick(r, IDENTS)} = Vec::with_capacity(${g}.len() / ${int(r, 2, 8)});`,
        `    for (i, &b) in ${g}.iter().enumerate() {`,
        `        if b >= 0x${int(r, 16, 255).toString(16)} || i % ${int(r, 2, 16)} == 0 {`,
        `            ${pick(r, IDENTS)}.push((i as u32) << ${int(r, 1, 7)} | ${int(r, 0, 4095)});`,
        `        }`,
        `    }`,
        `    ${pick(r, IDENTS)}.sort_unstable_by_key(|x| x & 0x${int(r, 1, 65535).toString(16)});`,
        `    Ok(${pick(r, IDENTS)}.len() as ${pick(r, ["u32", "usize"])})`,
        `}`,
        ``,
    ].join("\n");
}

function tsSnippet(r: Rng): string {
    const f = pick(r, IDENTS);
    return [
        `export async function ${f}${pick(r, IDENTS)}(${pick(r, IDENTS)}: ${pick(r, TYPES)}, opts?: { retries?: number }): Promise<${pick(r, TYPES)}> {`,
        `  const ${pick(r, IDENTS)} = opts?.retries ?? ${int(r, 0, 9)};`,
        `  const items = await Promise.all(list.map((x, i) => x.${pick(r, IDENTS)}(i * ${int(r, 2, 99)}, "${pick(r, WORDS)}-${int(r, 0, 999)}")));`,
        `  if (!items.length) throw new Error(\`no ${pick(r, WORDS)} for \${${pick(r, IDENTS)}}\`);`,
        `  return items.filter((v) => v !== undefined && v.${pick(r, IDENTS)} > ${int(r, 0, 100)}).length as unknown as ${pick(r, TYPES)};`,
        `}`,
        ``,
    ].join("\n");
}

function jsonSnippet(r: Rng): string {
    const fields = int(r, 3, 8);
    const kv: string[] = [];
    for (let i = 0; i < fields; i++) {
        const k = pick(r, IDENTS);
        const v = chance(r, 0.4) ? String(int(r, 0, 100000)) : chance(r, 0.5) ? `"${pick(r, WORDS)}_${pick(r, WORDS)}"` : pick(r, ["true", "false", "null", `[${int(r, 1, 9)}, ${int(r, 1, 9)}, ${int(r, 1, 9)}]`]);
        kv.push(`    "${k}": ${v}`);
    }
    return `{\n${kv.join(",\n")}\n}\n`;
}

export function code(r: Rng, bytes: number): string {
    const out: string[] = [];
    let n = 0;
    while (n < bytes) {
        const s = pick(r, [rustSnippet, rustSnippet, tsSnippet, jsonSnippet])(r);
        out.push(s);
        n += s.length;
    }
    return out.join("");
}

function cjkChar(r: Rng): string {
    const roll = r();
    if (roll < 0.62) return String.fromCodePoint(int(r, 0x4e00, 0x9fa5));
    if (roll < 0.82) return String.fromCodePoint(int(r, 0x3041, 0x3096));
    if (roll < 0.95) return String.fromCodePoint(int(r, 0x30a1, 0x30fa));
    return String.fromCodePoint(int(r, 0xac00, 0xd7a3));
}

export function cjk(r: Rng, bytes: number): string {
    const out: string[] = [];
    let n = 0;
    while (n < bytes) {
        const len = int(r, 3, 24);
        let s = "";
        for (let i = 0; i < len; i++) s += cjkChar(r);
        s += pick(r, ["、", "。", "，", "」", "「", "！", "？", "・", "\n", "。\n"]);
        if (chance(r, 0.05)) s += ` ${pick(r, WORDS)} `;
        if (chance(r, 0.04)) s += String(int(r, 0, 2030));
        out.push(s);
        n += Buffer.byteLength(s);
    }
    return out.join("");
}

const EMOJI = ["👋", "🌍", "🚀", "👨‍👩‍👧‍👦", "🏳️‍🌈", "😀", "🎉", "✅", "❤️", "🧠", "🇯🇵", "👍🏽"];
const COMBINING = ["\u0301", "\u0300", "\u0303", "\u0308", "\u030a", "\u0327"];
const RTL_WORDS = ["مرحبا", "بالعالم", "العربية", "שלום", "עולם", "ישראל", "كتاب", "ספר"];
const ODD_SPACE = ["\u00a0", "\ufeff", "\u0085", "\u3000", "\u202f", "\u2028", "\u200b", "\u2009"];
const SCRIPTS = ["Привет", "мир", "Ελληνικά", "αβγδε", "café", "naïve", "Zürich", "日本語", "한글", "𝕳𝖊𝖑𝖑𝖔", "𐍈", "ǅ", "ß"];

export function mixedUnicode(r: Rng, bytes: number): string {
    const out: string[] = [];
    let n = 0;
    while (n < bytes) {
        const roll = r();
        let s: string;
        if (roll < 0.25) s = pick(r, WORDS) + " ";
        else if (roll < 0.4) s = pick(r, SCRIPTS) + " ";
        else if (roll < 0.52) s = pick(r, EMOJI) + (chance(r, 0.5) ? " " : "");
        else if (roll < 0.62) s = pick(r, WORDS) + pick(r, COMBINING) + pick(r, COMBINING) + " ";
        else if (roll < 0.75) s = pick(r, RTL_WORDS) + " ";
        else if (roll < 0.9) s = pick(r, ODD_SPACE);
        else s = pick(r, [".", ",", "\n", "—", "…", "«", "»", "¿", "¡", "·"]);
        out.push(s);
        n += Buffer.byteLength(s);
    }
    return out.join("");
}

export function whitespaceHeavy(r: Rng, bytes: number): string {
    const out: string[] = [];
    let n = 0;
    while (n < bytes) {
        const depth = int(r, 0, 12);
        const indent = chance(r, 0.5) ? "    ".repeat(depth) : "\t".repeat(depth);
        const body = chance(r, 0.15) ? "" : `${pick(r, IDENTS)} ${pick(r, ["=", "+=", "->", ":"])} ${pick(r, WORDS)}`;
        const trailing = chance(r, 0.4) ? " ".repeat(int(r, 1, 6)) : "";
        const eol = chance(r, 0.35) ? "\r\n" : "\n";
        const extra = chance(r, 0.2) ? eol.repeat(int(r, 1, 3)) : "";
        const s = indent + body + trailing + eol + extra;
        out.push(s);
        n += s.length;
    }
    return out.join("");
}

export function numeric(r: Rng, bytes: number): string {
    const out: string[] = [];
    let n = 0;
    while (n < bytes) {
        const roll = r();
        let s: string;
        if (roll < 0.25) s = String(int(r, 0, 2 ** 31));
        else if (roll < 0.45) s = `${int(r, 0, 9999)}.${int(r, 0, 999999)}`;
        else if (roll < 0.6) s = `0x${int(r, 0, 2 ** 31).toString(16)}`;
        else if (roll < 0.75) s = `${pick(r, ["id", "ref", "sku", "txn"])}-${int(r, 100000, 999999)}-${int(r, 0, 99)}`;
        else if (roll < 0.85) s = `${int(r, 1, 999)},${String(int(r, 0, 999)).padStart(3, "0")},${String(int(r, 0, 999)).padStart(3, "0")}`;
        else s = `${int(r, 1970, 2030)}-${String(int(r, 1, 12)).padStart(2, "0")}-${String(int(r, 1, 28)).padStart(2, "0")}T${String(int(r, 0, 23)).padStart(2, "0")}:${String(int(r, 0, 59)).padStart(2, "0")}Z`;
        s += pick(r, [" ", " ", ", ", "\n", "\t", " | "]);
        out.push(s);
        n += s.length;
    }
    return out.join("");
}

/** One string of 1..64 bytes drawn from the arm distributions; used by `short_strings`. */
export function shortString(r: Rng): string {
    const target = int(r, 1, 64);
    const gen = pick(r, [prose, code, cjk, mixedUnicode, whitespaceHeavy, numeric]);
    let s = gen(r, target);
    // Whole code points only: a split surrogate pair would not survive UTF-8 encoding.
    const cps = Array.from(s);
    while (Buffer.byteLength(cps.join("")) > target) cps.pop();
    s = cps.join("");
    if (s.length === 0) s = pick(r, ["a", " ", "1", "!"]);
    return s;
}

export function shortStrings(r: Rng, count: number): string[] {
    const xs: string[] = [];
    for (let i = 0; i < count; i++) xs.push(shortString(r));
    return xs;
}

export function adversarialLongPiece(): string {
    return "a".repeat(64 * 1024);
}

export const ARM_GENERATORS: Record<string, (r: Rng, bytes: number) => string> = {
    ascii_prose: prose,
    code,
    cjk,
    mixed_unicode: mixedUnicode,
    whitespace_heavy: whitespaceHeavy,
    numeric,
};

const SIZE = 256 * 1024;

function main(): void {
    const dir = join(import.meta.dir, "..", "benches", "corpus");
    mkdirSync(dir, { recursive: true });
    let seed = 0x70_4b_33_6e; // "tok3n"
    for (const [name, gen] of Object.entries(ARM_GENERATORS)) {
        const text = gen(rng(seed++), SIZE);
        writeFileSync(join(dir, `${name}.txt`), text, "utf8");
        console.log(`${name}: ${Buffer.byteLength(text)} bytes`);
    }
    const shorts = shortStrings(rng(seed++), 10_000);
    writeFileSync(join(dir, "short_strings.json"), `${JSON.stringify(shorts)}\n`, "utf8");
    console.log(`short_strings: ${shorts.length} strings, ${shorts.reduce((n, s) => n + Buffer.byteLength(s), 0)} bytes`);
    writeFileSync(join(dir, "adversarial_long_piece.txt"), adversarialLongPiece(), "utf8");
    console.log("adversarial_long_piece: 65536 bytes");
}

if (import.meta.main) main();
