/**
 * The revision 3 transform pass core, shared by every harness adapter. One client owns, per
 * session, the declared boundary, the retained output a later pass may keep from or fail open to,
 * and the capture basis that lets a later window skip re-taping its verified prefix. A pass reads
 * plain data through a {@link TransformPassSource}: the host's length and ids for discovery, the
 * window's native values, and the pass inputs the adapter computes. The client builds and pages the
 * request, walks discovery, applies the recipe, and hands the applied values to the source's
 * `publish`; the source alone writes the host's array.
 */

import { randomUUID } from "node:crypto";

import { sessionLog } from "../../shared/logger";
import {
    applyRecipe,
    canonicalJsonLength,
    parseRecipe,
    type RecipeSourceBase,
} from "./edit-recipe";
import { type HarnessProfileIdentity, validateInvocation } from "./invocation-budget";
import {
    isModuleTransportGenerationChangedResult,
    TRANSFORM_SEND_TIMEOUT_MS,
} from "./module-transport";
import { buildPagedModuleTransformPayloads, type ModuleMethod } from "./module-wire";
import {
    CaptureBudgetExceeded,
    type CapturedHistory,
    type CapturedMessages,
    type CaptureLease,
    capturedMessagesUnchanged,
    captureHistory,
    captureMessages,
    filterMayHold,
    fnv1a32,
    type HistoryDigest,
    historyDigestsEqual,
    inspectReferenceableMessages,
} from "./transform-capture";
import { logTransformTiming } from "./transform-stage-logger";
import { type TransformWindowSize, withinHalfCap } from "./window-cap";

export interface RustModeModuleClient {
    call(args: {
        sessionId: string;
        projectRoot: string;
        method: ModuleMethod;
        body: unknown;
        signal?: AbortSignal;
        generationSensitive?: boolean;
        /** Capture flush waits up to 20 seconds in the daemon; its transport must outlive that wait. */
        timeoutMs?: number;
    }): Promise<unknown>;
    deleteSession?(sessionId: string, projectRoot: string): Promise<void>;
    closeSession?(sessionId: string): void;
    hasSessionRoute?(sessionId: string): boolean;
}

/** Retained outputs are bounded to the sessions one process keeps active. An evicted session captures its next pass in full and has no `previous` source. */
export const RETAINED_OUTPUT_SESSION_CAPACITY = 64;
/** Retained outputs are optional reuse state, budgeted apart from the frame and reconstruction caps. */
export const RETAINED_OUTPUT_BUDGET_BYTES = 64 * 1024 * 1024;
/** Each retained canonical length occupies one number slot. */
const LENGTH_SLOT_BYTES = 8;
/** Each retained message keeps an input length and a wire bound; the history digest is fixed size. */
const HISTORY_ENTRY_RETAINED_BYTES = 2 * LENGTH_SLOT_BYTES;
const CANDIDATE_SLOT_BYTES = 8;
/** Discovery walks from the pass start within the transform deadline less a reserve for the transform itself. */
const DISCOVERY_BUDGET_MS = TRANSFORM_SEND_TIMEOUT_MS - 4_000;
if (DISCOVERY_BUDGET_MS <= 0) throw new Error("the transform deadline leaves no discovery budget");
/** Charged on top of the heuristic estimate because the estimator undercounts relative to the provider's tokenizer. */
const INVOCATION_HEADROOM_PERMILLE = 250;
/** WIRE_PROJECTION_FACTOR accounts for the CK text, the native text, the paging parse copy, and the page texts. */
const WIRE_PROJECTION_FACTOR = 4;
/** `transform.boundary` lists at most this many anchors per page (docs/host-wire-protocol.md). */
const MAX_ANCHOR_PAGE = 4096;

/** One successfully applied output, eligible as the `previous` source of the next recipe. */
interface AppliedOutput {
    revision: string;
    values: readonly unknown[];
    lengths: readonly number[];
    capture: CapturedMessages;
    /** Canonical bytes, retained lengths, and snapshots; part of the record's charge. */
    charge: number;
}

/**
 * A session's retained output: the capture basis of the window its last published pass submitted,
 * which lets the next capture of a window declared at the same anchor verify that prefix instead
 * of taping it again and lets a failed pass at that anchor decide whether the applied output is
 * still served, and the output that pass applied.
 */
interface RetainedOutput {
    /** The acknowledgment basis: the anchor the submitted window started at. */
    readonly basis: TransformBoundary | null;
    readonly rawCount: number;
    /** Digest of every submitted message but the last. */
    readonly rawHistory: HistoryDigest;
    /** The former terminal alone; the host may edit it in place, so it is compared separately. */
    readonly rawTerminal?: HistoryDigest;
    /** Inspection wire bounds of the submitted messages, reused for the verified prefix. */
    readonly wireBytes: readonly number[];
    /** Canonical JSON length of each submitted native message, reused for the verified prefix. */
    readonly inputLengths: readonly number[];
    /** Cleared only by `RetainedOutputs`, which owns the charge. */
    readonly applied?: AppliedOutput;
    /**
     * Set by `RetainedOutputs` once a pass sourced from this record drew `boundary_unknown`; the
     * daemon may have dropped the basis, so the applied output no longer serves a failed pass.
     */
    readonly disowned?: boolean;
    /** One charge for the whole record, the applied output's included. */
    readonly charge: number;
}

/** The fields `RetainedOutputs` alone may change, keeping `used` equal to the summed charges. */
type OwnedRetainedOutput = { applied?: AppliedOutput; disowned?: boolean; charge: number };

/**
 * Holds one retained output per session under a session-count and a byte limit, evicting the
 * least recently used record first; a record is used when it is retained or read with `get`.
 * A failed pass reads its record too, so it refreshes its session. Eviction never releases a
 * capture lease.
 */
export class RetainedOutputs {
    private readonly records = new Map<string, RetainedOutput>();
    private used = 0;

    constructor(
        private readonly maxSessions: number,
        private readonly maxBytes: number,
    ) {}

    /** Returns the record and moves it to the back of the eviction order. */
    get(sessionId: string): RetainedOutput | undefined {
        const record = this.records.get(sessionId);
        if (record === undefined) return undefined;
        this.records.delete(sessionId);
        this.records.set(sessionId, record);
        return record;
    }

    /** Returns the record without changing the eviction order. */
    peek(sessionId: string): RetainedOutput | undefined {
        return this.records.get(sessionId);
    }

    /** Retains `record`; one over the whole budget is retained without its applied output, else refused. */
    retain(sessionId: string, record: RetainedOutput): boolean {
        this.release(sessionId);
        if (record.charge > this.maxBytes) {
            if (!record.applied || record.charge - record.applied.charge > this.maxBytes)
                return false;
            this.clearApplied(record);
        }
        for (const [oldest] of this.records) {
            if (this.records.size < this.maxSessions && this.used + record.charge <= this.maxBytes)
                break;
            this.release(oldest);
        }
        this.records.set(sessionId, record);
        this.used += record.charge;
        return true;
    }

    /** Keeps the record's basis and gives back the applied output's share of the charge. */
    dropApplied(sessionId: string, record: RetainedOutput): void {
        if (!record.applied || this.records.get(sessionId) !== record) return;
        this.used -= record.applied.charge;
        this.clearApplied(record);
    }

    /** Bars the record's applied output from fail-open until a publication retains a new record. */
    disown(sessionId: string, record: RetainedOutput): void {
        if (this.records.get(sessionId) === record) (record as OwnedRetainedOutput).disowned = true;
    }

    release(sessionId: string): void {
        const record = this.records.get(sessionId);
        if (record === undefined) return;
        this.records.delete(sessionId);
        this.used -= record.charge;
    }

    private clearApplied(record: RetainedOutput): void {
        const owned = record as OwnedRetainedOutput;
        owned.charge -= owned.applied?.charge ?? 0;
        owned.applied = undefined;
    }

