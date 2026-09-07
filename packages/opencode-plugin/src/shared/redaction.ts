import { homedir, userInfo } from "node:os";

/** Escape a literal string for interpolation into a RegExp. */
export function escapeRegex(value: string): string {
    return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// Whole-segment match: a key names a secret when one of its segments (split on separators
// and case changes) IS one of these words; `keyboard` and `monkey` have no secret segment.
// `passwd` extends the Rust key gate's `LABEL_WORDS`; `pwd` is excluded because `PWD` and `OLDPWD` identify working directories.
export const SECRET_WORDS = [
    "key",
    "token",
    "secret",
    "password",
    "passwd",
    "auth",
    "authorization",
    "bearer",
    "credential",
];
const SECRET_SEGMENT_PATTERN = new RegExp(
    `^(?:${SECRET_WORDS.map((w) => `${w}s?`).join("|")})$`,
    "i",
);

const SECRET_WORD_ALTERNATION = SECRET_WORDS.join("|");
const ASSIGNMENT_KEY_RUN = "[A-Za-z0-9_.-]";
/** A quoted body spans escape pairs so `"a\"b"` is one value rather than a value and a tail. */
const DOUBLE_QUOTED_BODY = String.raw`(?:[^"\\\n]|\\.)*`;
const SINGLE_QUOTED_BODY = String.raw`(?:[^'\\\n]|\\.)*`;
const BACKTICK_QUOTED_BODY = String.raw`(?:[^\`\\\n]|\\.)*`;
/** A bare value reads escape pairs as one character so `before\ AFTER` is one shell word. */
const BARE_VALUE = String.raw`(?:[^\s'"\`\\]|\\.)+`;
/** One `name=value` parameter of a `Digest`-style header; a quoted value reads escape pairs as one character so `username="a\"b"` does not end at the escaped quote. */
const AUTH_PARAM = String.raw`[A-Za-z]+=(?:"${DOUBLE_QUOTED_BODY}"|[^\s,"]+)`;
/** A PEM header with no footer stops the body scan here instead of reading to the end of the input. */
const PEM_BODY_MAX = 16_384;

/** `separateWords` splits camel case so `apiKey` yields the same segments as `api_key` and preserves acronym runs in `URLToken`. */
function separateWords(key: string): string {
    return key.replace(/([a-z0-9])([A-Z])/g, "$1_$2").replace(/([A-Z])([A-Z][a-z])/g, "$1_$2");
}

function keySegments(key: string): string[] {
    return separateWords(key)
        .toLowerCase()
        .split(/[^a-z0-9]+/)
        .filter(Boolean);
}

function isLabelWord(segment: string): boolean {
    return SECRET_SEGMENT_PATTERN.test(segment);
}

function isLabelSegment(segment: string): boolean {
    return isLabelWord(segment) || SECRET_QUALIFIERS.has(segment);
}

function redactionTypeForKey(key: string): string {
    return keySegments(key).filter(isLabelSegment).join("_") || "secret";
}

// Do not redact numeric, boolean, null, or undefined values solely because their key contains a secret word.
function isNonSecretScalarValue(value: string): boolean {
    const v = value.trim();
    if (v === "true" || v === "false" || v === "null" || v === "undefined") return true;
    return /^[+-]?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?$/.test(v);
}

export const SECRET_QUALIFIERS = new Set([
    "api",
    "access",
    "signing",
    "signature",
    "webhook",
    "encryption",
    "hmac",
    "master",
    "refresh",
    "private",
    "client",
    "auth",
    "authorization",
    "secret",
    "bearer",
    "session",
    "service",
    "x",
    "openai",
    "anthropic",
    "google",
    "github",
    "huggingface",
    "aws",
    "azure",
    "db",
    "database",
    "admin",
    "root",
    "app",
    "application",
    "consumer",
    "oauth",
    "jwt",
    "shared",
    "user",
    "vault",
    "gcp",
    "gitlab",
    "slack",
    "stripe",
    "twilio",
]);

/** Words that follow a label word without qualifying it, so `key_value` and `keyid` reduce to the bare `key` label. */
const LABEL_AFFIXES = new Set([
    "b64",
    "base64",
    "data",
    "env",
    "file",
    "hash",
    "hex",
    "id",
    "name",
    "path",
    "plain",
    "prefix",
    "ref",
    "string",
    "text",
    "value",
]);

/** Segments that mark a key as published rather than secret, as in `public_key`. */
const NON_SECRET_KEY_MARKERS = new Set(["public", "pubkey", "publishable"]);

/** Bounds separator-free cover spans to the longest vocabulary word plus a plural suffix. */
const MAX_VOCABULARY_WORD_LENGTH = "authorization".length + 1;

function isBareKeyLabel(label: string): boolean {
    return label === "key" || label === "keys";
}

function namesAVocabularyWord(word: string): boolean {
    const stem = word.endsWith("s") ? word.slice(0, -1) : word;
    return (
        isLabelWord(word) ||
        SECRET_QUALIFIERS.has(word) ||
        LABEL_AFFIXES.has(word) ||
        LABEL_AFFIXES.has(stem)
    );
}

function vocabularyReach(joined: string, reverse: boolean): boolean[] {
    const reach = new Array<boolean>(joined.length + 1).fill(false);
    reach[reverse ? joined.length : 0] = true;
    for (let step = 0; step <= joined.length; step++) {
        const at = reverse ? joined.length - step : step;
        if (!reach[at]) continue;
        for (let span = 1; span <= MAX_VOCABULARY_WORD_LENGTH; span++) {
            const start = reverse ? at - span : at;
            const end = reverse ? at : at + span;
            if (start < 0 || end > joined.length) break;
            if (namesAVocabularyWord(joined.slice(start, end))) {
                reach[reverse ? start : end] = true;
            }
        }
    }
    return reach;
}

/** A separator-free name (`apikey`, `OPENAIAPIKEY`) is a credential when vocabulary words cover the whole name and the cover holds a label word plus a label segment other than `key`; `keyvalue` reduces to the bare key and `monkey` has no cover. commentlint: allow(JUDGE) */
function undelimitedNamesACredential(joined: string): boolean {
    const fromStart = vocabularyReach(joined, false);
    const toEnd = vocabularyReach(joined, true);
    if (!fromStart[joined.length]) return false;
    let namesALabel = false;
    let namesAQualifiedSegment = false;
    for (let start = 0; start < joined.length; start++) {
        if (!fromStart[start]) continue;
        const last = Math.min(start + MAX_VOCABULARY_WORD_LENGTH, joined.length);
        for (let end = start + 1; end <= last; end++) {
            if (!toEnd[end]) continue;
            const word = joined.slice(start, end);
            if (!namesAVocabularyWord(word)) continue;
            namesALabel ||= isLabelWord(word);
            namesAQualifiedSegment ||= isLabelSegment(word) && !isBareKeyLabel(word);
        }
    }
    return namesALabel && namesAQualifiedSegment;
}

/** `isSecretKey` mirrors `secret_shaped_json_key` in `crates/context-core/src/redaction.rs`, whose vocabulary lists `SECRET_QUALIFIERS` and `LABEL_AFFIXES` copy: a label-word segment marks the key unless a public marker is present or the label reduces to the bare `key`, which names a map entry (`target_key`, `key_id`) rather than a credential. commentlint: allow(JUDGE) */
export function isSecretKey(key: string): boolean {
    const segments = keySegments(key);
    if (segments.length === 0) return false;
    if (segments.some((segment) => NON_SECRET_KEY_MARKERS.has(segment))) return false;
    if (segments.some(isLabelWord)) {
        return !isBareKeyLabel(redactionTypeForKey(key));
    }
    return undelimitedNamesACredential(segments.join(""));
}

/** `isSecretKey` without the bare-`key` carve-out: `key=` in a log line has no map to be an entry of, while `author` and `monkey` hold no label segment and stay visible. commentlint: allow(JUDGE) */
function textKeyNamesASecret(key: string): boolean {
    const segments = keySegments(key);
    if (segments.some((segment) => NON_SECRET_KEY_MARKERS.has(segment))) return false;
    return segments.some(isLabelWord) || undelimitedNamesACredential(segments.join(""));
}

/**
 * The `key: value` form is prose as often as YAML, so it keeps the bare-`key` carve-out of
 * `isSecretKey` (`key: press any`), and an `Authorization` header is left to the header rule,
 * which has already rewritten its credential.
 */
function colonSeparatedKeyNamesASecret(key: string, separator: string): boolean {
    if (!separator.includes(":")) return textKeyNamesASecret(key);
    if (keySegments(key).includes("authorization")) return false;
    return isSecretKey(key);
}

/** Role account names are not redacted: they occur in ordinary text and name no person. */
const ROLE_ACCOUNT_NAMES = new Set(["root", "nobody", "unknown", "user"]);

/**
 * Returns empty strings if the process cannot read the home directory or username.
 * `userInfo()` throws when the current uid has no passwd entry.
 */
function hostIdentity(): { home: string; username: string } {
    let home = "";
    let username = "";
    try {
        home = homedir();
    } catch {}
    try {
        username = userInfo().username;
    } catch {}
    return { home, username };
}

/** Whether `home` names a directory below a filesystem or drive root, such as `/home/x` or `C:\Users\x`. */
function isRedactableHome(home: string): boolean {
    return /^(?:[A-Za-z]:)?[\\/][^\\/]+/.test(home.replace(/[\\/]+$/, ""));
}

const IDENTIFIER_CHAR = "[A-Za-z0-9_]";
const PATH_END = String.raw`(?=$|[\\/\s"'\`,;:)\]])`;
/** A home-directory segment: everything up to a separator, without trailing punctuation. */
const HOME_SEGMENT = String.raw`[^/\\\s"'\`]*[^/\\\s"'\`.,;:)\]]`;
/** A Windows profile name that may hold spaces (`John Doe`), read only when a separator follows its last word; the words exclude the characters Windows forbids in a name so a run cannot cross into a second path. commentlint: allow(JUDGE) */
const WINDOWS_PROFILE_SEGMENT = String.raw`[^\\/:*?"<>|\s]+(?: [^\\/:*?"<>|\s]+)*(?=[\\/])`;

export function sanitizePathString(value: string): string {
    const { home, username } = hostIdentity();
    let sanitized = value;
    if (isRedactableHome(home)) {
        // Only a whole path prefix is the home directory: `/home/zed/x` is, `/home/zedd` is not.
        // A drive path compares case-insensitively because Windows paths do.
        sanitized = sanitized.replace(
            new RegExp(
                `(^|(?!${IDENTIFIER_CHAR}).)${escapeRegex(home)}${PATH_END}`,
                /^[A-Za-z]:/.test(home) ? "gi" : "g",
            ),
            "$1~",
        );
    }
    // The drive form goes first: `C:/Users/John Doe/x` also matches `/Users/John`, which would
    // leave ` Doe` behind.
    sanitized = sanitized
        .replace(
            new RegExp(
                `([A-Za-z]:[\\\\/]Users[\\\\/])(?:${WINDOWS_PROFILE_SEGMENT}|${HOME_SEGMENT})`,
                "gi",
            ),
            "$1<USER>",
        )
        .replace(new RegExp(`/Users/${HOME_SEGMENT}`, "gi"), "/Users/<USER>")
        .replace(new RegExp(`/home/${HOME_SEGMENT}`, "gi"), "/home/<USER>");
    if (username && !ROLE_ACCOUNT_NAMES.has(username.toLowerCase())) {
        // Only a whole word is the username: `zed's` is, `zedd` is not.
        sanitized = sanitized.replace(
            new RegExp(
                `(^|(?!${IDENTIFIER_CHAR}).)${escapeRegex(username)}(?!${IDENTIFIER_CHAR})`,
                "g",
            ),
            "$1<USER>",
        );
    }
    return sanitized;
}

const SECRET_TEXT_PATTERNS: Array<{
    pattern: RegExp;
    replacement: string | ((match: string, ...groups: string[]) => string);
}> = [
    {
        pattern: new RegExp(
            `-----BEGIN[ A-Z0-9_-]{0,100}PRIVATE KEY(?: BLOCK)?-----[\\s\\S]{0,${PEM_BODY_MAX}}?-----END[ A-Z0-9_-]{0,100}PRIVATE KEY(?: BLOCK)?-----`,
            "gi",
        ),
        replacement: "<PRIVATE_KEY_REDACTED>",
    },
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
        pattern: /\b(?:xoxe\.xox[bp]|xox[abceprsuv]|xapp)-[A-Za-z0-9-]{10,}/g,
        replacement: "<SLACK_TOKEN_REDACTED>",
    },
    {
        pattern: /\bAIza[A-Za-z0-9_-]{35}(?![A-Za-z0-9])/g,
        replacement: "<GOOGLE_API_KEY_REDACTED>",
    },
    {
        // The scheme is kept and the credential after it is replaced, whether it is one
        // opaque token (`Bearer`, `Basic`) or a `name=value` list (`Digest`). The credential
        // has no minimum length once the header names it: `Basic YTpi` encodes `a:b`. The gap
        // after the scheme stays on the header line so the next header's name is not consumed.
        // A scheme is an HTTP token, so `Api-Key` and `AWS4-HMAC-SHA256` are schemes too.
        pattern: new RegExp(
            `\\b(Authorization\\s*:\\s*)([A-Za-z][A-Za-z0-9._-]*)([ \\t]+)(?:${AUTH_PARAM}(?:,\\s*${AUTH_PARAM})*|[A-Za-z0-9._~+/=-]+)`,
            "gi",
        ),
        replacement: (_full: string, prefix: string, scheme: string, space: string) =>
            `${prefix}${scheme}${space}<REDACTED:${scheme.toLowerCase()}>`,
    },
    {
        pattern: /\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/g,
        replacement: "<JWT_REDACTED>",
    },
    {
        // URL userinfo: the user name, which may be empty, stays to identify the account; the
        // password goes.
        pattern: /(:\/\/[^\s/:@"'`]*:)[^\s/@"'`]+@/g,
        replacement: "$1<REDACTED:password>@",
    },
    {
        // Every quoted `key: value` pair is read whole and classified afterwards, so a key of
        // any length or with the other quote character inside it (`"client's_api_key"`) is
        // seen; the value spans escaped quotes so `"a\"b"` is one value and not a leaked tail.
        pattern: new RegExp(
            `(?:"(${DOUBLE_QUOTED_BODY})"|'(${SINGLE_QUOTED_BODY})')(\\s*:\\s*)(?:"(${DOUBLE_QUOTED_BODY})"|'(${SINGLE_QUOTED_BODY})')`,
            "g",
        ),
        replacement: (
            full: string,
            doubleQuotedKey: string | undefined,
            singleQuotedKey: string | undefined,
            separator: string,
            doubleQuoted: string | undefined,
            singleQuoted: string | undefined,
        ) => {
            const key = doubleQuotedKey ?? singleQuotedKey ?? "";
            const value = doubleQuoted ?? singleQuoted ?? "";
            if (!textKeyNamesASecret(key) || isNonSecretScalarValue(value)) return full;
            const keyQuote = doubleQuotedKey === undefined ? "'" : '"';
            const valueQuote = doubleQuoted === undefined ? "'" : '"';
            return `${keyQuote}${key}${keyQuote}${separator}${valueQuote}<REDACTED:${redactionTypeForKey(key)}>${valueQuote}`;
        },
    },
];

/**
 * The key and separator of a `key=value` or `key: value` assignment. The key is one whole run of
 * key characters from where the run starts, so a key of any length is read once; the lookahead
 * admits only a run holding a vocabulary word. The optional `>` matches a provider marker
 * standing where the key was, such as `<HUGGINGFACE_TOKEN_REDACTED>=v` after the token pattern
 * absorbed a trailing `key` run. commentlint: allow(JUDGE)
 */
const ASSIGNMENT_KEY_PATTERN = new RegExp(
    `(?<!${ASSIGNMENT_KEY_RUN})(?=${ASSIGNMENT_KEY_RUN}*(?:${SECRET_WORD_ALTERNATION}))(${ASSIGNMENT_KEY_RUN}+>?)(\\s*=\\s*|:[ \\t]+)`,
    "gi",
);
/** The value of an assignment, read at the position the key pattern stopped. */
const ASSIGNMENT_VALUE_PATTERN = new RegExp(
    `"(${DOUBLE_QUOTED_BODY})"|'(${SINGLE_QUOTED_BODY})'|\`(${BACKTICK_QUOTED_BODY})\`|(${BARE_VALUE})`,
    "y",
);

/**
 * The value is read only once the key is known to name a secret, so a run of non-secret keys
 * (`author=…`) costs one key match each rather than one scan of everything to the next space,
 * and the scan then resumes at the value so an assignment inside it (`AUTHOR=https://x?api_key=v`)
 * is still seen. A quoted value keeps its quotes so `.env` and shell assignments stay parseable.
 */
function redactKeyedAssignments(text: string): string {
    ASSIGNMENT_KEY_PATTERN.lastIndex = 0;
    let out = "";
    let copied = 0;
    for (
        let match = ASSIGNMENT_KEY_PATTERN.exec(text);
        match;
        match = ASSIGNMENT_KEY_PATTERN.exec(text)
    ) {
        const [, key, separator] = match;
        if (!colonSeparatedKeyNamesASecret(key, separator)) continue;
        ASSIGNMENT_VALUE_PATTERN.lastIndex = ASSIGNMENT_KEY_PATTERN.lastIndex;
        const valueMatch = ASSIGNMENT_VALUE_PATTERN.exec(text);
        if (!valueMatch) continue;
        ASSIGNMENT_KEY_PATTERN.lastIndex = ASSIGNMENT_VALUE_PATTERN.lastIndex;
        const [, doubleQuoted, singleQuoted, backtickQuoted, bare] = valueMatch;
        const value = doubleQuoted ?? singleQuoted ?? backtickQuoted ?? bare ?? "";
        if (isNonSecretScalarValue(value)) continue;
        const quote =
            doubleQuoted !== undefined
                ? '"'
                : singleQuoted !== undefined
                  ? "'"
                  : backtickQuoted !== undefined
                    ? "`"
                    : "";
        out += `${text.slice(copied, match.index)}${key}${separator}${quote}<REDACTED:${redactionTypeForKey(key)}>${quote}`;
        copied = ASSIGNMENT_VALUE_PATTERN.lastIndex;
    }
    return out + text.slice(copied);
}

export function redactSecretText(value: string): string {
    let redacted = value;
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
    return redactKeyedAssignments(redacted);
}

/** Secrets go first: a username that is itself a vocabulary word (`token`) would otherwise become `<USER>` and take the key of `token=…` with it. commentlint: allow(JUDGE) */
export function sanitizeDiagnosticText(value: string): string {
    return sanitizePathString(redactSecretText(value));
}

// `sanitizeDiagnosticText` excludes shareability-only patterns.
const SHAREABILITY_SENSITIVE_PATTERNS: RegExp[] = [
    /\b[A-Za-z]:[\\/]Users[\\/][^\\/\s]+/i,
    /(?:^|\s)~\/[^\s]+/,
    // `sanitizeDiagnosticText` redacts inline `key: value` and `key=value` secrets because keyed redaction only processes config object keys.
    /\b(?:api[_-]?key|secret|token|password|passwd|pwd|client[_-]?secret|access[_-]?key)\b\s*[:=]\s*\S+/i,
    // Redact local and private endpoints because they identify the environment.
    // The bracketed arm handles `[::1]` because `\b` does not match before `[` at the start of input or after a non-word character.
    // The bare IPv6 loopback arm requires a non-word, non-colon, non-dot prefix to avoid matching suffixes of addresses such as `2001:db8::1`.
    /(?:\b(?:localhost|127\.0\.0\.1|0\.0\.0\.0)\b|\[::1\]|(?:^|[^\w:.])::1\b)(?::\d+)?/i,
    // The expanded spellings of the IPv6 loopback address, `0:0:0:0:0:0:0:1` through `0000:…:0001`.
    /\b(?:0{1,4}:){7}0{0,3}1\b/,
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

export function sanitizeConfigValue(value: unknown, keyPath: string[] = []): unknown {
    if (value === null || typeof value === "number" || typeof value === "boolean") return value;
    const key = keyPath.at(-1) ?? "";
    if (key && isSecretKey(key)) {
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
