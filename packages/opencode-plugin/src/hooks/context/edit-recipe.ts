import { MAX_FRAME_BODY_LEN } from "../../shared/host-client/protocol";
import { isRecord } from "../../shared/record-type-guard";
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
 * length measured at capture or serialization time. The caller must keep the values and their
 * nested objects unchanged while the base is usable; the applier never re-serializes kept values.
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
/** `bytes` is the canonical JSON size of the reconstructed array, brackets and commas included. */
export type RecipeApplication =
    | { ok: true; values: unknown[]; lengths: number[]; bytes: number }
    | { ok: false; rejection: RecipeRejection };

const utf8 = new TextEncoder();
const MAX_JSON_NESTING = 127;

interface JsonChild {
    key?: string;
    value: unknown;
}

type JsonContainer = unknown[] | Record<string, unknown>;

function* dataChildren(value: JsonContainer): Generator<JsonChild | RecipeRejection> {
    if (Array.isArray(value)) {
        for (let index = 0; index < value.length; index += 1) {
            const property = Object.getOwnPropertyDescriptor(value, index);
            if (!property || !("value" in property)) {
                yield {
                    code: "malformed",
                    detail: `recipe array index ${index} is not a data property`,
                };
                return;
            }
            yield { value: property.value };
        }
        return;
    }
    const record = value as Record<string, unknown>;
    for (const key in record) {
        if (!Object.hasOwn(record, key)) continue;
        if (!isWellFormed(key)) {
            yield { code: "malformed", detail: "recipe contains an unpaired surrogate key" };
            return;
        }
        const property = Object.getOwnPropertyDescriptor(record, key);
        if (!property || !("value" in property)) {
            yield { code: "malformed", detail: `recipe field ${key} is not a data property` };
            return;
        }
        yield { key, value: property.value };
    }
}

function isWellFormed(value: string): boolean {
    for (let index = 0; index < value.length; index += 1) {
        const unit = value.charCodeAt(index);
        if (unit >= 0xd800 && unit <= 0xdbff) {
            if (index + 1 >= value.length) return false;
            const next = value.charCodeAt(index + 1);
            if (next < 0xdc00 || next > 0xdfff) return false;
            index += 1;
        } else if (unit >= 0xdc00 && unit <= 0xdfff) return false;
    }
    return true;
}

function reject(
    code: RecipeRejectionCode,
    detail: string,
): { ok: false; rejection: RecipeRejection } {
    return { ok: false, rejection: { code, detail } };
}

function parseRevision(value: unknown, field: string): string | RecipeRejection {
    if (typeof value !== "string") return { code: "malformed", detail: `${field} is not a string` };
    if (value.length === 0) return { code: "invalid_revision", detail: `${field} is empty` };
    if (value.length > MAX_REVISION_BYTES)
        return { code: "invalid_revision", detail: `${field} exceeds ${MAX_REVISION_BYTES} bytes` };
    if (!isWellFormed(value))
        return { code: "invalid_revision", detail: `${field} contains an unpaired surrogate` };
    const bytes = utf8.encode(value).length;
    if (bytes > MAX_REVISION_BYTES)
        return { code: "invalid_revision", detail: `${field} is ${bytes} bytes` };
    return value;
}

function validateJsonValue(value: unknown): RecipeRejection | undefined {
    type ValueWork = { kind: "value"; value: unknown; depth: number };
    type ChildrenWork = {
        kind: "children";
        owner: JsonContainer;
        depth: number;
        children: Generator<JsonChild | RecipeRejection>;
    };
    const work: (ValueWork | ChildrenWork)[] = [{ kind: "value", value, depth: 0 }];
    const active = new WeakSet<object>();
    while (work.length > 0) {
        const item = work[work.length - 1] as ValueWork | ChildrenWork;
        work.length -= 1;
        if (item.kind === "children") {
            const next = item.children.next();
            if (next.done) {
                active.delete(item.owner);
                continue;
            }
            work[work.length] = item;
            if (isRecipeRejection(next.value)) return next.value;
            work[work.length] = {
                kind: "value",
                value: next.value.value,
                depth: item.depth + 1,
            };
            continue;
        }
        const current = item.value;
        if (current === null || typeof current === "boolean") continue;
        if (typeof current === "number") {
            if (!Number.isFinite(current))
                return { code: "malformed", detail: "recipe contains a non-finite number" };
            continue;
        }
        if (typeof current === "string") {
            if (!isWellFormed(current))
                return { code: "malformed", detail: "recipe contains an unpaired surrogate" };
            continue;
        }
        if (typeof current !== "object")
            return { code: "malformed", detail: `recipe contains a ${typeof current} value` };
        if (item.depth >= MAX_JSON_NESTING)
            return { code: "malformed", detail: "recipe exceeds the JSON nesting limit" };
        if (active.has(current))
            return { code: "malformed", detail: "recipe contains a cyclic value" };
        const container = current as JsonContainer;
        active.add(container);
        work[work.length] = {
            kind: "children",
            owner: container,
            depth: item.depth,
            children: dataChildren(container),
        };
    }
    return undefined;
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
    const keys = Object.keys(record);
    for (let keyIndex = 0; keyIndex < keys.length; keyIndex += 1) {
        const key = keys[keyIndex] as string;
        let matched = false;
        for (let knownIndex = 0; knownIndex < known.length; knownIndex += 1) {
            if (key === known[knownIndex]) {
                matched = true;
                break;
            }
        }
        if (!matched) return key;
    }
    return undefined;
}