    get usedBytes(): number {
        return this.used;
    }
}

let baseRevisionCounter = 0;
const baseRevisionNonce = randomUUID().slice(0, 8);

/** Names one pass's submitted input; the daemon echoes it so the recipe binds to that snapshot. */
function nextBaseRevision(): string {
    baseRevisionCounter += 1;
    return `${baseRevisionNonce}-${baseRevisionCounter.toString(36)}`;
}

/** A history segment's end message, as `transform.boundary` lists it (Section 7.10.2). */
export interface TransformBoundary {
    readonly mid: string;
    readonly sequence: number;
}

function sameBoundary(left: TransformBoundary | null, right: TransformBoundary | null): boolean {
    return left === right || (left?.mid === right?.mid && left?.sequence === right?.sequence);
}

/** `undefined` when `value` is neither `null` nor a well-formed anchor. */
export function parseBoundary(value: unknown): TransformBoundary | null | undefined {
    if (value === null) return null;
    if (!isRecord(value) || !value.mid || typeof value.mid !== "string") return undefined;
    return Number.isSafeInteger(value.sequence)
        ? { mid: value.mid, sequence: value.sequence as number }
        : undefined;
}

/** One `transform.boundary` page, newest first and strictly below `before`; `undefined` when malformed or over the page cap. */
function parseAnchorPage(
    reply: unknown,
    before: number | undefined,
): TransformBoundary[] | undefined {
    const anchors = isRecord(reply) ? reply.anchors : undefined;
    if (!Array.isArray(anchors) || anchors.length > MAX_ANCHOR_PAGE) return undefined;
    const page: TransformBoundary[] = [];
    let bound = before ?? Number.POSITIVE_INFINITY;
    for (const entry of anchors) {
        const anchor = parseBoundary(entry);
        if (!anchor || anchor.sequence >= bound) return undefined;
        bound = anchor.sequence;
        page.push(anchor);
    }
    return page;
}

/** The client's per-session pass state. */
export interface TransformSessionState {
    initialized: boolean;
    consecutiveFailures: number;
    passCount: number;
    /** The declared anchor: `null` sends the whole array, `undefined` is not yet discovered. */
    boundary: TransformBoundary | null | undefined;
    /**
     * The head message of a cold import (spec D8): with no anchor, an array past half the
     * window cap sends the suffix from this message until the first anchor exists.
     */
    pinnedHead?: string;
    failureCount: number;
    routeRoot: string | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object";
}

/**
 * A failed pass may serve the retained output only when every window message the previous pass
 * submitted is unchanged and in place. A verified capture already proves the members before the
 * terminal, so this checks the terminal; an edit of it leaves the retained output stale.
 */
function isAppendOnlyExtension(previous: RetainedOutput, captured: CapturedHistory): boolean {
    return (
        previous.rawCount === 0 ||
        (previous.rawTerminal !== undefined &&
            captured.boundary !== undefined &&
            historyDigestsEqual(captured.boundary, previous.rawTerminal))
    );
}

/**
 * Lengths for the submitted native array: members the capture verified against the retained
 * digest keep the lengths measured when they were first sent, and only the rest are measured.
 */
function measureInputLengths(
    messages: readonly unknown[],
    previous: RetainedOutput | undefined,
    verifiedCount: number,
): number[] {
    const lengths = previous ? previous.inputLengths.slice(0, verifiedCount) : [];
    for (let index = lengths.length; index < messages.length; index += 1)
        lengths.push(canonicalJsonLength(messages[index]));
    return lengths;
}

export interface RustPassTimings {
    prefixGuard: number;
    clone: number;
    wireBuild: number;
    wireMessages: number;
    transport: number;
    transportPages: number;
    transportBytes: number;
    apply: number;
    /** Host slots the id scans and the membership filter read. */
    scannedItems: number;
    /** The pass's capture-lease charge and the retained record's charge. */
    chargedBytes: number;
    retainedBytes: number;
}

function emptyRustPassTimings(): RustPassTimings {
    return {
        prefixGuard: 0,
        clone: 0,
        wireBuild: 0,
        wireMessages: 0,
        transport: 0,
        transportPages: 0,
        transportBytes: 0,
        apply: 0,
        scannedItems: 0,
        chargedBytes: 0,
        retainedBytes: 0,
    };
}

export function formatRustPassLog(args: {
    decision: string;
    reason: string;
    servedFrom: string;
    inputCount: number;
    outputCount: number;
    applied: boolean;
    elapsedMs: number;
    moduleElapsedMs: number;
    rowVersion: number;
    emergencyWaitMs?: number;
    rediscovered?: boolean;
    timings?: RustPassTimings;
}): string {
    const timings = args.timings ?? emptyRustPassTimings();
    const measured =
        timings.prefixGuard + timings.clone + timings.wireBuild + timings.transport + timings.apply;
    const unattributed = Math.max(0, args.elapsedMs - measured);
    const rowVersion = Number.isSafeInteger(args.rowVersion) ? args.rowVersion : 0;
    return `rust pass: decision=${args.decision} reason=${args.reason} served_from=${args.servedFrom} in=${args.inputCount} out=${args.outputCount} applied=${args.applied} row_version=${rowVersion} emergency_wait=${(args.emergencyWaitMs ?? 0).toFixed(1)} rediscovered=${args.rediscovered === true} elapsed=${args.elapsedMs.toFixed(1)} ms module=${args.moduleElapsedMs.toFixed(1)} ms stages=prefix_guard:${timings.prefixGuard.toFixed(1)} clone:${timings.clone.toFixed(1)} wire_build:${timings.wireBuild.toFixed(1)} wire_messages:${timings.wireMessages} transport:${timings.transport.toFixed(1)} transport_pages:${timings.transportPages} transport_bytes:${timings.transportBytes} apply:${timings.apply.toFixed(1)} other:${unattributed.toFixed(1)} work=scanned:${timings.scannedItems} charged:${timings.chargedBytes} retained:${timings.retainedBytes}`;
}

function responseValue(response: unknown): Record<string, unknown> {
    if (isRecord(response) && isRecord(response.result)) return response.result;
    if (isRecord(response)) return response;
    throw new Error("module transform returned a non-object response");
}

function errorHasCode(error: unknown, code: string): boolean {
    const seen = new Set<unknown>();
    for (let current = error; isRecord(current) && !seen.has(current); current = current.cause) {
        seen.add(current);
        if (current.code === code) return true;
    }
    return false;
}

export function isTransformPageAttemptMismatch(error: unknown): boolean {
    let current = error;
    const seen = new Set<unknown>();
    while (isRecord(current) && !seen.has(current)) {
        seen.add(current);
        const code = typeof current.code === "string" ? current.code : "";
        const message = typeof current.message === "string" ? current.message : "";
        if (
            code === "attempt_mismatch" ||
            code === "authority_transform_page_attempt_mismatch" ||
            /\b(?:authority_transform_page_)?attempt_mismatch\b/.test(message)
        ) {
            return true;
        }
        current = current.cause;
    }
    return false;
}

function noteDeliveryPassIds(response: Record<string, unknown>): string[] {
    if (!Array.isArray(response.note_deliveries)) return [];
    return [
        ...new Set(
            response.note_deliveries.flatMap((delivery) => {
                if (!isRecord(delivery)) return [];
                const passId = delivery.transform_pass_id;
                return typeof passId === "string" && passId.length > 0 ? [passId] : [];
            }),
        ),
    ];
}

/**
 * Applies a `status: ok` response's recipe against the submitted input and, when the daemon named
 * it, the retained previous output. The result is a fresh array of shared references, sized and
 * validated before any allocation; nothing is published on failure.
 */
