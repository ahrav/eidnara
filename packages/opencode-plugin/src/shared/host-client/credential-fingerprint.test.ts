import { describe, expect, test } from "bun:test";
import hostRelease from "../../../../../release/host-release.json";
import { selectAwsSource } from "./aws-source";
import { MODEL_EXECUTION_CREDENTIAL_VALUE_CAP_BYTES } from "./credential-fingerprint";
import { sourceClaimPreimage, sourceClaims } from "./source-claim";

const ENVIRONMENT = selectAwsSource({});

describe("ModelExecution credential claims over environment rows", () => {
    test("derives its domain, canonicalization, and value cap from the release contract", () => {
        expect(hostRelease.credential_fingerprint.domain).toBe(
            "eidnara-model-execution-credential-v4",
        );
        expect(hostRelease.credential_fingerprint.canonicalization).toBe(
            "harness-provider-source/2",
        );
        expect(MODEL_EXECUTION_CREDENTIAL_VALUE_CAP_BYTES).toBe(
            hostRelease.harness_unavailable.value_cap_bytes,
        );
        expect(MODEL_EXECUTION_CREDENTIAL_VALUE_CAP_BYTES).toBe(16 * 1024);
    });

    test("pins the domain-derived cross-language row vector", () => {
        const key = Uint8Array.from({ length: 32 }, (_value, index) => index);
        expect(
            sourceClaimPreimage("opencode", "anthropic", {
                kind: "env",
                entries: [["ANTHROPIC_API_KEY", "secret"]],
            }),
        ).toBe(
            "25:harness-provider-source/28:opencode9:anthropic3:env17:ANTHROPIC_API_KEY1:66:secret",
        );
        expect(sourceClaims(key, "opencode", { ANTHROPIC_API_KEY: "secret" }, ENVIRONMENT)).toEqual(
            {
                anthropic: "77e2f22f3be6abf25f49883b92dd5f836d6c099346792d1e517a848ebe6cc218",
            },
        );
    });

    test("pins the Bedrock static and session rows to the vectors the host tests pin", () => {
        // The same hex literals live in `amazon_bedrock_row_orders_static_and_session_credentials`
        // (crates/host-runtime/src/model_execution/subprocess.rs): a row layout change on either
        // side fails one of the two suites instead of surfacing as `harness_unavailable`.
        const key = Uint8Array.from({ length: 32 }, (_value, index) => index);
        const staticRow = {
            "amazon-bedrock": "036efb950878e220f1988e87733835bda23a3b98ae017d4971d3be89fea7b2dd",
        };
        expect(
            sourceClaims(
                key,
                "opencode",
                {
                    AWS_REGION: "us-west-2",
                    AWS_SECRET_ACCESS_KEY: "s",
                    AWS_ACCESS_KEY_ID: "k",
                },
                ENVIRONMENT,
            ),
        ).toEqual(staticRow);
        // An empty optional variable is absent, not missing.
        expect(
            sourceClaims(
                key,
                "opencode",
                {
                    AWS_ACCESS_KEY_ID: "k",
                    AWS_SECRET_ACCESS_KEY: "s",
                    AWS_SESSION_TOKEN: "",
                    AWS_REGION: "us-west-2",
                },
                ENVIRONMENT,
            ),
        ).toEqual(staticRow);
        expect(
            sourceClaims(
                key,
                "opencode",
                {
                    AWS_ACCESS_KEY_ID: "k",
                    AWS_SECRET_ACCESS_KEY: "s",
                    AWS_SESSION_TOKEN: "t",
                    AWS_REGION: "us-west-2",
                },
                ENVIRONMENT,
            ),
        ).toEqual({
            "amazon-bedrock": "c6facdc901d90bf91034af2c09d9eb88be046ed57f0c90440a0b6d1793b14221",
        });
        expect(
            sourceClaims(
                key,
                "opencode",
                { AWS_ACCESS_KEY_ID: "k", AWS_SECRET_ACCESS_KEY: "s" },
                ENVIRONMENT,
            ),
        ).toEqual({});
    });

    test("omits incomplete rows and excludes unrelated ambient values", () => {
        const key = new Uint8Array(32);
        const claims = sourceClaims(
            key,
            "pi",
            {
                OPENAI_API_KEY: "direct",
                AWS_ACCESS_KEY_ID: "ambient",
                HTTPS_PROXY: "ambient",
                PATH: "/attacker/bin",
            },
            ENVIRONMENT,
        );
        expect(Object.keys(claims)).toEqual(["openai"]);
        expect(JSON.stringify(claims)).not.toContain("direct");
        expect(JSON.stringify(claims)).not.toContain("ambient");
    });

    test("a value exactly at the cap qualifies and only an oversize provider is omitted", () => {
        const key = new Uint8Array(32);
        const oversize = "x".repeat(MODEL_EXECUTION_CREDENTIAL_VALUE_CAP_BYTES + 1);
        const claims = sourceClaims(
            key,
            "pi",
            { ANTHROPIC_API_KEY: "direct", GEMINI_API_KEY: oversize, OPENAI_API_KEY: "direct" },
            ENVIRONMENT,
        );
        expect(Object.keys(claims).sort()).toEqual(["anthropic", "openai"]);
        expect(sourceClaims(key, "pi", { ANTHROPIC_API_KEY: oversize }, ENVIRONMENT)).toEqual({});
        const atCap = sourceClaims(
            key,
            "pi",
            { ANTHROPIC_API_KEY: "x".repeat(MODEL_EXECUTION_CREDENTIAL_VALUE_CAP_BYTES) },
            ENVIRONMENT,
        );
        expect(Object.keys(atCap)).toEqual(["anthropic"]);
    });
});
