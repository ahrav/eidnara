import {
    isCommentJsonObjectRoot,
    parseJsoncTree,
    readJsoncBytes,
    sanitizeParsedJson,
} from "@eidnara/opencode/shared/jsonc-parser";
import { parse as parseCommentJson } from "comment-json";
import type { Node } from "jsonc-parser";

export type JsoncReadResult =
    | { kind: "missing" }
    | { kind: "parsed"; value: Record<string, unknown> }
    | { kind: "parse-error"; error: ConfigParseError };

export class ConfigParseError extends Error {
    readonly path: string;

    constructor(path: string, content: string, cause: unknown) {
        const detail = cause instanceof Error ? cause.message : String(cause);
        const location = parseErrorLocation(content, cause);
        super(
            `Refusing to overwrite unparseable config ${path} at line ${location.line}, column ${location.column}: ${detail}`,
            { cause },
        );
        this.name = "ConfigParseError";
        this.path = path;
    }
}

function parseErrorLocation(content: string, error: unknown): { line: number; column: number } {
    const lines = content.split("\n");
    const message = error instanceof Error ? error.message : String(error);
    if (message.includes("Unexpected end of JSON input")) {
        return { line: lines.length, column: (lines.at(-1)?.length ?? 0) + 1 };
    }

    const parserLocation = error as { line?: unknown; column?: unknown };
    if (
        typeof parserLocation.line === "number" &&
        parserLocation.line >= 1 &&
        parserLocation.line <= lines.length &&
        typeof parserLocation.column === "number" &&
        parserLocation.column >= 0
    ) {
        return { line: parserLocation.line, column: parserLocation.column + 1 };
    }

    const messageLine = /Line (\d+)/.exec(message)?.[1];
    return { line: messageLine ? Number.parseInt(messageLine, 10) : 1, column: 1 };
}

type JsoncDocumentResult =
    | { kind: "missing" }
    | {
          kind: "parsed";
          tree: Record<string, unknown>;
          plain: Record<string, unknown>;
          /** `true` when a number lost precision in parsing, so serializing `tree` would alter it. */
          lossyNumber: boolean;
      }
    | { kind: "parse-error"; error: ConfigParseError };

/**
 * Splits a JSON number literal into a significand with no trailing zeros and a
 * power of ten, so two literals denote the same rational value exactly when
 * both parts are equal. Zero is `0 × 10^0`. No exponentiation happens, so a
 * literal such as `1e-100000000` costs only its own digit count.
 */
function normalizedDecimal(literal: string): { digits: bigint; exponent: number } | null {
    const match = /^(-?)(\d+)(?:\.(\d+))?(?:[eE]([+-]?\d+))?$/.exec(literal);
    if (!match) return null;
    const [, sign, whole, fraction = "", exponent = "0"] = match;
    let digits = BigInt(`${sign}${whole}${fraction}`);
    if (digits === 0n) return { digits: 0n, exponent: 0 };
    let scale = Number(exponent) - fraction.length;
    while (digits % 10n === 0n) {
        digits /= 10n;
        scale += 1;
    }
    return { digits, exponent: scale };
}

/**
 * A literal is lossy when the parsed double does not denote the same rational
 * value: an integer past 2^53, a fraction with more digits than a double holds,
 * or an exponent that underflows to zero. Serializing the parsed value would
 * then write a different number over the user's literal.
 */
function isLossyNumberLiteral(literal: string, value: number): boolean {
    if (!Number.isFinite(value)) return true;
    const source = normalizedDecimal(literal);
    const parsed = normalizedDecimal(String(value)) ?? normalizedDecimal(value.toFixed(0));
    if (source === null || parsed === null) return true;
    return source.digits !== parsed.digits || source.exponent !== parsed.exponent;
}

/** `content` must be the text the tree was parsed from, so node offsets line up. */
function containsLossyNumber(content: string, node: Node): boolean {
    if (node.type === "number") {
        const literal = content.slice(node.offset, node.offset + node.length);
        return typeof node.value === "number" && isLossyNumberLiteral(literal, node.value);
    }
    return (node.children ?? []).some((child) => containsLossyNumber(content, child));
}

