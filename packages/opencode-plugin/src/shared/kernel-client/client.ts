/**
 * Thin client over the daemon's `kernel.*` routes. Every method resolves to a
 * `MemoryState` plus the route's typed payload and never throws into the tool;
 * the caller owns the abort signal and the deadline.
 */

import { createHash } from "node:crypto";
import { Deadline, isConnectTransient, isHostCallError, type MonotonicClock } from "../host-client";
import { isRecord } from "../record-type-guard";
import { stableStringify } from "../stable-json";
import { cancelled, conflict, disabled, invalid, type MemoryState, unavailable } from "./state";
import { TokenCache, type TokenStore } from "./token";
import {
    type CommitPayload,
    type DispositionEvent,
    MAX_COMMIT_OPERATIONS,
    MAX_COMMIT_TOKENS,
    MAX_READ_OBJECT_IDS,
    type MutationToken,
    type Parsed,
    type PreviewPayload,
    parseCommitResponse,
    parsePreviewResponse,
    parseReadResponse,
    type ReadPayload,
    type ReadRow,
    type Sensitivity,
} from "./wire";

export interface KernelTransportCall {
    sessionId: string;
    projectRoot: string;
    method: string;
    body: unknown;
    signal?: AbortSignal;
    timeoutMs?: number;
    /** The `KernelTransport.connectionIdentity()` the body was built under; a transport whose identity has moved refuses to send it. */
    connectionIdentity?: string;
}

/** The bounds a rebind runs under: the same signal and remaining budget the failed request carried. */
export type KernelRebind = Omit<KernelTransportCall, "method" | "body">;

/** The transport surface the client depends on; `HostModuleTransport` is adapted onto it. */
export interface KernelTransport {
    /** False marks the daemon unreachable; a transport that starts the daemon during `call` answers true with no connection file. */
    connectionFileExists(): boolean;
    /** An opaque token that changes whenever the connection `call` would send on changes, so the daemon behind it may differ. Tokens and `as_of` positions are only valid against the daemon they were read from, so a body built under one identity must not be sent under another. */
    connectionIdentity?(): string;
    /** Resolves the daemon's raw response. Rejects with `StoreLifecycleError` when the daemon is reachable but its store is not ready to serve, with `ConnectionIdentityChangedError` when the body's `connectionIdentity` no longer matches and nothing was sent; any other rejection is a transport failure. */
    call(args: KernelTransportCall): Promise<unknown>;
    /** Rebinds the session route after the daemon reports `route_unbound`. The transport settles within `timeoutMs` and on `signal` exactly as `call` does, so a stalled rebind cannot hold a read or commit past the caller's deadline. */
    ensureRoute(args: KernelRebind): Promise<void>;
}

/** The store lifecycle states a transport can observe before any request reaches a kernel route. */
export type StoreLifecycleReason = "store_starting" | "store_unavailable";

/** Thrown by a transport whose daemon reports the store as not ready before the request is sent; the client maps `reason` to the same-named `unavailable` state instead of `invalid:internal`. */
export class StoreLifecycleError extends Error {
    constructor(readonly reason: StoreLifecycleReason) {
        super(`daemon store is ${reason === "store_starting" ? "starting" : "unavailable"}`);
        this.name = "StoreLifecycleError";
    }
}

/** Thrown by a transport that refused to send a body built under a previous connection identity. Nothing reached a daemon, so the client treats the outcome as `snapshot_diverged`: its tokens are dropped and the caller's read-then-retry path rebuilds the request against the daemon now behind the transport. */
export class ConnectionIdentityChangedError extends Error {
    constructor() {
        super("daemon connection changed before the request was sent");
        this.name = "ConnectionIdentityChangedError";
    }
}

export type Surface = "auto_inject" | "auto_search" | "explicit_search";
export type SourceKind = "assistant" | "model" | "dreamer" | "user";

export interface DecisionSpecInput {
    decision_id: string;
    object_id: string;
    domain_id: string;
    proposition_id?: string;
    anchor_id?: string;
    evidence_id?: string;
    decision_kind: string;
    payload: { summary: string; rationale: string };
    source_id: string;
    source_revision: number;
    sensitivity?: Sensitivity;
}

