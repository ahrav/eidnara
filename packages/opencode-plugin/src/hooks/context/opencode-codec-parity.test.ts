import { describe, expect, it } from "bun:test";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { encodeOpenCodeWindowToCk } from "./opencode-transform-adapter";

interface ParityCase {
    name: string;
    messages: unknown[];
    expected: unknown[];
    refusal?: { code: string; message: string };
}

const path = join(import.meta.dir, "__fixtures__", "opencode-codec-parity.json");
const fixture = JSON.parse(readFileSync(path, "utf8")) as {
    description: string;
    cases: ParityCase[];
    refusals: ParityCase[];
};

// The daemon's OpenCode codec reads the same file and must derive `expected` from `messages`
// (crates/daemon/src/codec/opencode.rs), so the plugin encoder stays the oracle.
if (process.env.EIDNARA_WRITE_OPENCODE_CODEC_PARITY !== undefined) {
    for (const parity of [...fixture.cases, ...fixture.refusals]) {
        // The plugin sends the window through `JSON.stringify`, so the daemon reads that form.
        parity.messages = JSON.parse(JSON.stringify(parity.messages));
        parity.expected = JSON.parse(JSON.stringify(encodeOpenCodeWindowToCk(parity.messages)));
    }
    writeFileSync(path, `${JSON.stringify(fixture, null, 2)}\n`);
}

describe("OpenCode codec parity", () => {
    for (const parity of [...fixture.cases, ...fixture.refusals]) {
        it(`encodes the ${parity.name} window to the CK the daemon derives`, () => {
            expect(encodeOpenCodeWindowToCk(parity.messages)).toStrictEqual(
                parity.expected as never,
            );
        });
    }
});