export function applyTransformRecipe(
    response: Record<string, unknown>,
    input: RecipeSourceBase,
    previous: RecipeSourceBase | undefined,
    reserve: (slots: number, insertedSlots: number) => boolean,
): { values: unknown[]; lengths: number[]; bytes: number; outputRevision: string } {
    const parsed = parseRecipe(response);
    if (!parsed.ok) {
        throw new Error(
            `rust transform recipe rejected: ${parsed.rejection.code}: ${parsed.rejection.detail}`,
        );
    }
    let slots = 0;
    let insertedSlots = 0;
    for (const operation of parsed.recipe.operations) {
        slots += operation.op === "keep" ? operation.count : operation.values.length;
        if (operation.op === "insert") insertedSlots += operation.values.length;
    }
    if (!Number.isSafeInteger(slots) || !reserve(slots, insertedSlots))
        throw new CaptureBudgetExceeded("recipe output array");
    const applied = applyRecipe(parsed.recipe, input, previous);
    if (!applied.ok) {
        throw new Error(
            `rust transform recipe rejected: ${applied.rejection.code}: ${applied.rejection.detail}`,
        );
    }
    return { ...applied, outputRevision: parsed.recipe.outputRevision };
}

/** The protocol fields of one revision 3 transform request, ahead of the adapter's pass inputs. */
export interface TransformRequestCore {
    sessionId: string;
    boundary: TransformBoundary | null;
    baseRevision: string;
    previousOutputRevision?: string;
    serializerProfile: string;
    input: unknown[];
    nativeMessages: readonly unknown[];
}

/**
 * A native-serving revision 3 transform body: the adapter's `fields`, then the protocol fields
 * and the window, so no pass input can replace a protocol field.
 */
export function buildTransformRequest(
    core: TransformRequestCore,
    fields: Record<string, unknown>,
): Record<string, unknown> {
    return {
        ...fields,
        method: "transform",
        kind: "transform",
        v: 3,
        boundary: core.boundary,
        serializer_profile: core.serializerProfile,
        serve_native: true,
        session_id: core.sessionId,
        base_revision: core.baseRevision,
        ...(core.previousOutputRevision
            ? { previous_output_revision: core.previousOutputRevision }
            : {}),
        messages: core.input,
        native_messages: core.nativeMessages,
    };
}

export type PassDeclineReason =
    | "cleared"
    | "superseded"
    | "capture_bytes"
    | "invocation_budget"
    | "unsupported_source"
    | "host_container"
    | "source_changed"
    | "publication_failed"
    | "deleted"
    | "internal_child"
    | "daemon_session_busy"
    | "daemon_status_unrecognized"
    | "daemon_revision_unsupported"
    | "boundary_unknown"
    | "discovery_declined";

/**
 * Declines that try `serveLastApplied` before raw output, because raw output carries the whole
 * uncompacted history.
 */
const LAST_APPLIED_DECLINES: ReadonlySet<PassDeclineReason> = new Set([
    "capture_bytes",
    "daemon_session_busy",
    "daemon_status_unrecognized",
]);

/**
 * A local refusal prevents publication without counting a daemon failure. Byte pressure and a
 * polluted built-in prototype recur on every call for the affected session, so they log at warn.
 */
export class PassDeclined extends Error {
    readonly logLevel: "debug" | "warn";
    constructor(
        sessionId: string,
        readonly reason: PassDeclineReason,
        detail?: string,
        logLevel: "debug" | "warn" = reason === "capture_bytes" ? "warn" : "debug",
    ) {
        super(`rust session ${sessionId} pass declined: ${reason}${detail ? ` (${detail})` : ""}`);
        this.logLevel = logLevel;
    }
}

interface DeliveryPlan {
    sessionId: string;
    projectRoot: string;
    attempted: Set<string>;
    applied: Set<string>;
    outcome: TransformPassOutcome;
}

async function deliverTransformNotes(
    moduleClient: RustModeModuleClient,
    plan: DeliveryPlan,
): Promise<void> {
    for (const disposition of ["nack", "ack"] as const) {
        const errors: unknown[] = [];
        for (const id of plan.attempted) {
            if (plan.applied.has(id) !== (disposition === "ack")) continue;
            const method = `transform.${disposition}` as const;
            try {
                await moduleClient.call({
                    sessionId: plan.sessionId,
                    projectRoot: plan.projectRoot,
                    method,
                    body: { method, v: 1, session_id: plan.sessionId, transform_pass_id: id },
                });
            } catch (error) {
                errors.push(error);
            }
        }
        if (errors.length > 0) {
            sessionLog.warn(
                plan.sessionId,
                `rust note delivery ${disposition} failed (${disposition === "ack" ? "will retry" : "ignored"}):`,
                new AggregateError(errors, `${errors.length} delivery disposition(s) failed`),
            );
        }
    }
}

/** The host's message array as discovery reads it: its length and the id at each slot. */
export interface TransformHostView {
    readonly length: number;
    /** The id at `index`, or `undefined` for a slot without one. */
    idAt(index: number): string | undefined;
    /**
     * `false` when no slot's `idAt` can return `id`, so a lookup skips the backward scan; `true`
     * when a slot may return `id`. Lookups scan hosts that omit `holds`.
     */
    holds?(id: string): boolean;
}

/** What the adapter computed for one pass after the window was captured. */
export interface TransformPassPreparation {
    /** The CK window, encoded from the captured members when the request is built. */
    encodeInput(): unknown[];
    /** Pass inputs the request carries after its protocol fields. */
    fields: Record<string, unknown>;
}

/**
 * One pass's view of its harness. Every method reads the host or the adapter's own state except
 * `publish`, the one host write, which runs after every check the pass makes.
 *
 * Each attempt calls, in order: `preflight`, `readWindow`, `idOf` on the window, `contextLimit`,
 * `prepare`, `liveWindow` for every recheck, `validateOutput`, `publicationRejection`, and
 * `publish`. A `boundary_unknown` answer starts one more attempt from `preflight`. A failed pass
 * may call `liveWindow`, `publicationRejection`, and `publish` after `contextLimit` to fail open.
 */
export interface TransformPassSource {
    readonly serializerProfile: string;
    /** The heuristic the invocation gate charges the candidate output under. */
    readonly invocationProfile: HarnessProfileIdentity;
    readonly host: TransformHostView;
    /**
     * Classifies the session and names the route root; a decline throws {@link PassDeclined}.
     * Runs once per attempt, before discovery.
     */
    preflight(): Promise<string>;
    /** Copies host slots `[start, end)`; `undefined` when a slot cannot be read as plain data. */
    readWindow(start: number, end: number): unknown[] | undefined;
    /** The message id of one window value. */
    idOf(value: unknown): string | undefined;
    /**
     * Copies the same window from the live host for a recheck; `undefined` when the host's
     * container, its length, or its readability changed.
     */
    liveWindow(start: number, end: number): unknown[] | undefined;
    /**
     * `true` when the pass holds the only references to the captured values from capture to
     * publication, so a recheck compares the live window's slots with the captured members by
     * reference.
     */
    readonly privateWindow?: boolean;
    /**
     * The trusted context limit the invocation gate reads, resolved from the captured members
     * before the pass can decline into fail-open.
     */
    contextLimit(members: readonly unknown[]): number | undefined;
    /**
     * Computes the pass inputs from the captured members. `assertCurrent` throws when the pass
     * was cleared or superseded during an await.
     */
    prepare(
        members: readonly unknown[],
        assertCurrent: () => void,
    ): Promise<TransformPassPreparation>;
    /** Throws when an applied output breaks a harness rule the daemon's boundary promises. */
    validateOutput?(values: readonly unknown[], boundaryId: string): void;
    /** Why publishing `slots` values would be refused, or `null`. */
    publicationRejection(slots: number): string | null;
    /** The CK size of host slot `index`, which cold import measures; absent disables cold import. */
    sizeOf?(index: number): TransformWindowSize | undefined;
    /** Host slots a cold import sends ahead of its suffix, such as a summary that heads the array. */
    readonly coldLead?: number;
    /** `false` for a harness that keeps its own array on a failed pass instead of the last applied output. */
    readonly failOpen?: boolean;
    /** Publishes `values` over the captured `window` at `boundaryIndex`; a failure names its cause. */
    publish(
        values: readonly unknown[],
        window: readonly unknown[],
        boundaryIndex: number,
    ): { detail: string } | undefined;
}

