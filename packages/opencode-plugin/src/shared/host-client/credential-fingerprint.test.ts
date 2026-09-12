import { describe, expect, test } from "bun:test";
import hostRelease from "../../../../../release/host-release.json";
import {
    BROCA_CREDENTIAL_VALUE_CAP_BYTES,
    canonicalCredentialRowEncoding,
    credentialFingerprints,
} from "./credential-fingerprint";

describe("Broca credential fingerprints", () => {
    test("derives its domain, canonicalization, and value cap from the release contract", () => {
        expect(hostRelease.credential_fingerprint.domain).toBe("eidnara-broca-credential-v1");
        expect(hostRelease.credential_fingerprint.canonicalization).toBe(
            "harness-provider-name-length-value/1",
        );
        expect(BROCA_CREDENTIAL_VALUE_CAP_BYTES).toBe(
            hostRelease.harness_unavailable.value_cap_bytes,
        );
        expect(BROCA_CREDENTIAL_VALUE_CAP_BYTES).toBe(16 * 1024);
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
            anthropic: "ecac831b94bb1d9e972ee993f7798c9ff7c6133b545e489ac1a3f60448127e80",
        });
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
        const oversize = "x".repeat(BROCA_CREDENTIAL_VALUE_CAP_BYTES + 1);
        const fingerprints = credentialFingerprints(key, "pi", {
            ANTHROPIC_API_KEY: "direct",
            GEMINI_API_KEY: oversize,
            OPENAI_API_KEY: "direct",
        });
        expect(Object.keys(fingerprints).sort()).toEqual(["anthropic", "openai"]);
        expect(credentialFingerprints(key, "pi", { ANTHROPIC_API_KEY: oversize })).toEqual({});

        const atCap = credentialFingerprints(key, "pi", {
            ANTHROPIC_API_KEY: "x".repeat(BROCA_CREDENTIAL_VALUE_CAP_BYTES),
        });
        expect(Object.keys(atCap)).toEqual(["anthropic"]);
    });
});
