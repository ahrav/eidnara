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

/**
 * A regular-expression literal is reported as a `string` span. The preceding-token heuristic
 * that detects it can misclassify `a++ / b`; the QuickJS sandbox, not this scan, is the
 * enforcement layer.
 */
export function scanSourceSpans(source: string): SourceSpan[] {
    const spans: SourceSpan[] = [];
    let codeStart = 0;
    let index = 0;

    const pushSpan = (kind: SourceSpanKind, end: number): void => {
        if (index > codeStart) spans.push({ kind: "code", start: codeStart, end: index });
        spans.push({ kind, start: index, end });
        index = end;
        codeStart = end;
    };

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
            pushSpan("template", endOfTemplate(source, index));
        } else if (char === "/" && regexCanStart(source, index)) {
            pushSpan("string", endOfRegex(source, index));
        } else {
            index += 1;
        }
    }
    if (index > codeStart) spans.push({ kind: "code", start: codeStart, end: index });
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

function endOfTemplate(source: string, open: number): number {
    let index = open + 1;
    while (index < source.length) {
        const char = source[index];
        if (char === "\\") {
            index += 2;
            continue;
        }
        if (char === "`") return index + 1;
        if (char === "$" && source[index + 1] === "{") {
            index = endOfTemplateExpression(source, index + 2);
            continue;
        }
        index += 1;
    }
    return source.length;
}

function endOfTemplateExpression(source: string, from: number): number {
    let depth = 1;
    let index = from;
    while (index < source.length && depth > 0) {
        const char = source[index];
        if (char === "{") depth += 1;
        else if (char === "}") depth -= 1;
        else if (char === '"' || char === "'") {
            index = endOfQuoted(source, index, char);
            continue;
        } else if (char === "`") {
            index = endOfTemplate(source, index);
            continue;
        }
        index += 1;
    }
    return index;
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

function regexCanStart(source: string, slash: number): boolean {
    let index = slash - 1;
    while (index >= 0 && /\s/.test(source[index])) index -= 1;
    if (index < 0) return true;
    const previous = source[index];
    if (/[(,=:[!&|?{};+\-*%<>~^]/.test(previous)) return true;
    if (/[\w$]/.test(previous)) {
        let start = index;
        while (start > 0 && /[\w$]/.test(source[start - 1])) start -= 1;
        return REGEX_PRECEDING_KEYWORDS.has(source.slice(start, index + 1));
    }
    return false;
}
