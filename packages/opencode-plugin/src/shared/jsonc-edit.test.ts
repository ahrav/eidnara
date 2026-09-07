import { describe, expect, it } from "bun:test";

import { parseTree, printParseErrorCode } from "jsonc-parser";

import { appendJsoncArrayValues, removeJsoncArrayEntries, setJsoncValue } from "./jsonc-edit";
import { parseConfigJsonc } from "./jsonc-parser";

/** Every editor output must re-parse under the editor's own grammar. */
function parseErrors(text: string): string[] {
    const errors: { error: number; offset: number }[] = [];
    parseTree(text, errors, { allowTrailingComma: true });
    return errors.map((error) => `${printParseErrorCode(error.error)}@${error.offset}`);
}

describe("appendJsoncArrayValues", () => {
    it("appends after the last entry when the closing bracket shares its line", () => {
        const text = '{"a": ["read",\n  "write"]}';
        const updated = appendJsoncArrayValues(text, ["a"], ["rm"]);

        expect(parseErrors(updated)).toEqual([]);
        expect(parseConfigJsonc(updated)).toEqual({ a: ["read", "write", "rm"] });
    });

    it("keeps entry order when the closing bracket shares the last entry's line", () => {
        const text = '{\n  "permission": [\n    "read"]\n}';
        const updated = appendJsoncArrayValues(text, ["permission"], ["exec"]);

        expect(parseErrors(updated)).toEqual([]);
        expect(parseConfigJsonc(updated)).toEqual({ permission: ["read", "exec"] });
    });

    it("appends several values when the closing bracket shares the last entry's line", () => {
        const text = '{"a": [\n  "xxxxxxxx"]\n}';
        const updated = appendJsoncArrayValues(text, ["a"], ["p", "q"]);

        expect(parseErrors(updated)).toEqual([]);
        expect(parseConfigJsonc(updated)).toEqual({ a: ["xxxxxxxx", "p", "q"] });
    });

    it("keeps a trailing comma that sits on the closing-bracket line", () => {
        const text = '{\n  "permission": [\n    "read"\n  ,]\n}';
        const updated = appendJsoncArrayValues(text, ["permission"], ["exec"]);

        expect(parseErrors(updated)).toEqual([]);
        expect(parseConfigJsonc(updated)).toEqual({ permission: ["read", "exec"] });
    });

    it("appends to a multi-line array with the bracket on its own line, preserving comments", () => {
        const text = '{\n  "a": [\n    "one", // keep\n    "two"\n  ]\n}';
        const updated = appendJsoncArrayValues(text, ["a"], ["three"]);

        expect(updated).toBe('{\n  "a": [\n    "one", // keep\n    "two",\n    "three"\n  ]\n}');
    });

    it("keeps the trailing comma style of a multi-line array", () => {
        const text = '{\n  "a": [\n    "one",\n  ]\n}';
        const updated = appendJsoncArrayValues(text, ["a"], ["two"]);

        expect(updated).toBe('{\n  "a": [\n    "one",\n    "two",\n  ]\n}');
    });

    it("appends to single-line and empty arrays", () => {
        expect(appendJsoncArrayValues('{"a": ["x"]}', ["a"], ["y"])).toBe('{"a": ["x","y"]}');
        expect(appendJsoncArrayValues('{"a": []}', ["a"], ["y"])).toBe('{"a": ["y"]}');
        expect(appendJsoncArrayValues('{"a": [\n]}', ["a"], ["y"])).toBe('{"a": [\n  "y"\n]}');
    });

    it("keeps a single-line array's trailing comma style", () => {
        const updated = appendJsoncArrayValues('{"a": [1, 2,]}', ["a"], [3]);

        expect(updated).toBe('{"a": [1, 2,3,]}');
        expect(parseErrors(updated)).toEqual([]);
        expect(parseConfigJsonc(updated)).toEqual({ a: [1, 2, 3] });
    });

    it("keeps a single-line entry's trailing comment attached to that entry", () => {
        expect(appendJsoncArrayValues('{"a": [1 /* one */]}', ["a"], [2])).toBe(
            '{"a": [1 /* one */,2]}',
        );
        expect(appendJsoncArrayValues('{"a": [1 /* one */,]}', ["a"], [2])).toBe(
            '{"a": [1 /* one */,2,]}',
        );
    });

    it("inserts outside a block comment that spans lines inside an empty array", () => {
        const updated = appendJsoncArrayValues('{"a": [/* heading\ncontinued */]}', ["a"], [1]);

        expect(updated).toBe('{"a": [/* heading\ncontinued */1]}');
        expect(parseConfigJsonc(updated)).toEqual({ a: [1] });
    });

    it("rejects values JSON cannot represent instead of writing the token `undefined`", () => {
        expect(() => appendJsoncArrayValues('{"list": ["a"]}', ["list"], [undefined])).toThrow(
            TypeError,
        );
        expect(() => appendJsoncArrayValues('{"x": 1}', ["list"], [1, undefined])).toThrow(
            TypeError,
        );
        expect(() => appendJsoncArrayValues('{"list": 1}', ["list"], [() => 1])).toThrow(TypeError);
    });
});

