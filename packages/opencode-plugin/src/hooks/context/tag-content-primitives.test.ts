import { describe, expect, it } from "bun:test";

import {
    byteSize,
    isThinkingPart,
    peelLeadingMcTagNotation,
    prependTag,
    stripDanglingTagNotationGlobally,
    stripPersistedAssistantText,
    stripTagPrefix,
    stripTagSectionCharacters,
    stripWellFormedLeadingTagPrefix,
} from "./tag-content-primitives";

const SECTION = "\u00a7";

const DEGREE = "\u00b0";
const CYRILLIC_HA = "\u04a9"; // ҩ — a stray closer a model improvised in the wild

describe("dangling-open tag cleanup (§N + improvised closer, no closing §)", () => {
    it("strips §N + a non-ASCII improvised closer (§11865ҩ → '')", () => {
        expect(stripPersistedAssistantText(`${SECTION}11865${CYRILLIC_HA} done`)).toBe("done");
        expect(stripDanglingTagNotationGlobally(`mid ${SECTION}11865${CYRILLIC_HA} text`)).toBe(
            "mid  text",
        );
    });

    it("strips a dangling §N with NO closer, keeping the content after the space", () => {
        expect(stripPersistedAssistantText(`${SECTION}42 files changed`)).toBe("files changed");
    });

    it("keeps the whole digit run of a multi-digit decimal section reference (§12.3 → 12.3)", () => {
        // `§42.1` must not backtrack to match `§4` and leave `2.1`.
        expect(stripDanglingTagNotationGlobally(`see ${SECTION}12.3 and ${SECTION}5.1`)).toBe(
            `see ${SECTION}12.3 and ${SECTION}5.1`,
        );
        expect(stripDanglingTagNotationGlobally(`see ${SECTION}42.1 for details`)).toBe(
            `see ${SECTION}42.1 for details`,
        );
        expect(stripPersistedAssistantText(`see ${SECTION}12.3 and ${SECTION}123.45`)).toBe(
            "see 12.3 and 123.45",
        );
        expect(stripPersistedAssistantText(`see ${SECTION}42.1 for details`)).toBe(
            "see 42.1 for details",
        );
        expect(stripTagPrefix(`${SECTION}12.3 of the plan`)).toBe(`${SECTION}12.3 of the plan`);
        expect(stripTagPrefix(`${SECTION}42.1 hello`)).toBe(`${SECTION}42.1 hello`);
    });

    it.each([
        ["ASCII letter", "important"],
        ["CJK", "修复完成"],
        ["Latin with diacritic", "éclair"],
        ["Greek", "αβγ"],
        ["Cyrillic word", "готово"],
        ["non-ASCII digit", "٣ items"],
        ["emoji", "😀 fixed"],
        ["heading marker", "# Heading"],
        ["bullet marker", "* item"],
        ["code fence", "```ts"],
        ["parenthesis", "(note)"],
    ])("does not consume the first %s character after a dangling tag", (_label, content) => {
        expect(stripPersistedAssistantText(`${SECTION}42${content}`)).toBe(content);
        expect(stripTagPrefix(`${SECTION}42${content}`)).toBe(content);
        expect(stripDanglingTagNotationGlobally(`${SECTION}42${content}`)).toBe(content);
    });

    it.each([
        ["dollar sign", "$"],
        ["double quote", '"'],
        ["single quote", "'"],
        ["xml hybrid tail", '">'],
        ["Cyrillic ha", CYRILLIC_HA],
    ])("consumes the %s improvised closer", (_label, closer) => {
        expect(stripPersistedAssistantText(`${SECTION}42${closer} done`)).toBe("done");
        expect(stripTagPrefix(`${SECTION}42${closer} done`)).toBe("done");
    });
});

