/**
 * Regenerates `testdata/token-golden.json` from the `ai-tokenizer` claude encoding, the
 * independent oracle the Rust port is checked against.
 *
 *   bun crates/tokenizer/gen/gen-token-golden.ts
 *
 * The tokenizer receives a null-prototype copy of `stringEncoder`. `ai-tokenizer` indexes the
 * encoder with plain property access, so with the stock object a pre-token such as `valueOf`
 * or `hasOwnProperty` resolves to an `Object.prototype` function instead of `undefined` and is
 * emitted as a non-numeric "token". The copy makes those lookups miss, as they should.
 *
 * Every id is checked to be a non-negative integer before the fixture is written, so a
 * defective oracle fails here with the case label instead of producing JSON that
 * `tests/token_golden.rs` cannot deserialize.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";

// `ai-tokenizer` is a root dev dependency; resolving from the repository root makes the
// generator independent of the current working directory.
const repoRoot = join(import.meta.dir, "..", "..", "..");
const claudeEntry = Bun.resolveSync("ai-tokenizer/encoding/claude", repoRoot);
const tokenizerEntry = Bun.resolveSync("ai-tokenizer", repoRoot);

interface GoldenCase {
    label: string;
    text: string;
    ids: number[];
}

async function main(): Promise<void> {
    const enc = (await import(claudeEntry)) as { stringEncoder: Record<string, number> };
    const { default: Tokenizer } = await import(tokenizerEntry);
    const ownKeyEncoding = {
        ...enc,
        stringEncoder: Object.assign(Object.create(null) as Record<string, number>, enc.stringEncoder),
    };
    const tk = new Tokenizer(ownKeyEncoding);
    const encode = (text: string): unknown[] => Array.from(tk.encode(text, "all") as unknown[]);

    const corpus: Array<[string, string]> = [
        ["empty", ""],
        ["single-space", " "],
        ["ascii-basic", "hello world"],
        ["sentence-punct", "The quick brown fox jumps over the lazy dog."],
        ["contractions", "I'm sure it's you've done well, they'll see, we're 'ready'."],
        ["leading-space-word", " leadingspace"],
        ["multi-space-run", "a    b\t\tc\n\nd"],
        ["trailing-whitespace", "trailing   "],
        ["digits-runs", "1234567890 42 007 3.14159 1,000,000"],
        ["mixed-alnum", "abc123def456 v2 gpt-5.5 claude-4"],
        ["punct-runs", "!!! ??? ... --- +++ === //// |||| ***"],
        ["symbols", "a→b + c // d | e \\ f ~ g @ h # i $ j % k ^"],
        ["code-snippet", "const x = foo(bar, baz).map((y) => y * 2);"],
        ["json-blob", '{"key":"value","n":42,"arr":[1,2,3],"nested":{"a":true}}'],
        ["path-like", "/Users/foo/Work/Projects/eidnara/crates/tokenizer/src/lib.rs"],
        ["special-substrings", "before <EOT> mid <META_START> after <SOS> end"],
        ["special-adjacent", "<EOT><META><META_START><META_END><SOS>"],
        ["unicode-accents", "café résumé naïve Zürich piñata Malmö"],
        ["cjk", "你好世界 これはテストです 안녕하세요 世界"],
        ["emoji", "hello 👋 world 🌍 rocket 🚀 family 👨‍👩‍👧‍👦 flag 🏳️‍🌈"],
        ["cyrillic-greek", "Привет мир αβγδε Ελληνικά Русский"],
        ["mixed-script", "user说hello и café🚀 test123"],
        ["newlines-heavy", "line1\nline2\n\nline3\n\n\nline4\r\nwindows"],
        ["repeated-token", "the the the the the the the the"],
        ["long-word", "supercalifragilisticexpialidocious"],
        ["url", "https://docs.eidnara.dev/reference/configuration/?q=x&y=1"],
        [
            "prose-paragraph",
            "Eidnara rewrites the message array on every LLM call to keep a long " +
                "session inside the context window without losing history. Durable SQLite " +
                "state, never ephemeral; if storage is unavailable the plugin fails closed.",
        ],
        ["rtl-arabic-hebrew", "مرحبا بالعالم שלום עולם mixed العربية with English"],
        ["combining-marks", "e\u0301 a\u0300 n\u0303 o\u0308 cafe\u0301 A\u030a\u0301"],
        ["zero-width", "a\u200bb\u200cc\u200dd\ufeffe word\u00a0nbsp"],
        ["control-chars", "tab\tvertical\x0bform\x0cbell text"],
        ["surrogate-pairs", "𝕳𝖊𝖑𝖑𝖔 𝟙𝟚𝟛 🄰🄱🄲 𐍈 𠀀𠀁"],
        ["repeated-char-run", `${"a".repeat(300)} ${"=".repeat(100)} ${" ".repeat(50)}x`],
        [
            "session-chunk",
            "U: Can you fix the tagger perf issue?\nA: I traced it to tag.initFromDb " +
                "reloading all 105k rows every pass. TC: read({filePath:'tagger.ts',startLine:1," +
                "endLine:80}) -> loaded 80 lines. The floor-scoped query is 2.8µs vs 32ms full scan.",
        ],
        [
            "stack-trace",
            "Error: QuickJSUseAfterFree\n    at Lifetime.assertAlive (quickjs.ts:412:11)\n" +
                "    at QuickJSContext.getProp (context.ts:88:5)\n    at <anonymous>:1:1",
        ],
        [
            "large-mixed-blob",
            Array.from({ length: 40 }, (_, i) =>
                `Line ${i}: the quick brown fox (café ${i * 3.14}) 你好 🚀 {"k":${i}} https://x.io/${i}`,
            ).join("\n"),
        ],
        // ECMAScript `\s` excludes U+0085 (NEL) and includes U+FEFF; the Rust pattern spells
        // that class out so these split the same way on both sides.
        ["nel-after-space", "wait \u0085 what\u0085 mojibake a \u0085b"],
        ["nel-runs", "a\u0085\u0085 b\u0085\nc"],
        ["bom-leading", "\ufeffconst x = 1;"],
        ["bom-between-punct", "x.\ufeff.y \ufeffz"],
        ["ideographic-space", "全角\u3000スペース\u3000test"],
        ["nbsp-and-narrow-nbsp", "a\u00a0b\u202fc\u205fd\u2028e\u2029f"],
        // Pre-tokens equal to Object.prototype member names that are not vocabulary entries.
        // With a plain-object encoder these come back as functions from the reference.
        [
            "proto-member-names",
            "obj.valueOf(); o.hasOwnProperty(k); a.isPrototypeOf(b); s.toLocaleString(); p.propertyIsEnumerable(q)",
        ],
        ["proto-member-own-keys", "x.constructor; y.toString(); z.__proto__"],
        ["dunder-proto-short", "__proto__"],
        ["valueof-short", "valueOf"],
    ];

    const cases: GoldenCase[] = corpus.map(([label, text]) => {
        const raw = encode(text);
        const bad = raw.findIndex((id) => typeof id !== "number" || !Number.isInteger(id) || id < 0);
        if (bad !== -1) {
            throw new Error(
                `case '${label}': ids[${bad}] is not a non-negative integer (${String(raw[bad])})`,
            );
        }
        return { label, text, ids: raw as number[] };
    });

    const outPath = join(import.meta.dir, "..", "testdata", "token-golden.json");
    writeFileSync(outPath, `${JSON.stringify(cases, null, 2)}\n`, "utf8");

    const totalToks = cases.reduce((n, c) => n + c.ids.length, 0);
    // eslint-disable-next-line no-console
    console.log(`wrote ${cases.length} golden cases (${totalToks} total tokens) -> ${outPath}`);
}

await main();