/**
 * `tree` preserves comment metadata during serialization; `plain` contains
 * the sanitized copy. A prototype-pollution key anywhere in the document, or
 * a non-object root, rejects the whole file.
 */
function readJsoncDocument(path: string): JsoncDocumentResult {
    // The read stays inside the failure boundary: a path that exists but
    // cannot be read (permissions, a FIFO or directory, malformed UTF-8,
    // deleted before the open) reports as parse-error instead of throwing, so
    // lenient diagnostic callers can explain the bad file rather than abort.
    // A fatal decoder matters here because a rewrite would otherwise serialize
    // U+FFFD over the user's original bytes.
    let content = "";
    try {
        content = readJsoncBytes(path);
    } catch (error) {
        if ((error as NodeJS.ErrnoException).code === "ENOENT") return { kind: "missing" };
        return { kind: "parse-error", error: new ConfigParseError(path, "", error) };
    }
    try {
        // The shared parser strips a leading BOM before it assigns node offsets, so the same
        // stripped text feeds both parsers and the offset-based literal slices.
        const text = content.charCodeAt(0) === 0xfeff ? content.slice(1) : content;
        // `comment-json` runs first because its syntax errors carry a line and column; the shared
        // parser then rejects what `comment-json` accepts but the daemon's reader does not, such
        // as an unpaired surrogate escape inside a string.
        const tree = parseCommentJson(text);
        const root = parseJsoncTree(text);
        const rejectedKeyPaths: string[] = [];
        const plain = sanitizeParsedJson(tree, {
            onRejectedKey: (keyPath) => rejectedKeyPaths.push(keyPath.join(".")),
        });
        if (rejectedKeyPaths.length > 0) {
            throw new Error(`unsafe prototype-pollution key at ${rejectedKeyPaths.join(", ")}`);
        }
        // A scalar root parses to a boxed primitive, which is an object but not a plain one.
        if (!isCommentJsonObjectRoot(tree)) {
            throw new Error("expected a JSON object at the document root");
        }
        return {
            kind: "parsed",
            tree,
            plain: plain as Record<string, unknown>,
            lossyNumber: containsLossyNumber(text, root),
        };
    } catch (error) {
        return { kind: "parse-error", error: new ConfigParseError(path, content, error) };
    }
}

/**
 * Callers may create a missing config but must not replace parse failures with empty objects.
 */
export function readJsoncConfig(path: string): JsoncReadResult {
    const result = readJsoncDocument(path);
    return result.kind === "parsed" ? { kind: "parsed", value: result.plain } : result;
}

/**
 * Returns the comment-json tree so a mutated config serializes with its
 * comments intact. A missing file yields an empty object; an unparseable or
 * unsafe one throws instead of being overwritten. A file holding an integer
 * that parsing rounded also throws, because serializing the tree would write
 * the rounded value back over the user's literal.
 */
export function readJsoncConfigForUpdate(path: string): Record<string, unknown> {
    const result = readJsoncDocument(path);
    if (result.kind === "missing") return {};
    if (result.kind === "parse-error") throw result.error;
    if (result.lossyNumber) {
        throw new ConfigParseError(
            path,
            "",
            new Error("a numeric literal the parser rounded would not survive a rewrite"),
        );
    }
    return result.tree;
}

export function assertJsoncConfigsParseable(paths: readonly string[]): void {
    for (const path of paths) {
        const result = readJsoncConfig(path);
        if (result.kind === "parse-error") throw result.error;
    }
}

/**
 * Lenient JSONC read for diagnostics surfaces: a missing file is an empty
 * config, and a parse failure — including a non-object document root — is
 * reported as a string instead of thrown.
 */
export function readJsoncLenient(path: string): {
    value: Record<string, unknown>;
    parseError?: string;
} {
    const result = readJsoncConfig(path);
    if (result.kind === "missing") return { value: {} };
    if (result.kind === "parse-error") {
        return { value: {}, parseError: result.error.message };
    }
    return { value: result.value };
}