describe("stripTagPrefix (transform §N§ notation only)", () => {
    it("#given well-formed leading prefix #when stripTagPrefix runs #then removes it", () => {
        expect(stripTagPrefix(`${SECTION}42${SECTION} Hello`)).toBe("Hello");
    });

    it("#given tags separated by U+0085 #when stripTagPrefix runs #then removes them all", () => {
        expect(
            stripTagPrefix(`${SECTION}42${SECTION}\u0085${SECTION}43${SECTION}\u0085Hello`),
        ).toBe("Hello");
        expect(stripTagPrefix(`${SECTION}42">${SECTION}\u0085${SECTION}7$\u0085Hello`)).toBe(
            "Hello",
        );
    });

    it("#given well-formed prefix hiding a malformed one #when stripTagPrefix runs #then removes both whole", () => {
        // Removing `§1§ ` exposes `§2">§2§ `; the dangling pass must not take only `§2"` and leave `>§2§`.
        expect(
            stripTagPrefix(`${SECTION}1${SECTION} ${SECTION}2">${SECTION}2${SECTION} hello`),
        ).toBe("hello");
        expect(
            stripTagPrefix(
                `${SECTION}1${SECTION} ${SECTION}2">${SECTION}2${SECTION} ${SECTION}3${SECTION} ${SECTION}4">${SECTION}4${SECTION} hello`,
            ),
        ).toBe("hello");
        expect(
            prependTag(7, `${SECTION}1${SECTION} ${SECTION}2">${SECTION}2${SECTION} hello`),
        ).toBe(`${SECTION}7${SECTION} hello`);
    });

    it("#given a dangling prefix before a well-formed tag #when stripTagPrefix runs #then removes both whole", () => {
        // The dangling rule must stop before `§2§` so the pair rule can take it as a unit.
        expect(stripTagPrefix(`${SECTION}1 ${SECTION}2${SECTION} hello`)).toBe("hello");
        expect(prependTag(9, `${SECTION}1 ${SECTION}2${SECTION} hello`)).toBe(
            `${SECTION}9${SECTION} hello`,
        );
        expect(stripPersistedAssistantText(`${SECTION}1 ${SECTION}2${SECTION} hello`)).toBe(
            "hello",
        );
    });

    it("#given a very long adversarial prefix chain #when stripTagPrefix runs #then finishes in linear time", () => {
        let value = "";
        for (let i = 0; i < 40_000; i++) {
            value +=
                i % 2 === 0
                    ? `${SECTION}${i}${SECTION} `
                    : `${SECTION}${i}">${SECTION}${i}${SECTION} `;
        }
        value += "hello";

        const started = performance.now();
        expect(stripTagPrefix(value)).toBe("hello");
        // The 40,000-prefix input distinguishes linear scans from quadratic scans.
        expect(performance.now() - started).toBeLessThan(200);
    });

    it("#given legitimate leading numbers #when stripTagPrefix runs #then preserves them", () => {
        expect(stripTagPrefix("99 files are located in folder zzz")).toBe(
            "99 files are located in folder zzz",
        );

        expect(stripTagPrefix("6 8 9 tasks from todo list completed")).toBe(
            "6 8 9 tasks from todo list completed",
        );

        expect(stripTagPrefix("1. do this now, 2. do that next")).toBe(
            "1. do this now, 2. do that next",
        );

        expect(stripTagPrefix("2024 roadmap")).toBe("2024 roadmap");
    });

    it("#given mid-text tag after bare digits #when stripTagPrefix runs #then leaves mid-text tag", () => {
        expect(stripTagPrefix(`2030  ${SECTION}42${SECTION} Hello`)).toBe(
            `2030  ${SECTION}42${SECTION} Hello`,
        );
    });
});