export interface DispositionOperation {
    op: "disposition";
    object_id: string;
    event: DispositionEvent;
    /** Names the approval that authorizes a move toward a less restrictive disposition; a tightening needs none. */
    approval_object_id?: string;
}

export type CommitOperation =
    | { op: "insert_decision"; spec: DecisionSpecInput }
    | { op: "supersede_decision"; replaced_object_id: string; spec: DecisionSpecInput }
    | { op: "retire_decision"; object_id: string }
    | DispositionOperation;

export interface CallOptions {
    signal?: AbortSignal;
    deadlineMs?: number;
}

export interface ReadArgs extends CallOptions {
    surface: Surface;
    asOf?: number | null;
    gated?: boolean;
    /** Scopes the read to the named objects before the daemon applies its row cap, so a targeted lookup reaches a live row a capped unfiltered snapshot drops. At most `MAX_READ_OBJECT_IDS` ids; a longer list answers `invalid_input` without a daemon round trip. */
    objectIds?: readonly string[];
}

export interface IntentArgs {
    actor: string;
    /** Stable identity the operation key hashes together with the producer and actor; a redelivered identity with different request bytes hits `operation_key_reused` instead of committing twice, so caller-controlled free text never rides here — it goes in `cause`. */
    operationId: string;
    /** Free-text audit trail carried in `intent.cause`; never key or digest material. The daemon records it against the commit that applied, and a redelivery that regenerates the text must still replay that receipt rather than hit `operation_key_reused`. */
    cause: string;
    /** Defaults to the client's producer. */
    producer?: string;
    /** Derived from the canonical JSON of the operations when omitted. */
    requestDigest?: string;
}

export interface CommitArgs extends CallOptions, IntentArgs {
    operations: CommitOperation[];
    /**
     * The complete token set for the envelope. When omitted, the cache supplies
     * one token per replaced or retired object and reads any object without a
     * cached token. When given, no cache lookup happens. The daemon fences only the tokens it receives, so a target left out of a given set is mutated unfenced; the client does not refuse that, because an empty set is how a caller replays a receipt whose targets are already superseded, where a refresh read would answer `retracted` before the receipt lookup runs.
     */
    tokens?: MutationToken[];
    sourceKind?: SourceKind;
    assertedSourceClass?: string;
    assertedTaintClass?: string;
}

export type MutationArgs = Omit<CommitArgs, "operations" | "tokens">;

/** A preview carries the same intent as the commit it stands in for, so the daemon parses the identity it would later record; tokens are refused, so none are collected. */
export interface PreviewArgs extends MutationArgs {
    operations: DispositionOperation[];
}

export type AvailableState = Extract<MemoryState, { kind: "available" }>;
export type NonAvailableState = Exclude<MemoryState, { kind: "available" }>;

/** `available` carries the payload; any other state carries only itself. */
export type KernelResult<P> = ({ state: AvailableState } & P) | { state: NonAvailableState };

/** Narrows a result to its payload-bearing arm. */
export function isAvailable<P>(result: KernelResult<P>): result is { state: AvailableState } & P {
    return result.state.kind === "available";
}

export type ReadResult = KernelResult<ReadPayload>;

/**
 * A read projected to the value injectors and status surfaces carry: the
 * state, the rows (empty unless `available`), and the snapshot position the
 * rows were read at (`null` unless `available`).
 */
export interface KernelMemorySnapshot {
    state: MemoryState;
    rows: ReadRow[];
    knownAsOf: number | null;
    /** Mirrors the read's `truncated` flag: the daemon dropped rows to fit its per-read bounds, so `rows` is a capped prefix and counts derived from it are lower bounds. */
    truncated?: boolean;
}

/** Resolves the client bound to one session and filesystem project root. */
export type KernelClientResolver = (args: {
    sessionId: string;
    projectRoot: string;
}) => KernelClient;

export function kernelMemorySnapshotFrom(result: ReadResult): KernelMemorySnapshot {
    return isAvailable(result)
        ? {
              state: result.state,
              rows: result.rows,
              knownAsOf: result.known_as_of,
              truncated: result.truncated,
          }
        : { state: result.state, rows: [], knownAsOf: null };
}
export type CommitResult = KernelResult<CommitPayload>;
export type PreviewResult = KernelResult<PreviewPayload>;

