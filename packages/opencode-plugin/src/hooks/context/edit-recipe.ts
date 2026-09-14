import { MAX_FRAME_BODY_LEN } from "../../shared/host-client/protocol";
import { serdeJsonCompact } from "./module-wire";

/** Revision tokens are opaque; this bound keeps a hostile daemon from turning them into a payload. */
export const MAX_REVISION_BYTES = 128;
/** The reconstructed array is bounded on its own, apart from the wire frame that carried the recipe. */
export const MAX_RECONSTRUCTED_BYTES = MAX_FRAME_BODY_LEN;

export type RecipeSource = "input" | "previous";

export type RecipeOperation =
    | { op: "keep"; source: RecipeSource; start: number; count: number }
    | { op: "insert"; values: readonly unknown[] };

/** The application contract of one `status: ok` response. */
export interface EditRecipe {
    baseRevision: string;
    outputRevision: string;
    /** Present exactly when an operation keeps from `previous`. */
    previousOutputRevision?: string;
    operations: readonly RecipeOperation[];
}

/**
 * One captured source: its revision, its whole-message values, and each value's canonical JSON
 * length measured at capture or serialization time. The applier never re-serializes a kept value.
 */
export interface RecipeSourceBase {
    revision: string;
    values: readonly unknown[];
    lengths: readonly number[];
}

export type RecipeRejectionCode =
    | "invalid_revision"
    | "wrong_base_revision"
    | "wrong_previous_revision"
    | "missing_previous_base"
    | "unused_previous_revision"
    | "unknown_operation"
    | "unknown_source"
    | "malformed"
    | "unsafe_integer"
    | "zero_count"
    | "empty_insert"
    | "backward_range"
    | "out_of_bounds"
    | "overflow"
    | "output_too_large"
    | "length_mismatch";

export interface RecipeRejection {
    code: RecipeRejectionCode;
    detail: string;
}

export type RecipeParse =
    | { ok: true; recipe: EditRecipe }
    | { ok: false; rejection: RecipeRejection };
/**
 * `bytes` is the canonical JSON size of the reconstructed array, brackets and commas included;
 * `lengths` holds each value's canonical length so the result can serve as a later previous base.
 */
export type RecipeApplication =
    | { ok: true; values: unknown[]; lengths: number[]; bytes: number }
    | { ok: false; rejection: RecipeRejection };

const utf8 = new TextEncoder();

function reject(
    code: RecipeRejectionCode,
    detail: string,
): { ok: false; rejection: RecipeRejection } {
    return { ok: false, rejection: { code, detail } };
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object" && !Array.isArray(value);
}

function parseRevision(value: unknown, field: string): string | RecipeRejection {
    if (typeof value !== "string") return { code: "malformed", detail: `${field} is not a string` };
    if (value.length === 0) return { code: "invalid_revision", detail: `${field} is empty` };
    const bytes = utf8.encode(value).length;
    if (bytes > MAX_REVISION_BYTES)
        return { code: "invalid_revision", detail: `${field} is ${bytes} bytes` };
    return value;
}

/** Indexes and counts both languages read exactly. */
function safeInteger(value: unknown, field: string): number | RecipeRejection {
    if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0)
        return { code: "unsafe_integer", detail: `${field} is not a safe nonnegative integer` };
    return value;
}

function unknownField(
    record: Record<string, unknown>,
    known: readonly string[],
): string | undefined {
    return Object.keys(record).find((key) => !known.includes(key));
}

function parseOperation(value: unknown, index: number): RecipeOperation | RecipeRejection {
    if (!isRecord(value))
        return { code: "malformed", detail: `operation ${index} is not an object` };
    const op = value.op;
    if (typeof op !== "string")
        return { code: "malformed", detail: `operation ${index} op is not a string` };
    if (op === "keep") {
        const extra = unknownField(value, ["op", "source", "start", "count"]);
        if (extra !== undefined)
            return { code: "malformed", detail: `operation ${index} has unknown field ${extra}` };
        const source = value.source;
        if (source !== "input" && source !== "previous") {
            return typeof source === "string"
                ? { code: "unknown_source", detail: `operation ${index} source ${source}` }
                : { code: "malformed", detail: `operation ${index} source is not a string` };
        }
        const start = safeInteger(value.start, `operation ${index} start`);
        if (typeof start !== "number") return start;
        const count = safeInteger(value.count, `operation ${index} count`);
        if (typeof count !== "number") return count;
        if (count === 0) return { code: "zero_count", detail: `operation ${index} count is zero` };
        return { op: "keep", source, start, count };
    }
    if (op === "insert") {
        const extra = unknownField(value, ["op", "values"]);
        if (extra !== undefined)
            return { code: "malformed", detail: `operation ${index} has unknown field ${extra}` };
        if (!Array.isArray(value.values))
            return { code: "malformed", detail: `operation ${index} values is not an array` };
        if (value.values.length === 0)
            return { code: "empty_insert", detail: `operation ${index} inserts nothing` };
        return { op: "insert", values: value.values };
    }
    return { code: "unknown_operation", detail: `operation ${index} op ${op}` };
}

