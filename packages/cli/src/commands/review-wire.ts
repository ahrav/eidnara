import {
    exactCount,
    exactI64,
    exactU64,
    formatExactInteger,
    type WireInteger,
} from "@eidnara/opencode/shared/host-client";
import { parseKernelState } from "@eidnara/opencode/shared/kernel-client/wire";
import { isRecord } from "@eidnara/opencode/shared/record-type-guard";

/** Kernel `MAX_REVIEW_TEXT_BYTES`: the longest text a staged proposal carries. */
export const MAX_TEXT_BYTES = 32 * 1024;
/** Kernel `MAX_REVIEW_IDENTITY_BYTES`: the longest identifier a staged proposal carries. */
export const MAX_IDENTITY_BYTES = 512;
/** Kernel `MAX_REVIEW_REFERENCES`: `check_references` bounds support and contradictions at this many each, not together; the daemon's own producer keeps their sum under it before staging, but the Kernel admits either list at the bound. */
const MAX_REFERENCES = 256;
const MAX_LIMITATIONS = 16;
export const MAX_PAGE_ITEMS = 64;

export const OUTCOMES = [
    "complete",
    "abstained",
    "failed",
    "cancelled",
    "unknown",
    "expired",
] as const;
export const ABSTAIN_REASONS = [
    "owner_sensitive",
    "wrong_scope",
    "secret",
    "expectation_changed",
    "undisclosed_citation",
    "partial_disclosure",
    "model_declined",
    "budget_exhausted",
    "invalid_proposal",
] as const;
export const READ_TERMINALS = [
    "not_selected",
    "incarnation_mismatch",
    "selection_mismatch",
    "kernel_refused",
    "review_expired",
    "dependency_refused",
    "disabled",
    "store_unavailable",
] as const;
export type ReadTerminal = (typeof READ_TERMINALS)[number];
export const LIST_TERMINALS = [
    "disabled",
    "store_unavailable",
] as const satisfies readonly ReadTerminal[];
const ACTIONS = ["create", "revise", "retain", "retire", "no_change"] as const;
const UNCERTAINTIES = ["low", "medium", "high"] as const;

export type Outcome = (typeof OUTCOMES)[number];
export type AbstainReason = (typeof ABSTAIN_REASONS)[number];

export interface ListItem {
    causal_identity: string;
    generation: WireInteger;
    outcome: Outcome;
    reason?: AbstainReason;
    selected: boolean;
}

export interface Span {
    alias: string;
    start: WireInteger;
    end: WireInteger;
}

export interface Reference {
    evidence_id: string;
    span?: Span;
}

export type Target =
    | { kind: "staged_candidate"; candidate_id: string }
    | {
          kind: "memory";
          object_id: string;
          source_revision: WireInteger;
          known_as_of: WireInteger;
          commit_token: WireInteger;
      };

export interface Proposal {
    action: (typeof ACTIONS)[number];
    target: Target;
    new_text?: string;
    support: Reference[];
    contradictions: Reference[];
    limitations: string[];
    uncertainty: (typeof UNCERTAINTIES)[number];
    manifest: { manifest_id: string; digest: string };
}

export type ReviewAnswer<Terminal extends string, Body> =
    | { kind: "body"; body: Body }
    | { kind: "terminal"; terminal: Terminal }
    | { kind: "state"; state: ReturnType<typeof parseKernelState> }
    | { kind: "malformed"; detail: string };

export interface Page {
    items: ListItem[];
    next: string | null;
}

export interface Selected {
    causal_identity: string;
    reference: { database_incarnation_id: string; candidate_id: string; payload_digest: string };
    proposal: Proposal;
    review_expires_at: WireInteger;
}

const HEX64 = /^[0-9a-f]{64}$/;
const utf8 = new TextEncoder();

export function isHex64(value: unknown): value is string {
    return typeof value === "string" && HEX64.test(value);
}

function boundedText(value: unknown, maxBytes: number): string | null {
    if (typeof value !== "string" || utf8.encode(value).byteLength > maxBytes) return null;
    return value;
}