export interface KernelClientOptions {
    transport: KernelTransport;
    enabled: boolean;
    sessionId: string;
    projectRoot: string;
    /** `intent.producer` on every write; part of the `(producer, operation_key)` identity. */
    producer?: string;
    tokens?: TokenStore;
    /** Bounds a call whose caller passes no `deadlineMs`. */
    defaultDeadlineMs?: number;
    /** Monotonic source every call deadline is measured against. Injectable for deterministic tests. */
    clock?: MonotonicClock;
}

const DEFAULT_PRODUCER = "plugin";
const DEFAULT_DEADLINE_MS = 10_000;
/**
 * Fields of the operation key are joined with the ASCII unit separator. Every field but the last must be separator-free; then the joined bytes parse back to exactly one field list of that arity, so distinct inputs cannot collide by concatenation. The last field may itself be a separator-joined composite.
 */
export const OPERATION_KEY_SEPARATOR = "\u001f";

export function sha256Hex(input: string | Uint8Array): string {
    return createHash("sha256").update(input).digest("hex");
}

/** A separator-free value can precede another key field without shifting its boundary. */
export function isSeparatorFree(value: string): boolean {
    return !value.includes(OPERATION_KEY_SEPARATOR);
}

/** Rejects a non-final separator because it shifts field boundaries; persisted ids pin the join encoding, so escaping is not an option. */
function joinKeyFields(fields: readonly string[]): string {
    for (let index = 0; index + 1 < fields.length; index += 1) {
        if (!isSeparatorFree(fields[index] as string)) {
            throw new RangeError(
                `key field ${index} contains the U+001F separator and would shift the field boundary`,
            );
        }
    }
    return fields.join(OPERATION_KEY_SEPARATOR);
}

/** Stores persist ids minted here, so the separator, field order, and 32-hex slice are frozen byte-for-byte: the same inputs always resolve to the same id. Throws `RangeError` when a non-final field contains the separator. */
export function deriveObjectId(prefix: string, ...fields: readonly string[]): string {
    return `${prefix}_${sha256Hex(joinKeyFields(fields)).slice(0, 32)}`;
}

/** The body fields a commit is admitted under. `tokens` and `deadline_ms` stay out because the divergence retry and reissue legitimately change them. */
export interface RequestDigestInput {
    operations: readonly CommitOperation[];
    sourceKind: SourceKind;
    assertedSourceClass?: string;
    assertedTaintClass?: string;
}

/** Covers the operations and the classification fields (`source_kind`, asserted classes) that decide the stored trust class, so a reused key that changes any of them reads as a different body rather than a replay. */
export function deriveRequestDigest(input: RequestDigestInput): string {
    const body = {
        operations: input.operations,
        source_kind: input.sourceKind,
        asserted_source_class: input.assertedSourceClass,
        asserted_taint_class: input.assertedTaintClass,
    };
    // The JSON round-trip drops explicitly-undefined keys so a spec built by spread and a spec that omits the field hash identically; `stableStringify` then fixes key order.
    return sha256Hex(stableStringify(JSON.parse(JSON.stringify(body))));
}

/** The key names only the stable operation identity — never the body, which travels in `request_digest`, and never the free-text `cause` — so a redelivered identity with different bytes hits the daemon's `operation_key_reused` rejection instead of committing as a second operation. The project is not hashed here: the daemon prefixes every receipt key with the digest of the canonicalized bound root, so a root reached through a symlink and through its resolved path share one receipt namespace, which a client-side hash of the raw spelling would split. Throws `RangeError` when `producer` or `actor` contains the separator; `operationId` may be a separator-joined composite. */
export function deriveOperationKey(parts: {
    producer: string;
    actor: string;
    operationId: string;
}): string {
    return sha256Hex(joinKeyFields([parts.producer, parts.actor, parts.operationId]));
}

/** A successful invocation carries the connection identity its body was sent under, so the tokens in the response are recorded against the connection that minted them. */
type Invoked =
    | { ok: true; raw: unknown; connectionIdentity?: string }
    | { ok: false; state: NonAvailableState };

