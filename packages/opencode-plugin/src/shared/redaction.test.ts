/// <reference types="bun-types" />

import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";
import * as os from "node:os";

import vocabulary from "./fixtures/redaction-vocabulary-v1.json";
import {
    describeProseLength,
    hasShareabilitySensitiveText,
    isSecretKey,
    keepsScalarValue,
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

    test("flags every spelling of the IPv6 loopback address", () => {
        for (const spelling of [
            "bound to http://[0:0:0:0:0:0:0:1]:8080/v1",
            "[0000:0000:0000:0000:0000:0000:0000:0001]",
            "[0:0:0:0:0:0::1]",
            "[0:0:0::1]",
            "[::0:1]",
            "0::1",
        ]) {
            expect(hasShareabilitySensitiveText(spelling), spelling).toBe(true);
        }
        expect(hasShareabilitySensitiveText("route via 2001:0:0:0:0:0:0:1")).toBe(false);
        expect(hasShareabilitySensitiveText("route via 2001:0:0::1")).toBe(false);
        expect(hasShareabilitySensitiveText("ratio 0:1 b")).toBe(false);
    });

    test("a key that merely contains a vocabulary substring is shareable", () => {
        expect(hasShareabilitySensitiveText('author="alice" wrote the module')).toBe(false);
        expect(hasShareabilitySensitiveText("monkey=banana keyboard=qwerty")).toBe(false);
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

    test("a quoted key may hold the other quote character", () => {
        expect(redactSecretText(`{"client's_api_key":"hunter2"}`)).toBe(
            `{"client's_api_key":"<REDACTED:client_api_key>"}`,
        );
        expect(redactSecretText(`{'api_key': "hunter2"}`)).toBe(
            `{'api_key': "<REDACTED:api_key>"}`,
        );
    });

    test("a key of any length is redacted on either side of the vocabulary word", () => {
        const namespace = "A".repeat(65);
        expect(redactSecretText(`${namespace}_api_key=hunter2`)).toBe(
            `${namespace}_api_key=<REDACTED:api_key>`,
        );
        expect(redactSecretText(`api_key_${namespace}=hunter2`)).toBe(
            `api_key_${namespace}=<REDACTED:api_key>`,
        );
        expect(redactSecretText(`{"${namespace}_api_key": "hunter2"}`)).toBe(
            `{"${namespace}_api_key": "<REDACTED:api_key>"}`,
        );
        expect(hasShareabilitySensitiveText(`api_key_${namespace}=hunter2`)).toBe(true);
    });

    test("an assignment inside a non-secret key's value is still seen", () => {
        expect(redactSecretText("URL=https://x/?api_key=abc")).toBe(
            "URL=https://x/?api_key=<REDACTED:api_key>",
        );
        expect(redactSecretText("AUTHOR=https://x/?api_key=abc")).toBe(
            "AUTHOR=https://x/?api_key=<REDACTED:api_key>",
        );
    });

    test("a backtick value spans an escaped backtick", () => {
        expect(redactSecretText("API_KEY=`before\\`AFTER_SECRET`")).toBe(
            "API_KEY=`<REDACTED:api_key>`",
        );
    });

    test("a shell word made of adjacent segments is one value", () => {
        expect(redactSecretText("API_KEY='before''AFTER_SECRET'")).toBe(
            "API_KEY='<REDACTED:api_key>'",
        );
        expect(redactSecretText("API_KEY=$'AFTER_SECRET'")).toBe("API_KEY=<REDACTED:api_key>");
        expect(redactSecretText('{api_key: "x", other: "y"}')).toBe(
            '{api_key: "<REDACTED:api_key>", other: "y"}',
        );
    });

    test("a structured value under a credential key is redacted whole when it carries text", () => {
        expect(redactSecretText('{"api_key":["hunter2"]}')).toBe(
            '{"api_key":"<REDACTED:api_key>"}',
        );
        expect(redactSecretText('{"credentials":{"value":"hunter2"}}')).toBe(
            '{"credentials":"<REDACTED:credentials>"}',
        );
        expect(redactSecretText('{"api_key":["a\\"]b"]}')).toBe('{"api_key":"<REDACTED:api_key>"}');
        expect(redactSecretText("password: [a, b]")).toBe('password: "<REDACTED:password>"');
        expect(hasShareabilitySensitiveText('{"api_key":["hunter2"]}')).toBe(true);
    });

    test("a structured value holding only counts, keys, and scalars stays", () => {
        for (const line of [
            '{"tokens":[100,200]}',
            '{"tokens":{"input":100,"output":200}}',
            "tokens: {input: 100, output: 200}",
            "tokens: [true, null, 1e5, -3.5]",
        ]) {
            expect(redactSecretText(line), line).toBe(line);
        }
        expect(redactSecretText('{"tokens":{"input":100},"api_key":"x"}')).toBe(
            '{"tokens":{"input":100},"api_key":"<REDACTED:api_key>"}',
        );
    });

    test("a structured value nothing closes is redacted to the end rather than shown", () => {
        expect(redactSecretText('{"api_key":["hunter2"')).toBe('{"api_key":"<REDACTED:api_key>"');
        expect(redactSecretText('{"api_key":["hunter2')).toBe('{"api_key":"<REDACTED:api_key>"');
        const big = `{"api_key":["${"s".repeat(5000)}"]}`;
        expect(redactSecretText(big)).toBe('{"api_key":"<REDACTED:api_key>"}');
    });

    test("a quoted YAML key takes a plain or block value like an unquoted one", () => {
        expect(redactSecretText('"password": hunter2')).toBe('"password": <REDACTED:password>');
        expect(redactSecretText('"password": |\n  hunter2\nhost: x')).toBe(
            '"password": <REDACTED:password>\nhost: x',
        );
        expect(hasShareabilitySensitiveText('"password": hunter2')).toBe(true);
    });

    test("a YAML plain scalar runs to the end of its line", () => {
        expect(redactSecretText("password: correct horse")).toBe("password: <REDACTED:password>");
        expect(redactSecretText("password: !!str hunter2")).toBe("password: <REDACTED:password>");
        expect(redactSecretText("password: hunter2 # dev only")).toBe(
            "password: <REDACTED:password> # dev only",
        );
        expect(redactSecretText("{api_key: process.env.KEY, other: 1}")).toBe(
            "{api_key: <REDACTED:api_key>, other: 1}",
        );
    });

    test("a TOML multi-line string is one value", () => {
        expect(redactSecretText('password = """before\nAFTER_SECRET"""')).toBe(
            'password = "<REDACTED:password>"',
        );
        expect(redactSecretText("password = '''before\nAFTER'''")).toBe(
            "password = '<REDACTED:password>'",
        );
    });

    test("a YAML block scalar is redacted with its indented body", () => {
        expect(redactSecretText("password: |\n  hunter2\n  more\nhost: x")).toBe(
            "password: <REDACTED:password>\nhost: x",
        );
        expect(redactSecretText("db:\n  password: >-\n    hunter2\n  host: x\ntop: 1")).toBe(
            "db:\n  password: <REDACTED:password>\n  host: x\ntop: 1",
        );
        expect(redactSecretText("password: | # c\n  hunter2")).toBe(
            "password: <REDACTED:password>",
        );
        // `| then` is not a block indicator, so it is a plain scalar that runs to the end of the line.
        expect(redactSecretText("token: | then")).toBe("token: <REDACTED:token>");
    });

    test("an unquoted `key: value` line is redacted when the key names a secret", () => {
        expect(redactSecretText("password: hunter2")).toBe("password: <REDACTED:password>");
        expect(redactSecretText("db:\n  password: hunter2\n  host: localhost")).toBe(
            "db:\n  password: <REDACTED:password>\n  host: localhost",
        );
        expect(redactSecretText('api_key: "hunter2"')).toBe('api_key: "<REDACTED:api_key>"');
        // The value after `: ` is a YAML plain scalar and runs to the end of the line, so prose
        // that follows a secret on the same line goes with it.
        expect(redactSecretText("Set api_key: sk-live-abc123 in the env.")).toBe(
            "Set api_key: <REDACTED:api_key>", // gitleaks:allow redaction-test fixture
        );
    });

    test("the `key: value` form keeps the bare-key carve-out and leaves headers to the header rule", () => {
        for (const line of [
            "press any key: continue",
            "tokens: 42",
            "at 10:30 token expired",
            "api_key:hunter2",
            "Authorization: Bearer <REDACTED:bearer>",
            "Proxy-Authorization: Bearer <REDACTED:bearer>",
        ]) {
            expect(redactSecretText(line), line).toBe(line);
        }
    });

    test("an escaped space is part of a bare shell value", () => {
        expect(redactSecretText("API_KEY=before\\ AFTER_SECRET")).toBe(
            "API_KEY=<REDACTED:api_key>",
        );
        expect(redactSecretText("API_KEY=abc next=1")).toBe("API_KEY=<REDACTED:api_key> next=1");
    });

    test("a key names a secret only through a whole segment, as `isSecretKey` reads it", () => {
        for (const line of [
            'author="alice"',
            "monkey=banana",
            "keyboard=qwerty",
            "public_key=ssh-ed25519 AAAA",
        ]) {
            expect(redactSecretText(line), line).toBe(line);
        }
        expect(redactSecretText("apikey=hunter2")).toBe("apikey=<REDACTED:secret>");
        expect(redactSecretText("_key=private-value")).toBe("_key=<REDACTED:key>");
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

    test("a short Authorization credential is still a credential", () => {
        expect(redactSecretText("Authorization: Basic YTpi")).toBe(
            "Authorization: Basic <REDACTED:basic>",
        );
        expect(hasShareabilitySensitiveText("Authorization: Basic YTpi")).toBe(true);
    });

    test("a known scheme is kept; an unknown first token is part of the credential", () => {
        expect(redactSecretText("Authorization: Api-Key short-secret")).toBe(
            "Authorization: Api-Key <REDACTED:api-key>",
        );
        expect(
            redactSecretText("Authorization: AWS4-HMAC-SHA256 Credential=AKIA/x, Signature=abc"),
        ).toBe("Authorization: AWS4-HMAC-SHA256 <REDACTED:aws4-hmac-sha256>");
        // An unknown scheme cannot be told from a credential, so both words go.
        expect(redactSecretText("Authorization: Foo+Bar AFTER_SECRET")).toBe(
            "Authorization: <REDACTED:authorization>",
        );
    });

    test("a scheme-less header value ends at the next field", () => {
        expect(redactSecretText("Authorization: abc123, Other: value")).toBe(
            "Authorization: <REDACTED:authorization>, Other: value",
        );
        expect(redactSecretText("Authorization: abc123 OTHER=value")).toBe(
            "Authorization: <REDACTED:authorization> OTHER=value",
        );
        expect(redactSecretText("Authorization: abc123; next")).toBe(
            "Authorization: <REDACTED:authorization>; next",
        );
        expect(redactSecretText("Authorization: abc123\nHost: y")).toBe(
            "Authorization: <REDACTED:authorization>\nHost: y",
        );
    });

    test("a header with no credential does not consume the next header line", () => {
        const input = "Authorization: Bearer\nContent-Type: x";
        expect(redactSecretText(input)).toBe(input);
    });

    test("Digest parameters may carry whitespace around the equals sign", () => {
        expect(
            redactSecretText('Authorization: Digest username = "alice", response = "AFTER_SECRET"'),
        ).toBe("Authorization: Digest <REDACTED:digest>");
    });

    test("cookie values are credentials whatever the cookie is named", () => {
        expect(redactSecretText("Cookie: session=abcdefghijklmnop; theme=dark")).toBe(
            "Cookie: session=<REDACTED:cookie>; theme=<REDACTED:cookie>",
        );
        expect(
            redactSecretText("Set-Cookie: connect.sid=s%3Aabcdef.signature; Path=/; HttpOnly"),
        ).toBe("Set-Cookie: connect.sid=<REDACTED:cookie>; Path=/; HttpOnly");
        expect(hasShareabilitySensitiveText("Cookie: session=abc")).toBe(true);
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
        expect(redactSecretText("redis://:hunter2@cache.example.com/0")).toBe(
            "redis://:<REDACTED:password>@cache.example.com/0",
        );
        expect(redactSecretText("//alice:hunter2@db.example/app")).toBe(
            "//alice:<REDACTED:password>@db.example/app",
        );
        expect(redactSecretText("// alice:hunter2@db")).toBe("// alice:hunter2@db");
    });

    test("redacts Slack legacy and Stripe tokens on their own", () => {
        expect(redactSecretText("xoxo-1234567890-1234567890-abcdef")).toBe(
            "<SLACK_TOKEN_REDACTED>",
        );
        expect(redactSecretText("sk_live_Ab3dE5fGh7Jk9Lm2Np4Qr6St")).toBe("<STRIPE_KEY_REDACTED>"); // gitleaks:allow redaction-test fixture
        expect(redactSecretText("key rk_test_abcdefghij done")).toBe(
            "key <STRIPE_KEY_REDACTED> done",
        );
        expect(redactSecretText("sk_live_short")).toBe("sk_live_short");
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

    test("a value assigned to a provider marker is redacted too", () => {
        // The token pattern absorbs the trailing `key` run, so the keyed rule sees the marker.
        expect(redactSecretText(`hf_${"A".repeat(30)}key=private-value`)).toBe(
            "<HUGGINGFACE_TOKEN_REDACTED>=<REDACTED:huggingface_token>",
        );
        expect(redactSecretText("count>token=42")).toBe("count>token=42");
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
            "db_passwd",
            "passwd",
        ]) {
            expect(isSecretKey(key), key).toBe(true);
        }
    });

    test("`pwd` names the working directory, not a password", () => {
        expect(isSecretKey("pwd")).toBe(false);
        expect(isSecretKey("PWD")).toBe(false);
        expect(redactSecretText("PWD=/home/zed/project OLDPWD=/tmp")).toBe(
            "PWD=/home/zed/project OLDPWD=/tmp",
        );
        expect(hasShareabilitySensitiveText("PWD=/tmp")).toBe(false);
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
                    db_passwd: "hunter2",
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
                db_passwd: "<REDACTED:db_passwd>",
                max_tokens: 4096,
                target_key: "row-7",
                public_key: "ssh-ed25519 AAAA",
            },
        });
    });

    test("a number or boolean under a credential key stays, as the Rust key gate reads it", () => {
        // `carries_text` in `crates/memory-store` treats numbers and booleans as non-text under a
        // secret-shaped key; `max_tokens` is a count and `isSecretKey` cannot tell it from a PIN.
        expect(sanitizeConfigValue({ password: 123456, api_key: 42, tls: true })).toEqual({
            password: 123456,
            api_key: 42,
            tls: true,
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

    test("a Windows profile name with a space is redacted whole when a separator follows", () => {
        mockHost({ homedir: () => "/home/zed", userInfo: () => withUsername("zed") });
        expect(sanitizeDiagnosticText("C:\\Users\\John Doe\\AppData\\x")).toBe(
            "C:\\Users\\<USER>\\AppData\\x",
        );
        expect(sanitizeDiagnosticText("copied C:\\Users\\alice\\a to C:\\Users\\bob\\b")).toBe(
            "copied C:\\Users\\<USER>\\a to C:\\Users\\<USER>\\b",
        );
        expect(sanitizeDiagnosticText("C:\\Users\\alice and C:\\Users\\bob\\x")).toBe(
            "C:\\Users\\<USER> and C:\\Users\\<USER>\\x",
        );
    });

    test("a home-directory name with a space is redacted whole in every path form", () => {
        mockHost({ homedir: () => "/home/zed", userInfo: () => withUsername("zed") });
        expect(sanitizeDiagnosticText("C:/Users/John Doe/AppData/x")).toBe(
            "C:/Users/<USER>/AppData/x",
        );
        expect(sanitizeDiagnosticText("/Users/John Doe/Documents/private.txt")).toBe(
            "/Users/<USER>/Documents/private.txt",
        );
        expect(sanitizeDiagnosticText("see /home/john and then /tmp/x")).toBe(
            "see /home/<USER> and then /tmp/x",
        );
        expect(sanitizeDiagnosticText("\\\\server\\Users\\alice\\AppData\\x")).toBe(
            "\\\\server\\Users\\<USER>\\AppData\\x",
        );
        expect(sanitizeDiagnosticText("copied C:/Users/john to /tmp/x")).toBe(
            "copied C:/Users/<USER> to /tmp/x",
        );
    });

    test("secrets are redacted before a username that is itself a vocabulary word", () => {
        mockHost({ homedir: () => "/home/token", userInfo: () => withUsername("token") });
        const sanitized = sanitizeDiagnosticText("token=hunter2 in /home/token/.env");
        expect(sanitized).not.toContain("hunter2");
        expect(sanitized).toEndWith(" in ~/.env");
    });

    test("a drive-letter home directory is matched without regard to case", () => {
        mockHost({ homedir: () => "C:\\Users\\Zed", userInfo: () => withUsername("Zed") });
        expect(sanitizeDiagnosticText("log at c:\\users\\zed\\AppData\\x")).toBe(
            "log at ~\\AppData\\x",
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

describe("redactSecretText — CLI flag arguments", () => {
    test("redacts the value of secret-bearing flags and leaves other flags alone", () => {
        expect(redactSecretText("tool --api-key abc123secret --verbose")).toBe(
            "tool --api-key <REDACTED:api_key> --verbose",
        );
        expect(redactSecretText("curl --token abc123secret https://x")).toBe(
            "curl --token <REDACTED:token> https://x",
        );
        expect(redactSecretText("cmd --password correct-horse")).toBe(
            "cmd --password <REDACTED:password>",
        );
        expect(redactSecretText('cmd --password "correct horse battery staple" --v')).toBe(
            'cmd --password "<REDACTED:password>" --v',
        );
        expect(redactSecretText("cmd --api-key 'a b' next")).toBe(
            "cmd --api-key '<REDACTED:api_key>' next",
        );
        // A following flag is not a value, and non-secret flags are untouched.
        expect(redactSecretText("cmd --password --verbose")).toBe("cmd --password --verbose");
        expect(redactSecretText("cmd --author alice")).toBe("cmd --author alice");
    });
});

describe("redactSecretText — URL passwords containing @", () => {
    test("redacts through the last @ before the host", () => {
        expect(redactSecretText("https://user:p@ss@example.com/path")).toBe(
            "https://user:<REDACTED:password>@example.com/path",
        );
        expect(redactSecretText("mail me at a@b.com then https://x.io/y@z")).toBe(
            "mail me at a@b.com then https://x.io/y@z",
        );
    });
});

describe("sanitizeConfigValue cookie keys", () => {
    test("treats a Cookie property as a secret", () => {
        expect(sanitizeConfigValue({ headers: { Cookie: "session=supersecret" } })).toEqual({
            headers: { Cookie: "<REDACTED:cookie>" },
        });
        expect(redactSecretText('{"Cookie":"session=supersecret"}')).toBe(
            '{"Cookie":"<REDACTED:cookie>"}',
        );
    });
});

describe("sanitizeConfigValue prompt-bearing fields", () => {
    test("keeps only presence and length for prompt prose", () => {
        expect(
            sanitizeConfigValue({
                prompt: "You are an internal assistant for ACME payroll",
                system_prompt: "secret instructions",
                description: "desc",
                prompt_surface: { tool_descriptions: { bash: "run it" } },
                skip_signatures: ["sig one", "sig two"],
                model: "anthropic/claude",
            }),
        ).toEqual({
            prompt: "<REDACTED 46 chars>",
            system_prompt: "<REDACTED 19 chars>",
            description: "<REDACTED 4 chars>",
            prompt_surface: { tool_descriptions: { bash: "<REDACTED 6 chars>" } },
            skip_signatures: ["<REDACTED 7 chars>", "<REDACTED 7 chars>"],
            model: "anthropic/claude",
        });
    });
});

describe("describeProseLength", () => {
    test("is idempotent so a second sanitization pass keeps the original length", () => {
        expect(describeProseLength("x".repeat(47))).toBe("<REDACTED 47 chars>");
        expect(describeProseLength("<REDACTED 47 chars>")).toBe("<REDACTED 47 chars>");
        expect(sanitizeConfigValue(sanitizeConfigValue({ prompt: "x".repeat(47) }))).toEqual({
            prompt: "<REDACTED 47 chars>",
        });
    });
});

describe("sanitizeConfigValue record keys", () => {
    test("sanitizes user-controlled keys as well as values", () => {
        const flags = {
            historian: {
                permission: {
                    bash: {
                        "psql postgres://app:s3cr3t@db.internal/prod": "allow",
                        "cat /home/alice/notes.txt": "deny",
                        "git status": "allow",
                    },
                },
            },
        };
        expect(sanitizeConfigValue(flags)).toEqual({
            historian: {
                permission: {
                    bash: {
                        "psql postgres://app:<REDACTED:password>@db.internal/prod": "allow",
                        "cat /home/<USER>/notes.txt": "deny",
                        "git status": "allow",
                    },
                },
            },
        });
    });
});

describe("keepsScalarValue", () => {
    test("keeps counts under token/key names and booleans anywhere, but not a number under a password-like key", () => {
        expect(keepsScalarValue("max_tokens", "4096")).toBe(true);
        expect(keepsScalarValue("maxTokens", "4096")).toBe(true);
        expect(keepsScalarValue("api_key", "42")).toBe(true);
        expect(keepsScalarValue("execute_threshold_tokens", "200000")).toBe(true);
        expect(keepsScalarValue("password", "true")).toBe(true);
        expect(keepsScalarValue("secret", "null")).toBe(true);
        expect(keepsScalarValue("password", "123456")).toBe(false);
        expect(keepsScalarValue("db_passwd", "987654")).toBe(false);
        expect(keepsScalarValue("pin_secret", "4242")).toBe(false);
        expect(keepsScalarValue("ldap_credential", "7")).toBe(false);
        expect(keepsScalarValue("api_key", "sk-live")).toBe(false);
    });
});

describe("redactSecretText — authorization assignments", () => {
    test("redacts the credential after a scheme in the `=` form and under other secret keys", () => {
        expect(redactSecretText("Authorization=Bearer abc123secret")).toBe(
            "Authorization=Bearer <REDACTED:bearer>",
        );
        expect(redactSecretText("AUTHORIZATION=Bearer abc123 next=1")).toBe(
            "AUTHORIZATION=Bearer <REDACTED:bearer> next=1",
        );
        expect(redactSecretText('Authorization=Digest username="a", response="b" tail')).toBe(
            "Authorization=Digest <REDACTED:digest> tail",
        );
        expect(redactSecretText("auth=Bearer abc123 next=1")).toBe(
            "auth=Bearer <REDACTED:bearer> next=1",
        );
        expect(redactSecretText("token=Basic dXNlcjpwYXNz")).toBe("token=Basic <REDACTED:basic>");
    });

    test("a quoted header value is replaced inside its quotes", () => {
        expect(redactSecretText('Authorization: "Bearer abc-secret"')).toBe(
            'Authorization: "<REDACTED:authorization>"',
        );
        expect(redactSecretText("headers:\n  Authorization: 'Basic YTpi'\n  Host: x")).toBe(
            "headers:\n  Authorization: '<REDACTED:authorization>'\n  Host: x",
        );
    });

    test("an assignment value without a known scheme ends at whitespace or its closing quote", () => {
        expect(redactSecretText("Authorization=abc123 OTHER=value")).toBe(
            "Authorization=<REDACTED:authorization> OTHER=value",
        );
        expect(redactSecretText('Authorization="Bearer abc" OTHER=1')).toBe(
            'Authorization="<REDACTED:authorization>" OTHER=1',
        );
        // The header form keeps an unknown scheme and redacts its parameter list whole.
        expect(
            redactSecretText(
                "Authorization: AWS4-HMAC-SHA256 Credential=AKIA/x, SignedHeaders=host, Signature=abc",
            ),
        ).toBe("Authorization: AWS4-HMAC-SHA256 <REDACTED:aws4-hmac-sha256>");
    });

    test("redacts a scheme-less authorization value and leaves a lone scheme alone", () => {
        expect(redactSecretText("authorization: abc123")).toBe(
            "authorization: <REDACTED:authorization>",
        );
        expect(redactSecretText("Authorization=abc123")).toBe(
            "Authorization=<REDACTED:authorization>",
        );
        expect(redactSecretText("Authorization: abc123\nHost: y")).toBe(
            "Authorization: <REDACTED:authorization>\nHost: y",
        );
        expect(redactSecretText("Authorization: Bearer\nContent-Type: x")).toBe(
            "Authorization: Bearer\nContent-Type: x",
        );
    });
});

describe("sanitizeConfigValue prompt record keys", () => {
    test("sanitizes keys under prompt-bearing fields as well as their values", () => {
        expect(
            sanitizeConfigValue({
                prompt_surface: {
                    tool_descriptions: {
                        "X-API-Key: live-abc": "run it",
                        "/home/alice/tool": "x",
                    },
                },
            }),
        ).toEqual({
            prompt_surface: {
                tool_descriptions: {
                    "X-API-Key: <REDACTED:x_api_key>": "<REDACTED 6 chars>",
                    "/home/<USER>/tool": "<REDACTED 1 chars>",
                },
            },
        });
    });
});
