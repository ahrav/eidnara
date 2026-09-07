import {
    applyEdits,
    createScanner,
    findNodeAtLocation,
    getNodeValue,
    type JSONPath,
    modify,
    type Node,
} from "jsonc-parser";

import { isPrototypePollutionKey, parseConfigJsonc, parseJsoncTree } from "./jsonc-parser";

interface Token {
    kind: number;
    offset: number;
    length: number;
}

/**
 * Isolated modules cannot reference jsonc-parser's const-enum SyntaxKind.
 */
const TOKEN_OPEN_BRACKET = 3;
const TOKEN_COMMA = 5;
const TOKEN_LINE_COMMENT = 12;
const TOKEN_BLOCK_COMMENT = 13;
const TOKEN_LINE_BREAK = 14;
const TOKEN_WHITESPACE = 15;
const TOKEN_EOF = 17;

function isComment(kind: number): boolean {
    return kind === TOKEN_LINE_COMMENT || kind === TOKEN_BLOCK_COMMENT;
}

function assertNoDuplicateKeys(node: Node): void {
    if (node.type === "object") {
        const seen = new Set<string>();
        for (const property of node.children ?? []) {
            const key = property.children?.[0]?.value;
            if (typeof key !== "string") continue;
            if (seen.has(key)) {
                throw new Error(`Cannot edit JSONC with duplicate key ${JSON.stringify(key)}`);
            }
            seen.add(key);
        }
    }
    for (const child of node.children ?? []) {
        assertNoDuplicateKeys(child);
    }
}

function parseDocument(text: string): Node {
    let root: Node;
    try {
        root = parseJsoncTree(text);
    } catch {
        throw new Error("Cannot edit invalid JSONC");
    }
    assertNoDuplicateKeys(root);
    return root;
}

/** A path segment the reader's sanitizer drops could be written but never read back. */
function findNode(text: string, path: JSONPath): Node | undefined {
    for (const segment of path) {
        if (typeof segment === "string" && isPrototypePollutionKey(segment)) {
            throw new TypeError(`Cannot edit JSONC at key ${JSON.stringify(segment)}`);
        }
    }
    return findNodeAtLocation(parseDocument(text), path);
}

/** JSON node offsets exclude a leading byte-order mark. */
function splitByteOrderMark(text: string): [bom: string, body: string] {
    return text.charCodeAt(0) === 0xfeff ? ["\uFEFF", text.slice(1)] : ["", text];
}

/** Reparse serialized JSONC to reject unpaired surrogates and sanitized keys before writing. */
function serializeJson(value: unknown): string {
    const serialized = JSON.stringify(value);
    if (typeof serialized !== "string") {
        throw new TypeError(`Cannot write a value of type ${typeof value} into JSONC`);
    }
    let rejectedKey: string | undefined;
    try {
        parseConfigJsonc(serialized, {
            onRejectedKey: (path) => {
                rejectedKey = path.join(".");
            },
        });
    } catch {
        throw new TypeError("Cannot write a string with an unpaired surrogate into JSONC");
    }
    if (rejectedKey !== undefined) {
        throw new TypeError(
            `Cannot write a value with key ${JSON.stringify(rejectedKey)} into JSONC`,
        );
    }
    return serialized;
}

/** Tokens whose start offset lies in `[start, end)`. `start` must be a token boundary. */
function scanTokens(text: string, start: number, end: number): Token[] {
    const scanner = createScanner(text, false);
    scanner.setPosition(start);
    const tokens: Token[] = [];

    for (;;) {
        const kind = scanner.scan();
        const offset = scanner.getTokenOffset();
        if (kind === TOKEN_EOF || offset >= end) return tokens;
        tokens.push({ kind, offset, length: scanner.getTokenLength() });
    }
}

function commaBetween(text: string, start: number, end: number): Token | undefined {
    return scanTokens(text, start, end).find((token) => token.kind === TOKEN_COMMA);
}

function tokenEnd(span: { offset: number; length: number }): number {
    return span.offset + span.length;
}