interface InvokeOptions {
    signal?: AbortSignal;
    deadline: Deadline;
    /** A write whose `outcome_unknown` is reissued once under the same identity and digest; the body is rebuilt per attempt so only `deadline_ms` reflects the budget left. */
    reissuable: boolean;
    /** A mutating call whose exhausted `outcome_unknown` must stay ambiguous: reads answer `daemon_absent` because re-reading is always safe, but a sent write may have committed and a definitive-looking failure invites a retry under a fresh identity. */
    mutating?: boolean;
}

function errorCodeOf(error: unknown): string | undefined {
    return isRecord(error) && typeof error.code === "string" ? error.code : undefined;
}

/** host-client owns which connect-time failures are transient; a terminal `ConnectionFileError` falls through to `invalid(internal)` rather than reading as an absent daemon. */
function isDaemonAbsent(error: unknown): boolean {
    return isConnectTransient(error) || errorCodeOf(error) === "EIDNARA_HOST_CONNECTION_BACKOFF";
}

/** The kernel routes refuse an over-limit or malformed envelope with this transport code; it is the caller's input, not a daemon fault. */
const INVALID_PARAMS_CODE = "invalid_params";

/**
 * Codes a daemon answers when no `kernel.*` route matches the request, so the
 * caller sees the version-skew state rather than an internal error.
 */
const UNKNOWN_METHOD_CODES: ReadonlySet<string> = new Set([
    "unrecognized_request_shape",
    "facade_envelope_not_supported",
]);

function isUnknownMethod(code: string | undefined): boolean {
    return code !== undefined && UNKNOWN_METHOD_CODES.has(code);
}

function nonAvailable(state: MemoryState): NonAvailableState {
    return state as NonAvailableState;
}

function isSnapshotDiverged(state: MemoryState): boolean {
    return state.kind === "unavailable" && state.reason === "snapshot_diverged";
}

export class KernelClient {
    private readonly transport: KernelTransport;
    private readonly enabled: boolean;
    private readonly sessionId: string;
    private readonly projectRoot: string;
    private readonly producer: string;
    private readonly defaultDeadlineMs: number;
    private readonly clock: MonotonicClock | undefined;
    readonly tokens: TokenStore;

    constructor(options: KernelClientOptions) {
        this.transport = options.transport;
        this.enabled = options.enabled;
        this.sessionId = options.sessionId;
        this.projectRoot = options.projectRoot;
        this.producer = options.producer ?? DEFAULT_PRODUCER;
        this.tokens = options.tokens ?? new TokenCache();
        this.defaultDeadlineMs = options.defaultDeadlineMs ?? DEFAULT_DEADLINE_MS;
        this.clock = options.clock;
    }

    private wireBody(method: string, fields: Record<string, unknown>): Record<string, unknown> {
        return {
            method,
            v: 1,
            session_id: this.sessionId,
            project_root: this.projectRoot,
            ...fields,
        };
    }

    /** An invalid budget is the caller's input, so it answers `invalid_input` rather than letting `Deadline.start`'s `RangeError` escape into the tool. */
    private deadline(options: CallOptions): Deadline | NonAvailableState {
        const timeoutMs = options.deadlineMs ?? this.defaultDeadlineMs;
        if (!Number.isFinite(timeoutMs) || timeoutMs < 0) {
            return nonAvailable(invalid("invalid_input"));
        }
        return Deadline.start(timeoutMs, this.clock);
    }

    /** Preconditions every call shares, checked before any transport work. */
    private gate(signal: AbortSignal | undefined): NonAvailableState | null {
        if (!this.enabled) return nonAvailable(disabled());
        if (signal?.aborted) return nonAvailable(cancelled());
        if (!this.transport.connectionFileExists()) {
            return nonAvailable(unavailable("daemon_absent"));
        }
        return null;
    }

