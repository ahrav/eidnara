import { createHash, createHmac } from "node:crypto";
import type { AwsProfileSource, AwsSourceSelection } from "./aws-source";
import { MODEL_EXECUTION_PROVIDERS, providerRowEntries } from "./credential-fingerprint";
import type { CredentialFingerprints, ModelExecutionProvider } from "./types";

export const SOURCE_CLAIM_DOMAIN = "eidnara-model-execution-credential-v4";
export const SOURCE_CLAIM_CANONICALIZATION = "harness-provider-source/2";
const SOURCE_CLAIM_VERSION_DOMAIN = "eidnara-host-route-source-claims-v1";

const PROFILE_PROVIDER: ModelExecutionProvider = "amazon-bedrock";

export type ClaimSource =
    | { readonly kind: "env"; readonly entries: readonly (readonly [string, string])[] }
    | { readonly kind: "aws_profile"; readonly profile: AwsProfileSource };

function field(value: string): string {
    return `${Buffer.byteLength(value)}:${value}`;
}

export function sourceClaimPreimage(
    harness: "opencode" | "pi",
    provider: ModelExecutionProvider,
    source: ClaimSource,
): string {
    let message =
        field(SOURCE_CLAIM_CANONICALIZATION) +
        field(harness) +
        field(provider) +
        field(source.kind);
    if (source.kind === "env") {
        for (const [name, value] of source.entries) {
            message += field(name) + field(String(Buffer.byteLength(value))) + field(value);
        }
    } else {
        const { profile, region, config_file, credentials_file, sso_cache_root } = source.profile;
        for (const part of [profile, region, config_file, credentials_file, sso_cache_root]) {
            message += field(part);
        }
    }
    return message;
}

function claimSource(
    provider: ModelExecutionProvider,
    env: Readonly<Record<string, string | undefined>>,
    selection: AwsSourceSelection,
): ClaimSource | undefined {
    if (provider === PROFILE_PROVIDER && selection.mode === "profile") {
        return { kind: "aws_profile", profile: selection.source };
    }
    const entries = providerRowEntries(provider, env);
    return entries === undefined ? undefined : { kind: "env", entries };
}

function preimages(
    harness: "opencode" | "pi",
    env: Readonly<Record<string, string | undefined>>,
    selection: AwsSourceSelection,
): [ModelExecutionProvider, string][] {
    const out: [ModelExecutionProvider, string][] = [];
    for (const provider of MODEL_EXECUTION_PROVIDERS) {
        const source = claimSource(provider, env, selection);
        if (source !== undefined)
            out.push([provider, sourceClaimPreimage(harness, provider, source)]);
    }
    return out;
}

export function sourceClaims(
    connectionKey: Uint8Array,
    harness: "opencode" | "pi",
    env: Readonly<Record<string, string | undefined>>,
    selection: AwsSourceSelection,
): CredentialFingerprints {
    if (connectionKey.byteLength !== 32) {
        throw new TypeError("connection key must be exactly 32 bytes");
    }
    const derivedKey = createHmac("sha256", connectionKey).update(SOURCE_CLAIM_DOMAIN).digest();
    const claims: Partial<Record<ModelExecutionProvider, string>> = {};
    for (const [provider, message] of preimages(harness, env, selection)) {
        claims[provider] = createHmac("sha256", derivedKey).update(message).digest("hex");
    }
    return Object.freeze(claims);
}

export function sourceClaimVersion(
    harness: "opencode" | "pi",
    env: Readonly<Record<string, string | undefined>>,
    selection: AwsSourceSelection,
): string {
    const hash = createHash("sha256").update(field(SOURCE_CLAIM_VERSION_DOMAIN));
    for (const [provider, message] of preimages(harness, env, selection)) {
        hash.update(field(provider) + field(message));
    }
    return hash.digest("hex");
}
