import { describe, expect, it, spyOn } from "bun:test";
import fixture from "../../../../../crates/daemon/tests/fixtures/transform-edit-recipe-v1.json";
import {
    applyRecipe,
    canonicalJsonLength,
    type EditRecipe,
    MAX_RECONSTRUCTED_BYTES,
    MAX_REVISION_BYTES,
    parseRecipe,
    type RecipeOperation,
    type RecipeRejectionCode,
    type RecipeSourceBase,
} from "./edit-recipe";
import * as moduleWire from "./module-wire";

interface FixtureCase {
    name: string;
    input_revision?: string;
    input: unknown[];
    previous?: { revision: string; values: unknown[] };
    recipe: unknown;
    expect: { ok: boolean; output?: unknown[]; canonical_bytes?: number };
}

/** The rejection class each shared case must land in; the fixture itself leaves taxonomies open. */
const expectedCodes: Record<string, RecipeRejectionCode> = {
    "wrong base revision": "wrong_base_revision",
    "empty base revision": "invalid_revision",
    "revision one byte over the limit": "invalid_revision",
    "empty output revision": "invalid_revision",
    "missing previous base for a previous keep": "missing_previous_base",
    "previous keep without previous_output_revision": "missing_previous_base",
    "previous revision does not match the retained output": "wrong_previous_revision",
    "previous_output_revision without a previous keep": "unused_previous_revision",
    "unknown opcode": "unknown_operation",
    "unknown source": "unknown_source",
    "zero count": "zero_count",
    "negative start": "unsafe_integer",
    "fractional start": "unsafe_integer",
    "string start": "unsafe_integer",
    "unsafe integer start": "unsafe_integer",
    "unsafe integer count": "unsafe_integer",
    "range end past the end of input": "out_of_bounds",
    "range end arithmetic near the safe limit": "overflow",
    "repeated range": "backward_range",
    "overlapping range": "backward_range",
    "backward range": "backward_range",
    "backward previous range with a forward input range": "backward_range",
    "empty insert": "empty_insert",
    "insert values not an array": "malformed",
    "missing operations": "malformed",
    "operations not an array": "malformed",
    "operation is not an object": "malformed",
    "unknown field on a keep": "malformed",
    "unknown field on an insert": "malformed",
    "valid prefix followed by a malformed final operation": "out_of_bounds",
    "valid prefix followed by an unknown final opcode": "unknown_operation",
    "a small recipe cannot repeat a source range to amplify output": "backward_range",
};

const cases = (fixture as { cases: FixtureCase[]; input_revision_default: string }).cases;
const defaultRevision = (fixture as { input_revision_default: string }).input_revision_default;

function base(revision: string, values: readonly unknown[]): RecipeSourceBase {
    return { revision, values, lengths: values.map((value) => canonicalJsonLength(value)) };
}

