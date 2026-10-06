import { describe, expect, test } from "bun:test";
import vectors from "../../../../crates/host-runtime/tests/fixtures/source-health-vectors.json";
import hostRelease from "../../../../release/host-release.json";
import {
    AWS_CREDENTIALS_FIELDS,
    AWS_CREDENTIALS_KINDS,
    AWS_CREDENTIALS_STATES,
    awsCredentialsHealth,
    formatAwsCredentialsHealth,
    MAX_CONSECUTIVE_FAILURES,
    parseAwsCredentialsBlock,
    UNKNOWN_AWS_CREDENTIALS,
} from "./source-health";

const VALID = { kind: "profile", state: "ready", consecutive_failures: 0 };

describe("AWS credential source health", () => {
    test("the TypeScript vocabulary is the release vocabulary and the shared field set", () => {
        expect([...AWS_CREDENTIALS_KINDS]).toEqual(hostRelease.aws_credentials.kinds as never);
        expect([...AWS_CREDENTIALS_STATES]).toEqual(hostRelease.aws_credentials.states as never);
        expect([...AWS_CREDENTIALS_FIELDS]).toEqual(vectors.fields as never);
        expect(MAX_CONSECUTIVE_FAILURES).toBe(vectors.max_consecutive_failures);
    });

    test("the parser matches the Rust sanitizer on every shared vector", () => {
        for (const vector of vectors.cases) {
            expect({ name: vector.name, parsed: parseAwsCredentialsBlock(vector.input) }).toEqual({
                name: vector.name,
                parsed: vector.closed as never,
            });
        }
    });

    test("every closed kind and state parses and renders without numbers it lacks", () => {
        for (const kind of hostRelease.aws_credentials.kinds) {
            for (const state of hostRelease.aws_credentials.states) {
                const health = awsCredentialsHealth({ kind, state, consecutive_failures: 0 });
                expect(health).toEqual({ kind, state, consecutive_failures: 0 } as never);
                expect(formatAwsCredentialsHealth(health)).toBe(`${kind} ${state}`);
            }
        }
    });

    test("bounded numbers render and out-of-bound numbers make the block unknown", () => {
        const health = awsCredentialsHealth({
            kind: "profile",
            state: "cooldown",
            expires_in_seconds: 86_400,
            next_retry_in_seconds: 300,
            consecutive_failures: 4_294_967_295,
        });
        expect(formatAwsCredentialsHealth(health)).toBe(
            "profile cooldown, expires in 86400s, retry in 300s, 4294967295 consecutive failures",
        );
        for (const [key, value] of [
            ["expires_in_seconds", 86_401],
            ["next_retry_in_seconds", 301],
            ["expires_in_seconds", -1],
            ["expires_in_seconds", 1.5],
            ["consecutive_failures", 4_294_967_296],
            ["consecutive_failures", "1"],
        ] as const) {
            expect(awsCredentialsHealth({ ...VALID, [key]: value })).toBe(UNKNOWN_AWS_CREDENTIALS);
        }
    });

    test("a missing, malformed, or extra-key block is unknown and never ready", () => {
        const canary = "canary-secret-value";
        for (const value of [
            undefined,
            null,
            "ready",
            [],
            { ...VALID, state: "expired" },
            { ...VALID, kind: "static" },
            { ...VALID, profile: canary },
            { ...VALID, role_arn: canary },
            { ...VALID, sso_cache_root: canary },
            { ...VALID, error: canary },
            { kind: "profile", state: "ready" },
        ]) {
            const health = awsCredentialsHealth(value);
            expect(health).toBe(UNKNOWN_AWS_CREDENTIALS);
            expect(health.state).not.toBe("ready");
            expect(formatAwsCredentialsHealth(health)).toBe("unknown");
            expect(JSON.stringify(health)).not.toContain(canary);
        }
        expect(parseAwsCredentialsBlock({ ...VALID, token: canary })).toBeNull();
    });
});