describe("stripPersistedAssistantText (persistence boundary)", () => {
    it("#given leading well-formed prefixes #when strip runs #then removes pairs cleanly", () => {
        expect(
            stripPersistedAssistantText(`${SECTION}2030${SECTION} ${SECTION}2030${SECTION} Run`),
        ).toBe("Run");
    });

    it("#given mid-text cargo-cult pair #when strip runs #then removes whole pair", () => {
        expect(
            stripPersistedAssistantText(`Looking at ${SECTION}40827${SECTION} the result is X`),
        ).toBe("Looking at  the result is X");
    });

    it("#given tag after bare digits #when strip runs #then removes mid-text pair only", () => {
        expect(stripPersistedAssistantText(`2030  ${SECTION}42${SECTION} Hello`)).toBe(
            "2030   Hello",
        );
    });

    it("#given malformed hybrid mid-text #when strip runs #then removes hybrid", () => {
        expect(stripPersistedAssistantText(`Hello ${SECTION}40827">Oracle confirmed`)).toBe(
            "Hello Oracle confirmed",
        );
    });

    it("#given bare digit residue without ? #when strip runs #then leaves digits", () => {
        expect(stripPersistedAssistantText(`2030  2030  2030${DEGREE} Run clippy`)).toBe(
            `2030  2030  2030${DEGREE} Run clippy`,
        );

        expect(stripPersistedAssistantText("99  Actually executing now. Running fmt:")).toBe(
            "99  Actually executing now. Running fmt:",
        );
    });
});

describe("stripWellFormedLeadingTagPrefix", () => {
    it("#given leading ?N? prefix #when stripWellFormedLeadingTagPrefix runs #then removes it", () => {
        expect(stripWellFormedLeadingTagPrefix(`${SECTION}42${SECTION} Hello`)).toBe("Hello");
    });
});

describe("stripTagSectionCharacters", () => {
    it("#given ? characters #when stripTagSectionCharacters runs #then removes them", () => {
        expect(stripTagSectionCharacters(`${SECTION}42${SECTION}`)).toBe("42");
    });
});

describe("prependTag", () => {
    it("#given bare digit residue #when prependTag runs #then does not strip digits", () => {
        expect(prependTag(7, `2030  2030  2030${DEGREE} Run clippy`)).toBe(
            `${SECTION}7${SECTION} 2030  2030  2030${DEGREE} Run clippy`,
        );
    });

    it("#given existing well-formed prefix #when prependTag runs #then replaces with new tag", () => {
        expect(prependTag(9, `${SECTION}3${SECTION} Hello`)).toBe(`${SECTION}9${SECTION} Hello`);
    });

    it("#given malformed xml hybrid prefix #when prependTag runs #then strips before prepending", () => {
        expect(prependTag(11, `${SECTION}15298">${SECTION}15298${SECTION} hello`)).toBe(
            `${SECTION}11${SECTION} hello`,
        );
    });
});

describe("peelLeadingMcTagNotation", () => {
    it("#given well-formed or malformed leading prefix #when peel runs #then splits the raw prefix from the body", () => {
        expect(peelLeadingMcTagNotation(`${SECTION}3${SECTION} hello`)).toEqual({
            tagPrefix: `${SECTION}3${SECTION} `,
            body: "hello",
        });
        expect(peelLeadingMcTagNotation(`${SECTION}9">${SECTION}9${SECTION} body`)).toEqual({
            tagPrefix: `${SECTION}9">${SECTION}9${SECTION} `,
            body: "body",
        });
    });
});

describe("stripPersistedAssistantText edge cases", () => {
    it("#given only tag notation or whitespace #when strip runs #then trims to empty", () => {
        expect(stripPersistedAssistantText(`${SECTION}15298">§15298§ `)).toBe("");
        expect(stripPersistedAssistantText(`${SECTION}42${SECTION} `)).toBe("");
        expect(stripPersistedAssistantText(`   `)).toBe("");
    });
});

describe("byteSize", () => {
    it("#given ascii, empty, and multibyte strings #when byteSize runs #then returns the UTF-8 byte length", () => {
        expect(byteSize("hello")).toBe(5);
        expect(byteSize("")).toBe(0);
        expect(byteSize("§42§")).toBe(6);
    });
});

describe("isThinkingPart", () => {
    it("#given thinking, reasoning, text, null, and primitive inputs #when isThinkingPart runs #then narrows on the reasoning types only", () => {
        expect(isThinkingPart({ type: "thinking", thinking: "..." })).toBe(true);
        expect(isThinkingPart({ type: "reasoning", reasoning: "..." })).toBe(true);
        expect(isThinkingPart({ type: "text", text: "hello" })).toBe(false);
        expect(isThinkingPart(null)).toBe(false);
        expect(isThinkingPart("string")).toBe(false);
    });
});
