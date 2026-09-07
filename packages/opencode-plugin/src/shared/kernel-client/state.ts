/**
 * The daemon-produced members mirror `crates/daemon/src/kernel_routes/state.rs`
 * one to one; the literal sets below must stay byte-identical to that file's
 * serde output. The client-only members describe outcomes the daemon never
 * sees: the feature is off, the caller cancelled, or no daemon could be
 * reached. `MEMORY_STATE_GUIDANCE` is total over `StateKey`, so adding a member
 * without guidance is a compile error.
 */

export const UNAVAILABLE_REASONS = [
    "store_starting",
    "store_unavailable",
    "store_unsupported",
    "store_busy",
    "no_required_consumer",
    "snapshot_diverged",
    "queue_full",
] as const;
export type DaemonUnavailableReason = (typeof UNAVAILABLE_REASONS)[number];

export const CONFLICT_REASONS = ["known_as_of_advanced", "retracted", "superseded"] as const;
export type ConflictReason = (typeof CONFLICT_REASONS)[number];

export const INVALID_REASONS = [
    "project_mismatch",
    "operation_key_reused",
    "class_over_declared",
    "invalid_input",
    "admission_policy",
    "not_found",
    "already_exists",
    "revision_not_advanced",
    "scope_reserved",
    "payload_too_large",
    "page_digest",
    "page_index",
    "page_too_large",
    "payload_digest",
    "upload_not_found",
    "ingestion_fail_closed",
    "artifact_unusable",
    "internal",
] as const;
export type DaemonInvalidReason = (typeof INVALID_REASONS)[number];

/** `daemon_absent` and `outcome_unknown` are minted by the client; the daemon cannot report its own absence or a response the transport lost after the request was sent. commentlint: allow(JUDGE) */
export type UnavailableReason = DaemonUnavailableReason | "daemon_absent" | "outcome_unknown";
/** `unrecognized_state` is minted by the client for a `state` it cannot classify. */
export type InvalidReason = DaemonInvalidReason | "unrecognized_state";

export interface LagFacts {
    lag_positions: number;
    oldest_unconsumed_age_ms: number;
}

export type MemoryState =
    | { kind: "available" }
    | ({ kind: "stale" } & LagFacts)
    | ({ kind: "abstained" } & LagFacts)
    | { kind: "unavailable"; reason: UnavailableReason }
    | { kind: "conflict"; reason: ConflictReason }
    | { kind: "invalid"; reason: InvalidReason }
    | { kind: "disabled" }
    | { kind: "cancelled" };

/** `kind` for members without a reason, `kind:reason` for members with one. */
export type StateKey = MemoryState extends infer S
    ? S extends { kind: infer K extends string; reason: infer R extends string }
        ? `${K}:${R}`
        : S extends { kind: infer K extends string }
          ? K
          : never
    : never;

export function stateKey(state: MemoryState): StateKey {
    return ("reason" in state ? `${state.kind}:${state.reason}` : state.kind) as StateKey;
}

export interface Guidance {
    /** One sentence for a tool result. */
    tool: string;
}

