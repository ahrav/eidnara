/// <reference types="bun-types" />

import { describe, expect, spyOn, test } from "bun:test";
import os from "node:os";

import vocabulary from "./fixtures/redaction-vocabulary-v1.json";
import {
    hasShareabilitySensitiveText,
    redactSecretText,
    SECRET_QUALIFIERS,
    SECRET_WORDS,
    sanitizeConfigValue,
    sanitizeDiagnosticText,
    sanitizePathString,
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
            "set api_key: <REDACTED:api_key>",
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

    test("redacts the whole value for unlisted Authorization schemes", () => {
        const apiKey = ["abc123", "secret"].join("");
        expect(redactSecretText(`Authorization: ApiKey ${apiKey}`)).toBe(
            "Authorization: <REDACTED:authorization>",
        );
        const sigv4 =
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host, Signature=fe5f80f77d5fa3beca038a248ff027";
        const redacted = redactSecretText(`Authorization: ${sigv4}`);
        expect(redacted).toBe("Authorization: <REDACTED:authorization>");
        expect(redacted).not.toContain("Signature=");
        // A known scheme with a credential too short for its rule still loses the credential.
        expect(redactSecretText("Authorization: Bearer abc")).toBe(
            "Authorization: <REDACTED:authorization>",
        );
    });

    test("does not treat a JSON `Authorization` key as a header", () => {
        expect(redactSecretText('"Authorization": "Bearer header-secret-value"')).toBe(
            '"Authorization": "<REDACTED:authorization>"',
        );
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
        expect(redactSecretText("passwd: hunter2")).toBe("passwd: <REDACTED:passwd>");
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

describe("sanitizeConfigValue key vocabulary", () => {
    test("treats passwd and pwd like password", () => {
        expect(
            sanitizeConfigValue({
                passwd: "hunter2",
                pwd: "hunter2",
                client_pwd: "hunter2",
                api_passwd: "hunter2",
                max_tokens: 4096,
            }),
        ).toEqual({
            passwd: "<REDACTED:passwd>",
            pwd: "<REDACTED:pwd>",
            client_pwd: "<REDACTED:client_pwd>",
            api_passwd: "<REDACTED:api_passwd>",
            max_tokens: 4096,
        });
    });
});

describe("sanitizePathString without a passwd entry", () => {
    test("still redacts home-style paths when the OS user lookup fails", () => {
        const spy = spyOn(os, "userInfo").mockImplementation(() => {
            throw Object.assign(new Error("uv_os_get_passwd returned ENOENT"), {
                code: "ERR_SYSTEM_ERROR",
            });
        });
        try {
            expect(sanitizePathString("/home/alice/project/eidnara.log")).toBe(
                "/home/<USER>/project/eidnara.log",
            );
            expect(spy).toHaveBeenCalled();
        } finally {
            spy.mockRestore();
        }
    });
});

describe("redactSecretText — escaped quotes inside secret values", () => {
    test("consumes escape sequences so the value is redacted through its real closing quote", () => {
        const secret = ["abc", '"def', "ghijk"].join("");
        const json = JSON.stringify({ password: secret });
        expect(redactSecretText(json)).toBe('{"password":"<REDACTED:password>"}');
        expect(redactSecretText(`token: "abc\\"def"`)).toBe('token: "<REDACTED:token>"');
        expect(redactSecretText(`API_KEY="abc\\"def"`)).toBe('API_KEY="<REDACTED:api_key>"');
    });
});

describe("redactSecretText — cookies, URL userinfo, and opposite quotes", () => {
    test("redacts Cookie and Set-Cookie header values whole", () => {
        expect(redactSecretText("Cookie: session=supersecret; theme=dark")).toBe(
            "Cookie: <REDACTED:cookie>",
        );
        expect(redactSecretText("set-cookie: sid=supersecret; HttpOnly")).toBe(
            "set-cookie: <REDACTED:cookie>",
        );
    });

    test("redacts userinfo in URLs and keeps scheme and host", () => {
        expect(redactSecretText("postgres://dbuser:s3cr3t@db.example.com/prod")).toBe(
            "postgres://<REDACTED:userinfo>@db.example.com/prod",
        );
        expect(redactSecretText("https://alice:password123@example.com/api")).toBe(
            "https://<REDACTED:userinfo>@example.com/api",
        );
        expect(redactSecretText("https://example.com/api?user=alice")).toBe(
            "https://example.com/api?user=alice",
        );
    });

    test("allows the opposite quote character inside a quoted secret value", () => {
        const secret = ["abc", "'def", "SECRET"].join("");
        expect(redactSecretText(JSON.stringify({ password: secret }))).toBe(
            '{"password":"<REDACTED:password>"}',
        );
        expect(redactSecretText(`token: 'it"s'`)).toBe("token: '<REDACTED:token>'");
        expect(redactSecretText(`API_KEY="it's"`)).toBe('API_KEY="<REDACTED:api_key>"');
    });
});

describe("sanitizeConfigValue unqualified password keys", () => {
    test("redacts password, secret, and credential keys regardless of prefix", () => {
        expect(
            sanitizeConfigValue({
                db_password: "hunter2",
                db_passwd: "hunter2",
                smtp_password: "hunter2",
                webhook_secret: "hunter2",
                ldap_credential: "hunter2",
                oauth_token: "tok_live_abc",
                signing_key: "k-abc",
                token_budget: 4096,
                cache_key: "sessions-v2",
                injection_budget_tokens: 12,
                pin_key_files: ["a.txt"],
            }),
        ).toEqual({
            db_password: "<REDACTED:password>",
            db_passwd: "<REDACTED:passwd>",
            smtp_password: "<REDACTED:password>",
            webhook_secret: "<REDACTED:secret>",
            ldap_credential: "<REDACTED:credential>",
            oauth_token: "<REDACTED:token>",
            signing_key: "<REDACTED:key>",
            token_budget: 4096,
            // A trailing `key` segment is treated as a secret name; the value is not worth the risk.
            cache_key: "<REDACTED:key>",
            injection_budget_tokens: 12,
            pin_key_files: ["a.txt"],
        });
    });
});

describe("redactSecretText — passphrases and CLI arguments", () => {
    test("consumes an unquoted multiword colon value", () => {
        expect(redactSecretText("password: correct horse battery staple")).toBe(
            "password: <REDACTED:password>",
        );
        expect(redactSecretText("token: abc123 def, temperature: 0.2")).toBe(
            "token: <REDACTED:token>, temperature: 0.2",
        );
    });

    test("redacts the value of secret-bearing CLI flags", () => {
        expect(redactSecretText("tool --api-key abc123secret --verbose")).toBe(
            "tool --api-key <REDACTED:api_key> --verbose",
        );
        expect(redactSecretText("curl --token abc123secret https://x")).toBe(
            "curl --token <REDACTED:token> https://x",
        );
        expect(redactSecretText("cmd --password correct-horse")).toBe(
            "cmd --password <REDACTED:password>",
        );
        // A following flag is not a value, and non-secret flags are untouched.
        expect(redactSecretText("cmd --password --verbose")).toBe("cmd --password --verbose");
        expect(redactSecretText("cmd --author alice")).toBe("cmd --author alice");
    });
});

describe("sanitizeConfigValue primitives under secret keys", () => {
    test("redacts numeric secrets but keeps null, booleans, and non-secret numbers", () => {
        expect(
            sanitizeConfigValue({
                password: 123456,
                pin_secret: 4242,
                // Numbers under `token`/`key` names stay: `*_tokens` budgets are counts, not credentials.
                api_key: 42,
                is_secret: true,
                secret: null,
                max_tokens: 4096,
                execute_threshold_tokens: 200000,
            }),
        ).toEqual({
            password: "<REDACTED:password>",
            pin_secret: "<REDACTED:secret>",
            api_key: 42,
            is_secret: true,
            secret: null,
            max_tokens: 4096,
            execute_threshold_tokens: 200000,
        });
    });
});

describe("sanitizePathString without a home directory", () => {
    test("falls back to path patterns when homedir() throws", () => {
        const spy = spyOn(os, "homedir").mockImplementation(() => {
            throw Object.assign(new Error("uv_os_homedir returned ENOENT"), {
                code: "ERR_SYSTEM_ERROR",
            });
        });
        try {
            expect(sanitizePathString("/home/alice/project/eidnara.log")).toBe(
                "/home/<USER>/project/eidnara.log",
            );
            expect(spy).toHaveBeenCalled();
        } finally {
            spy.mockRestore();
        }
    });
});

describe("redactSecretText — round-eight edge cases", () => {
    test("redacts a PEM private key block whole, terminated or not", () => {
        const pem = [
            "-----BEGIN PRIVATE KEY-----",
            "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC",
            "-----END PRIVATE KEY-----",
        ].join("\n");
        // The `=` rule then treats the marker as the assigned value, as it does for `AWS_ACCESS_KEY_ID=`.
        expect(redactSecretText(`PRIVATE_KEY=${pem} trailing`)).toBe(
            "PRIVATE_KEY=<REDACTED:private_key> trailing",
        );
        expect(redactSecretText(`key material:\n${pem}\ndone`)).toBe(
            "key material:\n<PRIVATE_KEY_REDACTED>\ndone",
        );
        const unterminated = "-----BEGIN RSA PRIVATE KEY-----\nMIIEvQIBADANBgkq\nhkiG9w0BAQEFAASC";
        expect(redactSecretText(`${unterminated}\nnext log line`)).toBe(
            "<PRIVATE_KEY_REDACTED>\nnext log line",
        );
    });

    test("redacts URL userinfo through the final at-sign", () => {
        expect(redactSecretText("https://user:p@ss@example.com/path")).toBe(
            "https://<REDACTED:userinfo>@example.com/path",
        );
        expect(redactSecretText("https://example.com/path?x=a@b")).toBe(
            "https://example.com/path?x=a@b",
        );
    });

    test("redacts numeric values under password-like keys in serialized objects", () => {
        expect(redactSecretText('{"password":123456}')).toBe('{"password":<REDACTED:password>}');
        expect(redactSecretText("pin_secret: 4242")).toBe("pin_secret: <REDACTED:secret>");
        expect(redactSecretText("DB_PASSWD=987654")).toBe("DB_PASSWD=<REDACTED:passwd>");
        // Numeric values under `api_key`/`token`/`key` stay visible, as the fixture requires.
        expect(redactSecretText('{"api_key":123456}')).toBe('{"api_key":123456}');
        expect(redactSecretText('"max_tokens": "4096"')).toBe('"max_tokens": "4096"');
    });

    test("consumes a quoted CLI argument value whole", () => {
        expect(redactSecretText('cmd --password "correct horse battery staple" --v')).toBe(
            'cmd --password "<REDACTED:password>" --v',
        );
        expect(redactSecretText("cmd --api-key 'a b' next")).toBe(
            "cmd --api-key '<REDACTED:api_key>' next",
        );
    });
});

describe("sanitizePathString with hostile OS identities", () => {
    test("ignores a root home directory", () => {
        const spy = spyOn(os, "homedir").mockImplementation(() => "/");
        try {
            expect(sanitizeDiagnosticText("postgres://user:pass@example.com/db")).toBe(
                "postgres://<REDACTED:userinfo>@example.com/db",
            );
            expect(spy).toHaveBeenCalled();
        } finally {
            spy.mockRestore();
        }
    });

    test("does not substitute a username that is a secret vocabulary word", () => {
        const spy = spyOn(os, "userInfo").mockImplementation(
            () => ({ username: "token" }) as ReturnType<typeof os.userInfo>,
        );
        try {
            expect(sanitizeDiagnosticText("token: abc123 at /home/token/app")).toBe(
                "token: <REDACTED:token>",
            );
            expect(sanitizeDiagnosticText("/home/token/app.log")).toBe("/home/<USER>/app.log");
            expect(spy).toHaveBeenCalled();
        } finally {
            spy.mockRestore();
        }
    });
});

describe("redactSecretText — round-nine edge cases", () => {
    test("does not let a username that is a substring of a key word erase the key", () => {
        const spy = spyOn(os, "userInfo").mockImplementation(
            () => ({ username: "pass" }) as ReturnType<typeof os.userInfo>,
        );
        try {
            expect(sanitizeDiagnosticText("password: hunter2 by /home/pass/x")).toBe(
                "password: <REDACTED:password>",
            );
            expect(sanitizeDiagnosticText("/srv/pass/app.log by pass")).toBe(
                "/srv/<USER>/app.log by <USER>",
            );
        } finally {
            spy.mockRestore();
        }
    });

    test("consumes the metadata lines of an unterminated encrypted PEM block", () => {
        const block = [
            "-----BEGIN RSA PRIVATE KEY-----",
            "Proc-Type: 4,ENCRYPTED",
            "DEK-Info: AES-128-CBC,0123456789ABCDEF0123456789ABCDEF",
            "",
            "MIIEpAIBAAKCAQEA7Vv3xkQzq0Fh6",
            "hkiG9w0BAQEFAASCBKcwggSjAgEA",
        ].join("\n");
        expect(redactSecretText(`${block}\nnext log line`)).toBe(
            "<PRIVATE_KEY_REDACTED>\nnext log line",
        );
    });
});

describe("redactSecretText — round-ten edge cases", () => {
    test("redacts cookies in serialized header objects and cookie-valued config", () => {
        expect(redactSecretText('{"Cookie":"session=supersecret"}')).toBe(
            '{"Cookie":"<REDACTED:cookie>"}',
        );
        expect(redactSecretText("headers: { Cookie: session=supersecret }")).toBe(
            "headers: { Cookie: <REDACTED:cookie> }",
        );
        expect(sanitizeConfigValue({ headers: { Cookie: "session=supersecret" } })).toEqual({
            headers: { Cookie: "<REDACTED:cookie>" },
        });
    });

    test("redacts a flat array or object value under a secret key", () => {
        expect(redactSecretText('{"passwords":["hunter2","second-secret"]}')).toBe(
            '{"passwords":<REDACTED:passwords>}',
        );
        expect(redactSecretText('{"authorization":["Opaque abc123secret"]}')).toBe(
            '{"authorization":<REDACTED:authorization>}',
        );
        expect(redactSecretText('{"api_key":{"value":"abc"}} tail')).toBe(
            '{"api_key":<REDACTED:api_key>} tail',
        );
    });
});