function oneOf<const T extends readonly string[]>(set: T, value: unknown): value is T[number] {
    return typeof value === "string" && (set as readonly string[]).includes(value);
}

/** Every review answer is a body under its own `kind`, a closed `terminal`, or a kernel `state`. */
function classify<Terminal extends string, Body>(
    raw: unknown,
    terminals: readonly Terminal[],
    bodyKind: string,
    decode: (body: Record<string, unknown>) => Body | string,
): ReviewAnswer<Terminal, Body> {
    if (!isRecord(raw)) return { kind: "malformed", detail: "response is not an object" };
    if ("state" in raw) return { kind: "state", state: parseKernelState(raw.state) };
    if (raw.kind === "terminal") {
        return oneOf(terminals, raw.terminal)
            ? { kind: "terminal", terminal: raw.terminal as Terminal }
            : { kind: "malformed", detail: "terminal is not in the protocol vocabulary" };
    }
    if (raw.kind !== bodyKind) return { kind: "malformed", detail: "kind is not recognized" };
    const decoded = decode(raw);
    return typeof decoded === "string"
        ? { kind: "malformed", detail: decoded }
        : { kind: "body", body: decoded };
}

function decodeItem(raw: unknown): ListItem | string {
    if (!isRecord(raw)) return "item is not an object";
    if (!isHex64(raw.causal_identity)) return "item causal_identity is not a hex64";
    const generation = exactU64(raw.generation);
    if (generation === null) return "item generation is not a u64";
    if (!oneOf(OUTCOMES, raw.outcome)) return "item outcome is not in the protocol vocabulary";
    if (typeof raw.selected !== "boolean") return "item selected is not a boolean";
    // Only a `Complete` receipt carries a selection, so the two fields must agree.
    if (raw.selected !== (raw.outcome === "complete"))
        return "item selected disagrees with its outcome";
    const item: ListItem = {
        causal_identity: raw.causal_identity,
        generation,
        outcome: raw.outcome,
        selected: raw.selected,
    };
    if (raw.outcome === "abstained") {
        if (!oneOf(ABSTAIN_REASONS, raw.reason)) return "abstained item reason is not recognized";
        item.reason = raw.reason;
    } else if ("reason" in raw) {
        return "reason is present on a non-abstained item";
    }
    return item;
}

/**
 * `limit` and `after` are the request's page size and cursor: the wire contract bounds the page by
 * `limit`, orders identities strictly after `after`, and sets `next` to the last identity exactly when the page is full.
 */
export function decodePage(
    raw: unknown,
    limit: number,
    after: string | null,
): ReviewAnswer<(typeof LIST_TERMINALS)[number], Page> {
    return classify(raw, LIST_TERMINALS, "page", (body) => {
        if (!Array.isArray(body.items) || body.items.length > limit) {
            return "items exceed the requested page";
        }
        const items: ListItem[] = [];
        let previous = after ?? "";
        for (const raw of body.items) {
            const item = decodeItem(raw);
            if (typeof item === "string") return item;
            if (item.causal_identity <= previous) return "items are not ordered after the cursor";
            previous = item.causal_identity;
            items.push(item);
        }
        if (body.next !== null && !isHex64(body.next)) return "next is not null or a hex64";
        const expectedNext =
            items.length === limit ? (items.at(-1)?.causal_identity ?? null) : null;
        if (body.next !== expectedNext) return "next does not follow the page";
        return { items, next: body.next };
    });
}

function decodeReferences(raw: unknown): Reference[] | string {
    if (!Array.isArray(raw) || raw.length > MAX_REFERENCES)
        return "references are not a bounded array";
    const references: Reference[] = [];
    for (const entry of raw) {
        if (!isRecord(entry)) return "reference is not an object";
        const evidence_id = boundedText(entry.evidence_id, MAX_IDENTITY_BYTES);
        if (evidence_id === null) return "reference evidence_id is not bounded text";
        const reference: Reference = { evidence_id };
        if (entry.span !== undefined) {
            if (!isRecord(entry.span)) return "span is not an object";
            const alias = boundedText(entry.span.alias, MAX_IDENTITY_BYTES);
            const start = exactU64(entry.span.start);
            const end = exactU64(entry.span.end);
            if (alias === null || start === null || end === null || BigInt(end) <= BigInt(start)) {
                return "span is not a bounded alias with end after start";
            }
            reference.span = { alias, start, end };
        }
        references.push(reference);
    }
    return references;
}