    private async invoke(
        method: string,
        bodyFor: () => Record<string, unknown>,
        options: InvokeOptions,
    ): Promise<Invoked> {
        const gated = this.gate(options.signal);
        if (gated) return { ok: false, state: gated };
        let rebound = false;
        let reissued = false;
        // After a mutating call's ambiguous send, every later failure exit stays `outcome_unknown`: a reissue that is never sent, an unbound route, or a transport error cannot resolve whether the first attempt committed, and any definitive-looking failure invites a retry under a fresh identity.
        const failed = (state: MemoryState): Invoked => ({
            ok: false,
            state:
                reissued && options.mutating
                    ? nonAvailable(unavailable("outcome_unknown"))
                    : nonAvailable(state),
        });
        const absent = (): Invoked => failed(unavailable("daemon_absent"));
        // The identity is read once, so every attempt — the first send, a reissue, a rebound route — carries the identity the call began under; a transport whose connection moved refuses it instead of delivering another daemon's tokens.
        const connectionIdentity = this.transport.connectionIdentity?.();
        // The caller's signal or the deadline ended the attempt. After a reissued unknown outcome on a write, or when the interrupted attempt itself threw one, the request may still be applied; a plain cancellation would invite a retry under a fresh identity.
        const interrupted = (unknownOutcome = false): Invoked => ({
            ok: false,
            state:
                (unknownOutcome || reissued) && options.mutating
                    ? nonAvailable(unavailable("outcome_unknown"))
                    : nonAvailable(cancelled()),
        });
        const bounds = (): Pick<KernelTransportCall, "signal" | "timeoutMs"> => ({
            ...(options.signal ? { signal: options.signal } : {}),
            timeoutMs: Math.max(1, options.deadline.remainingMs()),
        });
        for (;;) {
            if (options.deadline.isExpired()) return interrupted();
            try {
                const raw = await this.transport.call({
                    sessionId: this.sessionId,
                    projectRoot: this.projectRoot,
                    method,
                    body: bodyFor(),
                    ...bounds(),
                    ...(connectionIdentity === undefined ? {} : { connectionIdentity }),
                });
                return {
                    ok: true,
                    raw,
                    ...(connectionIdentity === undefined ? {} : { connectionIdentity }),
                };
            } catch (error) {
                // A refused identity means the tokens this body carried belong to a daemon that is gone; they are dropped before any exit, including a cancellation, so the caller's next body is not built from them.
                const identityChanged = error instanceof ConnectionIdentityChangedError;
                if (identityChanged) this.tokens.dropProject(this.projectRoot);
                // An `outcome_unknown` thrown from a write while cancellation or the deadline fires must keep its classification: the daemon may have committed, and reporting an ordinary cancellation would claim a definitively unapplied request.
                const unknownOutcome = isHostCallError(error) && error.kind === "outcome_unknown";
                if (options.signal?.aborted || options.deadline.isExpired()) {
                    return interrupted(unknownOutcome);
                }
                if (identityChanged) return failed(unavailable("snapshot_diverged"));
                if (isHostCallError(error)) {
                    if (error.kind === "not_sent") return absent();
                    if (error.kind === "outcome_unknown") {
                        if (options.reissuable && !reissued) {
                            reissued = true;
                            continue;
                        }
                        return options.mutating
                            ? { ok: false, state: nonAvailable(unavailable("outcome_unknown")) }
                            : absent();
                    }
                    if (error.code === "route_unbound") {
                        if (rebound) return absent();
                        rebound = true;
                        try {
                            await this.transport.ensureRoute({
                                sessionId: this.sessionId,
                                projectRoot: this.projectRoot,
                                ...bounds(),
                            });
                        } catch {
                            // A rebind cut short by the caller or the budget is a cancellation; any other failure leaves the route unbound and the daemon unreachable.
                            if (options.signal?.aborted || options.deadline.isExpired()) {
                                return interrupted();
                            }
                            return absent();
                        }
                        continue;
                    }
                    if (isUnknownMethod(error.code)) {
                        return failed(invalid("unrecognized_state"));
                    }
                    if (error.code === INVALID_PARAMS_CODE) {
                        return failed(invalid("invalid_input"));
                    }
                    return failed(invalid("internal"));
                }
                if (isDaemonAbsent(error)) return absent();
                if (error instanceof StoreLifecycleError) return failed(unavailable(error.reason));
                return failed(invalid("internal"));
            }
        }
    }