/**
 * Same-line trivia after the preceding `[` or comma belongs to the left side,
 * so the entry starts after it. When a line break follows the separator, the
 * entry owns the following lines; `lineBreak` reports that case.
 */
function ownedStart(
    text: string,
    separator: Token,
    entryOffset: number,
): { start: number; lineBreak: boolean } {
    let start = tokenEnd(separator);
    for (const token of scanTokens(text, tokenEnd(separator), entryOffset)) {
        if (token.kind === TOKEN_LINE_BREAK) return { start: tokenEnd(token), lineBreak: true };
        if (isComment(token.kind)) {
            start = tokenEnd(token);
        } else if (token.kind !== TOKEN_WHITESPACE) {
            break;
        }
    }
    return { start, lineBreak: false };
}

/**
 * Same-line comments after `from` belong to the entry. `wholeLines` extends
 * ownership through the line break; `lineBreak` reports whether one followed.
 */
function ownedEnd(
    text: string,
    from: number,
    limit: number,
    wholeLines: boolean,
): { end: number; lineBreak: boolean } {
    let end = from;
    for (const token of scanTokens(text, from, limit)) {
        if (token.kind === TOKEN_LINE_BREAK) {
            return { end: wholeLines ? tokenEnd(token) : end, lineBreak: true };
        }
        if (isComment(token.kind)) {
            end = tokenEnd(token);
        } else if (token.kind !== TOKEN_WHITESPACE) {
            break;
        }
    }
    return { end, lineBreak: false };
}

function removeArrayEntry(text: string, array: Node, index: number): string {
    const entries = array.children ?? [];
    const entry = entries[index];
    const previous = entries[index - 1];
    const next = entries[index + 1];
    const closingBracket = array.offset + array.length - 1;

    const leftSeparator: Token = previous
        ? (commaBetween(text, tokenEnd(previous), entry.offset) ?? missingComma())
        : { kind: TOKEN_OPEN_BRACKET, offset: array.offset, length: 1 };
    const rightSeparator = commaBetween(text, tokenEnd(entry), next?.offset ?? closingBracket);
    if (next && !rightSeparator) missingComma();

    const { start, lineBreak: wholeLines } = ownedStart(text, leftSeparator, entry.offset);
    const scanFrom = rightSeparator ? tokenEnd(rightSeparator) : tokenEnd(entry);
    const { end, lineBreak } = ownedEnd(text, scanFrom, closingBracket, wholeLines);

    // A next entry on the same line moves into the removed entry's place: it
    // keeps the entry's indentation, while the entry's own lines above it go.
    if (next && !lineBreak && (wholeLines || start === entry.offset)) {
        const indentation = wholeLines ? indentationAt(text, entry.offset) : "";
        return text.slice(0, start) + indentation + text.slice(next.offset);
    }

    const result = text.slice(0, start) + text.slice(end);

    const lastEntryWithoutTrailingComma = !next && previous && !rightSeparator;
    if (lastEntryWithoutTrailingComma) {
        return result.slice(0, leftSeparator.offset) + result.slice(tokenEnd(leftSeparator));
    }
    return result;
}

function missingComma(): never {
    throw new Error("Cannot edit invalid JSONC");
}

function lineStart(text: string, offset: number): number {
    const previousLineBreak = Math.max(
        text.lastIndexOf("\n", offset - 1),
        text.lastIndexOf("\r", offset - 1),
    );
    return previousLineBreak + 1;
}

/** Leading tabs and spaces before `offset` on its line. */
function indentationAt(text: string, offset: number): string {
    return /^[\t ]*/.exec(text.slice(lineStart(text, offset), offset))?.[0] ?? "";
}

function inferIndent(text: string, array: Node): string {
    const lastEntry = array.children?.at(-1);
    if (lastEntry) return indentationAt(text, lastEntry.offset);

    const closingBracket = array.offset + array.length - 1;
    const closingIndent = indentationAt(text, closingBracket);
    return `${closingIndent}${closingIndent.includes("\t") ? "\t" : "  "}`;
}