function isRecipeRejection(
    value: JsonChild | RecipeOperation | RecipeRejection,
): value is RecipeRejection {
    return Object.hasOwn(value, "code");
}

interface DataProperty {
    value: unknown;
}

function ownDataProperty(record: Record<string, unknown>, key: string): DataProperty | undefined {
    const property = Object.getOwnPropertyDescriptor(record, key);
    return property && property.enumerable && "value" in property
        ? { value: property.value as unknown }
        : undefined;
}

function parseOperation(value: unknown, index: number): RecipeOperation | RecipeRejection {
    if (!isRecord(value))
        return { code: "malformed", detail: `operation ${index} is not an object` };
    const op = ownDataProperty(value, "op")?.value;
    if (typeof op !== "string")
        return { code: "malformed", detail: `operation ${index} op is not a string` };
    if (op === "keep") {
        const extra = unknownField(value, ["op", "source", "start", "count"]);
        if (extra !== undefined)
            return { code: "malformed", detail: `operation ${index} has unknown field ${extra}` };
        const source = ownDataProperty(value, "source")?.value;
        if (source !== "input" && source !== "previous") {
            return typeof source === "string"
                ? { code: "unknown_source", detail: `operation ${index} source ${source}` }
                : { code: "malformed", detail: `operation ${index} source is not a string` };
        }
        const start = safeInteger(
            ownDataProperty(value, "start")?.value,
            `operation ${index} start`,
        );
        if (typeof start !== "number") return start;
        const count = safeInteger(
            ownDataProperty(value, "count")?.value,
            `operation ${index} count`,
        );
        if (typeof count !== "number") return count;
        if (count === 0) return { code: "zero_count", detail: `operation ${index} count is zero` };
        return { op: "keep", source, start, count };
    }
    if (op === "insert") {
        const extra = unknownField(value, ["op", "values"]);
        if (extra !== undefined)
            return { code: "malformed", detail: `operation ${index} has unknown field ${extra}` };
        const values = ownDataProperty(value, "values")?.value;
        if (!Array.isArray(values))
            return { code: "malformed", detail: `operation ${index} values is not an array` };
        if (values.length === 0)
            return { code: "empty_insert", detail: `operation ${index} inserts nothing` };
        return { op: "insert", values };
    }
    return { code: "unknown_operation", detail: `operation ${index} op ${op}` };
}

/** Structural validation of a decoded response body. Base-relative checks run in `applyRecipe`. */
export function parseRecipe(value: unknown): RecipeParse {
    if (!isRecord(value)) return reject("malformed", "recipe is not an object");
    const baseRevision = parseRevision(
        ownDataProperty(value, "base_revision")?.value,
        "base_revision",
    );
    if (typeof baseRevision !== "string") return { ok: false, rejection: baseRevision };
    const outputRevision = parseRevision(
        ownDataProperty(value, "output_revision")?.value,
        "output_revision",
    );
    if (typeof outputRevision !== "string") return { ok: false, rejection: outputRevision };
    let previousOutputRevision: string | undefined;
    const previousRevisionValue = ownDataProperty(value, "previous_output_revision")?.value;
    if (previousRevisionValue !== undefined) {
        const parsed = parseRevision(previousRevisionValue, "previous_output_revision");
        if (typeof parsed !== "string") return { ok: false, rejection: parsed };
        previousOutputRevision = parsed;
    }
    const invalidJson = validateJsonValue(value);
    if (invalidJson !== undefined) return { ok: false, rejection: invalidJson };
    const operationsValue = ownDataProperty(value, "operations")?.value;
    if (!Array.isArray(operationsValue)) return reject("malformed", "operations is not an array");
    let usesPrevious = false;
    for (let index = 0; index < operationsValue.length; index += 1) {
        const entry = Object.getOwnPropertyDescriptor(operationsValue, index);
        if (!entry || !("value" in entry))
            return reject("malformed", `operation ${index} is not a data property`);
        const operation = parseOperation(entry.value, index);
        if (isRecipeRejection(operation)) return { ok: false, rejection: operation };
        if (operation.op === "keep" && operation.source === "previous") usesPrevious = true;
    }
    if (usesPrevious && previousOutputRevision === undefined)
        return reject("missing_previous_base", "a previous keep has no previous_output_revision");
    if (!usesPrevious && previousOutputRevision !== undefined)
        return reject(
            "unused_previous_revision",
            "previous_output_revision without a previous keep",
        );
    const operations: RecipeOperation[] = [];
    for (let index = 0; index < operationsValue.length; index += 1) {
        const entry = Object.getOwnPropertyDescriptor(operationsValue, index);
        if (!entry || !("value" in entry))
            return reject("malformed", `operation ${index} is not a data property`);
        const operation = parseOperation(entry.value, index);
        if (isRecipeRejection(operation)) return { ok: false, rejection: operation };
        operations[operations.length] = operation;
    }
    return {
        ok: true,
        recipe:
            previousOutputRevision === undefined
                ? { baseRevision, outputRevision, operations }
                : { baseRevision, outputRevision, previousOutputRevision, operations },
    };
}

