import { existsSync, readFileSync } from "node:fs";
import { sanitizeParsedJson } from "@eidnara/opencode/shared/jsonc-parser";
import { parse as parseCommentJson } from "comment-json";

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
    | { kind: "parsed"; tree: Record<string, unknown>; plain: Record<string, unknown> }
    | { kind: "parse-error"; error: ConfigParseError };

/**
 * `tree` preserves comment metadata during serialization; `plain` contains
 * the sanitized copy. A prototype-pollution key anywhere in the document, or
 * a non-object root, rejects the whole file.
 */
function readJsoncDocument(path: string): JsoncDocumentResult {
    if (!existsSync(path)) return { kind: "missing" };

    // The read stays inside the failure boundary: a path that exists but
    // cannot be read (permissions, a directory, deleted between the existsSync
    // probe and the read) reports as parse-error instead of throwing, so
    // lenient diagnostic callers can explain the bad file rather than abort.
    let content = "";
    try {
        content = readFileSync(path, "utf-8");
        const tree = parseCommentJson(content);
        const rejectedKeyPaths: string[] = [];
        const plain = sanitizeParsedJson(tree, {
            onRejectedKey: (keyPath) => rejectedKeyPaths.push(keyPath.join(".")),
        });
        if (rejectedKeyPaths.length > 0) {
            throw new Error(`unsafe prototype-pollution key at ${rejectedKeyPaths.join(", ")}`);
        }
        if (tree === null || typeof tree !== "object" || Array.isArray(tree)) {
            throw new Error("expected a JSON object at the document root");
        }
        return {
            kind: "parsed",
            tree: tree as Record<string, unknown>,
            plain: plain as Record<string, unknown>,
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
 * unsafe one throws instead of being overwritten.
 */
export function readJsoncConfigForUpdate(path: string): Record<string, unknown> {
    const result = readJsoncDocument(path);
    if (result.kind === "missing") return {};
    if (result.kind === "parse-error") throw result.error;
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
