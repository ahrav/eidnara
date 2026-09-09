/**
 * The only place daemon bytes become a `MemoryState`. Every parser here is
 * total: a shape violation yields `invalid(unrecognized_state)` instead of a
 * throw, so a daemon/plugin version skew degrades to a typed state.
 */

import { isRecord } from "../record-type-guard";
import {
    CONFLICT_REASONS,
    type ConflictReason,
    type DaemonInvalidReason,
    type DaemonUnavailableReason,
    INVALID_REASONS,
    invalid,
    type MemoryState,
    UNAVAILABLE_REASONS,
} from "./state";

export const SENSITIVITIES = ["normal", "sensitive", "secret"] as const;
export type Sensitivity = (typeof SENSITIVITIES)[number];

export const VISIBILITIES = ["visible", "labeled", "hidden"] as const;
export type Visibility = (typeof VISIBILITIES)[number];

export interface ObjectRow {
    object_id: string;
    object_kind: string;
    domain_id: string;
    source_kind: string;
    source_id: string;
    source_revision: number;
    created_commit_seq: number;
    invalidated_commit_seq: number | null;
    superseded_by: string | null;
    sensitivity: Sensitivity;
}

export interface MutationToken {
    object_id: string;
    known_as_of: number;
}

export interface DecisionPayload {
    summary: string;
    rationale: string;
}

/** The decision row a `decision` object carries; other object kinds carry none. */
export interface ReadDecision {
    decision_kind: string;
    payload: DecisionPayload;
}

export interface ReadRow {
    object: ObjectRow;
    visibility: Visibility;
    labeled: boolean;
    scope_id: string | null;
    token: MutationToken;
    decision?: ReadDecision;
}

export const MEMORY_DOMAIN_ID = "memory";

/**
 * `kernel.read` rejects a longer `object_ids` filter with `invalid_params`.
 * A filtered read scopes visible rows to named objects before the daemon applies its row cap, so a targeted lookup reaches rows a capped unfiltered read drops. commentlint: allow(JUDGE)
 */
export const MAX_READ_OBJECT_IDS = 64;

/** `kernel.commit` rejects an envelope carrying more operations with `invalid_params` before any kernel work. Mirrors the daemon route's `MAX_OPERATIONS`. */
export const MAX_COMMIT_OPERATIONS = 256;

/** `kernel.commit` rejects an envelope carrying more tokens with `invalid_params` before any kernel work. Mirrors the daemon route's `MAX_TOKENS`. */
export const MAX_COMMIT_TOKENS = 1024;

/** A decision row in the memory domain; `decision_kind` carries the memory category, not the domain. commentlint: allow(JUDGE) */
export function isMemoryDecisionRow(row: ReadRow): row is ReadRow & { decision: ReadDecision } {
    return row.decision !== undefined && row.object.domain_id === MEMORY_DOMAIN_ID;
}

export interface ReadPayload {
    known_as_of: number;
    tip: number;
    gated: boolean;
    /** Whether the daemon dropped rows to fit its per-read row and byte bounds; dropped rows are the oldest, and objects they name still mutate through commit-side token checks. commentlint: allow(JUDGE) */
    truncated: boolean;
    rows: ReadRow[];
}

/** The event kinds a host command may name; each is a disposition transition the kernel's fixed table resolves, never a maturity promotion. commentlint: allow(JUDGE) */
export const DISPOSITION_EVENTS = [
    "mark_stale",
    "mark_disputed",
    "explicit_reject",
    "contradict",
    "quarantine",
] as const;
export type DispositionEvent = (typeof DISPOSITION_EVENTS)[number];

/** What one disposition operation did or would do. `outcome` is the kernel's command result and `disposition` the resulting state; `denied` marks a relaxation refused for want of a valid approval. commentlint: allow(JUDGE) */
export interface DispositionResult {
    object_id: string;
    event: DispositionEvent;
    outcome: string;
    previous_disposition: string;
    disposition: string;
    denied: boolean;
}

export interface SurfaceVisibilities {
    auto_inject: Visibility;
    auto_search: Visibility;
    explicit_search: Visibility;
}

/** `visibility_changes` is true when a surface that serves the object now would show a different verdict afterwards. */
export interface DispositionPreview extends DispositionResult {
    current: SurfaceVisibilities;
    projected: SurfaceVisibilities;
    visibility_changes: boolean;
}

