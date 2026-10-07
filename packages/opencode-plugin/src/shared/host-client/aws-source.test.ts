import { describe, expect, test } from "bun:test";
import vectors from "../../../../../crates/host-runtime/tests/fixtures/aws-source-vectors.json";
import {
    AwsSourceError,
    captureAwsSource,
    parseAwsProfileSource,
    selectAwsSource,
    sourceBinding,
} from "./aws-source";

function errorCode(run: () => unknown): string | undefined {
    try {
        run();
        return undefined;
    } catch (error) {
        if (error instanceof AwsSourceError) return error.code;
        throw error;
    }
}

describe("AWS source selector", () => {
    for (const vector of vectors.selector) {
        test(`selector vector: ${vector.name}`, () => {
            const env = vector.env as Record<string, string | undefined>;
            if ("error" in vector.expect) {
                expect(errorCode(() => selectAwsSource(env))).toBe(vector.expect.error);
            } else {
                expect(selectAwsSource(env)).toEqual(vector.expect as never);
            }
        });
    }

    for (const vector of vectors.dto) {
        test(`DTO vector: ${vector.name}`, () => {
            const code = errorCode(() => parseAwsProfileSource(vector.value));
            expect(code === undefined).toBe(vector.valid);
            expect(code).toBe((vector as { error?: string }).error);
        });
    }

    test("rejects a path that is not well-formed UTF-16 and so has no UTF-8 encoding", () => {
        const source = vectors.dto[0]?.value as Record<string, string>;
        expect(errorCode(() => parseAwsProfileSource({ ...source, config_file: "/a\ud800" }))).toBe(
            "not_utf8",
        );
    });

    test("captures one frozen selection shared by every owner with equal values", () => {
        const env = { HOME: "/home/u", AWS_PROFILE: "corp", AWS_REGION: "us-east-1" };
        const first = captureAwsSource(env);
        const second = captureAwsSource({ ...env, HOME: "/home/./u/" });
        expect(second).toBe(first);
        expect(Object.isFrozen(first)).toBe(true);
        expect(first.mode === "profile" && Object.isFrozen(first.source)).toBe(true);
        expect(captureAwsSource({ ...env, AWS_REGION: "eu-west-1" })).not.toBe(first);
        env.AWS_REGION = "eu-west-1";
        expect(first.mode === "profile" && first.source.region).toBe("us-east-1");
        expect(captureAwsSource({})).toBe(captureAwsSource({ HOME: "/x" }));
    });

    for (const vector of vectors.strip) {
        test(`strip vector: ${vector.name}`, () => {
            const profile = selectAwsSource({
                HOME: "/home/u",
                AWS_PROFILE: "corp",
                AWS_REGION: "us-east-1",
            });
            const credentials = vector.credentials as Record<string, string>;
            expect(sourceBinding(profile, credentials).credentials ?? {}).toEqual(
                vector.kept as never,
            );
        });
    }

    test("profile mode strips every static AWS variable and keeps direct keys", () => {
        const legacy = {
            ANTHROPIC_API_KEY: "k",
            AWS_ACCESS_KEY_ID: "AKIA",
            AWS_SECRET_ACCESS_KEY: "s",
            AWS_SESSION_TOKEN: "t",
            AWS_REGION: "us-east-1",
        };
        const profile = selectAwsSource({
            HOME: "/home/u",
            AWS_PROFILE: "corp",
            AWS_REGION: "us-east-1",
        });
        const binding = sourceBinding(profile, legacy);
        expect(binding.credentials).toEqual({ ANTHROPIC_API_KEY: "k" });
        expect(binding.aws_source).toEqual(profile.mode === "profile" ? profile.source : never());
        const environment = sourceBinding(selectAwsSource({}), legacy);
        expect(environment).toEqual({ credentials: legacy });
        expect(environment.credentials).not.toBe(legacy);
    });
});

function never(): never {
    throw new Error("unreachable");
}