export interface TransformSessionClientOptions {
    moduleClient: RustModeModuleClient;
    /** A fixed route root for every session; otherwise each pass routes by its preflight root. */
    projectRoot?: string;
    /** Retained-output budget across sessions; tests inject a smaller one. */
    retainedOutputBudgetBytes?: number;
    /** Largest transform body sent unpaged; tests inject the page limit to exercise paging. */
    unpagedTransformMaxBytes?: number;
}

/** How one pass ended. */
export type TransformPassOutcome =
    | {
          /** The daemon's output was published, and `boundary` is the rendered boundary it acknowledged. */
          readonly kind: "applied";
          readonly boundary: TransformBoundary | null | undefined;
      }
    | {
          /** The pass declined or failed; `servedLastApplied` names a fail-open publication. */
          readonly kind: "declined";
          readonly servedLastApplied: boolean;
      };

export interface TransformSessionClient {
    /**
     * Runs one pass under `lease` and resolves with its outcome once that is published or
     * declined; note delivery runs after `lease` is released.
     */
    run(
        sessionId: string,
        lease: CaptureLease,
        source: TransformPassSource,
    ): Promise<TransformPassOutcome>;
    /** Forgets the session's pass state and retained output; returns the root it last routed by. */
    clear(sessionId: string): string | null;
    state(sessionId: string): Readonly<TransformSessionState>;
    readonly retainedOutputs: RetainedOutputs;
}

function ensureState(
    states: Map<string, TransformSessionState>,
    sessionId: string,
): TransformSessionState {
    let state = states.get(sessionId);
    if (!state) {
        state = {
            initialized: false,
            consecutiveFailures: 0,
            passCount: 0,
            boundary: undefined,
            failureCount: 0,
            routeRoot: null,
        };
        states.set(sessionId, state);
    }
    return state;
}

function sameMembers(live: readonly unknown[], captured: readonly unknown[]): boolean {
    if (live.length !== captured.length) return false;
    for (let index = 0; index < live.length; index += 1)
        if (!Object.is(live[index], captured[index])) return false;
    return true;
}

/** Scans host ids from the end until `stop` accepts one and returns its index, or -1. */
export function scanHostIds(
    host: TransformHostView,
    stop: (id: string, index: number) => boolean,
): number {
    for (let index = host.length - 1; index >= 0; index -= 1) {
        const id = host.idAt(index);
        if (id !== undefined && stop(id, index)) return index;
    }
    return -1;
}

/**
 * The sorted id hashes of `host`, retaining no id string; `undefined` when `reserve` refuses the
 * four bytes per slot. A hit may be a collision, so the caller verifies it with an id scan.
 */
export function hostIdFilter(
    host: TransformHostView,
    reserve: (bytes: number) => boolean,
): Uint32Array | undefined {
    if (!reserve(host.length * 4)) return undefined;
    const hashes = new Uint32Array(host.length);
    let count = 0;
    scanHostIds(host, (id) => {
        hashes[count++] = fnv1a32(id);
        return false;
    });
    return hashes.subarray(0, count).sort();
}

/**
 * The cold-import head (spec D8): the pinned head while it is still in the host; otherwise, for
 * a host past half the window cap, the oldest slot of the longest suffix that, with the cold lead
 * slots ahead of it, stays within half the cap as the daemon counts it. The newest slot is sent
 * alone when nothing older fits, and the daemon's window cap folds it. `undefined` sends the whole
 * array: a host within half the cap, or one with a slot `sizeOf` cannot measure, which is a slot
 * the source's capture refuses, so that pass declines. A head found here is pinned in `state`
 * until the first anchor exists.
 */
function coldHead(state: TransformSessionState, source: TransformPassSource): number | undefined {
    const { host, sizeOf } = source;
    const lead = source.coldLead ?? 0;
    if (state.pinnedHead !== undefined) {
        const pinned = scanHostIds(host, (id) => id === state.pinnedHead);
        if (pinned >= lead) return pinned;
        state.pinnedHead = undefined;
    }
    if (!sizeOf || host.length <= lead) return undefined;
    const size: TransformWindowSize = { blocks: 0, bytes: 0 };
    let unmeasured = false;
    const fits = (index: number): boolean => {
        const slot = sizeOf(index);
        if (!slot) unmeasured = true;
        else {
            size.blocks += slot.blocks;
            size.bytes += slot.bytes;
        }
        return slot !== undefined && withinHalfCap(size);
    };
    let head = host.length;
    let leadFits = true;
    for (let slot = 0; slot < lead && leadFits; slot += 1) leadFits = fits(slot);
    while (leadFits && head > lead && fits(head - 1)) head -= 1;
    if (unmeasured || head === lead) return undefined;
    head = Math.min(head, host.length - 1);
    const id = host.idAt(head);
    if (id === undefined) return undefined;
    state.pinnedHead = id;
    return head;
}

/** `source` with its host narrowed to the cold lead slots followed by the slots from `head`. */
function coldSource(source: TransformPassSource, head: number): TransformPassSource {
    const lead = source.coldLead ?? 0;
    const at = (index: number) => (index < lead ? index : head + index - lead);
    const ranges = (start: number, end: number): [number, number][] => {
        const out: [number, number][] = [];
        if (start < lead) out.push([start, Math.min(end, lead)]);
        if (end > lead) out.push([at(Math.max(start, lead)), at(end - 1) + 1]);
        return out;
    };
    const read =
        (copy: (start: number, end: number) => unknown[] | undefined) =>
        (start: number, end: number): unknown[] | undefined => {
            const values: unknown[] = [];
            for (const [from, to] of ranges(start, end)) {
                const part = copy(from, to);
                if (!part) return undefined;
                values.push(...part);
            }
            return values;
        };
    const base = source.host;
    return {
        ...source,
        host: {
            get length() {
                return lead + base.length - head;
            },
            idAt: (index) => base.idAt(at(index)),
        },
        readWindow: read((start, end) => source.readWindow(start, end)),
        liveWindow: read((start, end) => source.liveWindow(start, end)),
        // The narrowed view is the cold window itself, so it measures nothing further.
        sizeOf: undefined,
        coldLead: 0,
        publish: (values, window, boundaryIndex) =>
            source.publish(values, window, at(boundaryIndex)),
    };
}