function splice(text: string, offset: number, insertion: string): string {
    return text.slice(0, offset) + insertion + text.slice(offset);
}

function appendArrayValue(text: string, array: Node, value: unknown): string {
    const entries = array.children ?? [];
    const closingBracket = array.offset + array.length - 1;
    const serialized = serializeJson(value);
    // A line break inside a block comment is part of the comment token, so it
    // neither makes the array multi-line nor offers a place to insert a line.
    const lastLineBreak = scanTokens(text, array.offset + 1, closingBracket)
        .filter((token) => token.kind === TOKEN_LINE_BREAK)
        .at(-1);
    const eol = lastLineBreak ? text.slice(lastLineBreak.offset, tokenEnd(lastLineBreak)) : "";
    const lastEntry = entries.at(-1);

    if (!lastEntry) {
        if (!lastLineBreak) return splice(text, closingBracket, serialized);
        return splice(
            text,
            tokenEnd(lastLineBreak),
            `${inferIndent(text, array)}${serialized}${eol}`,
        );
    }

    // Insertion goes after the last entry's trailing comma and same-line
    // comments so those stay attached to the last entry.
    const lastEnd = tokenEnd(lastEntry);
    const trailingComma = commaBetween(text, lastEnd, closingBracket);
    const lineContentEnd = ownedEnd(
        text,
        trailingComma ? tokenEnd(trailingComma) : lastEnd,
        closingBracket,
        false,
    ).end;

    if (!lastLineBreak) {
        return splice(text, lineContentEnd, trailingComma ? `${serialized},` : `,${serialized}`);
    }

    const inserted = `${eol}${inferIndent(text, array)}${serialized}${trailingComma ? "," : ""}`;

    // Splice the later offset first so the earlier one stays valid.
    const withValue = splice(text, lineContentEnd, inserted);
    return trailingComma ? withValue : splice(withValue, lastEnd, ",");
}

/**
 * The editor replaces values without reserializing the rest of the document.
 * When the target path is absent, the editor uses jsonc-parser's structural edit.
 * The structural edit preserves bytes outside its edit ranges.
 */
export function setJsoncValue(text: string, path: JSONPath, value: unknown): string {
    const serialized = serializeJson(value);
    const [bom, body] = splitByteOrderMark(text);
    const node = findNode(body, path);
    if (node) {
        if (Object.is(getNodeValue(node), value)) return text;
        return (
            bom + body.slice(0, node.offset) + serialized + body.slice(node.offset + node.length)
        );
    }

    return bom + applyEdits(body, modify(body, path, value, {}));
}

/**
 * The remover preserves the exact bytes of surviving comments and surrounding JSONC regions.
 */
export function removeJsoncArrayEntries(
    text: string,
    path: JSONPath,
    shouldRemove: (entry: unknown) => boolean,
): { text: string; removed: boolean } {
    const [bom, body] = splitByteOrderMark(text);
    let nextText = body;
    let removed = false;

    for (;;) {
        const array = findNode(nextText, path);
        if (array?.type !== "array") return { text: bom + nextText, removed };
        const index = (array.children ?? []).findIndex((entry) =>
            shouldRemove(getNodeValue(entry)),
        );
        if (index === -1) return { text: bom + nextText, removed };

        nextText = removeArrayEntry(nextText, array, index);
        removed = true;
    }
}

/** The appender adds values without reserializing sibling fields. */
export function appendJsoncArrayValues(text: string, path: JSONPath, values: unknown[]): string {
    // `JSON.stringify` of the whole array would turn a non-JSON element into
    // `null`; check each element the way the per-entry splice does.
    for (const value of values) serializeJson(value);
    const [bom, body] = splitByteOrderMark(text);
    let nextText = body;

    for (const value of values) {
        const array = findNode(nextText, path);
        if (array?.type !== "array") {
            return bom + setJsoncValue(nextText, path, values);
        }
        nextText = appendArrayValue(nextText, array, value);
    }

    return bom + nextText;
}