    private async call<P>(
        method: string,
        bodyFor: () => Record<string, unknown>,
        options: InvokeOptions,
        parse: (raw: unknown) => Parsed<P>,
    ): Promise<{ result: KernelResult<P>; connectionIdentity?: string }> {
        const invoked = await this.invoke(method, bodyFor, options);
        if (!invoked.ok) return { result: { state: invoked.state } };
        const identity =
            invoked.connectionIdentity === undefined
                ? {}
                : { connectionIdentity: invoked.connectionIdentity };
        const parsed = parse(invoked.raw);
        if (parsed.state.kind !== "available" || parsed.payload === null) {
            // A daemon-produced negative state proves a mutating request was not applied, but an undecodable response does not: the commit may have succeeded and only its receipt was lost to a malformed or version-skewed payload, so a definitive-looking error would invite a fresh-identity retry.
            const undecodable =
                (parsed.state.kind === "invalid" && parsed.state.reason === "unrecognized_state") ||
                (parsed.state.kind === "available" && parsed.payload === null);
            if (undecodable && options.mutating) {
                return { result: { state: nonAvailable(unavailable("outcome_unknown")) } };
            }
            return { result: { state: nonAvailable(parsed.state) }, ...identity };
        }
        return { result: { state: parsed.state, ...parsed.payload }, ...identity };
    }

    private async readAt(
        args: ReadArgs,
        asOf: number | null,
        deadline: Deadline,
    ): Promise<ReadResult> {
        const body = this.wireBody("kernel.read", {
            surface: args.surface,
            as_of: asOf,
            gated: args.gated ?? false,
            ...(args.objectIds === undefined ? {} : { object_ids: [...args.objectIds] }),
        });
        const { result, connectionIdentity } = await this.call(
            "kernel.read",
            () => body,
            // Reads have no side effects, so an ambiguous transport outcome reissues once instead of answering daemon_absent for a transient drop.
            { signal: args.signal, deadline, reissuable: true },
            parseReadResponse,
        );
        if (isAvailable(result)) {
            this.tokens.remember(
                this.projectRoot,
                result.rows,
                result.known_as_of,
                connectionIdentity,
            );
        }
        return result;
    }

    /**
     * A read at `asOf` that the daemon reports as diverged drops the project's
     * tokens and reads the tip once; a second divergence surfaces as-is.
     */
    async read(args: ReadArgs): Promise<ReadResult> {
        if (args.objectIds !== undefined && args.objectIds.length > MAX_READ_OBJECT_IDS) {
            return { state: nonAvailable(invalid("invalid_input")) };
        }
        const deadline = this.deadline(args);
        if (!(deadline instanceof Deadline)) return { state: deadline };
        const first = await this.readAt(args, args.asOf ?? null, deadline);
        if (!isSnapshotDiverged(first.state)) return first;
        this.tokens.dropProject(this.projectRoot);
        return await this.readAt(args, null, deadline);
    }

    private commitBody(
        args: CommitArgs,
        tokens: MutationToken[],
        deadline: Deadline,
        preview = false,
    ): Record<string, unknown> {
        const producer = args.producer ?? this.producer;
        const sourceKind = args.sourceKind ?? "assistant";
        const requestDigest =
            args.requestDigest ??
            deriveRequestDigest({
                operations: args.operations,
                sourceKind,
                assertedSourceClass: args.assertedSourceClass,
                assertedTaintClass: args.assertedTaintClass,
            });
        const operationKey = deriveOperationKey({
            producer,
            actor: args.actor,
            operationId: args.operationId,
        });
        return this.wireBody("kernel.commit", {
            intent: {
                producer,
                operation_key: operationKey,
                request_digest: requestDigest,
                actor: args.actor,
                cause: args.cause,
            },
            tokens,
            operations: args.operations,
            source_kind: sourceKind,
            ...(preview ? { preview: true } : {}),
            ...(args.assertedSourceClass === undefined
                ? {}
                : { asserted_source_class: args.assertedSourceClass }),
            ...(args.assertedTaintClass === undefined
                ? {}
                : { asserted_taint_class: args.assertedTaintClass }),
            // The daemon spends `deadline_ms` waiting for its writer, so each attempt sends what is left of the caller's budget after the refresh read, an earlier attempt, or a reissue consumed part of it; the original total would let a late attempt park a daemon thread past the client's own deadline.
            ...(args.deadlineMs === undefined
                ? {}
                : { deadline_ms: Math.max(1, Math.floor(deadline.remainingMs())) }),
        });
    }

