import { createHmac } from "node:crypto";
import hostRelease from "../../../../../release/host-release.json";
import type { CredentialFingerprints, ModelExecutionProvider } from "./types";

const DOMAIN: string = hostRelease.credential_fingerprint.domain;
const CANONICALIZATION: string = hostRelease.credential_fingerprint.canonicalization;
export const MODEL_EXECUTION_CREDENTIAL_VALUE_CAP_BYTES: number =
    hostRelease.harness_unavailable.value_cap_bytes;

/**
 * Row variables in row order, mirroring `provider_row_spec` in
 * `crates/host-runtime/src/model_execution/subprocess.rs`. A required variable must be present
 * and non-empty; an optional one joins the row only when it is, so both sides fingerprint the
 * same sequence.
 */
const PROVIDER_ROWS = {
    anthropic: { order: ["ANTHROPIC_API_KEY"], optional: [] },
    google: { order: ["GEMINI_API_KEY"], optional: [] },
    openai: { order: ["OPENAI_API_KEY"], optional: [] },
    // Explicit static or STS credentials plus region; `AWS_PROFILE` and credential files stay out.
    "amazon-bedrock": {
        order: ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN", "AWS_REGION"],
        optional: ["AWS_SESSION_TOKEN"],
    },
} as const satisfies Record<
    ModelExecutionProvider,
    { order: readonly string[]; optional: readonly string[] }
>;

export const STATIC_AWS_CREDENTIAL_NAMES: readonly string[] = Object.freeze([
    ...PROVIDER_ROWS["amazon-bedrock"].order,
]);

export const MODEL_EXECUTION_CREDENTIAL_NAMES = Object.freeze(
    Object.values(PROVIDER_ROWS).flatMap((row) => [...row.order]),
);

function encoded(field: string): string {
    return `${Buffer.byteLength(field)}:${field}`;
}

export function canonicalCredentialRowEncoding(
    harness: "opencode" | "pi",
    provider: ModelExecutionProvider,
    entries: readonly (readonly [string, string])[],
): string {
    let message = encoded(CANONICALIZATION) + encoded(harness) + encoded(provider);
    for (const [name, value] of entries) {
        message += encoded(name) + encoded(String(Buffer.byteLength(value))) + encoded(value);
    }
    return message;
}

export function credentialFingerprints(
    connectionKey: Uint8Array,
    harness: "opencode" | "pi",
    source: Record<string, string | undefined>,
): CredentialFingerprints {
    if (connectionKey.byteLength !== 32) {
        throw new TypeError("connection key must be exactly 32 bytes");
    }
    const derivedKey = createHmac("sha256", connectionKey).update(DOMAIN).digest();
    const fingerprints: Partial<Record<ModelExecutionProvider, string>> = {};
    for (const provider of MODEL_EXECUTION_PROVIDERS) {
        const entries = providerRowEntries(provider, source);
        if (entries === undefined) continue;
        const message = canonicalCredentialRowEncoding(harness, provider, entries);
        fingerprints[provider] = createHmac("sha256", derivedKey).update(message).digest("hex");
    }
    return Object.freeze(fingerprints);
}

export const MODEL_EXECUTION_PROVIDERS = Object.freeze(
    Object.keys(PROVIDER_ROWS) as ModelExecutionProvider[],
);

export function providerRowEntries(
    provider: ModelExecutionProvider,
    source: Readonly<Record<string, string | undefined>>,
): [string, string][] | undefined {
    const row: { order: readonly string[]; optional: readonly string[] } = PROVIDER_ROWS[provider];
    const entries: [string, string][] = [];
    for (const name of row.order) {
        const value = source[name];
        if (value === undefined || value.length === 0) {
            if (row.optional.includes(name)) continue;
            return undefined;
        }
        if (Buffer.byteLength(value) > MODEL_EXECUTION_CREDENTIAL_VALUE_CAP_BYTES) return undefined;
        entries.push([name, value]);
    }
    return entries;
}
