import hostRelease from "../../../../release/host-release.json";

const VOCABULARY = hostRelease.aws_credentials;

export const AWS_CREDENTIALS_KINDS = ["none", "environment", "profile"] as const;
export const AWS_CREDENTIALS_STATES = [
    "unknown",
    "ready",
    "refreshing",
    "cooldown",
    "login_required",
    "invalid",
] as const;
export const AWS_CREDENTIALS_FIELDS = [
    "kind",
    "state",
    "expires_in_seconds",
    "next_retry_in_seconds",
    "consecutive_failures",
] as const;
export const MAX_CONSECUTIVE_FAILURES = 0xffff_ffff;

export type AwsCredentialsKind = (typeof AWS_CREDENTIALS_KINDS)[number];
export type AwsCredentialsState = (typeof AWS_CREDENTIALS_STATES)[number];

/** One closed block as the host emits it. */
export interface AwsCredentialsHealth {
    readonly kind: AwsCredentialsKind;
    readonly state: AwsCredentialsState;
    readonly expires_in_seconds?: number;
    readonly next_retry_in_seconds?: number;
    readonly consecutive_failures: number;
}

/** What renderers show for an absent or malformed block; it carries no kind. */
export const UNKNOWN_AWS_CREDENTIALS = Object.freeze({
    state: "unknown",
    consecutive_failures: 0,
} as const);

export type AwsCredentialsView = AwsCredentialsHealth | typeof UNKNOWN_AWS_CREDENTIALS;

function isMember<T extends string>(set: readonly T[], value: unknown): value is T {
    return typeof value === "string" && (set as readonly string[]).includes(value);
}

function boundedInteger(value: unknown, max: number): number | null {
    return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= max
        ? value
        : null;
}

export function parseAwsCredentialsBlock(value: unknown): AwsCredentialsHealth | null {
    if (value === null || typeof value !== "object" || Array.isArray(value)) return null;
    const record = value as Record<string, unknown>;
    if (Object.keys(record).some((key) => !isMember(AWS_CREDENTIALS_FIELDS, key))) return null;
    const { kind, state } = record;
    if (!isMember(AWS_CREDENTIALS_KINDS, kind) || !isMember(AWS_CREDENTIALS_STATES, state)) {
        return null;
    }
    const failures = boundedInteger(record.consecutive_failures, MAX_CONSECUTIVE_FAILURES);
    if (failures === null) return null;
    const optional = (key: string, max: number): number | undefined | null =>
        record[key] === undefined ? undefined : boundedInteger(record[key], max);
    const expires = optional("expires_in_seconds", VOCABULARY.max_expires_in_seconds);
    const retry = optional("next_retry_in_seconds", VOCABULARY.max_next_retry_in_seconds);
    if (expires === null || retry === null) return null;
    return Object.freeze({
        kind,
        state,
        ...(expires === undefined ? {} : { expires_in_seconds: expires }),
        ...(retry === undefined ? {} : { next_retry_in_seconds: retry }),
        consecutive_failures: failures,
    });
}

export function awsCredentialsHealth(value: unknown): AwsCredentialsView {
    return parseAwsCredentialsBlock(value) ?? UNKNOWN_AWS_CREDENTIALS;
}

export function formatAwsCredentialsHealth(view: AwsCredentialsView): string {
    if (!("kind" in view)) return view.state;
    const parts: string[] = [`${view.kind} ${view.state}`];
    if (view.expires_in_seconds !== undefined) {
        parts.push(`expires in ${view.expires_in_seconds}s`);
    }
    if (view.next_retry_in_seconds !== undefined) {
        parts.push(`retry in ${view.next_retry_in_seconds}s`);
    }
    if (view.consecutive_failures > 0) {
        const noun = view.consecutive_failures === 1 ? "failure" : "failures";
        parts.push(`${view.consecutive_failures} consecutive ${noun}`);
    }
    return parts.join(", ");
}
