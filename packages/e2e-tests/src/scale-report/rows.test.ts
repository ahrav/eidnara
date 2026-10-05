import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { type PassRow, serializePassRow } from "./rows";
import { WRITER_FIXTURE_ROWS } from "./writer-fixture";

/** The committed fixture `crates/eval-core/tests/scale.rs` parses with the Rust parser. */
const FIXTURE = resolve(
    import.meta.dir,
    "../../../../crates/eval-core/testdata/scale/writer-rows.jsonl",
);

describe("scale report pass row writer", () => {
    it("writes the bytes the Rust parser's fixture holds", () => {
        const written = WRITER_FIXTURE_ROWS.map((row) => `${serializePassRow(row)}\n`).join("");
        expect(written).toBe(readFileSync(FIXTURE, "utf8"));
    });

    it("refuses a row the Rust parser would refuse", () => {
        const base = WRITER_FIXTURE_ROWS[0] as PassRow;
        expect(() => serializePassRow({ ...base, response_us: 1.5 })).toThrow("response_us");
        expect(() => serializePassRow({ ...base, rss_bytes: 2 ** 60 })).toThrow("rss_bytes");
        expect(() => serializePassRow({ ...base, refusal: "declined" })).toThrow("refusal");
        expect(() => serializePassRow({ ...base, tier: "1m" as never })).toThrow("tier");
        expect(() => serializePassRow({ ...base, session: " " })).toThrow("session");
    });
});