    /** Tokens the commit needs: one per object an operation replaces or retires. */
    private targetIds(operations: readonly CommitOperation[]): string[] {
        const ids = new Set<string>();
        for (const operation of operations) {
            if (operation.op === "supersede_decision") ids.add(operation.replaced_object_id);
            if (operation.op === "retire_decision") ids.add(operation.object_id);
        }
        return [...ids];
    }

    private collectTokens(args: CommitArgs): { tokens: MutationToken[]; missing: string[] } {
        if (args.tokens !== undefined) return { tokens: [...args.tokens], missing: [] };
        // Reads name the transport's current identity so the store drops tokens minted under a previous connection before they can enter this body.
        const connectionIdentity = this.transport.connectionIdentity?.();
        const tokens: MutationToken[] = [];
        const missing: string[] = [];
        for (const objectId of this.targetIds(args.operations)) {
            const cached = this.tokens.get(this.projectRoot, objectId, connectionIdentity);
            if (cached) tokens.push(cached);
            else missing.push(objectId);
        }
        return { tokens, missing };
    }

    private readTargets(
        objectIds: readonly string[],
        args: CallOptions,
        deadline: Deadline,
    ): Promise<ReadResult> {
        return this.readAt(
            { surface: "explicit_search", gated: false, signal: args.signal, objectIds },
            null,
            deadline,
        );
    }

    /**
     * A filtered read bypasses the daemon's newest-rows cap but remains subject to its byte budget, which keeps a newest-first prefix; so an id absent from a truncated batch is re-read alone before it is judged, since one row always fits the budget.
     * Returns the first non-available state, or `null` once every id has been read to a complete snapshot.
     */
    private async refreshTokens(
        ids: readonly string[],
        args: CallOptions,
        deadline: Deadline,
    ): Promise<NonAvailableState | null> {
        for (let start = 0; start < ids.length; start += MAX_READ_OBJECT_IDS) {
            const batch = ids.slice(start, start + MAX_READ_OBJECT_IDS);
            const read = await this.readTargets(batch, args, deadline);
            if (!isAvailable(read)) return read.state;
            if (!read.truncated) continue;
            for (const id of batch) {
                if (this.tokens.get(this.projectRoot, id)) continue;
                const single =
                    batch.length === 1 ? read : await this.readTargets([id], args, deadline);
                if (!isAvailable(single)) return single.state;
                // A one-object filter cannot exceed the byte budget, so a truncated empty reply is a daemon contract violation, not proof of retraction.
                if (single.truncated && single.rows.length === 0) {
                    return nonAvailable(invalid("internal"));
                }
            }
        }
        return null;
    }

    /**
     * A target still absent after a complete refresh read is not live in this project's scope (retired, hidden, or foreign), so the envelope is never sent: the daemon checks only the tokens it receives and would otherwise mutate the object unfenced.
     */
    private async commitOnce(args: CommitArgs, deadline: Deadline): Promise<CommitResult> {
        let { tokens, missing } = this.collectTokens(args);
        if (missing.length > 0) {
            const failure = await this.refreshTokens(missing, args, deadline);
            if (failure) return { state: failure };
            ({ tokens, missing } = this.collectTokens(args));
            if (missing.length > 0) return { state: nonAvailable(conflict("retracted")) };
        }
        const { result, connectionIdentity } = await this.call(
            "kernel.commit",
            () => this.commitBody(args, tokens, deadline),
            { signal: args.signal, deadline, reissuable: true, mutating: true },
            parseCommitResponse,
        );
        if (isAvailable(result)) {
            this.tokens.rememberTokens(
                this.projectRoot,
                result.tokens,
                result.known_as_of,
                connectionIdentity,
            );
        }
        return result;
    }