describe("edit recipe fixtures", () => {
    it("agree with the shared cases on acceptance and exact output", () => {
        expect((fixture as { schema: string }).schema).toBe("transform-edit-recipe-fixtures-v1");
        let accepted = 0;
        let rejected = 0;
        for (const testCase of cases) {
            const input = base(testCase.input_revision ?? defaultRevision, testCase.input);
            const previous = testCase.previous
                ? base(testCase.previous.revision, testCase.previous.values)
                : undefined;
            const parsed = parseRecipe(testCase.recipe);
            const result = parsed.ok ? applyRecipe(parsed.recipe, input, previous) : parsed;
            if (testCase.expect.ok) {
                if (!result.ok) throw new Error(`${testCase.name}: ${result.rejection.detail}`);
                expect(result.values).toEqual(testCase.expect.output as unknown[]);
                expect(result.lengths).toEqual(
                    result.values.map((value) => canonicalJsonLength(value)),
                );
                expect(result.bytes).toBe(testCase.expect.canonical_bytes as number);
                expect(result.bytes).toBe(canonicalJsonLength(result.values));
                expect(result.lengths).toEqual(result.values.map(canonicalJsonLength));
                // Kept entries are the source's own objects, not copies.
                for (const [index, value] of result.values.entries()) {
                    if (value !== null && typeof value === "object") {
                        const fromInput = testCase.input.includes(value);
                        const fromPrevious = testCase.previous?.values.includes(value) ?? false;
                        const literal = (parsed.ok ? parsed.recipe.operations : []).some(
                            (operation) =>
                                operation.op === "insert" && operation.values.includes(value),
                        );
                        expect([fromInput, fromPrevious, literal].some(Boolean)).toBe(true);
                        expect(result.values[index]).toBe(value);
                    }
                }
                accepted += 1;
            } else {
                if (result.ok)
                    throw new Error(`${testCase.name}: accepted ${result.values.length} entries`);
                const code = expectedCodes[testCase.name];
                if (code === undefined) throw new Error(`${testCase.name}: no expected code`);
                expect([testCase.name, result.rejection.code]).toEqual([testCase.name, code]);
                rejected += 1;
            }
        }
        expect(accepted).toBeGreaterThanOrEqual(15);
        expect(rejected).toBeGreaterThanOrEqual(30);
        expect(Object.keys(expectedCodes)).toHaveLength(rejected);
    });

    it("keeps a previous entry by reference and never edits either source", () => {
        const testCase = cases.find((entry) => entry.name.startsWith("AE2"));
        if (!testCase?.previous) throw new Error("AE2 case missing");
        const inputValues = structuredClone(testCase.input);
        const previousValues = structuredClone(testCase.previous.values);
        const inputJson = JSON.stringify(inputValues);
        const previousJson = JSON.stringify(previousValues);
        const parsed = parseRecipe(testCase.recipe);
        if (!parsed.ok) throw new Error(parsed.rejection.detail);
        const result = applyRecipe(
            parsed.recipe,
            base(defaultRevision, inputValues),
            base(testCase.previous.revision, previousValues),
        );
        if (!result.ok) throw new Error(result.rejection.detail);
        expect(result.values[0]).toBe(previousValues[0]);
        expect(result.values[1]).toBe(previousValues[1]);
        expect(result.values[2]).toBe(inputValues[3]);
        expect(result.lengths).toEqual(result.values.map((value) => canonicalJsonLength(value)));
        expect(JSON.stringify(inputValues)).toBe(inputJson);
        expect(JSON.stringify(previousValues)).toBe(previousJson);
    });
});

describe("canonical JSON length", () => {
    it("matches compact bytes for nested values, omitted fields, Unicode and numeric boundaries", () => {
        const values: unknown[] = [
            undefined,
            null,
            true,
            false,
            "",
            'x"y\\\n\b\f\r\t\u0000\u001f\u007fé☃😀\u2028\ud800x\udfff',
            [],
            {},
            { omitted: undefined },
            { z: undefined, b: [undefined, {}, [], { omitted: undefined, value: false }], a: null },
            -0,
            1.5,
            0.00001,
            0.000001,
            -0.000001,
            0.30000000000000004,
            1e15,
            1e16,
            2 ** 60,
            -(2 ** 63) + 1024,
            -(2 ** 63),
            2 ** 64 - 4096,
            2 ** 64,
            1e21,
            1e23,
            Number.MIN_VALUE,
            Number.MAX_VALUE,
            Number.NaN,
            Number.POSITIVE_INFINITY,
            Number.NEGATIVE_INFINITY,
        ];
        for (const value of values) {
            for (const nested of [value, [value, undefined], { "😀": value, "\uE000": [value] }]) {
                expect(canonicalJsonLength(nested)).toBe(
                    Buffer.byteLength(moduleWire.serdeJsonCompact(nested)),
                );
            }
        }
    });

    it("counts nested values without composite serialization or key sorting", () => {
        const value = { z: [undefined, { b: "é☃😀", a: 0.000001 }, []], a: {}, omitted: undefined };
        const expected = Buffer.byteLength(moduleWire.serdeJsonCompact(value));
        const compactSpy = spyOn(moduleWire, "serdeJsonCompact");
        const stringifySpy = spyOn(JSON, "stringify");
        const sortSpy = spyOn(Array.prototype, "sort");
        try {
            const length = canonicalJsonLength(value);
            const compositeSerializations = [
                ...compactSpy.mock.calls,
                ...stringifySpy.mock.calls,
            ].filter(([input]) => input !== null && typeof input === "object").length;
            const keySorts = sortSpy.mock.calls.length;
            expect(length).toBe(expected);
            expect({ compositeSerializations, keySorts }).toEqual({
                compositeSerializations: 0,
                keySorts: 0,
            });
        } finally {
            sortSpy.mockRestore();
            stringifySpy.mockRestore();
            compactSpy.mockRestore();
        }
    });
});

