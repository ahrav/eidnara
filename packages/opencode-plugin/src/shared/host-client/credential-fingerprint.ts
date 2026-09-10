import { createHmac } from "node:crypto";
import hostRelease from "../../../../../release/host-release.json";
import type { BrocaProvider, CredentialFingerprints } from "./types";

const DOMAIN: string = hostRelease.credential_fingerprint.domain;
const CANONICALIZATION: string = hostRelease.credential_fingerprint.canonicalization;
export const BROCA_CREDENTIAL_VALUE_CAP_BYTES: number =
    hostRelease.harness_unavailable.value_cap_bytes;

const PROVIDER_ROWS = {
    anthropic: ["ANTHROPIC_API_KEY"],
    google: ["GEMINI_API_KEY"],
    openai: ["OPENAI_API_KEY"],
} as const satisfies Record<BrocaProvider, readonly string[]>;

export const BROCA_CREDENTIAL_NAMES = Object.freeze(Object.values(PROVIDER_ROWS).flat());

function encoded(field: string): string {
    return `${Buffer.byteLength(field)}:${field}`;
}

export function canonicalCredentialRowEncoding(
    harness: "opencode" | "pi",
    provider: BrocaProvider,
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
    const fingerprints: Partial<Record<BrocaProvider, string>> = {};
    for (const [provider, names] of Object.entries(PROVIDER_ROWS) as [
        BrocaProvider,
        readonly string[],
    ][]) {
        const entries: [string, string][] = [];
        let complete = true;
        for (const name of names) {
            const value = source[name];
            // An unqualified value drops only this provider's row, matching the host's per-provider `provider_row` in `crates/host-runtime/src/broca/subprocess.rs`.
            if (
                value === undefined ||
                value.length === 0 ||
                Buffer.byteLength(value) > BROCA_CREDENTIAL_VALUE_CAP_BYTES
            ) {
                complete = false;
                break;
            }
            entries.push([name, value]);
        }
        if (!complete) continue;
        const message = canonicalCredentialRowEncoding(harness, provider, entries);
        fingerprints[provider] = createHmac("sha256", derivedKey).update(message).digest("hex");
    }
    return Object.freeze(fingerprints);
}
