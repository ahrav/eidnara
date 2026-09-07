import { existsSync, readFileSync } from "node:fs";

import { getNodeValue, type Node, type ParseError, parseTree, visit } from "jsonc-parser";
import { isRecord } from "./record-type-guard";

const PROTOTYPE_POLLUTION_KEYS = new Set(["__proto__", "constructor", "prototype"]);

export function isPrototypePollutionKey(key: string): boolean {
    return PROTOTYPE_POLLUTION_KEYS.has(key);
}

/**
 * comment-json boxes a scalar root (`"x"` parses to a `String` object), which `isRecord` cannot tell from an object root. commentlint: allow(JUDGE)
 * Only a plain-prototype object counts as an object root.
 */
export function isCommentJsonObjectRoot(value: unknown): value is Record<string, unknown> {
    return isRecord(value) && Object.getPrototypeOf(value) === Object.prototype;
}

/**
 * `true` when the text holds no JSON value: empty, whitespace, or comments only.
 * Writers use this to seed an initial object while keeping the existing comments.
 * Missing input surfaces as a zero-length error at end of text; an unexpected
 * token has a non-zero length and makes the text malformed rather than empty.
 */
export function isJsoncEmpty(content: string): boolean {
    const text = content.charCodeAt(0) === 0xfeff ? content.slice(1) : content;
    let empty = true;
    const sawValue = (): void => {
        empty = false;
    };
    visit(text, {
        onObjectBegin: sawValue,
        onArrayBegin: sawValue,
        onLiteralValue: sawValue,
        onError: (_code, _offset, length) => {
            if (length > 0) empty = false;
        },
    });
    return empty;
}

export interface ParsedJsonSanitizerOptions {
    onRejectedKey?: (path: readonly (string | number)[]) => void;
}

/**
 * sanitizeParsedJson copies parsed JSON into fresh own-property-only containers while rejecting keys that can alter an object's prototype during a later merge.
 */
export function sanitizeParsedJson<T>(
    value: T,
    options: ParsedJsonSanitizerOptions = {},
    path: readonly (string | number)[] = [],
): T {
    if (Array.isArray(value)) {
        return value.map((entry, index) =>
            sanitizeParsedJson(entry, options, [...path, index]),
        ) as T;
    }
    if (value === null || typeof value !== "object") return value;

    const source = value as Record<string, unknown>;
    const sourcePrototype = Object.getPrototypeOf(source);
    // Any non-plain prototype (a parser-injected object OR null) is treated
    // as an attempted __proto__ override; the rebuilt object always gets a
    // plain prototype either way.
    if (sourcePrototype !== Object.prototype) {
        options.onRejectedKey?.([...path, "__proto__"]);
    }

    const sanitized: Record<string, unknown> = {};
    for (const key of Object.keys(source)) {
        if (isPrototypePollutionKey(key)) {
            options.onRejectedKey?.([...path, key]);
            continue;
        }
        Object.defineProperty(sanitized, key, {
            value: sanitizeParsedJson(source[key], options, [...path, key]),
            enumerable: true,
            configurable: true,
            writable: true,
        });
    }
    return sanitized as T;
}

/**
 * `jsonc-parser` reads an out-of-range literal such as `1e400` as `Infinity`
 * without reporting an error; the daemon's serde reader rejects the file.
 */
function assertFiniteNumbers(node: Node): void {
    if (node.type === "number" && !Number.isFinite(node.value)) {
        throw new SyntaxError("Invalid JSONC");
    }
    for (const child of node.children ?? []) {
        assertFiniteNumbers(child);
    }
}

/** Allows trailing commas and removes a leading byte-order mark before parsing. */
export function parseJsoncTree(content: string): Node {
    const text = content.charCodeAt(0) === 0xfeff ? content.slice(1) : content;
    const errors: ParseError[] = [];
    const root = parseTree(text, errors, { allowTrailingComma: true });
    if (!root || errors.length > 0) {
        throw new SyntaxError("Invalid JSONC");
    }
    assertFiniteNumbers(root);
    return root;
}

/**
 * Builds values the way `JSON.parse` does: objects have `Object.prototype`,
 * `__proto__` is an own property, and the last duplicate key wins.
 * `getNodeValue` returns null-prototype objects instead.
 */
function nodeToJsonValue(node: Node): unknown {
    switch (node.type) {
        case "array":
            return (node.children ?? []).map(nodeToJsonValue);
        case "object": {
            const object: Record<string, unknown> = {};
            for (const property of node.children ?? []) {
                const [keyNode, valueNode] = property.children ?? [];
                if (!keyNode || !valueNode || typeof keyNode.value !== "string") continue;
                Object.defineProperty(object, keyNode.value, {
                    value: nodeToJsonValue(valueNode),
                    enumerable: true,
                    configurable: true,
                    writable: true,
                });
            }
            return object;
        }
        default:
            return getNodeValue(node);
    }
}

/** Sanitizes parsed JSONC to reject prototype-pollution keys. */
export function parseConfigJsonc<T = unknown>(
    content: string,
    options: ParsedJsonSanitizerOptions = {},
): T {
    return sanitizeParsedJson(nodeToJsonValue(parseJsoncTree(content)) as T, options);
}

/** Returns `null` for a missing or unreadable file and for malformed JSONC. */
export function readJsoncFile<T = unknown>(
    filePath: string,
    options: ParsedJsonSanitizerOptions = {},
): T | null {
    try {
        return parseConfigJsonc<T>(readFileSync(filePath, "utf-8"), options);
    } catch (_error) {
        return null;
    }
}

export function detectConfigFile(basePath: string): {
    format: "json" | "jsonc" | "none";
    path: string;
} {
    const jsoncPath = `${basePath}.jsonc`;
    const jsonPath = `${basePath}.json`;

    if (existsSync(jsoncPath)) {
        return { format: "jsonc", path: jsoncPath };
    }

    if (existsSync(jsonPath)) {
        return { format: "json", path: jsonPath };
    }

    return { format: "none", path: jsoncPath };
}