describe("edit recipe bounds", () => {
    const recipe: EditRecipe = {
        baseRevision: "b",
        outputRevision: "o",
        operations: [{ op: "keep", source: "input", start: 0, count: 2 }],
    };

    it("accepts the exact reconstructed limit and rejects one byte over before allocating", () => {
        const half = Math.floor((MAX_RECONSTRUCTED_BYTES - 3) / 2);
        const exact = [half, MAX_RECONSTRUCTED_BYTES - 3 - half];
        const accepted = applyRecipe(recipe, { revision: "b", values: [1, 2], lengths: exact });
        expect(accepted).toEqual({
            ok: true,
            values: [1, 2],
            lengths: exact,
            bytes: MAX_RECONSTRUCTED_BYTES,
        });
        const over = applyRecipe(recipe, {
            revision: "b",
            values: [1, 2],
            lengths: [exact[0] + 1, exact[1]],
        });
        expect(over.ok).toBe(false);
        if (!over.ok) expect(over.rejection.code).toBe("output_too_large");
        const overflow = applyRecipe(recipe, {
            revision: "b",
            values: [1, 2],
            lengths: [Number.MAX_SAFE_INTEGER, 1],
        });
        expect(overflow.ok).toBe(false);
        if (!overflow.ok) expect(overflow.rejection.code).toBe("overflow");
    });

    it("measures literals with the daemon's compact rule and counts brackets and commas", () => {
        expect(canonicalJsonLength({ b: [1, 2.5, "x\ny"], a: null })).toBe(
            Buffer.byteLength('{"a":null,"b":[1,2.5,"x\\ny"]}'),
        );
        const literal = { info: { id: "x" }, parts: [] };
        const lengthOf = canonicalJsonLength(literal);
        const fill = MAX_RECONSTRUCTED_BYTES - 2 - lengthOf - 1;
        const inserted: EditRecipe = {
            baseRevision: "b",
            outputRevision: "o",
            operations: [
                { op: "keep", source: "input", start: 0, count: 1 },
                { op: "insert", values: [literal] },
            ],
        };
        expect(applyRecipe(inserted, { revision: "b", values: [1], lengths: [fill] }).ok).toBe(
            true,
        );
        expect(applyRecipe(inserted, { revision: "b", values: [1], lengths: [fill + 1] }).ok).toBe(
            false,
        );
    });

    it("bounds revisions by UTF-8 bytes", () => {
        const exact = "r".repeat(MAX_REVISION_BYTES);
        expect(parseRecipe({ base_revision: exact, output_revision: "o", operations: [] }).ok).toBe(
            true,
        );
        expect(
            parseRecipe({ base_revision: `${exact}r`, output_revision: "o", operations: [] }).ok,
        ).toBe(false);
        expect(
            parseRecipe({ base_revision: "☃".repeat(43), output_revision: "o", operations: [] }).ok,
        ).toBe(false);
        expect(
            parseRecipe({ base_revision: "☃".repeat(42), output_revision: "o", operations: [] }).ok,
        ).toBe(true);
    });

    it("requires own data properties without invoking accessors", () => {
        const inherited = Object.assign(Object.create({ base_revision: "b" }) as object, {
            output_revision: "o",
            operations: [],
        });
        expect(parseRecipe(inherited)).toMatchObject({
            ok: false,
            rejection: { code: "malformed" },
        });

        let accessed = false;
        const operation = {};
        Object.defineProperty(operation, "op", {
            enumerable: true,
            get() {
                accessed = true;
                throw new Error("accessor invoked");
            },
        });
        let parsed: ReturnType<typeof parseRecipe> | undefined;
        expect(() => {
            parsed = parseRecipe({
                base_revision: "b",
                output_revision: "o",
                operations: [operation],
            });
        }).not.toThrow();
        expect(accessed).toBe(false);
        expect(parsed).toMatchObject({ ok: false, rejection: { code: "malformed" } });

        const operations = [null];
        Object.defineProperty(operations, 0, {
            enumerable: true,
            get() {
                accessed = true;
                throw new Error("indexed accessor invoked");
            },
        });
        parsed = undefined;
        expect(() => {
            parsed = parseRecipe({
                base_revision: "b",
                output_revision: "o",
                operations,
            });
        }).not.toThrow();
        expect(accessed).toBe(false);
        expect(parsed).toMatchObject({ ok: false, rejection: { code: "malformed" } });
    });

    it("stops container traversal before reading later children", () => {
        let laterRead = false;
        const values = new Proxy<unknown[]>([1n, null], {
            get(target, property, receiver) {
                if (property === "1") {
                    laterRead = true;
                    throw new Error("later child read");
                }
                return Reflect.get(target, property, receiver);
            },
            getOwnPropertyDescriptor(target, property) {
                if (property === "1") {
                    laterRead = true;
                    throw new Error("later child inspected");
                }
                return Reflect.getOwnPropertyDescriptor(target, property);
            },
        });
        expect(() =>
            parseRecipe({
                base_revision: "b",
                output_revision: "o",
                operations: [{ op: "insert", values }],
            }),
        ).not.toThrow();
        expect(laterRead).toBe(false);

        expect(() => canonicalJsonLength(values)).toThrow();
        expect(laterRead).toBe(false);
    });

    it("rejects recipes outside serde_json's value domain", () => {
        const loneSurrogate = String.fromCharCode(0xd800);
        expect(
            parseRecipe({
                base_revision: "b",
                output_revision: loneSurrogate.repeat(42),
                operations: [],
            }),
        ).toMatchObject({ ok: false, rejection: { code: "invalid_revision" } });
        expect(
            parseRecipe({
                base_revision: "b",
                output_revision: "o",
                operations: [{ op: "insert", values: [Number.POSITIVE_INFINITY] }],
            }),
        ).toMatchObject({ ok: false, rejection: { code: "malformed" } });
        expect(
            parseRecipe({
                base_revision: "b",
                output_revision: "o",
                operations: [{ op: "insert", values: [loneSurrogate] }],
            }),
        ).toMatchObject({ ok: false, rejection: { code: "malformed" } });
    });

    it("rejects nested literals beyond serde_json's recursion limit without throwing", () => {
        let nested: unknown = null;
        for (let depth = 0; depth < 123; depth += 1) nested = [nested];
        const accepted = parseRecipe({
            base_revision: "b",
            output_revision: "o",
            operations: [{ op: "insert", values: [nested] }],
        });
        expect(accepted.ok).toBe(true);
        if (accepted.ok) expect(applyRecipe(accepted.recipe, base("b", [])).ok).toBe(true);
        nested = [nested];
        expect(
            parseRecipe({
                base_revision: "b",
                output_revision: "o",
                operations: [{ op: "insert", values: [nested] }],
            }),
        ).toMatchObject({ ok: false, rejection: { code: "malformed" } });
    });

    it("does not use inherited array methods while parsing", () => {
        let methodRead = false;
        const operations = new Proxy([{ op: "insert", values: [null] }], {
            get(target, property, receiver) {
                if (property === "entries" || property === "some") {
                    methodRead = true;
                    throw new Error(`inherited ${String(property)} read`);
                }
                return Reflect.get(target, property, receiver);
            },
        });
        expect(() =>
            parseRecipe({ base_revision: "b", output_revision: "o", operations }),
        ).not.toThrow();
        expect(methodRead).toBe(false);
    });

    it("does not use inherited array methods to check operation fields", () => {
        const find = Object.getOwnPropertyDescriptor(Array.prototype, "find");
        const includes = Object.getOwnPropertyDescriptor(Array.prototype, "includes");
        let parsed: ReturnType<typeof parseRecipe> | undefined;
        try {
            Object.defineProperty(Array.prototype, "find", {
                configurable: true,
                value() {
                    throw new Error("inherited find called");
                },
            });
            Object.defineProperty(Array.prototype, "includes", {
                configurable: true,
                value() {
                    throw new Error("inherited includes called");
                },
            });
            parsed = parseRecipe({
                base_revision: "b",
                output_revision: "o",
                operations: [{ op: "insert", values: [null] }],
            });
        } finally {
            if (find) Object.defineProperty(Array.prototype, "find", find);
            if (includes) Object.defineProperty(Array.prototype, "includes", includes);
        }
        expect(parsed?.ok).toBe(true);
    });

    it("does not accept a rejection through an inherited operation discriminant", () => {
        const op = Object.getOwnPropertyDescriptor(Object.prototype, "op");
        let parsed: ReturnType<typeof parseRecipe> | undefined;
        try {
            Object.defineProperty(Object.prototype, "op", {
                configurable: true,
                value: "insert",
            });
            parsed = parseRecipe({
                base_revision: "b",
                output_revision: "o",
                operations: [{}],
            });
        } finally {
            if (op) Object.defineProperty(Object.prototype, "op", op);
            else Reflect.deleteProperty(Object.prototype, "op");
        }
        expect(parsed).toMatchObject({ ok: false, rejection: { code: "malformed" } });
    });

    it("uses own tags for internal traversal frames", () => {
        const children = Object.getOwnPropertyDescriptor(Object.prototype, "children");
        const code = Object.getOwnPropertyDescriptor(Object.prototype, "code");
        try {
            Object.defineProperty(Object.prototype, "children", {
                configurable: true,
                value: {
                    next: () => {
                        throw new Error("inherited children used");
                    },
                },
            });
            Object.defineProperty(Object.prototype, "code", {
                configurable: true,
                value: "malformed",
            });
            expect(
                parseRecipe({
                    base_revision: "b",
                    output_revision: "o",
                    operations: [{ op: "insert", values: [null] }],
                }).ok,
            ).toBe(true);
            expect(canonicalJsonLength([null])).toBe(6);
        } finally {
            if (children) Object.defineProperty(Object.prototype, "children", children);
            else Reflect.deleteProperty(Object.prototype, "children");
            if (code) Object.defineProperty(Object.prototype, "code", code);
            else Reflect.deleteProperty(Object.prototype, "code");
        }
    });

    it("rejects non-enumerable known fields skipped by validation", () => {
        const cyclic: unknown[] = [];
        cyclic[0] = cyclic;
        const operation = { op: "insert" };
        Object.defineProperty(operation, "values", {
            enumerable: false,
            value: [cyclic],
        });
        expect(
            parseRecipe({
                base_revision: "b",
                output_revision: "o",
                operations: [operation],
            }),
        ).toMatchObject({ ok: false, rejection: { code: "malformed" } });
    });

    it("does not retain metadata per rejected operation", () => {
        const operations: RecipeOperation[] = Array.from({ length: 100_000 }, () => ({
            op: "insert",
            values: [null],
        }));
        let heapAtFinalOperation = 0;
        operations.push(
            new Proxy({ op: "keep", source: "input", start: 1, count: 1 } as RecipeOperation, {
                get(target, property, receiver) {
                    if (property === "op") heapAtFinalOperation = process.memoryUsage().heapUsed;
                    return Reflect.get(target, property, receiver);
                },
            }),
        );
        Bun.gc(true);
        const heapBefore = process.memoryUsage().heapUsed;
        const result = applyRecipe(
            { baseRevision: "b", outputRevision: "o", operations },
            base("b", []),
        );
        expect(result).toMatchObject({ ok: false, rejection: { code: "out_of_bounds" } });
        expect(heapAtFinalOperation - heapBefore).toBeLessThan(8 * 1024 * 1024);
    });

    it("stops at the size cap before inspecting later literals", () => {
        const result = applyRecipe(
            {
                baseRevision: "b",
                outputRevision: "o",
                operations: [
                    { op: "keep", source: "input", start: 0, count: 1 },
                    { op: "insert", values: [null, 1n] },
                ],
            },
            { revision: "b", values: [null], lengths: [MAX_RECONSTRUCTED_BYTES - 2] },
        );
        expect(result).toMatchObject({ ok: false, rejection: { code: "output_too_large" } });
    });

    it("does not retain parsed operations before rejecting unused previous revision", () => {
        const operations = Array.from({ length: 1_000 }, () => ({
            op: "insert",
            values: [null],
        }));
        const original = Object.getOwnPropertyDescriptor(Array.prototype, "999");
        let copied = false;
        let result: ReturnType<typeof parseRecipe> | undefined;
        try {
            Object.defineProperty(Array.prototype, "999", {
                configurable: true,
                set(value: unknown) {
                    copied = true;
                    Object.defineProperty(this, "999", {
                        value,
                        writable: true,
                        enumerable: true,
                        configurable: true,
                    });
                },
            });
            result = parseRecipe({
                base_revision: "b",
                output_revision: "o",
                previous_output_revision: "unused",
                operations,
            });
        } finally {
            if (original) Object.defineProperty(Array.prototype, "999", original);
            else Reflect.deleteProperty(Array.prototype, "999");
        }
        expect(result).toMatchObject({
            ok: false,
            rejection: { code: "unused_previous_revision" },
        });
        expect(copied).toBe(false);
    });

    it("rejects oversized kept ranges before slicing sources", () => {
        const values = new Proxy([1, 2], {
            get(target, property, receiver) {
                if (property === "slice") throw new Error("values sliced before validation");
                return Reflect.get(target, property, receiver);
            },
        });
        const lengths = new Proxy([MAX_RECONSTRUCTED_BYTES, 1], {
            get(target, property, receiver) {
                if (property === "slice") throw new Error("lengths sliced before validation");
                return Reflect.get(target, property, receiver);
            },
        });
        let result: ReturnType<typeof applyRecipe> | undefined;
        expect(() => {
            result = applyRecipe(recipe, { revision: "b", values, lengths });
        }).not.toThrow();
        expect(result).toMatchObject({
            ok: false,
            rejection: { code: "output_too_large" },
        });
    });

    it("rejects a mismatched length table without reading values", () => {
        const result = applyRecipe(recipe, { revision: "b", values: [1, 2], lengths: [1] });
        expect(result.ok).toBe(false);
        if (!result.ok) expect(result.rejection.code).toBe("length_mismatch");
    });
});