    /**
     * One idempotent envelope. A target without a cached token triggers one ungated `explicit_search` read first; `snapshot_diverged` drops the project's tokens and reruns the read-then-commit once.
     * Caller-supplied tokens are sent verbatim, so their divergence is returned as-is: no re-read can change them, and a second identical send could only replace the definitive state with an ambiguous transport outcome.
     */
    async commit(input: CommitArgs): Promise<CommitResult> {
        // Every attempt of one call must send the identity and digest of the first: a reissue or divergence retry that read a caller-mutated argument object would leave as a second operation. The operations are cloned through JSON, which is the form the wire and the digest both see.
        const args: CommitArgs = {
            ...input,
            operations: JSON.parse(JSON.stringify(input.operations)) as CommitOperation[],
            ...(input.tokens === undefined
                ? {}
                : { tokens: input.tokens.map((token) => ({ ...token })) }),
        };
        // The key fields are hashed under the separator; a field carrying it is refused here so the derivation's `RangeError` never escapes into the tool.
        const producer = args.producer ?? this.producer;
        if (![producer, args.actor].every(isSeparatorFree)) {
            return { state: nonAvailable(invalid("invalid_input")) };
        }
        // The daemon refuses an envelope over its limits before any kernel work; refusing here spares the refresh reads that would otherwise spend the budget on a doomed envelope.
        if (
            args.operations.length > MAX_COMMIT_OPERATIONS ||
            (args.tokens?.length ?? 0) > MAX_COMMIT_TOKENS
        ) {
            return { state: nonAvailable(invalid("invalid_input")) };
        }
        const deadline = this.deadline(args);
        if (!(deadline instanceof Deadline)) return { state: deadline };
        const first = await this.commitOnce(args, deadline);
        if (!isSnapshotDiverged(first.state) || args.tokens !== undefined) return first;
        this.tokens.dropProject(this.projectRoot);
        const retried = await this.commitOnce(args, deadline);
        if (isSnapshotDiverged(retried.state)) this.tokens.dropProject(this.projectRoot);
        return retried;
    }

    private async previewOnce(args: PreviewArgs, deadline: Deadline): Promise<PreviewResult> {
        const { result } = await this.call(
            "kernel.commit",
            () => this.commitBody(args, [], deadline, true),
            // A preview writes nothing, so an ambiguous transport outcome reissues once, as a read does.
            { signal: args.signal, deadline, reissuable: true },
            parsePreviewResponse,
        );
        return result;
    }

    /**
     * Judges disposition operations at the tip without writing: the reply
     * carries each surface's verdict before and after, and whether any surface
     * serving the object would change. No receipt is created, so the same
     * identity is still free for `commit`.
     *
     * Previews carry no tokens or `as_of`, so after a connection identity refusal the same body is sent once more against the new connection, as a diverged read re-reads the tip.
     */
    async previewDispositions(input: PreviewArgs): Promise<PreviewResult> {
        const args: PreviewArgs = {
            ...input,
            operations: JSON.parse(JSON.stringify(input.operations)) as DispositionOperation[],
        };
        const producer = args.producer ?? this.producer;
        if (![producer, args.actor].every(isSeparatorFree)) {
            return { state: nonAvailable(invalid("invalid_input")) };
        }
        if (args.operations.length > MAX_COMMIT_OPERATIONS) {
            return { state: nonAvailable(invalid("invalid_input")) };
        }
        const deadline = this.deadline(args);
        if (!(deadline instanceof Deadline)) return { state: deadline };
        const first = await this.previewOnce(args, deadline);
        if (!isSnapshotDiverged(first.state)) return first;
        return await this.previewOnce(args, deadline);
    }

    create(spec: DecisionSpecInput, args: MutationArgs): Promise<CommitResult> {
        return this.commit({ ...args, operations: [{ op: "insert_decision", spec }] });
    }

    revise(objectId: string, spec: DecisionSpecInput, args: MutationArgs): Promise<CommitResult> {
        return this.commit({
            ...args,
            operations: [{ op: "supersede_decision", replaced_object_id: objectId, spec }],
        });
    }

    /** Every merged object is superseded by the one survivor inside one envelope. */
    merge(
        objectIds: readonly string[],
        survivorSpec: DecisionSpecInput,
        args: MutationArgs,
    ): Promise<CommitResult> {
        return this.commit({
            ...args,
            operations: objectIds.map((objectId) => ({
                op: "supersede_decision" as const,
                replaced_object_id: objectId,
                spec: survivorSpec,
            })),
        });
    }

    archive(objectId: string, args: MutationArgs): Promise<CommitResult> {
        return this.commit({
            ...args,
            operations: [{ op: "retire_decision", object_id: objectId }],
        });
    }
}