describe("setJsoncValue", () => {
    it("rejects values JSON cannot represent instead of writing the token `undefined`", () => {
        expect(() => setJsoncValue('{"enabled": true}', ["enabled"], undefined)).toThrow(TypeError);
        expect(() => setJsoncValue('{"enabled": true}', ["enabled"], () => 1)).toThrow(TypeError);
    });

    it("replaces an existing value in place and inserts a missing one structurally", () => {
        expect(setJsoncValue('{"a": 1} // c', ["a"], 2)).toBe('{"a": 2} // c');
        expect(parseConfigJsonc(setJsoncValue("{}", ["a", "b"], true))).toEqual({
            a: { b: true },
        });
    });

    it("edits a document with a leading byte-order mark and keeps the mark", () => {
        expect(setJsoncValue('\uFEFF{"a": 1}', ["a"], 2)).toBe('\uFEFF{"a": 2}');
        const inserted = setJsoncValue('\uFEFF{"a": 1}', ["b"], 2);
        expect(inserted.startsWith("\uFEFF")).toBe(true);
        expect(parseConfigJsonc(inserted)).toEqual({ a: 1, b: 2 });
        expect(appendJsoncArrayValues('\uFEFF{"a": [1]}', ["a"], [2])).toBe('\uFEFF{"a": [1,2]}');
        expect(removeJsoncArrayEntries('\uFEFF{"a": [1]}', ["a"], () => true).text).toBe(
            '\uFEFF{"a": []}',
        );
    });

    it("refuses to edit a document with duplicate object keys", () => {
        const text = '{"permission": {"bash": "allow"}, "permission": {"bash": "allow"}}';

        expect(() => setJsoncValue(text, ["permission", "bash"], "deny")).toThrow(
            /duplicate key "permission"/,
        );
    });

    it("refuses duplicate keys that are nested below the edited path", () => {
        const text = '{"a": {"x": 1, "x": 2}, "b": 1}';

        expect(() => setJsoncValue(text, ["b"], 2)).toThrow(/duplicate key "x"/);
    });
});

