/// <reference types="bun-types" />

import { describe, expect, test } from "bun:test";

import vocabulary from "./fixtures/redaction-vocabulary-v1.json";
import {
    hasShareabilitySensitiveText,
    redactSecretText,
    SECRET_QUALIFIERS,
    SECRET_WORDS,
} from "./redaction";

describe("redaction vocabulary fixture", () => {
    test("matches the cross-runtime label vocabulary", () => {
        expect(SECRET_WORDS).toEqual(vocabulary.label_words);
        expect([...SECRET_QUALIFIERS]).toEqual(vocabulary.label_qualifiers);
    });

    test("matches the cross-runtime redacted output", () => {
        for (const fixture of vocabulary.cases) {
            expect(redactSecretText(fixture.input)).toBe(fixture.expected_redacted);
            for (const detection of fixture.detections) {
                const bytes = Buffer.from(fixture.input, "utf8");
                expect(detection.offset + detection.length).toBeLessThanOrEqual(bytes.length);
                expect(detection.secret_type.length).toBeGreaterThan(0);
            }
        }
    });

    test("preserves scalar exemptions and documents known misses", () => {
        for (const unchanged of [...vocabulary.exemptions, ...vocabulary.known_misses]) {
            expect(redactSecretText(unchanged)).toBe(unchanged);
        }
    });
});

describe("redactSecretText — token counts and scalar diagnostics stay visible", () => {
    test("keeps numeric/boolean values whose key merely contains a secret word", () => {
        // These log shapes are counts/flags, not secrets, so they must stay readable.
        expect(redactSecretText("tokens.input=45000 cache.read=0 cache.write=0")).toBe(
            "tokens.input=45000 cache.read=0 cache.write=0",
        );
        expect(redactSecretText("hasUsageTokens=true")).toBe("hasUsageTokens=true");
        expect(redactSecretText("totalInputTokens=132000")).toBe("totalInputTokens=132000");
        expect(redactSecretText("max_tokens=4096")).toBe("max_tokens=4096");
    });

    test("keeps quoted numeric values matched only on the key word", () => {
        expect(redactSecretText('"max_tokens": "4096"')).toBe('"max_tokens": "4096"');
    });

    test("still redacts real secret string values", () => {
        // Key-based matching exempts numeric and boolean scalar values.
        const syntheticApiKey = "sk-abc123XYZ" + "secretvalue"; // gitleaks:allow redaction-test fixture
        expect(redactSecretText(`api_key=${syntheticApiKey}`)).toContain("<REDACTED:");
        expect(redactSecretText(`api_key=${syntheticApiKey}`)).not.toContain(syntheticApiKey);
        const syntheticAuthToken = "tok_live_" + "9f8e7d6c5b";
        expect(redactSecretText(`"auth_token": "${syntheticAuthToken}"`)).toContain("<REDACTED:");
    });

    test("value-shaped secret patterns still fire independent of key name", () => {
        // A bearer/JWT value is caught by its own pattern even if its key is bland.
        expect(redactSecretText("Authorization: Bearer abc123def456ghi789")).toContain(
            "<REDACTED:bearer>",
        );
        const syntheticJwt = ["eyJhbGciOi", "eyJzdWIiOiIx", "SflKxwRJSMeKKF2QT4"].join(".");
        expect(redactSecretText(`blob=${syntheticJwt}`)).toContain("<JWT_REDACTED>");
    });
});

