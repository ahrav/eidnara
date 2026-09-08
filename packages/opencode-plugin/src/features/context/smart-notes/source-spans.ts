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
const BLOCK_KEYWORDS = new Set(["else", "do", "try", "finally"]);

interface OpenParen {
    control: boolean;
    open: number;
}

/**
 * `${...}` inside a template yields `code` spans, so template expressions stay visible to
 * callers that scan code; each template piece around them is its own `template` span.
 *
 * A regular-expression literal is reported as a `string` span. After `)`, a slash starts a
 * literal only if the parenthesis closed an `if`, `while`, `for`, or `with` head; after `}`,
 * only if the brace closed a block or a declaration rather than an object literal or a
 * function or class expression.
 */
export function scanSourceSpans(source: string): SourceSpan[] {
    const spans: SourceSpan[] = [];
    let index = 0;
    let codeStart = 0;
    const openParens: OpenParen[] = [];
    const openBraces: boolean[] = [];
    let lastCloseParen: OpenParen | null = null;
    let lastCloseBraceWasValue = false;

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
            } else if (
                char === "/" &&
                regexCanStart(
                    source,
                    index,
                    lastCloseParen?.control ?? false,
                    lastCloseBraceWasValue,
                )
            ) {
                pushSpan("string", endOfRegex(source, index));
            } else {
                if (char === "(") {
                    openParens.push({
                        control: CONTROL_KEYWORDS.has(wordBefore(source, index)),
                        open: index,
                    });
                } else if (char === ")") {
                    lastCloseParen = openParens.pop() ?? null;
                } else if (char === "{") {
                    braceDepth += 1;
                    openBraces.push(braceOpensValue(source, index, lastCloseParen));
                } else if (char === "}") {
                    if (stopAtClosingBrace && braceDepth === 0) {
                        flushCode();
                        index += 1;
                        codeStart = index;
                        return;
                    }
                    braceDepth -= 1;
                    lastCloseBraceWasValue = openBraces.pop() ?? false;
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
    return source.slice(wordStartBefore(source, position), wordEndBefore(source, position));
}

function wordEndBefore(source: string, position: number): number {
    let end = position;
    while (end > 0 && /\s/.test(source[end - 1])) end -= 1;
    return end;
}

function wordStartBefore(source: string, position: number): number {
    let start = wordEndBefore(source, position);
    while (start > 0 && /[\w$]/.test(source[start - 1])) start -= 1;
    return start;
}

/**
 * True when the `}` that closes this brace ends an operand, so a following slash divides:
 * object literals, and the bodies of function and class expressions. False for blocks and for
 * function and class declarations, after which a slash starts a regular-expression literal.
 */
function braceOpensValue(source: string, brace: number, lastCloseParen: OpenParen | null): boolean {
    const index = wordEndBefore(source, brace) - 1;
    if (index < 0) return false;
    const previous = source[index];
    if (previous === ")") {
        const keyword = lastCloseParen ? functionKeywordBefore(source, lastCloseParen.open) : -1;
        return keyword >= 0 && expressionPrecedes(source, keyword);
    }
    const classKeyword = classKeywordBefore(source, brace);
    if (classKeyword >= 0) return expressionPrecedes(source, classKeyword);
    if (previous === ">" && source[index - 1] === "=") return false;
    if (/[(,=:[?+\-*/%&|^!~<>]/.test(previous)) return true;
    if (/[\w$]/.test(previous)) {
        const word = wordBefore(source, index + 1);
        if (BLOCK_KEYWORDS.has(word)) return false;
        return REGEX_PRECEDING_KEYWORDS.has(word);
    }
    return false;
}

/** Start offset of the `function` keyword whose parameter list opens at `paren`, or -1. */
function functionKeywordBefore(source: string, paren: number): number {
    let position = paren;
    for (let words = 0; words < 2; words += 1) {
        let end = wordEndBefore(source, position);
        if (source[end - 1] === "*") end -= 1;
        const start = wordStartBefore(source, end);
        const word = source.slice(start, wordEndBefore(source, end));
        if (word === "function") return start;
        if (!/^[\w$]+$/.test(word)) return -1;
        position = start;
    }
    return -1;
}

/** Start offset of the `class` keyword whose body opens at `brace`, or -1. */
function classKeywordBefore(source: string, brace: number): number {
    let position = brace;
    for (let words = 0; words < 4; words += 1) {
        const start = wordStartBefore(source, position);
        const word = source.slice(start, wordEndBefore(source, position));
        if (word === "class") return start;
        if (!/^[\w$]+$/.test(word)) return -1;
        position = start;
    }
    return -1;
}

/** True when the token before `keyword` places a function or class in expression position. */
function expressionPrecedes(source: string, keyword: number): boolean {
    let end = wordEndBefore(source, keyword);
    if (wordBefore(source, keyword) === "async")
        end = wordEndBefore(source, wordStartBefore(source, keyword));
    if (end === 0) return false;
    const previous = source[end - 1];
    if (previous === ">" && source[end - 2] === "=") return true;
    if (/[(,=:[?+\-*/%&|^!~<>]/.test(previous)) return true;
    if (/[\w$]/.test(previous)) return REGEX_PRECEDING_KEYWORDS.has(wordBefore(source, end));
    return false;
}

function regexCanStart(
    source: string,
    slash: number,
    lastCloseParenWasControl: boolean,
    lastCloseBraceWasValue: boolean,
): boolean {
    const index = wordEndBefore(source, slash) - 1;
    if (index < 0) return true;
    const previous = source[index];
    if (previous === ")") return lastCloseParenWasControl;
    if (previous === "}") return !lastCloseBraceWasValue;
    // A postfix `++` or `--` ends an operand, so the slash that follows divides.
    if ((previous === "+" || previous === "-") && source[index - 1] === previous) return false;
    if (/[(,=:[!&|?{;+\-*%<>~^]/.test(previous)) return true;
    if (/[\w$]/.test(previous)) {
        return REGEX_PRECEDING_KEYWORDS.has(wordBefore(source, index + 1));
    }
    return false;
}
