import { describe, expect, it } from "bun:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { setJsoncValue } from "./jsonc-edit";
import { parseConfigJsonc, readJsoncFile, sanitizeParsedJson } from "./jsonc-parser";

describe("parseConfigJsonc prototype-pollution hardening", () => {
    it("rejects dangerous keys recursively, including inside arrays", () => {
        const rejected: string[] = [];
        const parsed = parseConfigJsonc<Record<string, unknown>>(
            `{
                "safe": 1,
                "constructor": { "hidden": true },
                "nested": { "prototype": { "hidden": true }, "safe": 2 },
                "items": [
                    { "__proto__": { "polluted": true }, "safe": 3 },
                    { "safe": 4 }
                ]
            }`,
            { onRejectedKey: (path) => rejected.push(path.join(".")) },
        );

        expect(rejected).toEqual(["constructor", "nested.prototype", "items.0.__proto__"]);
        expect(Object.hasOwn(parsed, "constructor")).toBe(false);

        const nested = parsed.nested as Record<string, unknown>;
        expect(nested).toEqual({ safe: 2 });
        expect(Object.getPrototypeOf(nested)).toBe(Object.prototype);

        const items = parsed.items as Array<Record<string, unknown>>;
        expect(items).toEqual([{ safe: 3 }, { safe: 4 }]);
        expect(Object.getPrototypeOf(items[0])).toBe(Object.prototype);
        expect("polluted" in items[0]).toBe(false);
        expect(({} as Record<string, unknown>).polluted).toBeUndefined();
    });

    it("reports a primitive-valued __proto__ key like any other rejected key", () => {
        const rejected: string[] = [];
        const parsed = parseConfigJsonc<Record<string, unknown>>('{"__proto__": 5, "a": 1}', {
            onRejectedKey: (path) => rejected.push(path.join(".")),
        });

        expect(rejected).toEqual(["__proto__"]);
        expect(parsed).toEqual({ a: 1 });
    });

    it("does not report benign documents", () => {
        const rejected: string[] = [];
        parseConfigJsonc(
            `{
                // comment
                "a": [1, { "b": [true, null] }], /* block */
                "c": { "d": "e" },
            }`,
            { onRejectedKey: (path) => rejected.push(path.join(".")) },
        );

        expect(rejected).toEqual([]);
    });

    it("rebuilds a null-prototype object with a plain prototype and reports it", () => {
        const rejected: string[] = [];
        const raw = Object.create(null) as Record<string, unknown>;
        raw.a = 1;
        const sanitized = sanitizeParsedJson(raw, {
            onRejectedKey: (path) => rejected.push(path.join(".")),
        });

        expect(rejected).toEqual(["__proto__"]);
        expect(Object.getPrototypeOf(sanitized)).toBe(Object.prototype);
        expect(sanitized).toEqual({ a: 1 });
    });
});

describe("parseConfigJsonc grammar", () => {
    it("returns primitives for scalar-root documents", () => {
        expect(parseConfigJsonc("false")).toBe(false);
        expect(parseConfigJsonc("2")).toBe(2);
        expect(parseConfigJsonc('"deny"')).toBe("deny");
        expect(parseConfigJsonc("null")).toBe(null);
    });

    it("accepts comments, trailing commas, CRLF, lone-CR line endings, and a leading BOM", () => {
        expect(parseConfigJsonc('{\r\n// c\r\n"a": 1,\r\n}')).toEqual({ a: 1 });
        expect(parseConfigJsonc('{\r// comment\r"permission": "deny"\r}')).toEqual({
            permission: "deny",
        });
        expect(parseConfigJsonc('\uFEFF{"model": "a/b"}')).toEqual({ model: "a/b" });
        expect(parseConfigJsonc('{"a": 1 /*c*/, "b": [1, 2,],}')).toEqual({ a: 1, b: [1, 2] });
    });

    it("rejects array elision instead of inserting null", () => {
        expect(() => parseConfigJsonc('{"allowedTools": ["read",, "write"]}')).toThrow();
    });

    it("rejects an unterminated block comment and other malformed input", () => {
        expect(() => parseConfigJsonc('{"model": "a/b"}\n/* TODO')).toThrow();
        expect(() => parseConfigJsonc('{"a": 1/*c*/2}')).toThrow();
        expect(() => parseConfigJsonc("")).toThrow();
        expect(() => parseConfigJsonc("// only a comment")).toThrow();
        expect(() => parseConfigJsonc('{"a": 1} {"b": 2}')).toThrow();
    });

    it("rejects a number literal outside the double range, as serde does", () => {
        expect(() => parseConfigJsonc('{"a": 1e400}')).toThrow(SyntaxError);
        expect(() => parseConfigJsonc('{"a": [-1e999]}')).toThrow(SyntaxError);
        expect(parseConfigJsonc('{"a": 1e308}')).toEqual({ a: 1e308 });
    });

    it("keeps the last duplicate key, matching JSON.parse", () => {
        const text = '{"permission": {"bash": "deny"}, "permission": {"bash": "allow"}}';

        expect(parseConfigJsonc(text)).toEqual(JSON.parse(text));
    });

    it("accepts exactly the documents the editor accepts", () => {
        const documents = [
            '\uFEFF{"model": "a/b"}',
            '{\r// c\r"a": 1\r}',
            '{"a": 1}\n/* TODO',
            '{"a": [1,,2]}',
            '{"a": 1/*c*/2}',
            '{"a": [1, 2,],}',
            '{"a": 1e400}',
        ];

        for (const document of documents) {
            const readerAccepts = (() => {
                try {
                    parseConfigJsonc(document);
                    return true;
                } catch {
                    return false;
                }
            })();
            const editorAccepts = (() => {
                try {
                    setJsoncValue(document, ["zzz"], 1);
                    return true;
                } catch {
                    return false;
                }
            })();
            expect({ document, readerAccepts }).toEqual({ document, readerAccepts: editorAccepts });
        }
    });
});

describe("readJsoncFile", () => {
    const directory = mkdtempSync(join(tmpdir(), "jsonc-parser-test-"));

    function write(name: string, content: string): string {
        const path = join(directory, name);
        writeFileSync(path, content, "utf-8");
        return path;
    }

    it("reads the same grammar as parseConfigJsonc", () => {
        expect(readJsoncFile(write("bom.jsonc", '\uFEFF{"model": "a/b"}'))).toEqual({
            model: "a/b",
        });
        expect(readJsoncFile(write("cr.jsonc", '{\r// c\r"permission": "deny"\r}'))).toEqual({
            permission: "deny",
        });
    });

    it("returns null for a missing or malformed file and forwards sanitizer options", () => {
        const rejected: string[] = [];

        expect(readJsoncFile(join(directory, "missing.jsonc"))).toBeNull();
        expect(readJsoncFile(write("bad.jsonc", '{"a": [1,,2]}'))).toBeNull();
        expect(
            readJsoncFile(write("proto.jsonc", '{"__proto__": {"x": 1}, "a": 1}'), {
                onRejectedKey: (path) => rejected.push(path.join(".")),
            }),
        ).toEqual({ a: 1 });
        expect(rejected).toEqual(["__proto__"]);
    });
});
