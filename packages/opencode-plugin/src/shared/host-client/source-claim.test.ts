import { describe, expect, test } from "bun:test";
import vectors from "../../../../../crates/host-runtime/tests/fixtures/source-claim-vectors.json";
import { type AwsProfileSource, selectAwsSource } from "./aws-source";
import { credentialFingerprints } from "./credential-fingerprint";
import {
    type ClaimSource,
    SOURCE_CLAIM_CANONICALIZATION,
    SOURCE_CLAIM_DOMAIN,
    sourceClaimPreimage,
    sourceClaims,
    sourceClaimVersion,
} from "./source-claim";
import type { ModelExecutionProvider } from "./types";

const KEY = Uint8Array.from({ length: 32 }, (_value, index) => index);
const PROFILE_ENV = { HOME: "/home/u", AWS_PROFILE: "corp", AWS_REGION: "us-east-1" };

describe("source-bound credential claims", () => {
    test("pins the release domain and canonicalization", () => {
        expect(vectors.domain).toBe(SOURCE_CLAIM_DOMAIN);
        expect(vectors.canonicalization).toBe(SOURCE_CLAIM_CANONICALIZATION);
    });

    for (const vector of vectors.vectors) {
        test(`vector: ${vector.name}`, () => {
            const source: ClaimSource =
                vector.source.kind === "env"
                    ? {
                          kind: "env",
                          entries: (vector.source.entries ?? []) as [string, string][],
                      }
                    : {
                          kind: "aws_profile",
                          profile: vector.source.profile as AwsProfileSource,
                      };
            const harness = vector.harness as "opencode" | "pi";
            const provider = vector.provider as ModelExecutionProvider;
            expect(sourceClaimPreimage(harness, provider, source)).toBe(vector.preimage);
            const key = Buffer.from(vector.key_hex, "hex");
            const env =
                source.kind === "env" ? Object.fromEntries(source.entries) : { ...PROFILE_ENV };
            const selection =
                source.kind === "env"
                    ? selectAwsSource({})
                    : ({ mode: "profile", source: source.profile } as const);
            expect(sourceClaims(key, harness, env, selection)[provider]).toBe(vector.claim);
        });
    }

    const pinned = (name: string): string | undefined =>
        vectors.vectors.find((vector) => vector.name === name)?.claim;

    test("an empty optional token claims the tokenless row", () => {
        const env = {
            AWS_ACCESS_KEY_ID: "AKIA1",
            AWS_SECRET_ACCESS_KEY: "s3cr3t",
            AWS_SESSION_TOKEN: "",
            AWS_REGION: "us-east-1",
        };
        expect(sourceClaims(KEY, "pi", env, selectAwsSource({}))["amazon-bedrock"]).toBe(
            pinned("bedrock env without token"),
        );
    });

    test("profile claims ignore row rotation and follow the bearer and the selector", () => {
        const profile = selectAwsSource(PROFILE_ENV);
        const first = {
            ...PROFILE_ENV,
            AWS_ACCESS_KEY_ID: "ASIA1",
            AWS_SECRET_ACCESS_KEY: "secret-1",
            AWS_SESSION_TOKEN: "token-1",
            ANTHROPIC_API_KEY: "k",
        };
        const rotated = {
            ...first,
            AWS_ACCESS_KEY_ID: "ASIA2",
            AWS_SECRET_ACCESS_KEY: "secret-2",
            AWS_SESSION_TOKEN: "token-2",
        };
        const claims = sourceClaims(KEY, "pi", first, profile);
        expect(claims["amazon-bedrock"]).toBe(pinned("bedrock profile"));
        expect(sourceClaims(KEY, "pi", rotated, profile)).toEqual(claims);
        const environment = selectAwsSource({});
        expect(sourceClaims(KEY, "pi", rotated, environment)["amazon-bedrock"]).not.toBe(
            sourceClaims(KEY, "pi", first, environment)["amazon-bedrock"],
        );
        expect(sourceClaimVersion("pi", rotated, profile)).toBe(
            sourceClaimVersion("pi", first, profile),
        );
        const otherBearer = sourceClaims(new Uint8Array(32).fill(9), "pi", first, profile);
        expect(otherBearer["amazon-bedrock"]).not.toBe(claims["amazon-bedrock"]);
        const replaced = selectAwsSource({ ...PROFILE_ENV, AWS_PROFILE: "other" });
        expect(sourceClaims(KEY, "pi", first, replaced)["amazon-bedrock"]).not.toBe(
            claims["amazon-bedrock"],
        );
        expect(sourceClaimVersion("pi", first, replaced)).not.toBe(
            sourceClaimVersion("pi", first, profile),
        );
    });

    test("environment claims change with the row and direct keys invalidate the version", () => {
        const environment = selectAwsSource({});
        const row = {
            AWS_ACCESS_KEY_ID: "AKIA",
            AWS_SECRET_ACCESS_KEY: "s",
            AWS_REGION: "us-east-1",
            ANTHROPIC_API_KEY: "k",
        };
        const rotated = { ...row, AWS_SESSION_TOKEN: "t" };
        expect(sourceClaims(KEY, "pi", rotated, environment)["amazon-bedrock"]).not.toBe(
            sourceClaims(KEY, "pi", row, environment)["amazon-bedrock"],
        );
        expect(sourceClaimVersion("pi", rotated, environment)).not.toBe(
            sourceClaimVersion("pi", row, environment),
        );
        const profile = selectAwsSource(PROFILE_ENV);
        expect(sourceClaimVersion("pi", { ...row, ANTHROPIC_API_KEY: "k2" }, profile)).not.toBe(
            sourceClaimVersion("pi", row, profile),
        );
    });

    test("profile mode keeps the other providers' environment rows and never matches v3", () => {
        const env = { ...PROFILE_ENV, ANTHROPIC_API_KEY: "k" };
        const claims = sourceClaims(KEY, "opencode", env, selectAwsSource(env));
        expect(Object.keys(claims).sort()).toEqual(["amazon-bedrock", "anthropic"]);
        const legacy = credentialFingerprints(KEY, "opencode", env);
        expect(claims.anthropic).not.toBe(legacy.anthropic);
        expect(() => sourceClaims(new Uint8Array(31), "pi", env, selectAwsSource({}))).toThrow(
            /exactly 32/,
        );
    });
});