export const MEMORY_STATE_GUIDANCE = {
    available: {
        tool: "Memory is current.",
    },
    stale: {
        tool: "Memory results may lag recent changes; the projector has not caught up.",
    },
    abstained: {
        tool: "Automatic memory search was withheld because the projector is behind; use explicit search if needed.",
    },
    "unavailable:store_starting": {
        tool: "Memory is unavailable while the store opens; it becomes available once opening completes.",
    },
    "unavailable:store_unavailable": {
        tool: "Memory is unavailable because the store failed or lost its lease.",
    },
    "unavailable:store_unsupported": {
        tool: "Memory is unavailable because this build cannot open the store; run the doctor check.",
    },
    "unavailable:store_busy": {
        tool: "Memory is unavailable because the store is busy; the next natural request re-probes.",
    },
    "unavailable:no_required_consumer": {
        tool: "Memory freshness cannot be judged because no consumer is registered.",
    },
    "unavailable:snapshot_diverged": {
        tool: "Memory is unavailable because the known snapshot is ahead of the store; cached tokens were dropped.",
    },
    "unavailable:queue_full": {
        tool: "Memory is unavailable because the artifact upload queue is full.",
    },
    "unavailable:daemon_absent": {
        tool: "Memory is unavailable because the daemon is not running.",
    },
    "unavailable:outcome_unknown": {
        tool: "The memory request was interrupted after it was sent and may have been applied; read the memory back to see whether it took effect.",
    },
    "conflict:known_as_of_advanced": {
        tool: "The object changed since it was read; read it again before writing.",
    },
    "conflict:retracted": {
        tool: "The object was retracted; read again and choose a live object.",
    },
    "conflict:superseded": {
        tool: "The object was superseded; read again and target its replacement.",
    },
    "invalid:project_mismatch": {
        tool: "The request named a project other than the bound one.",
    },
    "invalid:operation_key_reused": {
        tool: "The operation key was reused with a different request digest.",
    },
    "invalid:class_over_declared": {
        tool: "The asserted source or sensitivity class is above what the daemon derives.",
    },
    "invalid:invalid_input": {
        tool: "The kernel rejected the request as invalid input.",
    },
    "invalid:admission_policy": {
        tool: "Admission policy rejected the request.",
    },
    "invalid:not_found": {
        tool: "The named object does not exist, is not live, or is not scoped to this project.",
    },
    "invalid:already_exists": {
        tool: "The write named an id the registry already holds; retrying the same write cannot succeed.",
    },
    "invalid:revision_not_advanced": {
        tool: "The successor's source revision does not exceed its predecessor's; use a higher revision.",
    },
    "invalid:scope_reserved": {
        tool: "A foreign scope occupies this project's reserved scope id; the write cannot succeed until it is removed.",
    },
    "invalid:payload_too_large": {
        tool: "The payload exceeds the size the kernel accepts.",
    },
    "invalid:page_digest": {
        tool: "An upload page did not match its declared digest.",
    },
    "invalid:page_index": {
        tool: "An upload page fell outside the declared layout.",
    },
    "invalid:page_too_large": {
        tool: "An upload page decodes to more bytes than one page may carry.",
    },
    "invalid:payload_digest": {
        tool: "The assembled payload did not hash to the declared digest.",
    },
    "invalid:upload_not_found": {
        tool: "The named upload is not in flight on this route.",
    },
    "invalid:ingestion_fail_closed": {
        tool: "Artifact ingestion is fail-closed until the store reopens.",
    },
    "invalid:artifact_unusable": {
        tool: "The artifact is not live or holds a secret the redactor cannot rewrite.",
    },
    "invalid:internal": {
        tool: "Memory hit an internal error; see the plugin log.",
    },
    "invalid:unrecognized_state": {
        tool: "The daemon answered with a state this client does not recognize; update the plugin or daemon.",
    },
    disabled: {
        tool: "Memory is disabled by configuration (memory.enabled = false).",
    },
    cancelled: {
        tool: "The memory request was cancelled before it completed.",
    },
} as const satisfies Record<StateKey, Guidance>;

export function guidanceFor(state: MemoryState): Guidance {
    return MEMORY_STATE_GUIDANCE[stateKey(state)];
}

export const ALL_STATE_KEYS = Object.keys(MEMORY_STATE_GUIDANCE) as StateKey[];

export const available = (): MemoryState => ({ kind: "available" });
export const unavailable = (reason: UnavailableReason): MemoryState => ({
    kind: "unavailable",
    reason,
});
export const conflict = (reason: ConflictReason): MemoryState => ({ kind: "conflict", reason });
export const invalid = (reason: InvalidReason): MemoryState => ({ kind: "invalid", reason });
export const disabled = (): MemoryState => ({ kind: "disabled" });
export const cancelled = (): MemoryState => ({ kind: "cancelled" });
export const abstained = (lag: LagFacts): MemoryState => ({ kind: "abstained", ...lag });