function decodeTarget(raw: unknown): Target | string {
    if (!isRecord(raw)) return "target is not an object";
    if (raw.kind === "staged_candidate") {
        const candidate_id = boundedText(raw.candidate_id, MAX_IDENTITY_BYTES);
        return candidate_id === null
            ? "staged candidate id is not bounded text"
            : { kind: "staged_candidate", candidate_id };
    }
    if (raw.kind !== "memory") return "target kind is not recognized";
    const object_id = boundedText(raw.object_id, MAX_IDENTITY_BYTES);
    const source_revision = exactI64(raw.source_revision);
    const known_as_of = exactI64(raw.known_as_of);
    const commit_token = exactI64(raw.commit_token);
    if (
        object_id === null ||
        source_revision === null ||
        known_as_of === null ||
        commit_token === null
    ) {
        return "memory target carries a field outside its domain";
    }
    return { kind: "memory", object_id, source_revision, known_as_of, commit_token };
}

function decodeProposal(raw: unknown): Proposal | string {
    if (!isRecord(raw)) return "proposal is not an object";
    if (!oneOf(ACTIONS, raw.action)) return "action is not recognized";
    const target = decodeTarget(raw.target);
    if (typeof target === "string") return target;
    const support = decodeReferences(raw.support);
    if (typeof support === "string") return `support ${support}`;
    const contradictions = decodeReferences(raw.contradictions);
    if (typeof contradictions === "string") return `contradictions ${contradictions}`;
    if (!Array.isArray(raw.limitations) || raw.limitations.length > MAX_LIMITATIONS) {
        return "limitations are not a bounded array";
    }
    const limitations: string[] = [];
    for (const entry of raw.limitations) {
        const text = boundedText(entry, MAX_TEXT_BYTES);
        if (text === null) return "limitation is not bounded text";
        limitations.push(text);
    }
    if (!oneOf(UNCERTAINTIES, raw.uncertainty)) return "uncertainty is not recognized";
    if (!isRecord(raw.manifest)) return "manifest is not an object";
    const manifest_id = boundedText(raw.manifest.manifest_id, MAX_IDENTITY_BYTES);
    const digest = raw.manifest.digest;
    if (manifest_id === null || !isHex64(digest)) return "manifest is not an id and a hex64 digest";
    const proposal: Proposal = {
        action: raw.action,
        target,
        support,
        contradictions,
        limitations,
        uncertainty: raw.uncertainty,
        manifest: { manifest_id, digest },
    };
    if (raw.new_text !== undefined) {
        const text = boundedText(raw.new_text, MAX_TEXT_BYTES);
        if (text === null) return "new_text is not bounded text";
        proposal.new_text = text;
    }
    return proposal;
}

/** `requested` is the identity the read named; the answer echoes it, so another identity is a skewed or unrelated proposal. */
export function decodeSelected(
    raw: unknown,
    requested: string,
): ReviewAnswer<(typeof READ_TERMINALS)[number], Selected> {
    return classify(raw, READ_TERMINALS, "proposal", (body) => {
        if (!isHex64(body.causal_identity)) return "causal_identity is not a hex64";
        if (body.causal_identity !== requested)
            return "causal_identity is not the requested identity";
        if (!isRecord(body.reference)) return "reference is not an object";
        const database_incarnation_id = boundedText(
            body.reference.database_incarnation_id,
            MAX_IDENTITY_BYTES,
        );
        const candidate_id = boundedText(body.reference.candidate_id, MAX_IDENTITY_BYTES);
        const payload_digest = body.reference.payload_digest;
        if (database_incarnation_id === null || candidate_id === null || !isHex64(payload_digest)) {
            return "reference carries a field outside its domain";
        }
        const proposal = decodeProposal(body.proposal);
        if (typeof proposal === "string") return proposal;
        const review_expires_at = exactI64(body.review_expires_at);
        if (review_expires_at === null) return "review_expires_at is not an i64";
        return {
            causal_identity: body.causal_identity,
            reference: { database_incarnation_id, candidate_id, payload_digest },
            proposal,
            review_expires_at,
        };
    });
}