/**
 * Compact `serde_json` length of a literal. Object order does not affect the byte count, so this
 * walker avoids canonical key sorting and does not recurse on the JavaScript stack.
 */
export function canonicalJsonLength(value: unknown): number {
    type ValueWork = { kind: "value"; value: unknown };
    type ChildrenWork = {
        kind: "children";
        array: boolean;
        entries: number;
        children: Generator<JsonChild | RecipeRejection>;
    };
    let bytes = 0;
    const work: (ValueWork | ChildrenWork)[] = [{ kind: "value", value }];
    while (work.length > 0) {
        const item = work[work.length - 1] as ValueWork | ChildrenWork;
        work.length -= 1;
        if (item.kind === "children") {
            const next = item.children.next();
            if (next.done) continue;
            if (isRecipeRejection(next.value)) throw new TypeError(next.value.detail);
            work[work.length] = item;
            if (item.array) {
                work[work.length] = {
                    kind: "value",
                    value: next.value.value === undefined ? null : next.value.value,
                };
                continue;
            }
            if (next.value.value === undefined) continue;
            bytes +=
                (item.entries > 0 ? 1 : 0) + Buffer.byteLength(JSON.stringify(next.value.key)) + 1;
            item.entries += 1;
            work[work.length] = { kind: "value", value: next.value.value };
            continue;
        }
        const current = item.value;
        if (Array.isArray(current)) {
            bytes += 2 + Math.max(0, current.length - 1);
            work[work.length] = {
                kind: "children",
                array: true,
                entries: 0,
                children: dataChildren(current),
            };
            continue;
        }
        if (current !== null && typeof current === "object") {
            bytes += 2;
            work[work.length] = {
                kind: "children",
                array: false,
                entries: 0,
                children: dataChildren(current as JsonContainer),
            };
            continue;
        }
        bytes += Buffer.byteLength(serdeJsonCompact(current));
    }
    return bytes;
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
    let entries = 0;
    // Brackets first; each entry then pays its bytes plus one comma after the first.
    let bytes = 2;
    const insertedLengths: number[] = [];
    const addEntry = (length: number): RecipeRejectionCode | undefined => {
        if (!Number.isSafeInteger(length) || length < 0) return "overflow";
        bytes += length + (entries > 0 ? 1 : 0);
        entries += 1;
        if (!Number.isSafeInteger(bytes)) return "overflow";
        return bytes > MAX_RECONSTRUCTED_BYTES ? "output_too_large" : undefined;
    };
    for (let index = 0; index < recipe.operations.length; index += 1) {
        const operation = recipe.operations[index] as RecipeOperation;
        if (operation.op === "insert") {
            for (let valueIndex = 0; valueIndex < operation.values.length; valueIndex += 1) {
                const length = canonicalJsonLength(operation.values[valueIndex]);
                const failure = addEntry(length);
                if (failure) return reject(failure, `operation ${index} exceeded the size bound`);
                insertedLengths[insertedLengths.length] = length;
            }
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
            const failure = addEntry(base.lengths[position] as number);
            if (failure) return reject(failure, `operation ${index} exceeded the size bound`);
        }
    }
    if (bytes > MAX_RECONSTRUCTED_BYTES)
        return reject("output_too_large", `reconstructed array is ${bytes} bytes`);
    const values = new Array<unknown>(entries);
    const lengths = new Array<number>(entries);
    let outputIndex = 0;
    let insertedIndex = 0;
    for (let index = 0; index < recipe.operations.length; index += 1) {
        const operation = recipe.operations[index] as RecipeOperation;
        if (operation.op === "insert") {
            for (let valueIndex = 0; valueIndex < operation.values.length; valueIndex += 1) {
                values[outputIndex] = operation.values[valueIndex];
                lengths[outputIndex] = insertedLengths[insertedIndex] as number;
                outputIndex += 1;
                insertedIndex += 1;
            }
            continue;
        }
        const base = operation.source === "input" ? input : previous;
        if (!base)
            return reject(
                "missing_previous_base",
                `operation ${index} keeps from an absent previous output`,
            );
        const end = operation.start + operation.count;
        for (let position = operation.start; position < end; position += 1) {
            values[outputIndex] = base.values[position];
            lengths[outputIndex] = base.lengths[position] as number;
            outputIndex += 1;
        }
    }
    return { ok: true, values, lengths, bytes };
}