describe("redactSecretText — unquoted colon assignments and quoted env values", () => {
    test("redacts `key: value` with an unquoted key", () => {
        expect(redactSecretText("token: abc123")).toBe("token: <REDACTED:token>");
        expect(redactSecretText("set api_key: sk-live-abc in the env")).toBe(
            "set api_key: <REDACTED:api_key> in the env",
        );
        expect(redactSecretText('password: "hunter two"')).toBe('password: "<REDACTED:password>"');
    });

    test('redacts quoted `KEY="value"` assignments and keeps the quotes', () => {
        expect(redactSecretText('API_KEY="abc def"')).toBe('API_KEY="<REDACTED:api_key>"');
        expect(redactSecretText("export TOKEN='abc def'")).toBe("export TOKEN='<REDACTED:token>'");
    });

    test("keeps numeric and boolean colon values whose key merely contains a secret word", () => {
        expect(redactSecretText("tokens: 4096")).toBe("tokens: 4096");
        expect(redactSecretText("max_tokens: 4096, temperature: 0.2")).toBe(
            "max_tokens: 4096, temperature: 0.2",
        );
        expect(redactSecretText("hasUsageTokens: true")).toBe("hasUsageTokens: true");
    });

    test("keeps the scheme word for Authorization headers and redacts the credential", () => {
        // Secret scanners flag contiguous `<scheme> <token>` literals.
        const bearer = ["abc123", "def456", "ghi789"].join("");
        expect(redactSecretText(`Authorization: Bearer ${bearer}`)).toBe(
            "Authorization: Bearer <REDACTED:bearer>",
        );
        const basic = Buffer.from("user:pass" + "word").toString("base64");
        expect(redactSecretText(`Authorization: Basic ${basic}`)).toBe(
            "Authorization: Basic <REDACTED:basic>",
        );
        const token = ["abcdefghij", "1234567890"].join("");
        expect(redactSecretText(`authorization: token ${token}`)).toBe(
            "authorization: token <REDACTED:token>",
        );
        const negotiate = ["YIIB", "kwYGKwYBBQUC", "oIIBhzCCAYM"].join("");
        expect(redactSecretText(`Authorization: Negotiate ${negotiate}`)).toBe(
            "Authorization: Negotiate <REDACTED:negotiate>",
        );
    });

    test("redacts the whole Digest parameter list", () => {
        const digest = [
            'username="alice"',
            'realm="api"',
            'nonce="dcd98b7102dd2f0e"',
            'response="6629fae49393a05397450978507c4ef1"',
        ].join(", ");
        const redacted = redactSecretText(`Authorization: Digest ${digest} trailing`);
        expect(redacted).toBe("Authorization: Digest <REDACTED:digest>");
        expect(redacted).not.toContain("alice");
        expect(redacted).not.toContain("6629fae4");
    });

    test("a bare `key:` at end of line does not consume the next line", () => {
        expect(redactSecretText("token:\nnext line stays")).toBe("token:\nnext line stays");
    });

    test("keeps keys that merely contain a secret word as a substring", () => {
        expect(redactSecretText("author: Alice")).toBe("author: Alice");
        expect(redactSecretText("keyboard: qwerty")).toBe("keyboard: qwerty");
        expect(redactSecretText("tokenizer: cl100k_base")).toBe("tokenizer: cl100k_base");
        expect(redactSecretText("monkey=banana")).toBe("monkey=banana");
        expect(redactSecretText('"authored": "by alice"')).toBe('"authored": "by alice"');
    });

    test("still redacts fused compounds and common abbreviations", () => {
        expect(redactSecretText("apikey: abc123")).toBe("apikey: <REDACTED:secret>");
        expect(redactSecretText("accessToken=abc123")).toBe("accessToken=<REDACTED:access_token>");
        expect(redactSecretText("passwd: hunter2")).toBe("passwd: <REDACTED:secret>");
        expect(redactSecretText("DB_PASSWORD=hunter2")).toBe("DB_PASSWORD=<REDACTED:password>");
    });

    test("a colon value stops at punctuation that closes a structure", () => {
        expect(redactSecretText("{token: abc123}")).toBe("{token: <REDACTED:token>}");
        expect(redactSecretText("token: abc123; next")).toBe("token: <REDACTED:token>; next");
    });
});

describe("hasShareabilitySensitiveText", () => {
    test("safe project facts are shareable", () => {
        expect(
            hasShareabilitySensitiveText(
                "The historian runs as a hidden subagent and never busts the prompt cache.",
            ),
        ).toBe(false);
        expect(
            hasShareabilitySensitiveText("Migration v45 adds the retrospective watermark column."),
        ).toBe(false);
    });

    test("flags inline key:value / key=value secrets the keyed redactor misses in prose", () => {
        expect(hasShareabilitySensitiveText("Set api_key: sk-live-abc123 in the env.")).toBe(true); // gitleaks:allow redaction-test fixture
        expect(hasShareabilitySensitiveText("password=hunter2 for the staging box")).toBe(true);
        expect(hasShareabilitySensitiveText("client_secret = abcdef in the OAuth app")).toBe(true);
    });

    test("flags Windows forward-slash home paths", () => {
        expect(hasShareabilitySensitiveText("logs are under C:/Users/ufuk/AppData/tool")).toBe(
            true,
        );
    });

    test("flags ~/ rooted personal paths", () => {
        expect(hasShareabilitySensitiveText("config lives at ~/.config/opencode/x.jsonc")).toBe(
            true,
        );
    });

    test("flags local / private endpoints", () => {
        expect(hasShareabilitySensitiveText("embed endpoint is http://localhost:1234/v1")).toBe(
            true,
        );
        expect(hasShareabilitySensitiveText("the box answers on 127.0.0.1:8080")).toBe(true);
        expect(hasShareabilitySensitiveText("LAN host 192.168.1.42 runs the model")).toBe(true);
        expect(hasShareabilitySensitiveText("internal 10.0.0.5 endpoint")).toBe(true);
    });

    test("a public IP / port alone is not flagged by the private-range rules", () => {
        // 8.8.8.8 is public; no private-range or localhost pattern should match.
        expect(hasShareabilitySensitiveText("DNS resolver at 8.8.8.8")).toBe(false);
    });
});
