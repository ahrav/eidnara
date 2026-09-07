/// <reference types="bun-types" />

import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";
import * as os from "node:os";

import vocabulary from "./fixtures/redaction-vocabulary-v1.json";
import {
    hasShareabilitySensitiveText,
    isSecretKey,
    redactSecretText,
    SECRET_QUALIFIERS,
    SECRET_WORDS,
    sanitizeConfigValue,
    sanitizeDiagnosticText,
} from "./redaction";

describe("redaction vocabulary fixture", () => {
    test("matches this package's label vocabulary", () => {
        expect(SECRET_WORDS).toEqual(vocabulary.label_words);
        expect([...SECRET_QUALIFIERS]).toEqual(vocabulary.label_qualifiers);
    });

    test("matches this package's redacted output", () => {
        for (const fixture of vocabulary.cases) {
            expect(redactSecretText(fixture.input)).toBe(fixture.expected_redacted);
            const bytes = Buffer.from(fixture.input, "utf8");
            for (const detection of fixture.detections) {
                expect(detection.offset + detection.length).toBeLessThanOrEqual(bytes.length);
                expect(detection.secret_type.length).toBeGreaterThan(0);
                // Spans shorter than 4 bytes can recur elsewhere in the output by coincidence.
                if (detection.length < 4) continue;
                const span = bytes
                    .subarray(detection.offset, detection.offset + detection.length)
                    .toString("utf8");
                expect(fixture.expected_redacted, fixture.name).not.toContain(span);
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
        const syntheticJwt = ["eyJhbGciOi", "eyJzdWIiOiIx", "SflKxwRJSMeKKF2QT4"].join("."); // gitleaks:allow redaction-test fixture
        expect(redactSecretText(`blob=${syntheticJwt}`)).toContain("<JWT_REDACTED>");
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

    test("flags Windows home paths with either separator", () => {
        expect(hasShareabilitySensitiveText("logs are under C:/Users/ufuk/AppData/tool")).toBe(
            true,
        );
        expect(hasShareabilitySensitiveText("logs are under D:\\Users\\ufuk\\AppData\\tool")).toBe(
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

    test("flags POSIX home paths that end at the username", () => {
        expect(hasShareabilitySensitiveText("crash log from /Users/janedoe")).toBe(true);
        expect(hasShareabilitySensitiveText("see /home/bobsmith.")).toBe(true);
    });

    test("flags a Basic authorization header and a Slack refresh token", () => {
        expect(hasShareabilitySensitiveText("Authorization: Basic dXNlcjpwYXNzd29yZA==")).toBe(
            true,
        );
        expect(hasShareabilitySensitiveText(`slack refresh ${slackRefreshToken()}`)).toBe(true);
    });
});

function slackRefreshToken(): string {
    return `xoxe-1-${"A1B2C3D4E5".repeat(15).slice(0, 146)}`;
}

describe("redactSecretText — quoted values", () => {
    test("redacts a double-quoted value that contains an apostrophe", () => {
        const input = `{"client_secret": "Gh7'kQ2mZp9XvR4tLnB1"}`;
        expect(redactSecretText(input)).toBe(`{"client_secret": "<REDACTED:client_secret>"}`);
    });

    test("redacts the whole value when it contains an escaped quote", () => {
        const input = `{"api_key":"AAAA\\"BBBBBBBBBBBBBBBBBBBB"}`;
        expect(redactSecretText(input)).toBe(`{"api_key":"<REDACTED:api_key>"}`);
    });

    test("redacts quoted values after an equals sign", () => {
        expect(redactSecretText('API_KEY="sk-live-8f3d9c2b1a4e"')).toBe(
            'API_KEY="<REDACTED:api_key>"',
        );
        expect(redactSecretText("PASSWORD='hunter2'")).toBe("PASSWORD='<REDACTED:password>'");
        expect(redactSecretText("export CLIENT_SECRET=`abc-def`")).toBe(
            "export CLIENT_SECRET=`<REDACTED:client_secret>`",
        );
    });

    test("keeps quoted scalar values after an equals sign", () => {
        expect(redactSecretText('MAX_TOKENS="4096"')).toBe('MAX_TOKENS="4096"');
        expect(redactSecretText("API_KEY='null'")).toBe("API_KEY='null'");
    });

    test("does not read a quoted value across a line break", () => {
        const input = `"api_key":"\nplain text\n"`;
        expect(redactSecretText(input)).toBe(input);
    });
});

describe("redactSecretText — credential shapes", () => {
    test("redacts every Authorization scheme, not only Bearer", () => {
        expect(redactSecretText("Authorization: Basic dXNlcjpwYXNzd29yZA==")).toBe(
            "Authorization: Basic <REDACTED:basic>",
        );
        expect(redactSecretText("authorization: Digest username=abc, response=0123456789")).toBe(
            "authorization: Digest <REDACTED:digest>",
        );
        expect(redactSecretText("Authorization: Bearer abc123def456")).toBe(
            "Authorization: Bearer <REDACTED:bearer>",
        );
    });

    test("redacts Slack refresh tokens", () => {
        expect(redactSecretText(`refresh ${slackRefreshToken()} done`)).toBe(
            "refresh <SLACK_TOKEN_REDACTED> done",
        );
        expect(redactSecretText(`xoxe.xoxb-1-${"Z9".repeat(82)}`)).toBe("<SLACK_TOKEN_REDACTED>");
    });

    test("a Digest parameter with an escaped quote does not end the header early", () => {
        const input = `Authorization: Digest username="a\\"b", response=0123456789abcdef`;
        expect(redactSecretText(input)).toBe("Authorization: Digest <REDACTED:digest>");
    });

    test("redacts the password in URL userinfo and keeps the user name", () => {
        expect(redactSecretText("postgres://alice:hunter2@db.example/app")).toBe(
            "postgres://alice:<REDACTED:password>@db.example/app",
        );
        expect(redactSecretText("DATABASE_URL=mongodb://svc:p%40ss:word@10.0.0.5:27017/db")).toBe(
            "DATABASE_URL=mongodb://svc:<REDACTED:password>@10.0.0.5:27017/db",
        );
        expect(hasShareabilitySensitiveText("postgres://alice:hunter2@db.example/app")).toBe(true);
    });

    test("a URL with a port, a path colon, or no password is not userinfo", () => {
        for (const url of [
            "http://localhost:8080/health",
            "https://example.com/a:b@c",
            "ssh://git@github.com/eidnara/eidnara.git",
        ]) {
            expect(redactSecretText(url), url).toBe(url);
        }
    });

    test("redacts a PEM private-key block whole, header and footer included", () => {
        const pem = pemPrivateKey("RSA ");
        expect(redactSecretText(`cert bundle:\n${pem}\ntrailer`)).toBe(
            "cert bundle:\n<PRIVATE_KEY_REDACTED>\ntrailer",
        );
        expect(redactSecretText(pemPrivateKey(""))).toBe("<PRIVATE_KEY_REDACTED>");
        expect(redactSecretText(pemPrivateKey("OPENSSH "))).toBe("<PRIVATE_KEY_REDACTED>");
        expect(hasShareabilitySensitiveText(pem)).toBe(true);
    });

    test("a PEM header with no footer leaves the text alone", () => {
        const input = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\nno footer here";
        expect(redactSecretText(input)).toBe(input);
    });
});

/** A synthetic PEM block whose body carries no real key material. */
function pemPrivateKey(kind: string): string {
    const bodyLine = "MIIEpAIBAAKCAQEA7bq2k0v9xR3sY1nQ4dJ6fH8zL2mW5cP0uT9eG7iK3oB1aV"; // gitleaks:allow redaction-test fixture
    return `-----BEGIN ${kind}PRIVATE KEY-----\n${bodyLine}\n${bodyLine}\n-----END ${kind}PRIVATE KEY-----`;
}

describe("redactSecretText — bounded backtracking", () => {
    // Each input takes seconds under an unbounded quantifier around the vocabulary alternation.
    const budgetMs = 250;

    function elapsed(fn: () => unknown): number {
        const start = performance.now();
        fn();
        return performance.now() - start;
    }

    test("a quote-sparse, vocabulary-dense log stays linear", () => {
        const line =
            "warning: unused variable `token_budget` in module auth_key_cache (see secret_scanner)\n";
        const body = `error: couldn't build\n${line.repeat(2200)}`;
        expect(body.length).toBeGreaterThan(170_000);
        expect(elapsed(() => redactSecretText(body))).toBeLessThan(budgetMs);
    });

    test("a dash-joined run of vocabulary words stays linear", () => {
        const run = "key-".repeat(1000);
        expect(elapsed(() => redactSecretText(run))).toBeLessThan(budgetMs);
        expect(elapsed(() => redactSecretText("a-".repeat(40_000)))).toBeLessThan(budgetMs);
    });
});

describe("isSecretKey", () => {
    test("any label-word segment names a secret, as the Rust key gate reads it", () => {
        for (const key of [
            "DATABASE_PASSWORD",
            "db_password",
            "smtp_password",
            "NPM_TOKEN",
            "SLACK_TOKEN",
            "signing_key",
            "webhook_secret",
            "api_key",
            "apiKey",
            "private_key",
            "aws_secret_access_key",
            "api_key_id",
            "bearerToken",
            "Authorization",
            "URLToken",
            "passWord",
            "apikey",
            "APIKEY",
            "OPENAIAPIKEY",
            "authtoken",
        ]) {
            expect(isSecretKey(key), key).toBe(true);
        }
    });

    test("a bare key, a public marker, or a non-vocabulary word stays structural", () => {
        for (const key of [
            "key",
            "keys",
            "key_id",
            "target_key",
            "last_model_key",
            "foreign_key",
            "pin_key_files",
            "keyvalue",
            "public_key",
            "publishable_key",
            "pubkey",
            "public_signing_key",
            "author",
            "authored_by",
            "monkey",
            "keyboard",
            "display_path",
            "models",
            "baseURL",
        ]) {
            expect(isSecretKey(key), key).toBe(false);
        }
    });
});

describe("sanitizeConfigValue", () => {
    test("redacts string values under every credential-shaped key", () => {
        expect(
            sanitizeConfigValue({
                env: {
                    DATABASE_PASSWORD: "pg-prod-pw",
                    NPM_TOKEN: "npm_abcdefgh",
                    API_KEY: "plain",
                    max_tokens: 4096,
                    target_key: "row-7",
                    public_key: "ssh-ed25519 AAAA",
                },
            }),
        ).toEqual({
            env: {
                DATABASE_PASSWORD: "<REDACTED:database_password>",
                NPM_TOKEN: "<REDACTED:token>",
                API_KEY: "<REDACTED:api_key>",
                max_tokens: 4096,
                target_key: "row-7",
                public_key: "ssh-ed25519 AAAA",
            },
        });
    });
});

describe("sanitizeDiagnosticText — host identity", () => {
    const realOs = { ...os, homedir: os.homedir, userInfo: os.userInfo };

    function mockHost(host: { homedir?: () => string; userInfo?: () => os.UserInfo<string> }) {
        mock.module("node:os", () => ({ ...realOs, ...host }));
    }

    function withUsername(username: string): os.UserInfo<string> {
        return { ...realOs.userInfo(), username };
    }

    afterEach(() => {
        mock.module("node:os", () => realOs);
    });

    afterAll(() => {
        mock.module("node:os", () => realOs);
    });

    test("sanitizes host paths that end at the username", () => {
        mockHost({ homedir: () => "/home/zed", userInfo: () => withUsername("zed") });
        expect(sanitizeDiagnosticText("crash log from /Users/janedoe")).toBe(
            "crash log from /Users/<USER>",
        );
        expect(sanitizeDiagnosticText("see /home/bobsmith.")).toBe("see /home/<USER>.");
        expect(sanitizeDiagnosticText("in /Users/jane.doe/Projects/app")).toBe(
            "in /Users/<USER>/Projects/app",
        );
        expect(sanitizeDiagnosticText("logs at C:/Users/ufuk/AppData/tool")).toBe(
            "logs at C:/Users/<USER>/AppData/tool",
        );
    });

    test("keeps redacting when the process has no passwd entry", () => {
        mockHost({
            homedir: () => "/",
            userInfo: () => {
                throw Object.assign(new Error("uv_os_get_passwd returned ENOENT"), {
                    code: "ERR_SYSTEM_ERROR",
                });
            },
        });
        expect(sanitizeDiagnosticText("password=hunter2 at /usr/local/bin")).toBe(
            "password=<REDACTED:password> at /usr/local/bin",
        );
        expect(sanitizeConfigValue({ note: "/home/alice/notes" })).toEqual({
            note: "/home/<USER>/notes",
        });
        expect(hasShareabilitySensitiveText("plain prose")).toBe(false);
    });

    test("a root home directory does not rewrite every path separator", () => {
        mockHost({ homedir: () => "/", userInfo: () => withUsername("zed") });
        expect(sanitizeDiagnosticText("/usr/local/bin/opencode failed")).toBe(
            "/usr/local/bin/opencode failed",
        );
        expect(sanitizeDiagnosticText("/Users/janedoe/Projects/app")).toBe(
            "/Users/<USER>/Projects/app",
        );
    });

    test("the home directory is replaced only as a whole path prefix", () => {
        mockHost({ homedir: () => "/home/zed", userInfo: () => withUsername("zed") });
        expect(sanitizeDiagnosticText("/home/zed/.config and /home/zedd/other")).toBe(
            "~/.config and /home/<USER>/other",
        );
        expect(sanitizeDiagnosticText("HOME=/home/zed")).toBe("HOME=~");
    });

    test("the username is replaced only as a whole word", () => {
        mockHost({ homedir: () => "/home/zed", userInfo: () => withUsername("zed") });
        expect(sanitizeDiagnosticText("zed ran it; zedd did not; zed's log")).toBe(
            "<USER> ran it; zedd did not; <USER>'s log",
        );
    });

    test("a role account name is not treated as a personal identity", () => {
        mockHost({ homedir: () => "/root", userInfo: () => withUsername("root") });
        expect(sanitizeDiagnosticText("chroot failed; root cause: rootDir=/srv/app")).toBe(
            "chroot failed; root cause: rootDir=/srv/app",
        );
        expect(hasShareabilitySensitiveText("root cause: the tool timed out")).toBe(false);
        mockHost({ homedir: () => "/", userInfo: () => withUsername("unknown") });
        expect(sanitizeDiagnosticText("unknown tool error")).toBe("unknown tool error");
    });
});