/** Structural validation of a decoded response body. Base-relative checks run in `applyRecipe`. */
export function parseRecipe(value: unknown): RecipeParse {
    if (!isRecord(value)) return reject("malformed", "recipe is not an object");
    const baseRevision = parseRevision(value.base_revision, "base_revision");
    if (typeof baseRevision !== "string") return { ok: false, rejection: baseRevision };
    const outputRevision = parseRevision(value.output_revision, "output_revision");
    if (typeof outputRevision !== "string") return { ok: false, rejection: outputRevision };
    let previousOutputRevision: string | undefined;
    if (value.previous_output_revision !== undefined) {
        const parsed = parseRevision(value.previous_output_revision, "previous_output_revision");
        if (typeof parsed !== "string") return { ok: false, rejection: parsed };
        previousOutputRevision = parsed;
    }
    if (!Array.isArray(value.operations)) return reject("malformed", "operations is not an array");
    const operations: RecipeOperation[] = [];
    for (const [index, entry] of value.operations.entries()) {
        const operation = parseOperation(entry, index);
        if (!("op" in operation)) return { ok: false, rejection: operation };
        operations.push(operation);
    }
    const usesPrevious = operations.some(
        (operation) => operation.op === "keep" && operation.source === "previous",
    );
    if (usesPrevious && previousOutputRevision === undefined)
        return reject("missing_previous_base", "a previous keep has no previous_output_revision");
    if (!usesPrevious && previousOutputRevision !== undefined)
        return reject(
            "unused_previous_revision",
            "previous_output_revision without a previous keep",
        );
    return {
        ok: true,
        recipe:
            previousOutputRevision === undefined
                ? { baseRevision, outputRevision, operations }
                : { baseRevision, outputRevision, previousOutputRevision, operations },
    };
}

/**
 * Compact `serde_json` length of a literal, the same rule the daemon's `canonical_len` follows.
 * A value the daemon parsed from `1.0` re-emits as `1.0` there and as `1` here, so sizes for
 * non-integer numeric content can differ by a few bytes while acceptance still agrees.
 */
export function canonicalJsonLength(value: unknown): number {
    return Buffer.byteLength(serdeJsonCompact(value));
}

/**
 * Validates every operation against the bases, sizes the result with checked arithmetic, and only
 * then builds a new array of shared references. A failure leaves the caller with nothing to roll
 * back; the sources are never edited. Field-level invariants come from `parseRecipe`.
 */
export function applyRecipe(
    recipe: EditRecipe,
    input: RecipeSourceBase,
    previous?: RecipeSourceBase,
): RecipeApplication {
    if (input.values.length !== input.lengths.length)
        return reject("length_mismatch", "input lengths do not match its values");
    if (previous && previous.values.length !== previous.lengths.length)
        return reject("length_mismatch", "previous lengths do not match its values");
    if (recipe.baseRevision !== input.revision)
        return reject("wrong_base_revision", "base_revision does not name the input");
    const cursors = { input: 0, previous: 0 };
    const lengths: number[] = [];
    // Brackets first; each entry then pays its bytes plus one comma after the first.
    let bytes = 2;
    const segments: (readonly unknown[])[] = [];
    const addEntry = (length: number): boolean => {
        if (!Number.isSafeInteger(length) || length < 0) return false;
        bytes += length + (lengths.length > 0 ? 1 : 0);
        lengths.push(length);
        return Number.isSafeInteger(bytes);
    };
    for (const [index, operation] of recipe.operations.entries()) {
        if (operation.op === "insert") {
            for (const value of operation.values) {
                if (!addEntry(canonicalJsonLength(value)))
                    return reject("overflow", `operation ${index} overflowed the size sum`);
            }
            segments.push(operation.values);
            continue;
        }
        let base: RecipeSourceBase;
        if (operation.source === "input") base = input;
        else if (!previous || recipe.previousOutputRevision === undefined)
            return reject(
                "missing_previous_base",
                `operation ${index} keeps from an absent previous output`,
            );
        else if (previous.revision !== recipe.previousOutputRevision)
            return reject(
                "wrong_previous_revision",
                "previous_output_revision does not name the previous output",
            );
        else base = previous;
        const end = operation.start + operation.count;
        if (!Number.isSafeInteger(end))
            return reject("overflow", `operation ${index} range end overflowed`);
        if (operation.start < cursors[operation.source])
            return reject(
                "backward_range",
                `operation ${index} keeps a ${operation.source} range that repeats or moves backward`,
            );
        if (end > base.values.length)
            return reject(
                "out_of_bounds",
                `operation ${index} keeps past the end of ${operation.source}`,
            );
        cursors[operation.source] = end;
        for (let position = operation.start; position < end; position += 1) {
            if (!addEntry(base.lengths[position] as number))
                return reject("overflow", `operation ${index} overflowed the size sum`);
        }
        segments.push(base.values.slice(operation.start, end));
    }
    if (bytes > MAX_RECONSTRUCTED_BYTES)
        return reject("output_too_large", `reconstructed array is ${bytes} bytes`);
    const values: unknown[] = [];
    for (const segment of segments) for (const value of segment) values.push(value);
    return { ok: true, values, lengths, bytes };
}
