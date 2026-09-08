import os from "node:os";

/** Escape a literal string for interpolation into a RegExp. */
export function escapeRegex(value: string): string {
    return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// Whole-segment match: the key (or its components when split on common
// separators) must BE one of these words, not merely contain them as a
// substring. Bare substring matching wrongly redacts benign fields like
// `pin_key_files`, `token_budget`, and `injection_budget_tokens`.
export const SECRET_WORDS = [
    "key",
    "token",
    "secret",
    "password",
    "auth",
    "authorization",
    "bearer",
    "credential",
];
// Abbreviations the key matcher accepts in addition to the fixture-pinned `SECRET_WORDS`.
const SECRET_WORD_ALIASES = ["passwd", "pwd", "cookie"];
const SECRET_SEGMENT_PATTERN = new RegExp(
    `^(?:${[...SECRET_WORDS, ...SECRET_WORD_ALIASES].map((w) => `${w}s?`).join("|")})$`,
    "i",
);
const TRAILING_DESCRIPTORS = new Set(["id", "ids", "value", "values", "header", "headers"]);

function redactionTypeForKey(key: string): string {
    return (
        key
            // `apiKey` needs camel-case splitting; lowercasing first produces
            // `apikey`, which no vocabulary word matches.
            .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
            .toLowerCase()
            .split(/[^a-z0-9]+/)
            .filter(
                (segment) => SECRET_SEGMENT_PATTERN.test(segment) || SECRET_QUALIFIERS.has(segment),
            )
            .join("_") || "secret"
    );
}

// `password`, `secret`, `credential`, and `cookie` identify secrets without a preceding qualifier
// and redact even numeric values; `token` and `key` also count as a final segment (`oauth_token`,
// `signing_key`) but keep numeric values so `injection_budget_tokens: 12` stays readable.
const UNQUALIFIED_SECRET_SEGMENT_PATTERN = /^(?:password|passwd|pwd|secret|credential|cookie)s?$/i;
const TRAILING_SECRET_SEGMENT_PATTERN = /^(?:token|key)s?$/i;

// Do not redact numeric, boolean, null, or undefined values solely because their key contains a secret word.
function isNonSecretScalarValue(value: string): boolean {
    const v = value.trim();
    if (v === "true" || v === "false" || v === "null" || v === "undefined") return true;
    return /^[+-]?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?$/.test(v);
}

// A number under `password`, `secret`, or `credential` is a PIN or numeric token and is redacted;
// `api_key`, `token`, and `key` keep numeric values (`max_tokens: 4096`, and the fixture-pinned `"api_key": "4096"`).
function keepsScalarValue(key: string, value: string): boolean {
    if (!isNonSecretScalarValue(value)) return false;
    const v = value.trim();
    if (v === "true" || v === "false" || v === "null" || v === "undefined") return true;
    return !key
        .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
        .toLowerCase()
        .split(/[^a-z0-9]+/)
        .some((segment) => UNQUALIFIED_SECRET_SEGMENT_PATTERN.test(segment));
}

export const SECRET_QUALIFIERS = new Set([
    "api",
    "access",
    "private",
    "client",
    "auth",
    "authorization",
    "secret",
    "bearer",
    "session",
    "refresh",
    "service",
    "x",
    "openai",
    "anthropic",
    "google",
    "github",
    "huggingface",
    "aws",
    "azure",
]);

// A segment is a secret word, or a qualifier fused to one (`apikey`, `accesstoken`, `clientsecret`).
// Whole-segment matching keeps `author`, `keyboard`, and `tokenizer` readable in diagnostics.
const SECRET_KEY_SEGMENT_PATTERN = new RegExp(
    `^(?:${[...SECRET_QUALIFIERS].join("|")})?(?:${[...SECRET_WORDS, ...SECRET_WORD_ALIASES].join("|")})s?$`,
    "i",
);

function hasSecretKeySegment(key: string): boolean {
    return key
        .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
        .toLowerCase()
        .split(/[^a-z0-9]+/)
        .some((segment) => SECRET_KEY_SEGMENT_PATTERN.test(segment));
}

export function isSecretKey(key: string): boolean {
    const segments = key
        .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
        .toLowerCase()
        .split(/[._-]+/)
        .filter(Boolean);
    if (segments.length === 0) return false;

    if (segments.length === 1) {
        const first = segments[0];
        return Boolean(first && SECRET_SEGMENT_PATTERN.test(first));
    }

    for (let i = 0; i < segments.length; i++) {
        const seg = segments[i];
        if (!seg || !SECRET_SEGMENT_PATTERN.test(seg)) continue;

        let trailingOk = true;
        for (let j = i + 1; j < segments.length; j++) {
            const tail = segments[j];
            if (!tail) continue;
            if (TRAILING_DESCRIPTORS.has(tail)) continue;
            if (SECRET_SEGMENT_PATTERN.test(tail)) continue;
            trailingOk = false;
            break;
        }
        if (!trailingOk) continue;

        if (UNQUALIFIED_SECRET_SEGMENT_PATTERN.test(seg)) return true;
        if (TRAILING_SECRET_SEGMENT_PATTERN.test(seg)) return true;

        for (let k = i - 1; k >= 0; k--) {
            const lead = segments[k];
            if (lead && SECRET_QUALIFIERS.has(lead)) return true;
        }
    }
    return false;
}

// `userInfo()` throws `ERR_SYSTEM_ERROR` when the process UID has no passwd entry, which is
// common in containers running as an arbitrary UID.
function currentUsername(): string | null {
    try {
        return os.userInfo().username || null;
    } catch {
        return null;
    }
}

// `homedir()` throws the same way when `HOME` is unset and the UID has no passwd entry.
// Replacing `/` with `~` would replace every path separator.
function currentHomeDir(): string | null {
    try {
        const home = os.homedir();
        if (!home || /^(?:[A-Za-z]:)?[\\/]*$/.test(home)) return null;
        return home;
    } catch {
        return null;
    }
}

export function sanitizePathString(value: string): string {
    const home = currentHomeDir();
    const username = currentUsername();
    let sanitized = value;
    if (home) {
        sanitized = sanitized.replace(new RegExp(escapeRegex(home), "g"), "~");
    }
    sanitized = sanitized
        .replace(/\/Users\/[^/]+\//gi, "/Users/<USER>/")
        .replace(/\/home\/[^/]+\//gi, "/home/<USER>/")
        .replace(/[A-Za-z]:[\\/]Users[\\/][^\\/]+[\\/]/gi, "C:\\Users\\<USER>\\");
    // A username that is itself a secret vocabulary word (`token`, `auth`) would erase the key
    // the secret rules match on; the path patterns above still cover its home directory.
    // Whole-word matching keeps a username such as `pass` out of `password:`.
    if (username && !hasSecretKeySegment(username)) {
        sanitized = sanitized.replace(new RegExp(`\\b${escapeRegex(username)}\\b`, "g"), "<USER>");
    }
    return sanitized;
}

const SECRET_TEXT_PATTERNS: Array<{
    pattern: RegExp;
    replacement: string | ((match: string, ...groups: string[]) => string);
}> = [
    {
        pattern: /\bsk-ant-(?:api03-)?[A-Za-z0-9_-]{32,}/g,
        replacement: "<ANTHROPIC_API_KEY_REDACTED>",
    },
    {
        pattern: /\bsk-(?:proj-)?[A-Za-z0-9_-]{32,}/g,
        replacement: "<OPENAI_API_KEY_REDACTED>",
    },
    {
        pattern: /\bgithub_pat_[A-Za-z0-9_]{20,}/g,
        replacement: "<GITHUB_PAT_REDACTED>",
    },
    {
        pattern: /\b(?:gh[opsu]|ghr)_[A-Za-z0-9]{30,}/g,
        replacement: "<GITHUB_TOKEN_REDACTED>",
    },
    {
        pattern: /\bhf_[A-Za-z0-9]{30,}/g,
        replacement: "<HUGGINGFACE_TOKEN_REDACTED>",
    },
    {
        pattern: /\b(?:AKIA|ASIA)[0-9A-Z]{16}(?![A-Za-z0-9])/g,
        replacement: "<AWS_ACCESS_KEY_ID_REDACTED>",
    },
    {
        pattern: /\b(?:xox[abprsuvc]|xapp)-[A-Za-z0-9-]{10,}/g,
        replacement: "<SLACK_TOKEN_REDACTED>",
    },
    {
        pattern: /\bAIza[A-Za-z0-9_-]{35}(?![A-Za-z0-9])/g,
        replacement: "<GOOGLE_API_KEY_REDACTED>",
    },
    {
        pattern:
            /\b(Authorization\s*:\s*(Bearer|Basic|Token|Negotiate|NTLM)\s+)([A-Za-z0-9._~+/=-]{8,})/gi,
        replacement: (_full: string, prefix: string, scheme: string) =>
            `${prefix}<REDACTED:${scheme.toLowerCase()}>`,
    },
    // Digest carries `username`, `nonce`, and `response` as comma-separated parameters, so the whole parameter list is replaced.
    {
        pattern: /\b(Authorization\s*:\s*Digest\s+)([^\r\n]+)/gi,
        replacement: (_full: string, prefix: string) => `${prefix}<REDACTED:digest>`,
    },
    // Known schemes already followed by a redaction marker are preserved; all other Authorization values are replaced.
    // The lookahead absorbs leading whitespace so backtracking on `\s*` cannot slip past it.
    {
        pattern:
            /\b(Authorization\s*:\s*)(?!\s*(?:Bearer|Basic|Token|Negotiate|NTLM|Digest)\s+<(?:REDACTED:[a-z_]+|[A-Z0-9_]+_REDACTED)>)(\S[^\r\n]*)/gi,
        replacement: (_full: string, prefix: string) => `${prefix}<REDACTED:authorization>`,
    },
    {
        pattern: /\b((?:Set-)?Cookie\s*:\s*)(\S(?:[^\r\n}]*[^\s}])?)/gi,
        replacement: (_full: string, prefix: string) => `${prefix}<REDACTED:cookie>`,
    },
    // URL userinfo (`scheme://user:pass@host/...`) carries credentials in
    // package sources, git remotes, and proxy settings. The match runs through
    // the final at-sign before the first slash so a password containing `@`
    // does not leak its tail; the scheme and host stay so the endpoint remains
    // identifiable.
    {
        pattern: /\b([a-z][a-z0-9+.-]*:\/\/)([^\s/]+)@/gi,
        replacement: (_full: string, scheme: string) => `${scheme}<REDACTED:userinfo>@`,
    },
    {
        pattern: /\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/g,
        replacement: "<JWT_REDACTED>",
    },
    {
        // A URL query string or fragment can carry a token under any parameter name.
        pattern: /\b([a-z][a-z0-9+.-]*:\/\/[^\s?#]+)[?#][^\s]*/gi,
        replacement: (_full: string, base: string) => `${base}?<REDACTED:query>`,
    },
    {
        pattern:
            /(["'])([^"']*(?:key|token|secret|password|passwd|pwd|auth|bearer|credential|cookie)[^"']*)\1(\s*:\s*)(?:(["'])((?:\\.|(?!\4)[^\\\r\n])*)\4|(-?\d+(?:\.\d+)?))/gi,
        replacement: (
            full: string,
            quote: string,
            key: string,
            separator: string,
            valueQuote: string | undefined,
            quoted: string | undefined,
            bare: string | undefined,
        ) => {
            const value = valueQuote ? (quoted ?? "") : (bare ?? "");
            if (!hasSecretKeySegment(key) || keepsScalarValue(key, value)) return full;
            const q = valueQuote ?? "";
            return `${quote}${key}${quote}${separator}${q}<REDACTED:${redactionTypeForKey(key)}>${q}`;
        },
    },
    // The negative lookahead excludes recognized authorization schemes from this pattern.
    // `[ \t]*` around the colon keeps a bare `key:` at end of line from consuming the next line's first word.
    // A bare value ends at whitespace so the prose after `token: abc` in a log line survives; a
    // multiword value must be quoted to be consumed whole.
    {
        pattern:
            /\b([A-Za-z0-9_.-]*(?:key|token|secret|password|passwd|pwd|auth|bearer|credential|cookie)[A-Za-z0-9_.-]*)([ \t]*:[ \t]*)(?!<|Bearer\b|Basic\b|Token\b|Negotiate\b|NTLM\b|Digest\b)(?:(["'`])((?:\\.|(?!\3)[^\\\r\n])*)\3|([^\s'"`,;}\])]+))/gi,
        replacement: (
            full: string,
            key: string,
            separator: string,
            quote: string | undefined,
            quoted: string | undefined,
            bare: string | undefined,
        ) => {
            const value = quote ? (quoted ?? "") : (bare ?? "");
            if (!hasSecretKeySegment(key) || value === "" || keepsScalarValue(key, value)) {
                return full;
            }
            const q = quote ?? "";
            return `${key}${separator}${q}<REDACTED:${redactionTypeForKey(key)}>${q}`;
        },
    },
    {
        pattern:
            /\b([A-Za-z0-9_.-]*(?:key|token|secret|password|passwd|pwd|auth|bearer|credential|cookie)[A-Za-z0-9_.-]*)\s*=\s*(?:(["'`])((?:\\.|(?!\2)[^\\\r\n])*)\2|([^\s'"`]+))/gi,
        replacement: (
            full: string,
            key: string,
            quote: string | undefined,
            quoted: string | undefined,
            bare: string | undefined,
        ) => {
            const value = quote ? (quoted ?? "") : (bare ?? "");
            if (!hasSecretKeySegment(key) || value === "" || keepsScalarValue(key, value)) {
                return full;
            }
            const q = quote ?? "";
            return `${key}=${q}<REDACTED:${redactionTypeForKey(key)}>${q}`;
        },
    },
    // `--api-key abc` / `-p hunter2` style arguments; a value beginning with `-` is the next flag.
    {
        pattern:
            /(^|\s)(--?[A-Za-z0-9-]*(?:key|token|secret|password|passwd|pwd|auth|credential|cookie)[A-Za-z0-9-]*)(\s+)(?:(["'])((?:\\.|(?!\4)[^\\\r\n])*)\4|([^\s"'-]\S*))/gi,
        replacement: (
            full: string,
            lead: string,
            flag: string,
            space: string,
            quote: string | undefined,
            quoted: string | undefined,
            bare: string | undefined,
        ) => {
            const key = flag.replace(/^--?/, "");
            const value = quote ? (quoted ?? "") : (bare ?? "");
            if (!hasSecretKeySegment(key) || keepsScalarValue(key, value)) return full;
            const q = quote ?? "";
            return `${lead}${flag}${space}${q}<REDACTED:${redactionTypeForKey(key)}>${q}`;
        },
    },
];

// `JSON.stringify` output nests arbitrarily; a regex cannot pair brackets, so a quoted secret key
// followed by `[` or `{` has its value consumed by a bracket-aware scan that skips string literals.
const QUOTED_SECRET_KEY_WITH_CONTAINER =
    /(["'])([^"'\r\n]*(?:key|token|secret|password|passwd|pwd|auth|bearer|credential|cookie)[^"'\r\n]*)\1(\s*:\s*)(?=[[{])/gi;

function containerEnd(text: string, open: number): number {
    const stack: string[] = [];
    let quote: string | null = null;
    for (let i = open; i < text.length; i += 1) {
        const ch = text[i];
        if (quote) {
            if (ch === "\\") i += 1;
            else if (ch === quote) quote = null;
            continue;
        }
        if (ch === '"' || ch === "'") quote = ch;
        else if (ch === "[" || ch === "{") stack.push(ch === "[" ? "]" : "}");
        else if (ch === "]" || ch === "}") {
            if (stack.pop() !== ch) return -1;
            if (stack.length === 0) return i + 1;
        } else if (ch === "\n" || ch === "\r") return -1;
    }
    return -1;
}

function redactStructuredSecretValues(value: string): string {
    let out = "";
    let cursor = 0;
    QUOTED_SECRET_KEY_WITH_CONTAINER.lastIndex = 0;
    for (;;) {
        const match = QUOTED_SECRET_KEY_WITH_CONTAINER.exec(value);
        if (!match) break;
        const [full, quote, key, separator] = match;
        const valueStart = match.index + full.length;
        const end = hasSecretKeySegment(key) ? containerEnd(value, valueStart) : -1;
        if (end === -1) continue;
        out += value.slice(cursor, match.index);
        out += `${quote}${key}${quote}${separator}<REDACTED:${redactionTypeForKey(key)}>`;
        cursor = end;
        QUOTED_SECRET_KEY_WITH_CONTAINER.lastIndex = end;
    }
    return out + value.slice(cursor);
}

export function redactSecretText(value: string): string {
    let redacted = redactStructuredSecretValues(value);
    for (const { pattern, replacement } of SECRET_TEXT_PATTERNS) {
        if (typeof replacement === "string") {
            redacted = redacted.replace(pattern, replacement);
        } else {
            redacted = redacted.replace(
                pattern,
                replacement as (match: string, ...groups: string[]) => string,
            );
        }
    }
    return redacted;
}

// A terminated PEM block is replaced whole; an unterminated header takes its `Proc-Type:`/`DEK-Info:`
// lines and base64 body with it. Callers of `redactSecretText` alone keep their own PEM handling.
const PRIVATE_KEY_BLOCK_PATTERN =
    /-----BEGIN [A-Z ]*PRIVATE KEY-----(?:[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----|(?:\r?\n(?:[A-Za-z-]+:[^\r\n]*|[A-Za-z0-9+/=]{16,}(?=\r?\n|$)|(?=\r?\n)))*)/g;

export function redactPrivateKeyBlocks(value: string): string {
    return value.replace(PRIVATE_KEY_BLOCK_PATTERN, "<PRIVATE_KEY_REDACTED>");
}

export function sanitizeDiagnosticText(value: string): string {
    return redactSecretText(redactPrivateKeyBlocks(sanitizePathString(value)));
}

// `sanitizeDiagnosticText` excludes shareability-only patterns.
const SHAREABILITY_SENSITIVE_PATTERNS: RegExp[] = [
    /\bC:\/Users\/[^/\s]+/i,
    /(?:^|\s)~\/[^\s]+/,
    // `sanitizeDiagnosticText` redacts inline `key: value` and `key=value` secrets because keyed redaction only processes config object keys.
    /\b(?:api[_-]?key|secret|token|password|passwd|pwd|client[_-]?secret|access[_-]?key)\b\s*[:=]\s*\S+/i,
    // Redact local and private endpoints because they identify the environment.
    // The bracketed arm handles `[::1]` because `\b` does not match before `[` at the start of input or after a non-word character.
    // The bare IPv6 loopback arm requires a non-word, non-colon, non-dot prefix to avoid matching suffixes of addresses such as `2001:db8::1`.
    /(?:\b(?:localhost|127\.0\.0\.1|0\.0\.0\.0)\b|\[::1\]|(?:^|[^\w:.])::1\b)(?::\d+)?/i,
    /\b(?:10|127)\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/,
    /\b192\.168\.\d{1,3}\.\d{1,3}\b/,
    /\b172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3}\b/,
    // The redactor removes IPv4 link-local (APIPA), IPv6 unique-local (`fc00::/7`), and IPv6 link-local (`fe80::/10`) addresses because they identify the environment.
    /\b169\.254\.\d{1,3}\.\d{1,3}\b/,
    /(?:^|[\s"'`=([])\[?(?:f[cd][0-9a-f]{2}|fe[89ab][0-9a-f]):[0-9a-f:]*[0-9a-f\]]/i,
];

export function hasShareabilitySensitiveText(text: string): boolean {
    try {
        if (sanitizeDiagnosticText(text) !== text) return true;
        return SHAREABILITY_SENSITIVE_PATTERNS.some((pattern) => pattern.test(text));
    } catch {
        return true;
    }
}

// Prompt fields hold arbitrary private prose; a shareable report keeps only presence and length.
const PROMPT_KEY_PATTERN =
    /^(?:prompt|system_prompt|description|tool_descriptions|skip_signatures)$/;

const PROSE_MARKER_PATTERN = /^<REDACTED \d+ chars>$/;

/** Idempotent: a value already reduced to its marker keeps the original length. */
export function describeProseLength(text: string): string {
    return PROSE_MARKER_PATTERN.test(text) ? text : `<REDACTED ${text.length} chars>`;
}

function redactProse(value: unknown): unknown {
    if (typeof value === "string") return describeProseLength(value);
    if (Array.isArray(value)) return value.map(redactProse);
    if (value && typeof value === "object") {
        return Object.fromEntries(
            Object.entries(value).map(([entryKey, entry]) => [entryKey, redactProse(entry)]),
        );
    }
    return value;
}

export function sanitizeConfigValue(value: unknown, keyPath: string[] = []): unknown {
    const key = keyPath.at(-1) ?? "";
    if (PROMPT_KEY_PATTERN.test(key)) return redactProse(value);
    const secretKey = Boolean(key) && isSecretKey(key);
    // Null and booleans carry no credential. A number stays unless the key names a password-like
    // secret, where it can be a PIN (`keepsScalarValue`); a string under a secret key is always redacted.
    if (value === null || typeof value === "boolean") return value;
    if (typeof value === "number" && (!secretKey || keepsScalarValue(key, String(value)))) {
        return value;
    }
    if (secretKey) {
        return `<REDACTED:${redactionTypeForKey(key)}>`;
    }
    if (typeof value === "string") return sanitizeDiagnosticText(value);
    if (Array.isArray(value)) {
        return value.map((entry, index) => sanitizeConfigValue(entry, [...keyPath, String(index)]));
    }
    if (value && typeof value === "object") {
        return Object.fromEntries(
            Object.entries(value).map(([entryKey, entry]) => [
                entryKey,
                sanitizeConfigValue(entry, [...keyPath, entryKey]),
            ]),
        );
    }
    return value;
}