describe("removeJsoncArrayEntries", () => {
    it("refuses to edit a document with duplicate object keys", () => {
        const text = '{"plugins": ["evil"], "plugins": ["evil"]}';

        expect(() =>
            removeJsoncArrayEntries(text, ["plugins"], (entry) => entry === "evil"),
        ).toThrow(/duplicate key "plugins"/);
    });

    it("keeps the preceding entry's inline comment when removing the last entry", () => {
        const text =
            '{\n  "permission": [\n    "read", // safe\n    "write", // also safe\n    "rm" // dangerous\n  ]\n}';
        const result = removeJsoncArrayEntries(text, ["permission"], (entry) => entry === "rm");

        expect(result.removed).toBe(true);
        expect(result.text).toBe(
            '{\n  "permission": [\n    "read", // safe\n    "write" // also safe\n  ]\n}',
        );
    });

    it("keeps an own-line comment before the closing bracket when removing the last entry", () => {
        const text =
            '{\n  "permission": [\n    "read",\n    "rm"\n    // NOTE: keep this list alphabetized\n  ]\n}';
        const result = removeJsoncArrayEntries(text, ["permission"], (entry) => entry === "rm");

        expect(result.text).toBe(
            '{\n  "permission": [\n    "read"\n    // NOTE: keep this list alphabetized\n  ]\n}',
        );
    });

    it("removes the only entry even when a trailing comma sits on the closing-bracket line", () => {
        const text = '{\n  "permission": [\n    "rm"\n  ,]\n}';
        const result = removeJsoncArrayEntries(text, ["permission"], (entry) => entry === "rm");

        expect(parseErrors(result.text)).toEqual([]);
        expect(result.removed).toBe(true);
        expect(parseConfigJsonc(result.text)).toEqual({ permission: [] });
    });

    it("removes every entry when a trailing comma sits on the closing-bracket line", () => {
        const text = '{"a": ["p",\n  "x"\n  ,]}';
        const result = removeJsoncArrayEntries(text, ["a"], () => true);

        expect(parseErrors(result.text)).toEqual([]);
        expect(parseConfigJsonc(result.text)).toEqual({ a: [] });
    });

    it("removes first, middle, last, and only entries from single-line arrays", () => {
        const text = '{"a": ["x", "y", "z"]}';
        const drop = (value: string) =>
            removeJsoncArrayEntries(text, ["a"], (entry) => entry === value).text;

        expect(drop("x")).toBe('{"a": ["y", "z"]}');
        expect(drop("y")).toBe('{"a": ["x", "z"]}');
        expect(drop("z")).toBe('{"a": ["x", "y"]}');
        expect(removeJsoncArrayEntries('{"a": ["x"]}', ["a"], () => true).text).toBe('{"a": []}');
    });

    it("keeps the surviving entries' comments when removing a middle entry", () => {
        const text =
            '{\n  "a": [\n    "x", // keep x\n    "y", // drop y\n    "z" // keep z\n  ]\n}';
        const result = removeJsoncArrayEntries(text, ["a"], (entry) => entry === "y");

        expect(result.text).toBe('{\n  "a": [\n    "x", // keep x\n    "z" // keep z\n  ]\n}');
    });

    it("removes a middle entry whose comma is followed by a block comment on the same line", () => {
        const text = '{"a": ["x", "y", /* c */ "z"]}';
        const result = removeJsoncArrayEntries(text, ["a"], (entry) => entry === "y");

        expect(result.text).toBe('{"a": ["x", "z"]}');
    });

    it("keeps same-line trivia after the preceding separator when removing the entry to its right", () => {
        const drop = (text: string, value: unknown) =>
            removeJsoncArrayEntries(text, ["a"], (entry) => entry === value).text;

        expect(drop('{"a": [1, /* keep with 1\ncontinued */ 2]}', 2)).toBe(
            '{"a": [1 /* keep with 1\ncontinued */]}',
        );
        expect(drop('{"a": ["x", "y", /* c */ "z"]}', "z")).toBe('{"a": ["x", "y" /* c */]}');
        expect(drop('{"a": ["x", /* c */ "y", "z"]}', "y")).toBe('{"a": ["x", /* c */ "z"]}');
        expect(drop('{"a": [ /* header */ "x", "y"]}', "x")).toBe('{"a": [ /* header */ "y"]}');
    });

    it("removes own-line comments that belong to the removed entry", () => {
        const text = '{\n  "a": [\n    "x",\n    // about y\n    "y",\n    "z"\n  ]\n}';
        const result = removeJsoncArrayEntries(text, ["a"], (entry) => entry === "y");

        expect(result.text).toBe('{\n  "a": [\n    "x",\n    "z"\n  ]\n}');
    });

    it("keeps own-line comments that belong to the next entry when removing the first", () => {
        const text = '{\n  "a": [\n    "x",\n    // about y\n    "y"\n  ]\n}';
        const result = removeJsoncArrayEntries(text, ["a"], (entry) => entry === "x");

        expect(result.text).toBe('{\n  "a": [\n    // about y\n    "y"\n  ]\n}');
    });

    it("reports removed=false when nothing matches or the path is not an array", () => {
        expect(removeJsoncArrayEntries('{"a": ["x"]}', ["a"], () => false)).toEqual({
            text: '{"a": ["x"]}',
            removed: false,
        });
        expect(removeJsoncArrayEntries('{"a": 1}', ["a"], () => true)).toEqual({
            text: '{"a": 1}',
            removed: false,
        });
    });
});