export interface PreviewPayload {
    known_as_of: number;
    previews: DispositionPreview[];
    /** Present when the intent's identity is already recorded: the commit would replay this receipt, so nothing was judged and `previews` is empty. commentlint: allow(JUDGE) */
    receipt?: { commit_seq: number; replayed: true };
}

export interface CommitPayload {
    receipt: { commit_seq: number; replayed: boolean };
    known_as_of: number;
    tokens: MutationToken[];
    /** IDs of supersede survivors whose replacement spec was discarded because the survivor was already live; the daemon only re-pointed the predecessor, so the submitted content was not written. commentlint: allow(JUDGE) */
    merged: string[];
    /** One entry per disposition operation, in request order. */
    dispositions: DispositionResult[];
}

export interface ParsedResponse {
    state: MemoryState;
    /** The response body minus `state`; empty when the state is not `available`. */
    payload: Record<string, unknown>;
}

const UNRECOGNIZED: MemoryState = invalid("unrecognized_state");

function isNonNegativeInteger(value: unknown): value is number {
    return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function oneOf<const T extends readonly string[]>(set: T, value: unknown): value is T[number] {
    return typeof value === "string" && (set as readonly string[]).includes(value);
}

/** The transport hands back either the body or `{result: body}`. */
function responseBody(raw: unknown): Record<string, unknown> | null {
    if (!isRecord(raw)) return null;
    if (isRecord(raw.result) && !("state" in raw)) return raw.result;
    return raw;
}

export function parseKernelState(raw: unknown): MemoryState {
    if (!isRecord(raw) || typeof raw.kind !== "string") return UNRECOGNIZED;
    switch (raw.kind) {
        case "available":
            return { kind: "available" };
        case "stale":
        case "abstained":
            if (
                !isNonNegativeInteger(raw.lag_positions) ||
                !isNonNegativeInteger(raw.oldest_unconsumed_age_ms)
            ) {
                return UNRECOGNIZED;
            }
            return {
                kind: raw.kind,
                lag_positions: raw.lag_positions,
                oldest_unconsumed_age_ms: raw.oldest_unconsumed_age_ms,
            };
        case "unavailable":
            return oneOf(UNAVAILABLE_REASONS, raw.reason)
                ? { kind: "unavailable", reason: raw.reason as DaemonUnavailableReason }
                : UNRECOGNIZED;
        case "conflict":
            return oneOf(CONFLICT_REASONS, raw.reason)
                ? { kind: "conflict", reason: raw.reason as ConflictReason }
                : UNRECOGNIZED;
        case "invalid":
            return oneOf(INVALID_REASONS, raw.reason)
                ? { kind: "invalid", reason: raw.reason as DaemonInvalidReason }
                : UNRECOGNIZED;
        default:
            return UNRECOGNIZED;
    }
}

export function parseKernelResponse(raw: unknown): ParsedResponse {
    const body = responseBody(raw);
    if (!body) return { state: UNRECOGNIZED, payload: {} };
    const state = parseKernelState(body.state);
    if (state.kind !== "available") return { state, payload: {} };
    const { state: _state, ...payload } = body;
    return { state, payload };
}

function parseToken(raw: unknown): MutationToken | null {
    if (!isRecord(raw)) return null;
    if (typeof raw.object_id !== "string" || !isNonNegativeInteger(raw.known_as_of)) return null;
    return { object_id: raw.object_id, known_as_of: raw.known_as_of };
}

function parseTokens(raw: unknown): MutationToken[] | null {
    if (!Array.isArray(raw)) return null;
    const tokens: MutationToken[] = [];
    for (const item of raw) {
        const token = parseToken(item);
        if (!token) return null;
        tokens.push(token);
    }
    return tokens;
}

function parseStrings(raw: unknown): string[] | null {
    if (!Array.isArray(raw)) return null;
    const strings: string[] = [];
    for (const item of raw) {
        if (typeof item !== "string") return null;
        strings.push(item);
    }
    return strings;
}

function parseObjectRow(raw: unknown): ObjectRow | null {
    if (!isRecord(raw)) return null;
    const strings = ["object_id", "object_kind", "domain_id", "source_kind", "source_id"] as const;
    for (const key of strings) {
        if (typeof raw[key] !== "string") return null;
    }
    if (!isNonNegativeInteger(raw.source_revision)) return null;
    if (!isNonNegativeInteger(raw.created_commit_seq)) return null;
    // The registry's CHECK constraint keeps an invalidation strictly after creation. commentlint: allow(JUDGE)
    if (
        raw.invalidated_commit_seq !== null &&
        (!isNonNegativeInteger(raw.invalidated_commit_seq) ||
            raw.invalidated_commit_seq <= raw.created_commit_seq)
    ) {
        return null;
    }
    if (raw.superseded_by !== null && typeof raw.superseded_by !== "string") return null;
    if (!oneOf(SENSITIVITIES, raw.sensitivity)) return null;
    return {
        object_id: raw.object_id as string,
        object_kind: raw.object_kind as string,
        domain_id: raw.domain_id as string,
        source_kind: raw.source_kind as string,
        source_id: raw.source_id as string,
        source_revision: raw.source_revision,
        created_commit_seq: raw.created_commit_seq,
        invalidated_commit_seq: raw.invalidated_commit_seq,
        superseded_by: raw.superseded_by,
        sensitivity: raw.sensitivity,
    };
}

/** `undefined` for an absent or null decision; `null` for a malformed one. */
function parseReadDecision(raw: unknown): ReadDecision | undefined | null {
    if (raw === undefined || raw === null) return undefined;
    if (!isRecord(raw) || typeof raw.decision_kind !== "string") return null;
    const payload = raw.payload;
    if (!isRecord(payload)) return null;
    if (typeof payload.summary !== "string" || typeof payload.rationale !== "string") return null;
    return {
        decision_kind: raw.decision_kind,
        payload: { summary: payload.summary, rationale: payload.rationale },
    };
}

function parseReadRow(raw: unknown): ReadRow | null {
    if (!isRecord(raw)) return null;
    const object = parseObjectRow(raw.object);
    const token = parseToken(raw.token);
    if (!object || !token) return null;
    if (!oneOf(VISIBILITIES, raw.visibility) || typeof raw.labeled !== "boolean") return null;
    if (raw.scope_id !== null && typeof raw.scope_id !== "string") return null;
    if (token.object_id !== object.object_id) return null;
    const decision = parseReadDecision(raw.decision);
    if (decision === null) return null;
    // A decision object always carries its decision row; a daemon that omits it
    // predates the field, and the row would otherwise vanish silently.
    if (decision === undefined && object.object_kind === "decision") return null;
    if (decision !== undefined && object.object_kind !== "decision") return null;
    return {
        object,
        visibility: raw.visibility,
        labeled: raw.labeled,
        scope_id: raw.scope_id,
        token,
        ...(decision === undefined ? {} : { decision }),
    };
}

export type Parsed<P> = { state: MemoryState; payload: P | null };

function failed<P>(): Parsed<P> {
    return { state: UNRECOGNIZED, payload: null };
}

export function parseReadResponse(raw: unknown): Parsed<ReadPayload> {
    const { state, payload } = parseKernelResponse(raw);
    if (state.kind !== "available") return { state, payload: null };
    // A snapshot must not be newer than the store tip.
    if (
        !isNonNegativeInteger(payload.known_as_of) ||
        !isNonNegativeInteger(payload.tip) ||
        payload.known_as_of > payload.tip
    ) {
        return failed();
    }
    if (typeof payload.gated !== "boolean" || !Array.isArray(payload.rows)) return failed();
    // A daemon that predates the flag omits it. commentlint: allow(JUDGE)
    if (payload.truncated !== undefined && typeof payload.truncated !== "boolean") {
        return failed();
    }
    const truncated = payload.truncated === true;
    const rows: ReadRow[] = [];
    for (const item of payload.rows) {
        const row = parseReadRow(item);
        if (!row || row.token.known_as_of !== payload.known_as_of) return failed();
        rows.push(row);
    }
    return {
        state,
        payload: {
            known_as_of: payload.known_as_of,
            tip: payload.tip,
            gated: payload.gated,
            truncated,
            rows,
        },
    };
}

function parseDispositionResult(raw: unknown): DispositionResult | null {
    if (!isRecord(raw)) return null;
    if (typeof raw.object_id !== "string" || !oneOf(DISPOSITION_EVENTS, raw.event)) return null;
    for (const key of ["outcome", "previous_disposition", "disposition"] as const) {
        if (typeof raw[key] !== "string") return null;
    }
    if (typeof raw.denied !== "boolean") return null;
    return {
        object_id: raw.object_id,
        event: raw.event,
        outcome: raw.outcome as string,
        previous_disposition: raw.previous_disposition as string,
        disposition: raw.disposition as string,
        denied: raw.denied,
    };
}

function parseDispositionResults(raw: unknown): DispositionResult[] | null {
    if (!Array.isArray(raw)) return null;
    const results: DispositionResult[] = [];
    for (const item of raw) {
        const result = parseDispositionResult(item);
        if (!result) return null;
        results.push(result);
    }
    return results;
}

function parseSurfaceVisibilities(raw: unknown): SurfaceVisibilities | null {
    if (!isRecord(raw)) return null;
    const { auto_inject, auto_search, explicit_search } = raw;
    if (
        !oneOf(VISIBILITIES, auto_inject) ||
        !oneOf(VISIBILITIES, auto_search) ||
        !oneOf(VISIBILITIES, explicit_search)
    ) {
        return null;
    }
    return { auto_inject, auto_search, explicit_search };
}

const SURFACE_KEYS = ["auto_inject", "auto_search", "explicit_search"] as const;

/** The daemon's rule: a surface serving the object now would show a different verdict; a hidden surface changing is not a visible change. */
export function visibilityChanges(
    current: SurfaceVisibilities,
    projected: SurfaceVisibilities,
): boolean {
    return SURFACE_KEYS.some(
        (surface) => current[surface] !== "hidden" && current[surface] !== projected[surface],
    );
}

function parseDispositionPreview(raw: unknown): DispositionPreview | null {
    const result = parseDispositionResult(raw);
    if (!result || !isRecord(raw)) return null;
    const current = parseSurfaceVisibilities(raw.current);
    const projected = parseSurfaceVisibilities(raw.projected);
    if (!current || !projected || typeof raw.visibility_changes !== "boolean") return null;
    // The flag gates the confirmation prompt, so a reply whose flag disagrees with its own verdicts is refused rather than trusted. commentlint: allow(JUDGE)
    if (raw.visibility_changes !== visibilityChanges(current, projected)) return null;
    return { ...result, current, projected, visibility_changes: raw.visibility_changes };
}

export function parsePreviewResponse(raw: unknown): Parsed<PreviewPayload> {
    const { state, payload } = parseKernelResponse(raw);
    if (state.kind !== "available") return { state, payload: null };
    if (!isNonNegativeInteger(payload.known_as_of) || !Array.isArray(payload.previews)) {
        return failed();
    }
    const previews: DispositionPreview[] = [];
    for (const item of payload.previews) {
        const preview = parseDispositionPreview(item);
        if (!preview) return failed();
        previews.push(preview);
    }
    if (payload.receipt === undefined) {
        return { state, payload: { known_as_of: payload.known_as_of, previews } };
    }
    const receipt = payload.receipt;
    if (!isRecord(receipt) || !isNonNegativeInteger(receipt.commit_seq)) return failed();
    if (receipt.replayed !== true || previews.length > 0) return failed();
    return {
        state,
        payload: {
            known_as_of: payload.known_as_of,
            previews,
            receipt: { commit_seq: receipt.commit_seq, replayed: true },
        },
    };
}

export function parseCommitResponse(raw: unknown): Parsed<CommitPayload> {
    const { state, payload } = parseKernelResponse(raw);
    if (state.kind !== "available") return { state, payload: null };
    const receipt = payload.receipt;
    if (!isRecord(receipt) || !isNonNegativeInteger(receipt.commit_seq)) return failed();
    if (typeof receipt.replayed !== "boolean") return failed();
    // `known_as_of` and every token position are `receipt.commit_seq` on the daemon side; a payload that disagrees would cache a mutation boundary that masks an intervening change or forces a spurious conflict. commentlint: allow(JUDGE)
    if (payload.known_as_of !== receipt.commit_seq) return failed();
    const tokens = parseTokens(payload.tokens);
    if (!tokens || tokens.some((token) => token.known_as_of !== receipt.commit_seq)) {
        return failed();
    }
    // A daemon that predates the field omits it. commentlint: allow(JUDGE)
    const merged = payload.merged === undefined ? [] : parseStrings(payload.merged);
    if (!merged) return failed();
    // The daemon omits the list when the envelope carried no disposition. commentlint: allow(JUDGE)
    const dispositions =
        payload.dispositions === undefined ? [] : parseDispositionResults(payload.dispositions);
    if (!dispositions) return failed();
    return {
        state,
        payload: {
            receipt: { commit_seq: receipt.commit_seq, replayed: receipt.replayed },
            known_as_of: receipt.commit_seq,
            tokens,
            merged,
            dispositions,
        },
    };
}