export const MEMORY_REVIEWER_STATES = ["ready", "starting", "unavailable"] as const;
export const ACTIVATION_STATES = [
    "open",
    "unknown",
    "stale",
    "missing",
    "refused",
    "unreadable",
    "malformed",
    "identity_mismatch",
    "unacknowledged",
    "unknown_credential",
    "unavailable",
    "store",
] as const;
/** Every counter the wire document lists for `metrics.memory_reviewer`, in its order. */
export const STATUS_COUNTERS = [
    "swept_jobs",
    "swept_selections",
    "jobs_reserved",
    "jobs_ready",
    "jobs_expired",
    "jobs_expired_unseen",
    "jobs_nonadmitted",
    "jobs_failed",
    "jobs_unknown",
    "jobs_completed",
    "jobs_abstained",
    "selections_frozen",
    "selections_enqueued",
    "selections_expired",
    "selections_failed_slot",
    "attempts_attempted",
    "attempts_acknowledged",
    "attempts_failed",
    "attempts_cancelled",
    "attempts_unknown",
    "attempts_not_dispatched",
    "attempts_open",
    "receipts_in_progress",
    "receipts_complete",
    "receipt_charge_bytes",
    "allowance_bytes",
    "metadata_bytes",
    "metadata_quota_bytes",
    "metadata_headroom_bytes",
    "nonadmissions",
    "sessions_with_reservation",
    "latest_nonadmission_memory_reviewer_unavailable",
    "latest_nonadmission_capacity_full",
    "latest_nonadmission_evidence_unavailable",
    "latest_nonadmission_fact_set_rejected",
    "latest_nonadmission_subject_refused",
] as const;

export interface ReviewStatus {
    /** `null` when the block is absent or its state is not recognized. */
    memory_reviewer_state: (typeof MEMORY_REVIEWER_STATES)[number] | null;
    activation_state: (typeof ACTIVATION_STATES)[number] | null;
    sampled_at_ms: number | null;
    /** Every counter is present; `null` means unavailable, never zero. */
    counters: Record<(typeof STATUS_COUNTERS)[number], number | null>;
}

function memoryReviewerBlock(metrics: Record<string, unknown>): Record<string, unknown> | null {
    if (!isRecord(metrics.components) || !isRecord(metrics.components.context)) return null;
    const context = metrics.components.context;
    if (!isRecord(context.metrics) || !isRecord(context.metrics.memory_reviewer)) return null;
    return context.metrics.memory_reviewer;
}

export function decodeReviewStatus(metrics: Record<string, unknown>): ReviewStatus {
    const block = memoryReviewerBlock(metrics);
    const state = oneOf(MEMORY_REVIEWER_STATES, block?.memory_reviewer_state)
        ? block?.memory_reviewer_state
        : null;
    const counters = Object.fromEntries(
        STATUS_COUNTERS.map((name) => [
            name,
            state === "ready" && block !== null ? exactCount(block[name]) : null,
        ]),
    ) as ReviewStatus["counters"];
    return {
        memory_reviewer_state: state ?? null,
        activation_state:
            state !== null && oneOf(ACTIVATION_STATES, block?.activation_state)
                ? block?.activation_state
                : null,
        sampled_at_ms: state !== null && block !== null ? exactCount(block.sampled_at_ms) : null,
        counters,
    };
}

export function integerText(value: WireInteger): string {
    return formatExactInteger(value);
}
