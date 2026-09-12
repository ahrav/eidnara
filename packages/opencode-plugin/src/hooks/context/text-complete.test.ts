import { describe, expect, it } from "bun:test";

import { createTextCompleteHandler } from "./text-complete";

const SECTION = "\u00a7"; // U+00A7, the section sign character used in Eidnara tag prefixes.

async function run(text: string): Promise<string> {
    const handler = createTextCompleteHandler();
    const output = { text };
    await handler({ sessionID: "s1", messageID: "m1", partID: "p1" }, output);
    return output.text;
}

async function expectEach(cases: ReadonlyArray<readonly [string, string]>): Promise<void> {
    for (const [input, expected] of cases) {
        expect(await run(input)).toBe(expected);
    }
}

describe("text-complete handler", () => {
    describe("leading tag prefix (canonical Eidnara tagger output)", () => {
        it("#given leading §N§ prefixes of any count, digit width, or spacing #when handler runs #then strips them all", async () => {
            await expectEach([
                [`${SECTION}42${SECTION} Hello world`, "Hello world"],
                [`${SECTION}55${SECTION} ${SECTION}56${SECTION} Response text`, "Response text"],
                [
                    `${SECTION}56${SECTION} ${SECTION}56${SECTION} Bailan Kimi 2.5 done`,
                    "Bailan Kimi 2.5 done",
                ],
                [`${SECTION}999${SECTION} Large tag content`, "Large tag content"],
                [`${SECTION}42${SECTION}Response without space`, "Response without space"],
                [`${SECTION}2030${SECTION} ${SECTION}2030${SECTION} Run`, "Run"],
            ]);
        });
    });

    describe("cargo-culted tag emission (models mimicking Eidnara tag notation mid-text)", () => {
        it('#given §N§ pairs, §N"> hybrids, or stray § anywhere in the text #when handler runs #then removes every occurrence', async () => {
            await expectEach([
                [
                    `Looking at ${SECTION}40827${SECTION} the result is X`,
                    "Looking at  the result is X",
                ],
                [`Hello ${SECTION}40827">Oracle confirmed`, "Hello Oracle confirmed"],
                [`See ${SECTION} marker for details`, "See  marker for details"],
                [
                    `${SECTION}42${SECTION} The pattern ${SECTION}40827${SECTION} appeared.`,
                    "The pattern  appeared.",
                ],
                [
                    `First ${SECTION}100${SECTION}, then ${SECTION}200${SECTION}, finally ${SECTION}300${SECTION}.`,
                    "First , then , finally .",
                ],
            ]);
        });
    });

    describe("legitimate § usage", () => {
        it("#given §-prefixed section reference (§5.1) #when handler runs #then strips § (cosmetic loss, by design)", async () => {
            expect(await run(`As described in ${SECTION}5.1 of the plan`)).toBe(
                "As described in 5.1 of the plan",
            );
        });
    });

    describe("text without § (no-op)", () => {
        it("#given bare digit residue, tag-like numbers, plain text, or empty text #when handler runs #then leaves the text unchanged", async () => {
            for (const text of [
                `2030  2030  2030\u00b0 Run clippy`,
                "99  Actually executing now. Running fmt:",
                "No tag here, just normal text",
                "",
            ]) {
                expect(await run(text)).toBe(text);
            }
        });
    });
});
