/** `JSON.parse` rounds unequal integers beyond 2^53 to the same `number`; the reviver keeps the lexeme's value instead. */

const INTEGER_LEXEME = /^-?(?:0|[1-9][0-9]*)$/;
/** No 64-bit integer has more digits than `u64::MAX`; a longer lexeme is refused whole rather than converted. */
const MAX_INTEGER_DIGITS = 20;

/** An integer as decoded from the wire: a `number` when it is a safe integer, otherwise a `bigint`. */
export type WireInteger = number | bigint;

/** Largest count `exactCount` accepts: 2^53, inclusive. */
const MAX_EXACT_COUNT = 9007199254740992n;
export const U64_MAX = 18446744073709551615n;
export const I64_MIN = -9223372036854775808n;
const I64_MAX = 9223372036854775807n;

/** `parseExactJson` preserves integer lexemes exactly and throws `SyntaxError` for invalid JSON. */
export function parseExactJson(text: string): unknown {
    return JSON.parse(text, exactIntegerReviver);
}

function exactIntegerReviver(
    this: unknown,
    _key: string,
    value: unknown,
    context?: { source?: string },
): unknown {
    if (typeof value !== "number" || Number.isSafeInteger(value)) return value;
    const source = context?.source;
    if (source === undefined || !INTEGER_LEXEME.test(source)) return value;
    if (source.length - (source.startsWith("-") ? 1 : 0) > MAX_INTEGER_DIGITS) {
        throw new SyntaxError("integer lexeme wider than 64 bits");
    }
    return BigInt(source);
}

function isWireInteger(value: unknown): value is WireInteger {
    return typeof value === "bigint" || (typeof value === "number" && Number.isInteger(value));
}

function exactIntegerWithin(value: unknown, min: bigint, max: bigint): WireInteger | null {
    if (!isWireInteger(value)) return null;
    const exact = typeof value === "bigint" ? value : BigInt(value);
    return exact < min || exact > max ? null : value;
}

/** Accepts nonnegative counts through 2^53 inclusive. */
export function exactCount(value: unknown): number | null {
    const exact = exactIntegerWithin(value, 0n, MAX_EXACT_COUNT);
    return exact === null ? null : Number(exact);
}

export function exactU64(value: unknown): WireInteger | null {
    return exactIntegerWithin(value, 0n, U64_MAX);
}

export function exactI64(value: unknown): WireInteger | null {
    return exactIntegerWithin(value, I64_MIN, I64_MAX);
}

/** The exact decimal digits of `value`, for human output. */
export function formatExactInteger(value: WireInteger): string {
    return typeof value === "bigint" ? value.toString() : String(value);
}

// `JSON.rawJSON` is present on the Node and Bun floors this package declares and absent from TypeScript's lib declarations.
const json = JSON as typeof JSON & { rawJSON(text: string): unknown };

/** A raw JSON number token for `JSON.stringify`, so JSON output carries the exact digits rather than a string or a rounded double. */
export function rawJsonInteger(value: WireInteger): unknown {
    return json.rawJSON(formatExactInteger(value));
}
