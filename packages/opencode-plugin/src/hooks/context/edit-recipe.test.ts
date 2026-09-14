import { describe, expect, it } from "bun:test";
import fixture from "../../../../../crates/daemon/tests/fixtures/transform-edit-recipe-v1.json";
import {
    applyRecipe,
    canonicalJsonLength,
    type EditRecipe,
    MAX_RECONSTRUCTED_BYTES,
    MAX_REVISION_BYTES,
    parseRecipe,
    type RecipeRejectionCode,
    type RecipeSourceBase,
} from "./edit-recipe";

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
        expect(JSON.stringify(inputValues)).toBe(inputJson);
        expect(JSON.stringify(previousValues)).toBe(previousJson);
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

    it("rejects a mismatched length table without reading values", () => {
        const result = applyRecipe(recipe, { revision: "b", values: [1, 2], lengths: [1] });
        expect(result.ok).toBe(false);
        if (!result.ok) expect(result.rejection.code).toBe("length_mismatch");
    });
});
