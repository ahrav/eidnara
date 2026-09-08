export type SourceSpanKind = "code" | "comment" | "string" | "template";

export interface SourceSpan {
    kind: SourceSpanKind;
    start: number;
    end: number;
}

const REGEX_PRECEDING_KEYWORDS = new Set([
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
]);

const CONTROL_KEYWORDS = new Set(["if", "while", "for", "with"]);

/**
 * `${...}` inside a template yields `code` spans, so template expressions stay visible to
 * callers that scan code; each template piece around them is its own `template` span.
 *
 * A regular-expression literal is reported as a `string` span. After `)`, a slash starts a
 * literal only if the parenthesis closed an `if`, `while`, `for`, or `with` head.
 */
export function scanSourceSpans(source: string): SourceSpan[] {
    const spans: SourceSpan[] = [];
    let index = 0;
    let codeStart = 0;
    const openParens: boolean[] = [];
    let lastCloseParenWasControl = false;

    const flushCode = (): void => {
        if (index > codeStart) spans.push({ kind: "code", start: codeStart, end: index });
    };
    const pushSpan = (kind: SourceSpanKind, end: number): void => {
        flushCode();
        spans.push({ kind, start: index, end });
        index = end;
        codeStart = end;
    };

    const scanTemplate = (): void => {
        let pieceStart = index;
        index += 1;
        while (index < source.length) {
            const char = source[index];
            if (char === "\\") {
                index += 2;
                continue;
            }
            if (char === "`") {
                index += 1;
                spans.push({ kind: "template", start: pieceStart, end: index });
                codeStart = index;
                return;
            }
            if (char === "$" && source[index + 1] === "{") {
                index += 2;
                spans.push({ kind: "template", start: pieceStart, end: index });
                codeStart = index;
                scanCode(true);
                pieceStart = index - 1;
                continue;
            }
            index += 1;
        }
        spans.push({ kind: "template", start: pieceStart, end: source.length });
        codeStart = source.length;
    };

    const scanCode = (stopAtClosingBrace: boolean): void => {
        let braceDepth = 0;
        while (index < source.length) {
            const char = source[index];
            const next = source[index + 1];
            if (char === "/" && next === "/") {
                pushSpan("comment", endOfLine(source, index));
            } else if (char === "/" && next === "*") {
                const close = source.indexOf("*/", index + 2);
                pushSpan("comment", close < 0 ? source.length : close + 2);
            } else if (char === '"' || char === "'") {
                pushSpan("string", endOfQuoted(source, index, char));
            } else if (char === "`") {
                flushCode();
                scanTemplate();
            } else if (char === "/" && regexCanStart(source, index, lastCloseParenWasControl)) {
                pushSpan("string", endOfRegex(source, index));
            } else {
                if (char === "(") {
                    openParens.push(CONTROL_KEYWORDS.has(wordBefore(source, index)));
                } else if (char === ")") {
                    lastCloseParenWasControl = openParens.pop() ?? false;
                } else if (char === "{") {
                    braceDepth += 1;
                } else if (char === "}") {
                    if (stopAtClosingBrace && braceDepth === 0) {
                        flushCode();
                        index += 1;
                        codeStart = index;
                        return;
                    }
                    braceDepth -= 1;
                }
                index += 1;
            }
        }
        flushCode();
    };

    scanCode(false);
    return spans;
}

/** Interiors become spaces; offsets, line breaks, and string/template delimiters are preserved. */
export function maskSourceSpans(source: string, kinds: ReadonlySet<SourceSpanKind>): string {
    const chars = source.split("");
    for (const span of scanSourceSpans(source)) {
        if (!kinds.has(span.kind)) continue;
        const keepDelimiters = span.kind === "string" || span.kind === "template";
        const first = keepDelimiters ? span.start + 1 : span.start;
        const last = keepDelimiters ? span.end - 1 : span.end;
        for (let i = first; i < last; i += 1) {
            if (chars[i] !== "\n" && chars[i] !== "\r") chars[i] = " ";
        }
    }
    return chars.join("");
}

export function decodeStringLiteral(body: string): string {
    return body.replace(
        /\\(u\{([0-9a-fA-F]+)\}|u([0-9a-fA-F]{4})|x([0-9a-fA-F]{2})|\r\n|[\s\S])/g,
        (whole, escaped: string, braced?: string, unicode?: string, hex?: string) => {
            if (braced) return String.fromCodePoint(Number.parseInt(braced, 16));
            if (unicode) return String.fromCharCode(Number.parseInt(unicode, 16));
            if (hex) return String.fromCharCode(Number.parseInt(hex, 16));
            switch (escaped) {
                case "n":
                    return "\n";
                case "t":
                    return "\t";
                case "r":
                    return "\r";
                case "b":
                    return "\b";
                case "f":
                    return "\f";
                case "v":
                    return "\v";
                case "0":
                    return "\0";
                case "\n":
                case "\r\n":
                case "\u2028":
                case "\u2029":
                    return "";
                default:
                    return escaped.length === 1 ? escaped : whole;
            }
        },
    );
}

function endOfLine(source: string, from: number): number {
    const newline = source.indexOf("\n", from);
    return newline < 0 ? source.length : newline;
}

function endOfQuoted(source: string, open: number, quote: string): number {
    let index = open + 1;
    while (index < source.length) {
        const char = source[index];
        if (char === "\\") {
            index += 2;
            continue;
        }
        if (char === quote) return index + 1;
        if (char === "\n") return index;
        index += 1;
    }
    return source.length;
}

function endOfRegex(source: string, open: number): number {
    let index = open + 1;
    let inClass = false;
    while (index < source.length) {
        const char = source[index];
        if (char === "\\") {
            index += 2;
            continue;
        }
        if (char === "\n") return index;
        if (inClass) {
            if (char === "]") inClass = false;
        } else if (char === "[") {
            inClass = true;
        } else if (char === "/") {
            index += 1;
            while (index < source.length && /[a-z]/i.test(source[index])) index += 1;
            return index;
        }
        index += 1;
    }
    return source.length;
}

function wordBefore(source: string, position: number): string {
    let end = position;
    while (end > 0 && /\s/.test(source[end - 1])) end -= 1;
    let start = end;
    while (start > 0 && /[\w$]/.test(source[start - 1])) start -= 1;
    return source.slice(start, end);
}

function regexCanStart(source: string, slash: number, lastCloseParenWasControl: boolean): boolean {
    let index = slash - 1;
    while (index >= 0 && /\s/.test(source[index])) index -= 1;
    if (index < 0) return true;
    const previous = source[index];
    if (previous === ")") return lastCloseParenWasControl;
    if (/[(,=:[!&|?{};+\-*%<>~^]/.test(previous)) return true;
    if (/[\w$]/.test(previous)) {
        return REGEX_PRECEDING_KEYWORDS.has(wordBefore(source, index + 1));
    }
    return false;
}
