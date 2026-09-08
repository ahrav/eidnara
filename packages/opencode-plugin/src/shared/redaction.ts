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
// `cookie` is a label word for key matching only; the fixture-pinned `SECRET_WORDS` stays unchanged.
const SECRET_WORD_ALIASES = ["cookie"];
const SECRET_SEGMENT_PATTERN = new RegExp(
    `^(?:${[...SECRET_WORDS, ...SECRET_WORD_ALIASES].map((w) => `${w}s?`).join("|")})$`,
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
/** The characters of an HTTP `token`: an auth scheme or parameter name such as `AWS4-HMAC-SHA256` or `Foo+Bar`. */
const HTTP_TOKEN = "[A-Za-z0-9!#$%&*+.^_|~-]+";
/** HTTP authentication schemes whose credential follows as the next word. */
const AUTH_SCHEME_NAMES = "(?:Bearer|Basic|Digest|Token|ApiKey|Api-Key|Negotiate|NTLM|Hawk|OAuth)";
const AUTH_SCHEME_PATTERN = new RegExp(`^${AUTH_SCHEME_NAMES}$`, "i");
/** One `name=value` parameter of a `Digest`-style header, with the whitespace RFC 7235 allows around `=`; a quoted value reads escape pairs as one character so `username="a\"b"` does not end at the escaped quote. commentlint: allow(JUDGE) */
const AUTH_PARAM = String.raw`${HTTP_TOKEN}\s*=\s*(?:"${DOUBLE_QUOTED_BODY}"|[^\s,"]+)`;
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

// A number under `password`, `secret`, `credential`, or `cookie` is a PIN or numeric token; one under
// `token` or `key` is a count (`max_tokens: 4096`). `isSecretKey` alone cannot tell them apart.
const PIN_BEARING_SEGMENT_PATTERN = /^(?:password|passwd|pwd|secret|credential|cookie)s?$/;

/** Whether a scalar under a secret-shaped key stays visible: booleans and null always, numbers unless the key names a password-like secret. */
export function keepsScalarValue(key: string, value: string): boolean {
    if (!isNonSecretScalarValue(value)) return false;
    const v = value.trim();
    if (v === "true" || v === "false" || v === "null" || v === "undefined") return true;
    return !keySegments(key).some((segment) => PIN_BEARING_SEGMENT_PATTERN.test(segment));
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
    if (keySegments(key).includes("authorization")) return false;
    if (!separator.includes(":")) return textKeyNamesASecret(key);
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
/** A home-directory name that may hold spaces (`John Doe`), read only when a separator follows its last word; the words exclude the characters Windows forbids in a name so a run cannot cross into a second path. commentlint: allow(JUDGE) */
const SPACED_HOME_SEGMENT = String.raw`[^\\/:*?"<>|\s]+(?: [^\\/:*?"<>|\s]+)*(?=[\\/])`;
const HOME_NAME = `(?:${SPACED_HOME_SEGMENT}|${HOME_SEGMENT})`;

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
    // The drive and UNC form goes first: `C:/Users/John Doe/x` also matches `/Users/John`, which
    // would leave ` Doe` behind.
    sanitized = sanitized
        .replace(
            new RegExp(`((?:[A-Za-z]:|\\\\\\\\[^\\\\/\\s]+)[\\\\/]Users[\\\\/])${HOME_NAME}`, "gi"),
            "$1<USER>",
        )
        .replace(new RegExp(`/Users/${HOME_NAME}`, "gi"), "/Users/<USER>")
        .replace(new RegExp(`/home/${HOME_NAME}`, "gi"), "/home/<USER>");
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

function authorizationReplacement(
    _full: string,
    prefix: string,
    scheme: string | undefined,
    space: string | undefined,
): string {
    return scheme !== undefined && space !== undefined
        ? `${prefix}${scheme}${space}<REDACTED:${scheme.toLowerCase()}>`
        : `${prefix}<REDACTED:authorization>`;
}

function quotedAwareAuthorizationReplacement(
    full: string,
    prefix: string,
    scheme: string | undefined,
    space: string | undefined,
    quote: string | undefined,
): string {
    return quote !== undefined
        ? `${prefix}${quote}<REDACTED:authorization>${quote}`
        : authorizationReplacement(full, prefix, scheme, space);
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
        pattern: /\b(?:xoxe\.xox[bp]|xox[abceoprsuv]|xapp)-[A-Za-z0-9-]{10,}/g,
        replacement: "<SLACK_TOKEN_REDACTED>",
    },
    {
        pattern: /\bAIza[A-Za-z0-9_-]{35}(?![A-Za-z0-9])/g,
        replacement: "<GOOGLE_API_KEY_REDACTED>",
    },
    {
        pattern: /\b(?:sk|rk)_(?:test|live|prod)_[A-Za-z0-9]{10,99}(?![A-Za-z0-9])/g,
        replacement: "<STRIPE_KEY_REDACTED>",
    },
    {
        // Header form. The scheme is kept and the credential after it is replaced, whether it is
        // one opaque token (`Bearer`, `Basic`) or a `name=value` list (`Digest`). The credential
        // has no minimum length once the header names it: `Basic YTpi` encodes `a:b`. The gap
        // after the scheme stays on the header line so the next header's name is not consumed.
        // A lone value that is not a scheme name and ends the line is a credential with no scheme.
        // A quoted value (`Authorization: "Bearer x"` in YAML or JSON-like text) is replaced whole
        // inside its quotes.
        pattern: new RegExp(
            `\\b(Authorization\\s*:\\s*)(?:(${HTTP_TOKEN})([ \\t]+)(?:${AUTH_PARAM}(?:\\s*,\\s*${AUTH_PARAM})*|[A-Za-z0-9._~+/=-]+)|(["'])(?:${DOUBLE_QUOTED_BODY}|[^'\\n]*)\\4|(?!${AUTH_SCHEME_NAMES}(?![A-Za-z0-9]))[A-Za-z0-9._~+/=-]+(?=[ \\t]*(?:$|[\\r\\n])))`,
            "gi",
        ),
        replacement: quotedAwareAuthorizationReplacement,
    },
    {
        // Assignment form (`Authorization=Bearer x` in an environment dump). Only a known scheme
        // takes the following token or parameter list as its credential; any other first token is
        // the credential itself and ends at whitespace, so `Authorization=abc123 OTHER=v` keeps `OTHER=v`.
        pattern: new RegExp(
            `\\b(Authorization\\s*=\\s*)(?:(${AUTH_SCHEME_NAMES})([ \\t]+)(?:${AUTH_PARAM}(?:\\s*,\\s*${AUTH_PARAM})*|[A-Za-z0-9._~+/=-]+)|(["'])(?:${DOUBLE_QUOTED_BODY}|[^'\\n]*)\\4|[^\\s]+)`,
            "gi",
        ),
        replacement: quotedAwareAuthorizationReplacement,
    },
    {
        // `Cookie` carries `name=value` pairs and every value is a credential; `Set-Cookie`
        // carries one pair followed by attributes (`Path=/`), so only its first value goes.
        pattern: /\b(Set-Cookie|Cookie)(\s*:[ \t]*)([^\r\n]+)/gi,
        replacement: (_full: string, header: string, separator: string, cookies: string) =>
            `${header}${separator}${
                header.toLowerCase() === "cookie"
                    ? cookies.replace(/=[^;]*/g, "=<REDACTED:cookie>")
                    : cookies.replace(/^([^=;]+=)[^;]*/, "$1<REDACTED:cookie>")
            }`,
    },
    {
        pattern: /\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/g,
        replacement: "<JWT_REDACTED>",
    },
    {
        // URL userinfo, in a full or protocol-relative (`//user:pw@host`) URL: the user name,
        // which may be empty, stays to identify the account; the password goes.
        // The password runs to the last `@` before the host so a password containing `@` is
        // redacted whole.
        pattern: /(\/\/[^\s/:@"'`]*:)[^\s/"'`]+@/g,
        replacement: "$1<REDACTED:password>@",
    },
    // `--api-key abc` / `--password "a b"`: the value after a secret-bearing flag; a value beginning
    // with `-` is the next flag.
    {
        pattern: new RegExp(
            String.raw`(^|\s)(--?[A-Za-z0-9-]*(?:${SECRET_WORD_ALTERNATION}|cookie)[A-Za-z0-9-]*)(\s+)(?:"(${DOUBLE_QUOTED_BODY})"|'([^'\n]*)'|([^\s"'-]\S*))`,
            "gi",
        ),
        replacement: (
            full: string,
            lead: string,
            flag: string,
            space: string,
            doubleQuoted: string | undefined,
            singleQuoted: string | undefined,
            bare: string | undefined,
        ) => {
            const key = flag.replace(/^--?/, "");
            const value = doubleQuoted ?? singleQuoted ?? bare ?? "";
            if (!textKeyNamesASecret(key) || isNonSecretScalarValue(value)) return full;
            const q = doubleQuoted !== undefined ? '"' : singleQuoted !== undefined ? "'" : "";
            return `${lead}${flag}${space}${q}<REDACTED:${redactionTypeForKey(key)}>${q}`;
        },
    },
];

const BARE_WORD_PATTERN = /[A-Za-z0-9_][A-Za-z0-9_.-]*/y;

function isWordStart(code: number): boolean {
    return (
        (code >= 48 && code <= 57) || // 0-9
        (code >= 65 && code <= 90) || // A-Z
        (code >= 97 && code <= 122) || // a-z
        code === 95 // _
    );
}

/**
 * Reads the `[…]` or `{…}` value opening at `start`: the index just past its closing bracket
 * and whether it carries text. A value nothing closes runs to the end of the text and counts as
 * text, so a credential that cannot be inspected is redacted rather than shown. Text is a
 * non-empty quoted string or a bare word that is neither a key (followed by `:`) nor a scalar,
 * which is how the Rust key gate's `carries_text` reads a value: `{"input": 100, "output": 200}`
 * and `[1, 2]` are counts, `["hunter2"]` and `{"value": "hunter2"}` are credentials.
 */
function structuredValue(text: string, start: number): { end: number; carriesText: boolean } {
    let depth = 0;
    let carriesText = false;
    const isKey = (from: number): boolean => {
        let at = from;
        while (at < text.length && /[ \t\r\n]/.test(text[at])) at++;
        return text[at] === ":";
    };
    for (let at = start; at < text.length; at++) {
        const code = text.charCodeAt(at);
        if (code === 34 || code === 39) {
            // `"` or `'`
            const close = quotedStringEnd(text, at);
            if (close < 0) return { end: text.length, carriesText: true };
            if (!carriesText && close - at > 2 && !isKey(close)) carriesText = true;
            at = close - 1;
        } else if (!carriesText && isWordStart(code)) {
            // Once text is found only the brackets matter, so words are no longer classified.
            BARE_WORD_PATTERN.lastIndex = at;
            const word = BARE_WORD_PATTERN.exec(text)?.[0] ?? text[at];
            const after = at + word.length;
            if (!isKey(after) && !isNonSecretScalarValue(word)) carriesText = true;
            at = after - 1;
        } else if (code === 91 || code === 123) {
            // `[` or `{`
            depth++;
        } else if (code === 93 || code === 125) {
            // `]` or `}`
            depth--;
            if (depth === 0) return { end: at + 1, carriesText };
        }
    }
    return { end: text.length, carriesText: true };
}

/** The index just past the quote closing the string that opens at `start`, or -1 when none does; escape pairs are one character. */
function quotedStringEnd(text: string, start: number): number {
    const quote = text[start];
    for (let at = start + 1; at < text.length; at++) {
        if (text[at] === "\\") at++;
        else if (text[at] === quote) return at + 1;
    }
    return -1;
}

/**
 * Returns the index of the line break that ends a YAML block scalar whose indicator ends at
 * `indicatorEnd`: the body is every following line that is blank or indented deeper than
 * `keyIndent`. Returns the text length when the block runs to the end.
 */
function blockScalarEnd(text: string, indicatorEnd: number, keyIndent: number): number {
    let end = text.indexOf("\n", indicatorEnd);
    if (end < 0) return text.length;
    while (end < text.length) {
        const lineStart = end + 1;
        const lineEnd = text.indexOf("\n", lineStart);
        const line = text.slice(lineStart, lineEnd < 0 ? text.length : lineEnd);
        if (line.trim() !== "" && line.length - line.trimStart().length <= keyIndent) return end;
        if (lineEnd < 0) return text.length;
        end = lineEnd;
    }
    return end;
}

/**
 * A quoted key and its separator. Every quoted `key: value` pair is read whole and classified
 * afterwards, so a key of any length or with the other quote character inside it
 * (`"client's_api_key"`) is seen.
 */
const QUOTED_KEY_PATTERN = new RegExp(
    `(?:"(${DOUBLE_QUOTED_BODY})"|'(${SINGLE_QUOTED_BODY})')(\\s*:\\s*)`,
    "g",
);
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
/** A TOML multi-line string spans lines; the lazy body stops at the first closing triple quote. */
const TRIPLE_QUOTED_SEGMENT = String.raw`"""[\s\S]*?"""|'''[\s\S]*?'''`;
const QUOTED_SEGMENT = `${TRIPLE_QUOTED_SEGMENT}|"${DOUBLE_QUOTED_BODY}"|'${SINGLE_QUOTED_BODY}'|\`${BACKTICK_QUOTED_BODY}\``;
/**
 * The value of an assignment, read at the position the key pattern stopped: one shell word,
 * so adjacent segments (`'before''AFTER'`, `$'x'`) are one value. A bare run may follow a
 * quoted segment only when it does not open with list or pair punctuation, so `"x", "y"` and
 * `"x":"y"` stay two values.
 */
const SHELL_WORD_PATTERN = new RegExp(
    `(?:${QUOTED_SEGMENT}|${BARE_VALUE})(?:${QUOTED_SEGMENT}|(?![,;:)\\]}])${BARE_VALUE})*`,
    "y",
);
const SINGLE_SEGMENT_PATTERN = new RegExp(`^(?:${QUOTED_SEGMENT})$`);
/**
 * A YAML plain scalar runs to the end of its line, so `password: correct horse` is one value;
 * it stops before a comment, before a flow separator (`, ` or `: `), and before a flow closer,
 * so `{api_key: process.env.KEY, other: 1}` keeps its second field.
 */
const YAML_PLAIN_SCALAR_PATTERN = /(?:(?![,:]\s|\s#|[}\]])[^\n])+/y;
const BLOCK_SCALAR_INDICATOR = /^[|>][-+0-9]*$/;
/** After a block indicator only whitespace or a comment may follow on the line; `token: | then` is prose. */
const REST_OF_LINE_IS_BLANK = /^[ \t]*(?:#.*)?$/;

function lineEndAfter(text: string, from: number): number {
    const lineEnd = text.indexOf("\n", from);
    return lineEnd < 0 ? text.length : lineEnd;
}

function unquoteSegment(segment: string): string {
    const width = /^(?:"""|''')/.test(segment) ? 3 : 1;
    return segment.slice(width, -width);
}

/**
 * Reads the value that follows a credential key at `valueStart` and returns where it ends and
 * what replaces it; the replacement is null for a scalar that stays, and the result is null when
 * nothing is there. Callers resume after the value in both cases. A `[…]`
 * or `{…}` value is read to its close. In the YAML `key: value` form a block indicator that
 * ends its line takes the indented lines after it, and a bare value is a plain scalar running
 * to the end of the line; otherwise a bare value is one shell word. A quoted value keeps its
 * quotes so `.env`, shell, and JSON stay parseable.
 */
function keyedValue(
    text: string,
    valueStart: number,
    marker: string,
    yaml: boolean,
    keyIndent: number,
): { end: number; replacement: string | null } | null {
    const first = text[valueStart];
    if (first === "[" || first === "{") {
        // A value with no text holds no secret, so the scan skips it whole either way.
        const structured = structuredValue(text, valueStart);
        return { end: structured.end, replacement: structured.carriesText ? `"${marker}"` : null };
    }
    SHELL_WORD_PATTERN.lastIndex = valueStart;
    const wordMatch = SHELL_WORD_PATTERN.exec(text);
    if (!wordMatch) return null;
    const word = wordMatch[0];
    const wordEnd = SHELL_WORD_PATTERN.lastIndex;
    if (yaml && !/^["'`]/.test(word)) {
        if (
            BLOCK_SCALAR_INDICATOR.test(word) &&
            REST_OF_LINE_IS_BLANK.test(text.slice(wordEnd, lineEndAfter(text, wordEnd)))
        ) {
            return { end: blockScalarEnd(text, wordEnd, keyIndent), replacement: marker };
        }
        YAML_PLAIN_SCALAR_PATTERN.lastIndex = valueStart;
        const scalar = (YAML_PLAIN_SCALAR_PATTERN.exec(text)?.[0] ?? word).trimEnd();
        const end = valueStart + scalar.length;
        return { end, replacement: isNonSecretScalarValue(scalar) ? null : marker };
    }
    if (SINGLE_SEGMENT_PATTERN.test(word)) {
        const quoted = `${word[0]}${marker}${word[0]}`;
        return {
            end: wordEnd,
            replacement: isNonSecretScalarValue(unquoteSegment(word)) ? null : quoted,
        };
    }
    if (/^["'`]/.test(word)) return { end: wordEnd, replacement: `${word[0]}${marker}${word[0]}` };
    if (AUTH_SCHEME_PATTERN.test(word)) {
        // `auth=Bearer abc`: the scheme is a label, and the credential is the following word.
        SHELL_WORD_PATTERN.lastIndex = wordEnd;
        const credential = /^[ \t]+/.exec(text.slice(wordEnd));
        if (credential) {
            SHELL_WORD_PATTERN.lastIndex = wordEnd + credential[0].length;
            const credentialMatch = SHELL_WORD_PATTERN.exec(text);
            if (credentialMatch && !/^-/.test(credentialMatch[0])) {
                return {
                    end: SHELL_WORD_PATTERN.lastIndex,
                    replacement: `${word}${credential[0]}<REDACTED:${word.toLowerCase()}>`,
                };
            }
        }
    }
    return { end: wordEnd, replacement: isNonSecretScalarValue(word) ? null : marker };
}

function keyIndentAt(text: string, keyStart: number): number {
    return keyStart - (text.lastIndexOf("\n", keyStart) + 1);
}

/**
 * Redacts the value under a quoted credential key, whether it is a quoted string, a `[…]`/`{…}`
 * value whose nested text would otherwise stay visible (`{"credentials":{"value":"x"}}`), or a
 * YAML plain or block scalar (`"password": hunter2`).
 */
function redactQuotedKeys(text: string): string {
    QUOTED_KEY_PATTERN.lastIndex = 0;
    let out = "";
    let copied = 0;
    for (let match = QUOTED_KEY_PATTERN.exec(text); match; match = QUOTED_KEY_PATTERN.exec(text)) {
        const [, doubleQuotedKey, singleQuotedKey, separator] = match;
        const key = doubleQuotedKey ?? singleQuotedKey ?? "";
        if (!textKeyNamesASecret(key)) continue;
        const valueStart = QUOTED_KEY_PATTERN.lastIndex;
        const value = keyedValue(
            text,
            valueStart,
            `<REDACTED:${redactionTypeForKey(key)}>`,
            /^\s*:[ \t]+$/.test(separator),
            keyIndentAt(text, match.index),
        );
        if (!value) continue;
        QUOTED_KEY_PATTERN.lastIndex = value.end;
        if (value.replacement === null) continue;
        const keyQuote = doubleQuotedKey === undefined ? "'" : '"';
        out += `${text.slice(copied, match.index)}${keyQuote}${key}${keyQuote}${separator}${value.replacement}`;
        copied = value.end;
    }
    return out + text.slice(copied);
}

/**
 * The value is read only once the key is known to name a secret, so a run of non-secret keys
 * (`author=…`) costs one key match each rather than one scan of everything to the next space,
 * and the scan then resumes at the value so an assignment inside it (`AUTHOR=https://x?api_key=v`)
 * is still seen.
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
        const value = keyedValue(
            text,
            ASSIGNMENT_KEY_PATTERN.lastIndex,
            `<REDACTED:${redactionTypeForKey(key)}>`,
            separator.includes(":"),
            keyIndentAt(text, match.index),
        );
        if (!value) continue;
        ASSIGNMENT_KEY_PATTERN.lastIndex = value.end;
        if (value.replacement === null) continue;
        out += `${text.slice(copied, match.index)}${key}${separator}${value.replacement}`;
        copied = value.end;
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
    return redactKeyedAssignments(redactQuotedKeys(redacted));
}

/** URL queries and fragments can contain tokens under arbitrary parameter names, so diagnostic output replaces them with `<REDACTED:query>`. */
const URL_QUERY_PATTERN = /\b([a-z][a-z0-9+.-]*:\/\/[^\s?#"'`]+)[?#][^\s"'`]*/gi;

export function sanitizeDiagnosticText(value: string): string {
    return sanitizePathString(redactSecretText(value)).replace(
        URL_QUERY_PATTERN,
        (_full, base: string) => `${base}?<REDACTED:query>`,
    );
}

// `sanitizeDiagnosticText` excludes shareability-only patterns.
const SHAREABILITY_SENSITIVE_PATTERNS: RegExp[] = [
    /\b[A-Za-z]:[\\/]Users[\\/][^\\/\s]+/i,
    /(?:^|\s)~\/[^\s]+/,
    // `sanitizeDiagnosticText` redacts inline `key: value` and `key=value` secrets because keyed redaction only processes config object keys.
    /\b(?:api[_-]?key|secret|token|password|passwd|client[_-]?secret|access[_-]?key)\b\s*[:=]\s*\S+/i,
    // Redact local and private endpoints because they identify the environment.
    // The bracketed arm handles `[::1]` because `\b` does not match before `[` at the start of input or after a non-word character.
    // The bare IPv6 loopback arm requires a non-word, non-colon, non-dot prefix to avoid matching suffixes of addresses such as `2001:db8::1`.
    /(?:\b(?:localhost|127\.0\.0\.1|0\.0\.0\.0)\b|\[::1\]|(?:^|[^\w:.])::1\b)(?::\d+)?/i,
    // Every other spelling of the IPv6 loopback address: all eight groups written out, or zero
    // groups on either side of one `::`. The lookarounds keep `2001:0:0::1` from matching on its
    // tail.
    /(?<![0-9a-f:])(?:(?:0{1,4}:){7}|(?:0{1,4}:){1,6}:(?:0{1,4}:){0,6}|::(?:0{1,4}:){1,6})0{0,3}1(?![0-9a-f:])/i,
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
        // Record keys under a prompt field (`tool_descriptions`) are user data as well.
        return Object.fromEntries(
            Object.entries(value).map(([entryKey, entry]) => [
                sanitizeDiagnosticText(entryKey),
                redactProse(entry),
            ]),
        );
    }
    return value;
}

export function sanitizeConfigValue(value: unknown, keyPath: string[] = []): unknown {
    const key = keyPath.at(-1) ?? "";
    if (PROMPT_KEY_PATTERN.test(key)) return redactProse(value);
    // Numbers and booleans stay under any key, matching the Rust key gate in `crates/context-core`.
    if (value === null || typeof value === "number" || typeof value === "boolean") return value;
    if (key && isSecretKey(key)) {
        return `<REDACTED:${redactionTypeForKey(key)}>`;
    }
    if (typeof value === "string") return sanitizeDiagnosticText(value);
    if (Array.isArray(value)) {
        return value.map((entry, index) => sanitizeConfigValue(entry, [...keyPath, String(index)]));
    }
    if (value && typeof value === "object") {
        // Record keys are user data too (`permission.bash` maps command patterns, which can carry
        // paths or credentials), so they pass through the same text sanitizer as values.
        return Object.fromEntries(
            Object.entries(value).map(([entryKey, entry]) => [
                sanitizeDiagnosticText(entryKey),
                sanitizeConfigValue(entry, [...keyPath, entryKey]),
            ]),
        );
    }
    return value;
}
