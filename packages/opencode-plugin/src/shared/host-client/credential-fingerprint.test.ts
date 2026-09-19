import { describe, expect, test } from "bun:test";
import hostRelease from "../../../../../release/host-release.json";
import {
    canonicalCredentialRowEncoding,
    credentialFingerprints,
    MODEL_EXECUTION_CREDENTIAL_VALUE_CAP_BYTES,
} from "./credential-fingerprint";

describe("ModelExecution credential fingerprints", () => {
    test("derives its domain, canonicalization, and value cap from the release contract", () => {
        expect(hostRelease.credential_fingerprint.domain).toBe(
            "eidnara-model-execution-credential-v3",
        );
        expect(hostRelease.credential_fingerprint.canonicalization).toBe(
            "harness-provider-name-length-value/1",
        );
        expect(MODEL_EXECUTION_CREDENTIAL_VALUE_CAP_BYTES).toBe(
            hostRelease.harness_unavailable.value_cap_bytes,
        );
        expect(MODEL_EXECUTION_CREDENTIAL_VALUE_CAP_BYTES).toBe(16 * 1024);
    });

    test("pins the domain-derived cross-language row vector", () => {
        const key = Uint8Array.from({ length: 32 }, (_value, index) => index);
        expect(
            canonicalCredentialRowEncoding("opencode", "anthropic", [
                ["ANTHROPIC_API_KEY", "secret"],
            ]),
        ).toBe(
            "36:harness-provider-name-length-value/18:opencode9:anthropic17:ANTHROPIC_API_KEY1:66:secret",
        );
        expect(
            credentialFingerprints(key, "opencode", {
                ANTHROPIC_API_KEY: "secret",
            }),
        ).toEqual({
            anthropic: "77389364c8f8671636364d5f19b4788f6f2e990442798beb9338a99eae27e854",
        });
    });

    test("pins the Bedrock static and session rows to the vectors the host tests pin", () => {
        // The same hex literals live in `amazon_bedrock_row_orders_static_and_session_credentials`
        // (crates/host-runtime/src/model_execution/subprocess.rs): a row layout change on either
        // side fails one of the two suites instead of surfacing as `harness_unavailable`.
        const key = Uint8Array.from({ length: 32 }, (_value, index) => index);
        const staticRow = {
            "amazon-bedrock": "3c280e8e1a20d873f48a4b582e4776bdc5bd5d1534f80726d07df077da28fbda",
        };
        expect(
            credentialFingerprints(key, "opencode", {
                AWS_REGION: "us-west-2",
                AWS_SECRET_ACCESS_KEY: "s",
                AWS_ACCESS_KEY_ID: "k",
                AWS_PROFILE: "ignored",
            }),
        ).toEqual(staticRow);
        // An empty optional variable is absent, not missing.
        expect(
            credentialFingerprints(key, "opencode", {
                AWS_ACCESS_KEY_ID: "k",
                AWS_SECRET_ACCESS_KEY: "s",
                AWS_SESSION_TOKEN: "",
                AWS_REGION: "us-west-2",
            }),
        ).toEqual(staticRow);
        expect(
            credentialFingerprints(key, "opencode", {
                AWS_ACCESS_KEY_ID: "k",
                AWS_SECRET_ACCESS_KEY: "s",
                AWS_SESSION_TOKEN: "t",
                AWS_REGION: "us-west-2",
            }),
        ).toEqual({
            "amazon-bedrock": "773c92ad5ea4c9d06abfdd75263014f161d81a413e6ddbe4f3610027b6df80ef",
        });
        expect(
            credentialFingerprints(key, "opencode", {
                AWS_ACCESS_KEY_ID: "k",
                AWS_SECRET_ACCESS_KEY: "s",
            }),
        ).toEqual({});
    });

    test("omits incomplete rows and excludes unrelated ambient values", () => {
        const key = new Uint8Array(32);
        const fingerprints = credentialFingerprints(key, "pi", {
            OPENAI_API_KEY: "direct",
            AWS_ACCESS_KEY_ID: "ambient",
            HTTPS_PROXY: "ambient",
            PATH: "/attacker/bin",
        });
        expect(Object.keys(fingerprints)).toEqual(["openai"]);
        expect(fingerprints.anthropic).toBeUndefined();
        expect(fingerprints.google).toBeUndefined();
        expect(JSON.stringify(fingerprints)).not.toContain("direct");
        expect(JSON.stringify(fingerprints)).not.toContain("ambient");
    });

    test("rejects malformed keys", () => {
        expect(() => credentialFingerprints(new Uint8Array(31), "pi", {})).toThrow(/exactly 32/);
    });

    test("a value exactly at the cap qualifies and only an oversize provider is omitted", () => {
        // The host qualifies rows per provider, so one oversize value must not hide the
        // fingerprints of the other providers.
        const key = new Uint8Array(32);
        const oversize = "x".repeat(MODEL_EXECUTION_CREDENTIAL_VALUE_CAP_BYTES + 1);
        const fingerprints = credentialFingerprints(key, "pi", {
            ANTHROPIC_API_KEY: "direct",
            GEMINI_API_KEY: oversize,
            OPENAI_API_KEY: "direct",
        });
        expect(Object.keys(fingerprints).sort()).toEqual(["anthropic", "openai"]);
        expect(credentialFingerprints(key, "pi", { ANTHROPIC_API_KEY: oversize })).toEqual({});

        const atCap = credentialFingerprints(key, "pi", {
            ANTHROPIC_API_KEY: "x".repeat(MODEL_EXECUTION_CREDENTIAL_VALUE_CAP_BYTES),
        });
        expect(Object.keys(atCap)).toEqual(["anthropic"]);
    });
});