export function createTransformSessionClient(
    options: TransformSessionClientOptions,
): TransformSessionClient {
    const states = new Map<string, TransformSessionState>();
    const retainedOutputs = new RetainedOutputs(
        RETAINED_OUTPUT_SESSION_CAPACITY,
        options.retainedOutputBudgetBytes ?? RETAINED_OUTPUT_BUDGET_BYTES,
    );
    /** A digest keeps each symbol, and so its description, alive with the record. */
    const retainedSymbolBytes = (digest: HistoryDigest | undefined): number => {
        let bytes = 0;
        for (const symbol of digest?.symbols ?? [])
            bytes += CANDIDATE_SLOT_BYTES + (symbol.description?.length ?? 0) * 2;
        return bytes;
    };

    const logStage = (
        sessionId: string,
        stage: keyof RustPassTimings,
        startedAt: number,
        timings: RustPassTimings,
        extra?: string,
    ): void => {
        const elapsed = Math.max(0, performance.now() - startedAt);
        timings[stage] += elapsed;
        logTransformTiming(
            sessionId,
            `rust.${stage.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`)}`,
            startedAt,
            extra,
        );
    };

    const markFailure = (
        sessionId: string,
        state: TransformSessionState,
        error: unknown,
        servedLastApplied: boolean,
    ): void => {
        state.consecutiveFailures += 1;
        state.failureCount += 1;
        sessionLog.warn(
            sessionId,
            servedLastApplied
                ? "rust transform failed; serving the last applied output with the messages appended since:"
                : "rust transform failed; serving the input unchanged:",
            error,
        );
    };

    /** `rerun` resumes a pass on its validated root after its anchor drew `boundary_unknown`. */
    const execute = async (
        sessionId: string,
        hostSource: TransformPassSource,
        lease: CaptureLease,
        rerun?: {
            deliveries: DeliveryPlan;
            timings: RustPassTimings;
            startedAt: number;
        },
    ): Promise<DeliveryPlan> => {
        const passStartedAt = rerun?.startedAt ?? performance.now();
        const deliveries: DeliveryPlan = rerun?.deliveries ?? {
            sessionId,
            projectRoot: "",
            attempted: new Set(),
            applied: new Set(),
            outcome: { kind: "declined", servedLastApplied: false },
        };
        // A cold import narrows the source to its suffix once discovery finds no anchor.
        let source = hostSource;
        let host = source.host;
        const state = ensureState(states, sessionId);
        const timings = rerun?.timings ?? emptyRustPassTimings();
        let inputCount = 0;
        let decision = "error";
        let materializeReason = "none";
        let servedFrom = "none";
        let moduleElapsedMs = 0;
        let emergencyWaitMs = 0;
        let rowVersion = 0;
        let appliedAt: number | undefined;
        const finishPass = (applied: boolean): void => {
            const elapsedAt = applied && appliedAt !== undefined ? appliedAt : performance.now();
            const elapsedMs = Math.max(0, elapsedAt - passStartedAt);
            timings.chargedBytes = lease.chargedBytes;
            sessionLog.debug(
                sessionId,
                formatRustPassLog({
                    decision,
                    reason: materializeReason,
                    servedFrom,
                    inputCount,
                    outputCount: host.length,
                    applied,
                    elapsedMs,
                    moduleElapsedMs,
                    rowVersion,
                    emergencyWaitMs,
                    rediscovered: rerun !== undefined,
                    timings,
                }),
            );
        };
        const captureResponseTelemetry = (response: Record<string, unknown>): void => {
            decision =
                typeof response.decision === "string"
                    ? response.decision
                    : typeof response.action === "string"
                      ? response.action
                      : typeof response.status === "string"
                        ? response.status
                        : "unknown";
            servedFrom =
                typeof response.served_from === "string" ? response.served_from : "unknown";
            materializeReason =
                typeof response.materialize_reason === "string" &&
                response.materialize_reason.length > 0
                    ? response.materialize_reason
                    : "none";
            const timings = isRecord(response.timings) ? response.timings : undefined;
            const applyOnceTotal = timings?.total;
            emergencyWaitMs =
                typeof timings?.emergency_wait === "number" &&
                Number.isFinite(timings.emergency_wait)
                    ? timings.emergency_wait
                    : 0;
            const handlerTotal = timings?.handler_total;
            moduleElapsedMs =
                typeof handlerTotal === "number" && Number.isFinite(handlerTotal)
                    ? handlerTotal
                    : typeof applyOnceTotal === "number" && Number.isFinite(applyOnceTotal)
                      ? applyOnceTotal
                      : 0;
            rowVersion =
                typeof response.row_version === "number" &&
                Number.isSafeInteger(response.row_version)
                    ? response.row_version
                    : 0;
            if (
                timings &&
                (typeof timings.handler_total === "number" ||
                    typeof timings.native_cache_reused_messages === "number" ||
                    typeof timings.native_cache_encoded_messages === "number")
            ) {
                const stage = (name: string): string => {
                    const value = timings[name];
                    return typeof value === "number" && Number.isFinite(value)
                        ? value.toFixed(1)
                        : "n/a";
                };
                sessionLog.debug(
                    sessionId,
                    `rust module stages: handler=${stage("handler_total")} apply_once=${stage("total")} ` +
                        `request_to_handler=${stage("request_observed_to_handler")} ` +
                        `projection_cache_lookup=${stage("projection_cache_lookup")} projection=${stage("projection")} ` +
                        `selection=${stage("selection")} build_output=${stage("build_output")} ` +
                        `store_commit=${stage("store_commit")} trigger=${stage("trigger_ms")} ` +
                        `trigger_boundary=${stage("trigger_boundary_build")} trigger_eval=${stage("trigger_eval")} ` +
                        `projection_cache_store=${stage("projection_cache_store")} native_attach=${stage("native_attach")} ` +
                        `retained_size=${stage("retained_size")} snapshot_store=${stage("snapshot_store")} ` +
                        `post_attach=${stage("post_attach")} response_encode=${stage("response_encode")} ` +
                        `response_meta_encode=${stage("response_meta_encode")} response_splice=${stage("response_splice")} ` +
                        `native_cache_reused=${stage("native_cache_reused_messages")} ` +
                        `native_cache_encoded=${stage("native_cache_encoded_messages")}`,
                );
            }
            // Log numeric timing fields to identify the slow stage.
            if (timings && moduleElapsedMs >= 1000) {
                const detail = Object.entries(timings)
                    .filter(
                        ([key, value]) =>
                            key !== "total" && key !== "handler_total" && typeof value === "number",
                    )
                    .map(([key, value]) => `${key}:${(value as number).toFixed(1)}`)
                    .join(" ");
                if (detail)
                    sessionLog.debug(sessionId, `rust module stages (slow pass): ${detail}`);
            }
        };
        if (!rerun) state.passCount += 1;
        // Clearing a session replaces its state object; supersession, clearing, and ordinal invalidation abort the capture lease.
        const assertCurrentPass = (): void => {
            if (states.get(sessionId) !== state) throw new PassDeclined(sessionId, "cleared");
            if (lease.signal.aborted) throw new PassDeclined(sessionId, "superseded");
        };
        const charge = (bytes: number, detail: string): void => {
            if (!lease.reserve(bytes)) throw new CaptureBudgetExceeded(detail);
        };
        let failOpenSource:
            | {
                  previous: RetainedOutput;
                  captured: CapturedHistory;
                  boundaryIndex: number;
                  recheck: (phase: string) => void;
                  /** The trusted limit the main publication's invocation gate reads. */
                  contextLimit: number | undefined;
              }
            | undefined;
        /**
         * Without native compaction a raw fail-open can overflow the provider window, so a failed
         * pass declared at the retained basis republishes the applied output followed by the window
         * messages after the acknowledged prefix. Messages are appended whole, so a tool part keeps
         * its call and result together. No basis is promoted; any doubt serves the input unchanged.
         */
        const serveLastApplied = (): boolean => {
            const failOpen = failOpenSource;
            const applied = failOpen?.previous.applied;
            if (!failOpen || !applied || source.failOpen === false) return false;
            try {
                failOpen.recheck("fail-open");
                if (
                    retainedOutputs.peek(sessionId) !== failOpen.previous ||
                    !isAppendOnlyExtension(failOpen.previous, failOpen.captured)
                )
                    return false;
                if (!capturedMessagesUnchanged(applied.values, applied.capture)) {
                    retainedOutputs.dropApplied(sessionId, failOpen.previous);
                    return false;
                }
                const { members } = failOpen.captured;
                const rawCount = failOpen.previous.rawCount;
                const served = [...applied.values, ...members.slice(rawCount)];
                const gated = failOpen.contextLimit !== undefined;
                if (
                    source.publicationRejection(served.length) !== null ||
                    !lease.reserve(
                        served.length * CANDIDATE_SLOT_BYTES +
                            (gated ? members.length * LENGTH_SLOT_BYTES : 0),
                    )
                )
                    return false;
                // The fallback is a candidate too: it may not grow past the limit the pass would refuse.
                if (gated) {
                    const incoming = measureInputLengths(
                        members,
                        failOpen.previous,
                        failOpen.captured.verified?.count ?? 0,
                    );
                    const invocation = validateInvocation(
                        [...applied.lengths, ...incoming.slice(rawCount)],
                        incoming,
                        {
                            maxTokens: failOpen.contextLimit,
                            headroomPermille: INVOCATION_HEADROOM_PERMILLE,
                            profile: source.invocationProfile,
                        },
                    );
                    if (!invocation.ok) {
                        sessionLog.debug(
                            sessionId,
                            `rust transform fail-open declined: invocation_budget (${invocation.candidate.chargedTokens} charged tokens over ${invocation.limit}, growing from ${invocation.incoming.bytes} to ${invocation.candidate.bytes} bytes)`,
                        );
                        return false;
                    }
                }
                const failure = source.publish(served, members, failOpen.boundaryIndex);
                if (failure) {
                    sessionLog.warn(
                        sessionId,
                        `rust transform fail-open publication failed: ${failure.detail}`,
                    );
                    return false;
                }
                return true;
            } catch (error) {
                sessionLog.debug(sessionId, "rust transform fail-open reuse declined:", error);
                return false;
            }
        };
        const scan = (stop: (id: string, index: number) => boolean): number => {
            const index = scanHostIds(host, stop);
            timings.scannedItems += host.length - Math.max(index, 0);
            return index;
        };
        /**
         * Walks `transform.boundary` newest first to an anchor the host holds; an empty
         * page sends `null`. Budget (from the pass start, so a rerun gets what is left), timeout, a
         * malformed or repeated page, or a daemon without the method declines, never `null`.
         */
        const discover = async (
            projectRoot: string,
        ): Promise<{ boundary: TransformBoundary | null; index: number }> => {
            const deadline = passStartedAt + DISCOVERY_BUDGET_MS;
            let filter: Uint32Array | undefined;
            let before: number | undefined;
            for (;;) {
                const remainingMs = Math.floor(deadline - performance.now());
                if (remainingMs <= 0)
                    throw new PassDeclined(sessionId, "discovery_declined", "time budget");
                let reply: unknown;
                try {
                    reply = await options.moduleClient.call({
                        sessionId,
                        projectRoot,
                        method: "transform.boundary",
                        body: {
                            method: "transform.boundary",
                            v: 3,
                            session_id: sessionId,
                            ...(before === undefined ? {} : { before_sequence: before }),
                        },
                        signal: lease.signal,
                        timeoutMs: remainingMs,
                    });
                } catch (error) {
                    assertCurrentPass();
                    throw new PassDeclined(
                        sessionId,
                        "discovery_declined",
                        errorHasCode(error, "unrecognized_request_shape")
                            ? "the daemon lacks transform.boundary; upgrade it with the plugin"
                            : String(error),
                        "warn",
                    );
                }
                assertCurrentPass();
                const page = parseAnchorPage(reply, before);
                if (!page)
                    throw new PassDeclined(sessionId, "discovery_declined", "malformed page");
                const last = page.at(-1);
                if (!last) return { boundary: null, index: 0 };
                // Both paths declare the held anchor latest in the host, the newest by sequence
                // when two share a mid. Host order follows segment sequence (D16: writers append
                // or truncate a suffix), so that is the newest held anchor; if a host breaks the
                // order, the daemon re-validates the declared anchor (D10) and reverts to it.
                if (!filter) {
                    // D17: one backward id scan against the first page stops at the first hit.
                    const anchors = new Map<string, TransformBoundary>();
                    for (const anchor of page)
                        if (!anchors.has(anchor.mid) && host.holds?.(anchor.mid) !== false)
                            anchors.set(anchor.mid, anchor);
                    let hit: TransformBoundary | undefined;
                    const index =
                        anchors.size === 0
                            ? -1
                            : scan((id) => {
                                  hit = anchors.get(id);
                                  return hit !== undefined;
                              });
                    // `hit` is set by the last callback, so it is defined only when the scan stopped on one.
                    if (hit) return { boundary: hit, index };
                    before = last.sequence;
                    if (host.holds) continue;
                    // The whole host holds none of this page; later pages probe the filter.
                    timings.scannedItems += host.length;
                    filter = hostIdFilter(host, (bytes) => lease.reserve(bytes));
                    if (!filter) throw new CaptureBudgetExceeded("membership filter");
                    continue;
                }
                const wanted = new Set<string>();
                for (const anchor of page)
                    if (filterMayHold(filter, anchor.mid)) wanted.add(anchor.mid);
                const found = new Map<string, number>();
                if (wanted.size > 0)
                    scan((id, index) => {
                        if (wanted.has(id) && !found.has(id)) found.set(id, index);
                        return found.size === wanted.size;
                    });
                let held: { boundary: TransformBoundary; index: number } | undefined;
                for (const anchor of page) {
                    const index = found.get(anchor.mid);
                    if (index !== undefined && index > (held?.index ?? -1))
                        held = { boundary: anchor, index };
                }
                if (held) return held;
                before = last.sequence;
            }
        };
        try {
            const directory = await source.preflight();
            assertCurrentPass();
            // An unknown boundary, or one the scan cannot find, needs a discovery walk.
            const known = state.boundary;
            let boundary = known ?? null;
            let boundaryIndex = !known
                ? 0
                : host.holds?.(known.mid) === false
                  ? -1
                  : scan((id) => id === known.mid);
            if (known === undefined || boundaryIndex < 0) {
                const discovered = await discover(options.projectRoot ?? directory);
                boundary = discovered.boundary;
                boundaryIndex = discovered.index;
            }
            // An anchor ends a cold import whether or not this pass applies.
            if (boundary !== null) state.pinnedHead = undefined;
            else {
                const head = coldHead(state, source);
                if (head !== undefined) {
                    source = coldSource(source, head);
                    host = source.host;
                    boundaryIndex = 0;
                }
            }
            // The window is copied, inspected, and taped in one synchronous section.
            const prefixGuardStartedAt = performance.now();
            const previous = retainedOutputs.get(sessionId);
            const capturedLength = host.length;
            // The copy holds one slot reference per tail message, charged like a served or candidate array.
            charge((capturedLength - boundaryIndex) * CANDIDATE_SLOT_BYTES, "window slots");
            const window = source.readWindow(boundaryIndex, capturedLength);
            if (!window)
                throw new PassDeclined(sessionId, "unsupported_source", "window slot accessor");
            // Discovery fixed `boundaryIndex` before an await, so the host may have moved the anchor.
            const head = source.idOf(window[0]);
            if (boundary && head !== boundary.mid)
                throw new PassDeclined(sessionId, "source_changed", "boundary moved");
            // A full fallback inspection walks a superset of the partial one, so it pays only the difference.
            let inspectedBytes = 0;
            const inspect = (skip: number): number[] => {
                const inspection = inspectReferenceableMessages(
                    window,
                    lease.remainingBytes + inspectedBytes,
                    skip,
                );
                if (!inspection.ok) {
                    throw new PassDeclined(
                        sessionId,
                        "unsupported_source",
                        `${inspection.rejection.reason} at ${inspection.rejection.path}`,
                        inspection.rejection.reason === "prototype_accessor" ? "warn" : "debug",
                    );
                }
                charge(
                    inspection.estimatedBytes - inspectedBytes,
                    `capture charge=${inspection.estimatedBytes}`,
                );
                inspectedBytes = inspection.estimatedBytes;
                return inspection.messageWireBytes;
            };
            /**
             * A window declared at the retained basis checks its history before the former terminal
             * against the digest instead of re-taping it; the former terminal, which the host may still
             * edit in place, is taped again. Any prefix change falls back to a full capture.
             */
            const prefix =
                previous && sameBoundary(previous.basis, boundary)
                    ? previous.rawHistory
                    : undefined;
            let verified: CapturedHistory | undefined;
            let messageWireBytes: number[] = [];
            if (previous && prefix) {
                messageWireBytes = inspect(prefix.count);
                verified = captureHistory(window, lease, prefix);
                for (let index = 0; verified && index < prefix.count; index += 1)
                    messageWireBytes[index] = previous.wireBytes[index] ?? 0;
            }
            if (!verified) messageWireBytes = inspect(0);
            const captured = verified ?? captureHistory(window, lease);
            inputCount = messageWireBytes.length;
            // Later reads use the captured window; the live array is only rechecked against it.
            const messages = captured.members;
            const ids = new Set<unknown>();
            for (const message of messages) {
                const id = source.idOf(message);
                if (id !== undefined && ids.has(id))
                    throw new PassDeclined(sessionId, "unsupported_source", `duplicate id ${id}`);
                ids.add(id);
            }
            logStage(
                sessionId,
                "prefixGuard",
                prefixGuardStartedAt,
                timings,
                `phase=capture boundary_index=${boundaryIndex} verified=${verified?.verified?.count ?? 0}`,
            );
            const recheckCapture = (phase: string): void => {
                assertCurrentPass();
                const startedAt = performance.now();
                const live = source.liveWindow(boundaryIndex, capturedLength);
                const unchanged =
                    live !== undefined &&
                    (source.privateWindow === true
                        ? sameMembers(live, captured.members)
                        : capturedMessagesUnchanged(live, captured));
                logStage(sessionId, "prefixGuard", startedAt, timings, `phase=${phase}`);
                if (!unchanged) throw new PassDeclined(sessionId, "source_changed", phase);
            };
            // The trusted limit is resolved before the first decline a failed pass can fail open from.
            const reportedContextLimit = source.contextLimit(messages);
            // Tapes are never rebased: a verified prefix was declared at the retained basis anchor.
            // A disowned record, which every rerun reads, never fails open.
            if (previous && verified && !previous.disowned)
                failOpenSource = {
                    previous,
                    captured,
                    boundaryIndex,
                    recheck: recheckCapture,
                    contextLimit: reportedContextLimit,
                };
            // The wire charge derives from the capture, so byte pressure declines before the next await.
            let wireBytes = 0;
            for (let index = 0; index < messageWireBytes.length; index += 1)
                wireBytes += WIRE_PROJECTION_FACTOR * (messageWireBytes[index] ?? 0);
            charge(wireBytes, "wire projection");
            const preparation = await source.prepare(messages, assertCurrentPass);
            recheckCapture("wire-build");
            const projectRoot = options.projectRoot ?? directory;
            state.routeRoot = projectRoot;
            deliveries.projectRoot = projectRoot;
            const wireBuildStartedAt = performance.now();
            const encodedInput = preparation.encodeInput();
            timings.wireMessages = messages.length;
            charge(messages.length * LENGTH_SLOT_BYTES, "input lengths");
            const inputLengths = measureInputLengths(
                messages,
                previous,
                captured.verified?.count ?? 0,
            );
            // The retained output stays reusable only while the record that applied it survives.
            let previousApplied = previous?.applied;
            if (
                previous &&
                previousApplied &&
                !capturedMessagesUnchanged(previousApplied.values, previousApplied.capture)
            ) {
                previousApplied = undefined;
                retainedOutputs.dropApplied(sessionId, previous);
            }
            const baseRevision = nextBaseRevision();
            const body = buildTransformRequest(
                {
                    sessionId,
                    boundary,
                    baseRevision,
                    previousOutputRevision: previousApplied?.revision,
                    serializerProfile: source.serializerProfile,
                    input: encodedInput,
                    nativeMessages: messages,
                },
                preparation.fields,
            );
            logStage(sessionId, "wireBuild", wireBuildStartedAt, timings);
            type TransformSeriesRestart = {
                reason: "attempt_mismatch" | "reconnect";
                pages: number;
                atPage: number;
            };
            type TransformSeriesResult =
                | { response: Record<string, unknown> }
                | { restart: TransformSeriesRestart };
            /** Each series freezes its pages from revalidated source values. */
            const sendTransformSeries = async (
                payload: Record<string, unknown>,
                detail: string,
            ): Promise<TransformSeriesResult> => {
                const series = buildPagedModuleTransformPayloads(
                    payload,
                    options.unpagedTransformMaxBytes,
                );
                const paged = series.some(
                    (entry) => typeof entry.page.transform_page_id === "string",
                );
                let response: Record<string, unknown> | undefined;
                for (const [index, { page, bytes }] of series.entries()) {
                    const restart = (
                        reason: TransformSeriesRestart["reason"],
                    ): TransformSeriesResult => ({
                        restart: { reason, pages: series.length, atPage: index },
                    });
                    // A `session.deleted` that landed during preflight or an earlier page has already queued the daemon-side delete; sending now would recreate the session's durable state.
                    // Page bodies are frozen text, so a source walk here cannot change what is sent; the recheck before publication covers the series.
                    assertCurrentPass();
                    const transportStartedAt = performance.now();
                    let moduleResponse: unknown;
                    try {
                        moduleResponse = await options.moduleClient.call({
                            sessionId,
                            projectRoot,
                            method: "transform",
                            body: page,
                            signal: lease.signal,
                            generationSensitive: paged && index > 0,
                        });
                    } catch (error) {
                        if (paged && isTransformPageAttemptMismatch(error))
                            return restart("attempt_mismatch");
                        if (errorHasCode(error, "transform_revision_unsupported"))
                            throw new PassDeclined(
                                sessionId,
                                "daemon_revision_unsupported",
                                "the daemon speaks another transform revision; upgrade the plugin and daemon together",
                                "warn",
                            );
                        throw error;
                    }
                    if (paged && isModuleTransportGenerationChangedResult(moduleResponse))
                        return restart("reconnect");
                    if (paged && isTransformPageAttemptMismatch(moduleResponse))
                        return restart("attempt_mismatch");
                    response = responseValue(moduleResponse);
                    for (const id of noteDeliveryPassIds(response)) deliveries.attempted.add(id);
                    timings.transportBytes += bytes;
                    timings.transportPages += 1;
                    logStage(
                        sessionId,
                        "transport",
                        transportStartedAt,
                        timings,
                        `page=${index + 1}/${series.length}${detail}`,
                    );
                }
                if (!response) throw new Error("rust module returned no transform response");
                // session_busy: the daemon's session lane already holds an active and a waiting pass for this session.
                // An unrecognized status is declined the same way (Section 7.10.1 of the wire protocol).
                // boundary_unknown: the declared anchor names no segment; the pass rediscovers once.
                const busy = response.status === "session_busy";
                if (
                    busy ||
                    (response.status !== undefined &&
                        response.status !== "ok" &&
                        response.status !== "boundary_unknown")
                ) {
                    assertCurrentPass();
                    throw new PassDeclined(
                        sessionId,
                        busy ? "daemon_session_busy" : "daemon_status_unrecognized",
                    );
                }
                return { response };
            };
            let transformSeriesRestarted = false;
            // One bounded series restart is the only permitted page-level recovery; it is a fresh attempt over the same validated capture.
            const sendTransformSeriesWithSingleRestart = async (
                payload: Record<string, unknown>,
                detail: string,
            ): Promise<Record<string, unknown>> => {
                let result = await sendTransformSeries(payload, detail);
                if (!("restart" in result)) return result.response;
                if (transformSeriesRestarted) {
                    throw new Error(
                        `rust transform page series restart exhausted: reason=${result.restart.reason}`,
                    );
                }
                transformSeriesRestarted = true;
                sessionLog.warn(
                    sessionId,
                    `transform_series_restart reason=${result.restart.reason} pages=${result.restart.pages} at_page=${result.restart.atPage}`,
                );
                recheckCapture("series-restart");
                result = await sendTransformSeries(payload, `${detail} restart=series`);
                if ("restart" in result) {
                    throw new Error(
                        `rust transform page series restart exhausted: reason=${result.restart.reason}`,
                    );
                }
                return result.response;
            };
            const response = await sendTransformSeriesWithSingleRestart(body, "");
            if (response.status === "boundary_unknown") {
                assertCurrentPass();
                state.boundary = undefined;
                if (previous) retainedOutputs.disown(sessionId, previous);
                // Section 7.10.4: the first `boundary_unknown` rediscovers, whether or not this attempt walked; the second declines.
                if (rerun)
                    throw new PassDeclined(sessionId, "boundary_unknown", "after rediscovery");
                // Nothing of this attempt is kept, so the rerun pays only for its own capture. The
                // `return` is not awaited: this frame, its window, and its capture are gone before the rerun captures.
                lease.refund();
                return execute(sessionId, hostSource, lease, {
                    deliveries,
                    timings,
                    startedAt: passStartedAt,
                });
            }
            // A missing or malformed boundary leaves the next pass to rediscover it.
            const nextBoundary = parseBoundary(response.boundary);
            if (nextBoundary === undefined && response.boundary !== undefined)
                sessionLog.warn(sessionId, "rust transform response boundary is malformed");
            captureResponseTelemetry(response);
            const appliedDeliveryPassIds = new Set(noteDeliveryPassIds(response));
            const applyStartedAt = performance.now();
            try {
                // Previous outputs may share objects with an earlier host array, not this pass's input.
                if (
                    previousApplied &&
                    response.previous_output_revision !== undefined &&
                    !capturedMessagesUnchanged(previousApplied.values, previousApplied.capture)
                ) {
                    if (previous) retainedOutputs.dropApplied(sessionId, previous);
                    throw new PassDeclined(sessionId, "source_changed", "previous output");
                }
                // Recipe validation, sizing, and every boundary check run before the host array is touched.
                const application = applyTransformRecipe(
                    response,
                    { revision: baseRevision, values: messages, lengths: inputLengths },
                    previousApplied,
                    (slots, insertedSlots) =>
                        lease.reserve(
                            slots * (CANDIDATE_SLOT_BYTES + LENGTH_SLOT_BYTES) +
                                insertedSlots * LENGTH_SLOT_BYTES,
                        ),
                );
                const candidate = application.values;
                let applied: AppliedOutput | undefined;
                try {
                    // Only the charge is read here, so the per-unit escape scan is skipped.
                    const inspection = inspectReferenceableMessages(
                        candidate,
                        lease.remainingBytes,
                        0,
                        false,
                    );
                    if (inspection.ok && lease.reserve(inspection.estimatedBytes)) {
                        applied = {
                            revision: application.outputRevision,
                            values: candidate,
                            lengths: application.lengths,
                            capture: captureMessages(candidate, lease),
                            charge:
                                application.bytes +
                                application.lengths.length * LENGTH_SLOT_BYTES +
                                inspection.estimatedBytes,
                        };
                    }
                } catch (error) {
                    if (!(error instanceof CaptureBudgetExceeded)) throw error;
                }
                const boundaryId = response.boundary_id;
                if (typeof boundaryId === "string" && boundaryId.length > 0) {
                    source.validateOutput?.(candidate, boundaryId);
                }
                // Final synchronous guards: ownership, source membership and content, and the host container contract.
                recheckCapture("publish");
                const publishRejection = source.publicationRejection(candidate.length);
                if (publishRejection !== null) {
                    throw new PassDeclined(sessionId, "host_container", publishRejection);
                }
                // Every candidate entry's canonical length is charged, not the inserted payload alone.
                const invocation = validateInvocation(application.lengths, inputLengths, {
                    maxTokens: reportedContextLimit,
                    headroomPermille: INVOCATION_HEADROOM_PERMILLE,
                    profile: source.invocationProfile,
                });
                if (!invocation.ok) {
                    throw new PassDeclined(
                        sessionId,
                        "invocation_budget",
                        `${invocation.candidate.chargedTokens} charged tokens over ${invocation.limit}, growing from ${invocation.incoming.bytes} to ${invocation.candidate.bytes} bytes, under ${invocation.candidate.profile.identity} ${invocation.candidate.profile.revision}`,
                        "warn",
                    );
                }
                logStage(sessionId, "apply", applyStartedAt, timings);
                const applyReplaceStartedAt = performance.now();
                // Publication and state promotion are synchronous from here to the lease release.
                const failure = source.publish(candidate, messages, boundaryIndex);
                if (failure)
                    throw new PassDeclined(sessionId, "publication_failed", failure.detail, "warn");
                const record: RetainedOutput = {
                    basis: boundary,
                    rawCount: messages.length,
                    rawHistory: captured.history,
                    ...(captured.terminal ? { rawTerminal: captured.terminal } : {}),
                    wireBytes: messageWireBytes,
                    inputLengths,
                    ...(applied ? { applied } : {}),
                    charge:
                        messages.length * HISTORY_ENTRY_RETAINED_BYTES +
                        (boundary?.mid.length ?? 0) * 2 +
                        retainedSymbolBytes(captured.history) +
                        retainedSymbolBytes(captured.terminal) +
                        (applied?.charge ?? 0),
                };
                // A refused retention keeps the pass; the next pass loses its verified prefix, or only its `previous` source.
                retainedOutputs.retain(sessionId, record);
                timings.retainedBytes = record.charge;
                state.boundary = nextBoundary;
                if (nextBoundary) state.pinnedHead = undefined;
                state.initialized = true;
                state.consecutiveFailures = 0;
                deliveries.applied = appliedDeliveryPassIds;
                deliveries.outcome = { kind: "applied", boundary: nextBoundary };
                logStage(sessionId, "apply", applyReplaceStartedAt, timings);
            } catch (error) {
                logStage(sessionId, "apply", applyStartedAt, timings, "failed=true");
                throw error;
            }
            appliedAt = performance.now();
            finishPass(true);
        } catch (caught) {
            servedFrom = "raw";
            materializeReason = "none";
            const error =
                caught instanceof CaptureBudgetExceeded
                    ? new PassDeclined(sessionId, "capture_bytes", caught.message)
                    : caught;
            if (error instanceof PassDeclined || lease.signal.aborted) {
                decision = error instanceof PassDeclined ? `declined:${error.reason}` : "cancelled";
                sessionLog[error instanceof PassDeclined ? error.logLevel : "debug"](
                    sessionId,
                    error instanceof Error ? error.message : String(error),
                );
                if (
                    error instanceof PassDeclined &&
                    LAST_APPLIED_DECLINES.has(error.reason) &&
                    serveLastApplied()
                )
                    servedFrom = "last_applied";
            } else {
                decision = "error";
                const servedLastApplied = serveLastApplied();
                if (servedLastApplied) servedFrom = "last_applied";
                markFailure(sessionId, state, error, servedLastApplied);
            }
            deliveries.outcome = {
                kind: "declined",
                servedLastApplied: servedFrom === "last_applied",
            };
            finishPass(false);
        }
        return deliveries;
    };

    return {
        run(sessionId, lease, source) {
            // execute settles before admission is released; only route metadata reaches delivery.
            return execute(sessionId, source, lease)
                .finally(lease.release.bind(lease))
                .then(async (plan) => {
                    await deliverTransformNotes(options.moduleClient, plan);
                    return plan.outcome;
                });
        },
        clear(sessionId) {
            const routeRoot = states.get(sessionId)?.routeRoot ?? null;
            states.delete(sessionId);
            retainedOutputs.release(sessionId);
            return routeRoot;
        },
        state(sessionId) {
            return { ...ensureState(states, sessionId) };
        },
        retainedOutputs,
    };
}
