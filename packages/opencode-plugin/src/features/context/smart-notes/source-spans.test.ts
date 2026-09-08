import { describe, expect, test } from "bun:test";

import { decodeStringLiteral, maskSourceSpans, scanSourceSpans } from "./source-spans";

const NON_CODE = new Set(["comment", "string", "template"] as const);

function kinds(source: string): string[] {
    return scanSourceSpans(source).map(
        (span) => `${span.kind}:${source.slice(span.start, span.end)}`,
    );
}

describe("scanSourceSpans", () => {
    test("separates comments and quoted literals from code", () => {
        expect(kinds(`a // line "x"\n/* block 'y' */ "s\\"q" 'p' b`)).toEqual([
            "code:a ",
            `comment:// line "x"`,
            "code:\n",
            "comment:/* block 'y' */",
            "code: ",
            `string:"s\\"q"`,
            "code: ",
            "string:'p'",
            "code: b",
        ]);
    });

    test("treats a template with nested expressions and quotes as one span", () => {
        const source = 'x = `a ${ f("}", `${y}`) } b`; z';
        expect(kinds(source)).toEqual([
            "code:x = ",
            `template:${source.slice(4, source.length - 3)}`,
            "code:; z",
        ]);
    });

    test("does not start a string inside a regular-expression literal", () => {
        expect(kinds(`if (/"/.test(s)) { a = 1 / 2; b = x / y / z; }`)).toEqual([
            "code:if (",
            `string:/"/`,
            "code:.test(s)) { a = 1 / 2; b = x / y / z; }",
        ]);
    });

    test("closes an unterminated string at the end of its line", () => {
        expect(kinds(`a = "oops\nb = 1`)).toEqual(["code:a = ", `string:"oops`, "code:\nb = 1"]);
    });
});

describe("maskSourceSpans", () => {
    test("blanks interiors while keeping offsets, newlines, and delimiters", () => {
        const source = `f("ab") // c\ng('d')`;
        const masked = maskSourceSpans(source, NON_CODE);
        expect(masked).toHaveLength(source.length);
        expect(masked).toBe(`f("  ")     \ng(' ')`);
    });
});

describe("decodeStringLiteral", () => {
    test("decodes escape sequences the way the JavaScript parser does", () => {
        expect(decodeStringLiteral(String.raw`a\"b\'c\\d\/e`)).toBe(`a"b'c\\d/e`);
        expect(decodeStringLiteral(String.raw`\n\t\r\0\x41\u0042\u{1F600}`)).toBe("\n\t\r\0AB😀");
        expect(decodeStringLiteral("line\\\ncontinued")).toBe("linecontinued");
    });
});
