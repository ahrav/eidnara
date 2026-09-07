import { describe, expect, test } from "bun:test";
import { ConnectionFileError } from "../host-client/connection-file";
import { HostCallError } from "../host-client/errors";
import {
    type DecisionSpecInput,
    deriveObjectId,
    deriveOperationKey,
    deriveRequestDigest,
    isAvailable,
    KernelClient,
    type KernelClientOptions,
    type KernelRebind,
    type KernelTransport,
    type KernelTransportCall,
    kernelMemorySnapshotFrom,
} from "./client";
import { MAX_COMMIT_OPERATIONS, MAX_COMMIT_TOKENS, MAX_READ_OBJECT_IDS } from "./wire";

const PROJECT = "/repo/project";
const SESSION = "session-a";

type Reply = unknown | ((call: KernelTransportCall) => unknown | Promise<unknown>);

class FakeTransport implements KernelTransport {
    calls: KernelTransportCall[] = [];
    rebinds = 0;
    rebindArgs: KernelRebind[] = [];
    fileExists = true;
    private replies: Reply[] = [];
    rebindError: Error | null = null;
    onRebind: ((args: KernelRebind) => void) | null = null;

    queue(...replies: Reply[]): this {
        this.replies.push(...replies);
        return this;
    }

    connectionFileExists(): boolean {
        return this.fileExists;
    }

    async call(args: KernelTransportCall): Promise<unknown> {
        this.calls.push(args);
        const reply = this.replies.shift();
        if (reply === undefined) throw new Error(`no scripted reply for ${args.method}`);
        const value = typeof reply === "function" ? await reply(args) : reply;
        if (value instanceof Error) throw value;
        return value;
    }

    async ensureRoute(args: KernelRebind): Promise<void> {
        this.rebinds += 1;
        this.rebindArgs.push(args);
        this.onRebind?.(args);
        if (this.rebindError) throw this.rebindError;
    }

    bodies(method: string): Record<string, unknown>[] {
        return this.calls
            .filter((call) => call.method === method)
            .map((call) => call.body as Record<string, unknown>);
    }
}

function client(
    transport: FakeTransport,
    enabled = true,
    options: Partial<KernelClientOptions> = {},
): KernelClient {
    return new KernelClient({
        transport,
        enabled,
        sessionId: SESSION,
        projectRoot: PROJECT,
        ...options,
    });
}

function row(objectId: string, knownAsOf: number) {
    return {
        object: {
            object_id: objectId,
            object_kind: "decision",
            domain_id: "memory",
            source_kind: "assistant",
            source_id: "memory-lineage",
            source_revision: 1,
            created_commit_seq: 1,
            invalidated_commit_seq: null,
            superseded_by: null,
            sensitivity: "normal",
        },
        visibility: "labeled",
        labeled: true,
        scope_id: "project:x",
        token: { object_id: objectId, known_as_of: knownAsOf },
        decision: { decision_kind: "memory", payload: { summary: objectId, rationale: "" } },
    };
}

function readReply(knownAsOf: number, ...objectIds: string[]) {
    return {
        state: { kind: "available" },
        known_as_of: knownAsOf,
        tip: knownAsOf,
        gated: false,
        rows: objectIds.map((id) => row(id, knownAsOf)),
    };
}

function commitReply(commitSeq: number, replayed: boolean, ...objectIds: string[]) {
    return {
        state: { kind: "available" },
        receipt: { commit_seq: commitSeq, replayed },
        known_as_of: commitSeq,
        tokens: objectIds.map((id) => ({ object_id: id, known_as_of: commitSeq })),
    };
}

const DIVERGED = { state: { kind: "unavailable", reason: "snapshot_diverged" } };

const spec: DecisionSpecInput = {
    decision_id: "decision-1",
    object_id: "decision-object-1",
    domain_id: "memory",
    decision_kind: "memory",
    payload: { summary: "s", rationale: "r" },
    source_id: "memory-lineage",
    source_revision: 1,
};

const intent = { actor: "assistant", operationId: "session-1\u001fcall-1", cause: "ctx_memory" };

describe("KernelClient gating", () => {
    test("disabled returns before any transport work", async () => {
        const transport = new FakeTransport();
        transport.fileExists = false;
        const result = await client(transport, false).read({ surface: "auto_inject" });
        expect(result.state).toEqual({ kind: "disabled" });
        expect(transport.calls).toHaveLength(0);
    });

    test("a missing connection file is daemon_absent with no route opened", async () => {
        const transport = new FakeTransport();
        transport.fileExists = false;
        const result = await client(transport).read({ surface: "auto_inject" });
        expect(result.state).toEqual({ kind: "unavailable", reason: "daemon_absent" });
        expect(transport.calls).toHaveLength(0);
        expect(transport.rebinds).toBe(0);
    });

    test("an already-aborted signal is cancelled with no transport call", async () => {
        const transport = new FakeTransport();
        const controller = new AbortController();
        controller.abort();
        const result = await client(transport).read({
            surface: "auto_inject",
            signal: controller.signal,
        });
        expect(result.state).toEqual({ kind: "cancelled" });
        expect(transport.calls).toHaveLength(0);
    });

    test("a signal aborted mid-call is cancelled", async () => {
        const transport = new FakeTransport();
        const controller = new AbortController();
        transport.queue(() => {
            controller.abort();
            throw new Error("module transport call aborted");
        });
        const result = await client(transport).read({
            surface: "auto_inject",
            signal: controller.signal,
        });
        expect(result.state).toEqual({ kind: "cancelled" });
        expect(transport.calls).toHaveLength(1);
    });

    test("an outcome_unknown racing an abort keeps its classification instead of cancelled", async () => {
        const transport = new FakeTransport();
        const controller = new AbortController();
        transport.queue(() => {
            controller.abort();
            throw new HostCallError("outcome_unknown", "deadline", "request_deadline");
        });
        // The commit was sent and may have been applied; reporting a plain cancellation would invite a retry under a fresh identity and a duplicate commit. commentlint: allow(JUDGE)
        const result = await client(transport).create(spec, {
            ...intent,
            signal: controller.signal,
        });
        expect(result.state).toEqual({ kind: "unavailable", reason: "outcome_unknown" });
        expect(transport.bodies("kernel.commit")).toHaveLength(1);
    });
});

describe("KernelClient transport mapping", () => {
    test("not_sent is daemon_absent", async () => {
        const transport = new FakeTransport().queue(new HostCallError("not_sent", "never wrote"));
        const result = await client(transport).read({ surface: "auto_inject" });
        expect(result.state).toEqual({ kind: "unavailable", reason: "daemon_absent" });
        expect(transport.calls).toHaveLength(1);
    });

    test("ECONNREFUSED and a closed socket are daemon_absent", async () => {
        const refused = Object.assign(new Error("connect"), { code: "ECONNREFUSED" });
        const closed = Object.assign(new Error("closed"), { name: "SocketClosedError" });
        for (const error of [refused, closed]) {
            const transport = new FakeTransport().queue(error);
            const result = await client(transport).read({ surface: "auto_inject" });
            expect(result.state).toEqual({ kind: "unavailable", reason: "daemon_absent" });
        }
    });

    test("outcome_unknown on a read reissues once and serves the retried response", async () => {
        const transport = new FakeTransport().queue(
            new HostCallError("outcome_unknown", "deadline", "request_deadline"),
            readReply(1),
        );
        const result = await client(transport).read({ surface: "auto_inject" });
        expect(isAvailable(result)).toBe(true);
        expect(transport.calls).toHaveLength(2);
    });

    test("a second outcome_unknown on the same read is daemon_absent", async () => {
        const transport = new FakeTransport().queue(
            new HostCallError("outcome_unknown", "deadline"),
            new HostCallError("outcome_unknown", "deadline"),
        );
        const result = await client(transport).read({ surface: "auto_inject" });
        expect(result.state).toEqual({ kind: "unavailable", reason: "daemon_absent" });
        expect(transport.calls).toHaveLength(2);
    });

    test("outcome_unknown on a write is reissued once with identical bytes", async () => {
        const transport = new FakeTransport().queue(
            new HostCallError("outcome_unknown", "deadline", "request_deadline"),
            commitReply(5, true, "decision-object-1"),
        );
        const result = await client(transport).create(spec, intent);
        expect(isAvailable(result)).toBe(true);
        if (!isAvailable(result)) throw new Error("unreachable");
        expect(result.receipt.replayed).toBe(true);
        const bodies = transport.bodies("kernel.commit");
        expect(bodies).toHaveLength(2);
        expect(JSON.stringify(bodies[0])).toBe(JSON.stringify(bodies[1]));
    });

    test("an already-expired deadline is cancelled before any transport call", async () => {
        const transport = new FakeTransport().queue(commitReply(5, false, "decision-object-1"));
        const result = await client(transport).create(spec, { ...intent, deadlineMs: 0 });
        expect(result.state).toEqual({ kind: "cancelled" });
        expect(transport.calls).toHaveLength(0);
    });

    test("a negative or non-finite deadlineMs is invalid_input, never a thrown RangeError", async () => {
        for (const deadlineMs of [-1, Number.NaN, Number.POSITIVE_INFINITY]) {
            const transport = new FakeTransport().queue(readReply(1), commitReply(2, false));
            const read = await client(transport).read({ surface: "auto_inject", deadlineMs });
            expect(read.state).toEqual({ kind: "invalid", reason: "invalid_input" });
            const commit = await client(transport).create(spec, { ...intent, deadlineMs });
            expect(commit.state).toEqual({ kind: "invalid", reason: "invalid_input" });
            expect(transport.calls).toHaveLength(0);
        }
    });

    test("an invalid defaultDeadlineMs surfaces as invalid_input on the call that would use it", async () => {
        const transport = new FakeTransport().queue(readReply(1), readReply(1));
        const c = client(transport, true, { defaultDeadlineMs: -5 });
        const defaulted = await c.read({ surface: "auto_inject" });
        expect(defaulted.state).toEqual({ kind: "invalid", reason: "invalid_input" });
        const explicit = await c.read({ surface: "auto_inject", deadlineMs: 1_000 });
        expect(isAvailable(explicit)).toBe(true);
        expect(transport.calls).toHaveLength(1);
    });

    test("an envelope over the daemon's operation limit is refused before any read or commit", async () => {
        const transport = new FakeTransport();
        const targets = Array.from({ length: MAX_COMMIT_OPERATIONS + 1 }, (_, i) => `t${i}`);
        const result = await client(transport).merge(targets, spec, intent);
        expect(result.state).toEqual({ kind: "invalid", reason: "invalid_input" });
        expect(transport.calls).toHaveLength(0);
    });

    test("an envelope at the operation limit is sent", async () => {
        const targets = Array.from({ length: MAX_COMMIT_OPERATIONS }, (_, i) => `t${i}`);
        const reads: unknown[] = [];
        for (let start = 0; start < targets.length; start += MAX_READ_OBJECT_IDS) {
            reads.push(readReply(3, ...targets.slice(start, start + MAX_READ_OBJECT_IDS)));
        }
        const transport = new FakeTransport().queue(
            ...reads,
            commitReply(4, false, ...targets, spec.object_id),
        );
        const result = await client(transport).merge(targets, spec, intent);
        expect(isAvailable(result)).toBe(true);
        expect(transport.bodies("kernel.commit")[0]?.operations).toHaveLength(
            MAX_COMMIT_OPERATIONS,
        );
    });

    test("caller-supplied tokens over the daemon's token limit are refused before any commit", async () => {
        const transport = new FakeTransport();
        const tokens = Array.from({ length: MAX_COMMIT_TOKENS + 1 }, (_, i) => ({
            object_id: `t${i}`,
            known_as_of: 1,
        }));
        const result = await client(transport).commit({
            ...intent,
            operations: [{ op: "insert_decision", spec }],
            tokens,
        });
        expect(result.state).toEqual({ kind: "invalid", reason: "invalid_input" });
        expect(transport.calls).toHaveLength(0);
    });

    test("a second outcome_unknown on the same write stays ambiguous", async () => {
        const transport = new FakeTransport().queue(
            new HostCallError("outcome_unknown", "deadline"),
            new HostCallError("outcome_unknown", "deadline"),
        );
        const result = await client(transport).create(spec, intent);
        // Both attempts were sent and either may have committed; daemon_absent would read as a definitive failure and invite a fresh-identity retry. commentlint: allow(JUDGE)
        expect(result.state).toEqual({ kind: "unavailable", reason: "outcome_unknown" });
        expect(transport.bodies("kernel.commit")).toHaveLength(2);
    });

    test("a write reissue that is never sent keeps the first attempt's ambiguity", async () => {
        const transport = new FakeTransport().queue(
            new HostCallError("outcome_unknown", "deadline"),
            new HostCallError("not_sent", "connection retired"),
        );
        const result = await client(transport).create(spec, intent);
        // `not_sent` proves only that the reissue never left; the first attempt may still have committed. commentlint: allow(JUDGE)
        expect(result.state).toEqual({ kind: "unavailable", reason: "outcome_unknown" });
        expect(transport.bodies("kernel.commit")).toHaveLength(2);
    });

    test("an undecodable commit response stays ambiguous instead of a definitive error", async () => {
        const transport = new FakeTransport().queue({ garbage: true });
        const result = await client(transport).create(spec, intent);
        // The transport delivered a response, so the commit may have applied; only its receipt was lost to the malformed payload. commentlint: allow(JUDGE)
        expect(result.state).toEqual({ kind: "unavailable", reason: "outcome_unknown" });
    });

    test("route_unbound rebinds once, then retries once, then is daemon_absent", async () => {
        const unbound = () => new HostCallError("terminal", "no binding", "route_unbound");
        const recovered = new FakeTransport().queue(unbound(), readReply(1));
        const ok = await client(recovered).read({ surface: "auto_inject" });
        expect(ok.state).toEqual({ kind: "available" });
        expect(recovered.rebinds).toBe(1);
        expect(recovered.calls).toHaveLength(2);

        const stuck = new FakeTransport().queue(unbound(), unbound());
        const absent = await client(stuck).read({ surface: "auto_inject" });
        expect(absent.state).toEqual({ kind: "unavailable", reason: "daemon_absent" });
        expect(stuck.rebinds).toBe(1);
        expect(stuck.calls).toHaveLength(2);
    });

    test("the rebind carries the caller's signal and the budget left on the deadline", async () => {
        let now = 0;
        const controller = new AbortController();
        const transport = new FakeTransport().queue(() => {
            now += 2_500;
            return new HostCallError("terminal", "no binding", "route_unbound");
        }, readReply(1));
        const result = await client(transport, true, { clock: () => now }).read({
            surface: "auto_inject",
            signal: controller.signal,
            deadlineMs: 10_000,
        });
        expect(isAvailable(result)).toBe(true);
        expect(transport.rebindArgs).toEqual([
            {
                sessionId: SESSION,
                projectRoot: PROJECT,
                signal: controller.signal,
                timeoutMs: 7_500,
            },
        ]);
    });

    test("a rebind that fails once the deadline has passed is cancelled, not daemon_absent", async () => {
        let now = 0;
        const transport = new FakeTransport().queue(
            new HostCallError("terminal", "no binding", "route_unbound"),
        );
        transport.onRebind = () => {
            now += 10_000;
        };
        transport.rebindError = new Error("rebind deadline expired");
        const result = await client(transport, true, { clock: () => now }).read({
            surface: "auto_inject",
            deadlineMs: 5_000,
        });
        expect(result.state).toEqual({ kind: "cancelled" });
        expect(transport.calls).toHaveLength(1);
    });

    test("a rebind that fails after the caller aborts is cancelled", async () => {
        const controller = new AbortController();
        const transport = new FakeTransport().queue(
            new HostCallError("terminal", "no binding", "route_unbound"),
        );
        transport.onRebind = () => controller.abort();
        transport.rebindError = new Error("rebind aborted");
        const result = await client(transport).read({
            surface: "auto_inject",
            signal: controller.signal,
        });
        expect(result.state).toEqual({ kind: "cancelled" });
        expect(transport.calls).toHaveLength(1);
    });

    test("a rebind interrupted after a reissued write stays outcome_unknown", async () => {
        // The first attempt was sent and may have committed; the interrupted rebind cannot resolve that, and a plain cancellation would invite a retry under a fresh identity. commentlint: allow(JUDGE)
        const controller = new AbortController();
        const transport = new FakeTransport().queue(
            new HostCallError("outcome_unknown", "deadline", "request_deadline"),
            new HostCallError("terminal", "no binding", "route_unbound"),
        );
        transport.onRebind = () => controller.abort();
        transport.rebindError = new Error("rebind aborted");
        const result = await client(transport).create(spec, {
            ...intent,
            signal: controller.signal,
        });
        expect(result.state).toEqual({ kind: "unavailable", reason: "outcome_unknown" });
        expect(transport.bodies("kernel.commit")).toHaveLength(2);
    });

    test("other terminal codes and foreign errors are invalid(internal)", async () => {
        for (const error of [
            new HostCallError("terminal", "bad", "bad_request"),
            new HostCallError("terminal", "bad", "session_mismatch"),
            new TypeError("boom"),
        ]) {
            const transport = new FakeTransport().queue(error);
            const result = await client(transport).read({ surface: "auto_inject" });
            expect(result.state).toEqual({ kind: "invalid", reason: "internal" });
        }
    });

    test("a daemon invalid_params rejection is invalid_input, not an internal error", async () => {
        // The kernel routes refuse an over-limit or malformed envelope with this transport code, which is the caller's fault, not the daemon's. commentlint: allow(JUDGE)
        const transport = new FakeTransport().queue(
            new HostCallError("terminal", "too many operations", "invalid_params"),
        );
        const result = await client(transport).create(spec, intent);
        expect(result.state).toEqual({ kind: "invalid", reason: "invalid_input" });
    });

    test("a connection file that outlived its read budget is daemon_absent", async () => {
        const transport = new FakeTransport().queue(
            new ConnectionFileError("stage expired", "deadline_expired"),
        );
        const result = await client(transport).read({ surface: "auto_inject" });
        expect(result.state).toEqual({ kind: "unavailable", reason: "daemon_absent" });
    });

    test("a terminal connection-file fault is not reported as an absent daemon", async () => {
        // host-client treats every connection-file code but `deadline_expired` as terminal; a foreign-owned or insecure file is a misconfiguration, not a daemon that is not running. commentlint: allow(JUDGE)
        for (const code of ["foreign_owner", "insecure_permissions", "invalid_key"] as const) {
            const transport = new FakeTransport().queue(new ConnectionFileError("bad file", code));
            const result = await client(transport).read({ surface: "auto_inject" });
            expect(result.state).toEqual({ kind: "invalid", reason: "internal" });
        }
    });

    test("an unparseable success body is unrecognized_state", async () => {
        const transport = new FakeTransport().queue({ state: { kind: "available" }, rows: 3 });
        const result = await client(transport).read({ surface: "auto_inject" });
        expect(result.state).toEqual({ kind: "invalid", reason: "unrecognized_state" });
    });

    test("a daemon without kernel routes is unrecognized_state, not internal", async () => {
        for (const code of ["unrecognized_request_shape", "facade_envelope_not_supported"]) {
            const transport = new FakeTransport().queue(
                new HostCallError("terminal", "no such method", code),
            );
            const result = await client(transport).read({ surface: "auto_inject" });
            expect(result.state).toEqual({ kind: "invalid", reason: "unrecognized_state" });
        }
    });

    test("a decision row without its decision payload fails the whole read", async () => {
        const { decision: _decision, ...bare } = row("o1", 3);
        const transport = new FakeTransport().queue({
            state: { kind: "available" },
            known_as_of: 3,
            tip: 3,
            gated: false,
            rows: [bare],
        });
        const result = await client(transport).read({ surface: "explicit_search" });
        expect(result.state).toEqual({ kind: "invalid", reason: "unrecognized_state" });
    });
});

describe("KernelClient reads", () => {
    test("read sends the wire shape and mints tokens", async () => {
        const transport = new FakeTransport().queue(readReply(9, "o1", "o2"));
        const c = client(transport);
        const result = await c.read({ surface: "explicit_search", asOf: 9, gated: true });
        expect(transport.bodies("kernel.read")[0]).toEqual({
            method: "kernel.read",
            v: 1,
            session_id: SESSION,
            project_root: PROJECT,
            surface: "explicit_search",
            as_of: 9,
            gated: true,
        });
        expect(isAvailable(result) && result.rows.length).toBe(2);
        expect(c.tokens.get(PROJECT, "o2")).toEqual({ object_id: "o2", known_as_of: 9 });
    });

    test("a lagging read keeps its lag facts and carries no rows", async () => {
        const transport = new FakeTransport().queue({
            state: { kind: "stale", lag_positions: 12, oldest_unconsumed_age_ms: 900 },
        });
        const result = await client(transport).read({ surface: "explicit_search", gated: true });
        expect(result.state).toEqual({
            kind: "stale",
            lag_positions: 12,
            oldest_unconsumed_age_ms: 900,
        });
        expect("rows" in result).toBe(false);
    });

    test("snapshot_diverged on read drops tokens and re-reads the tip once", async () => {
        const transport = new FakeTransport().queue(
            readReply(3, "o1"),
            DIVERGED,
            readReply(4, "o1"),
        );
        const c = client(transport);
        await c.read({ surface: "auto_inject" });
        const result = await c.read({ surface: "auto_inject", asOf: 99 });
        expect(result.state).toEqual({ kind: "available" });
        const bodies = transport.bodies("kernel.read");
        expect(bodies[1]?.as_of).toBe(99);
        expect(bodies[2]?.as_of).toBeNull();
        expect(c.tokens.get(PROJECT, "o1")?.known_as_of).toBe(4);
    });

    test("a second divergence in one call stays unavailable with tokens dropped", async () => {
        const transport = new FakeTransport().queue(readReply(3, "o1"), DIVERGED, DIVERGED);
        const c = client(transport);
        await c.read({ surface: "auto_inject" });
        const result = await c.read({ surface: "auto_inject", asOf: 99 });
        expect(result.state).toEqual({ kind: "unavailable", reason: "snapshot_diverged" });
        expect(transport.calls).toHaveLength(3);
        expect(c.tokens.get(PROJECT, "o1")).toBeUndefined();
        expect(c.tokens.knownAsOfFor(PROJECT)).toBeUndefined();
    });

    test("a filtered read carries object_ids on the wire", async () => {
        const transport = new FakeTransport().queue(readReply(5, "o1"));
        const result = await client(transport).read({
            surface: "explicit_search",
            objectIds: ["o1", "o2"],
        });
        expect(isAvailable(result)).toBe(true);
        expect(transport.bodies("kernel.read")[0]?.object_ids).toEqual(["o1", "o2"]);
    });

    test("an unfiltered read omits object_ids from the wire", async () => {
        const transport = new FakeTransport().queue(readReply(5, "o1"));
        await client(transport).read({ surface: "explicit_search" });
        expect("object_ids" in (transport.bodies("kernel.read")[0] ?? {})).toBe(false);
    });

    test("an over-limit objectIds filter is invalid_input with no transport call", async () => {
        const transport = new FakeTransport();
        const objectIds = Array.from({ length: 65 }, (_, index) => `o${index}`);
        const result = await client(transport).read({ surface: "explicit_search", objectIds });
        expect(result.state).toEqual({ kind: "invalid", reason: "invalid_input" });
        expect(transport.calls).toHaveLength(0);
    });
});

describe("KernelClient mutations", () => {
    test("operation_key is a deterministic function of the operation identity and project", () => {
        const operations = [{ op: "insert_decision" as const, spec }];
        const digest = deriveRequestDigest({ operations, sourceKind: "assistant" });
        expect(digest).toBe(
            deriveRequestDigest({
                operations: [{ op: "insert_decision", spec: { ...spec } }],
                sourceKind: "assistant",
            }),
        );
        const key = deriveOperationKey({
            projectRoot: PROJECT,
            producer: "plugin",
            actor: "a",
            operationId: "s\u001fc",
        });
        expect(key).toMatch(/^[0-9a-f]{64}$/);
        expect(
            deriveOperationKey({
                projectRoot: "/other",
                producer: "plugin",
                actor: "a",
                operationId: "s\u001fc",
            }),
        ).not.toBe(key);
    });

    test("the same operation identity keeps its key while a changed body changes only the digest", () => {
        const key = deriveOperationKey({
            projectRoot: PROJECT,
            producer: "plugin",
            actor: "a",
            operationId: "session-1\u001fcall-1",
        });
        expect(
            deriveOperationKey({
                projectRoot: PROJECT,
                producer: "plugin",
                actor: "a",
                operationId: "session-1\u001fcall-1",
            }),
        ).toBe(key);
        const digest = deriveRequestDigest({
            operations: [{ op: "insert_decision", spec }],
            sourceKind: "assistant",
        });
        const otherDigest = deriveRequestDigest({
            operations: [{ op: "retire_decision", object_id: "mem_other" }],
            sourceKind: "assistant",
        });
        expect(digest).toBe(
            deriveRequestDigest({
                operations: [{ op: "insert_decision", spec: { ...spec } }],
                sourceKind: "assistant",
            }),
        );
        expect(otherDigest).not.toBe(digest);
    });

    test("the request digest covers the classification fields the daemon admits the write under", () => {
        // `source_kind` and the asserted classes decide the stored trust class, so a reused key that changes them must read as a different body, not a replay. commentlint: allow(JUDGE)
        const operations = [{ op: "insert_decision" as const, spec }];
        const base = deriveRequestDigest({ operations, sourceKind: "assistant" });
        expect(deriveRequestDigest({ operations, sourceKind: "user" })).not.toBe(base);
        expect(
            deriveRequestDigest({ operations, sourceKind: "assistant", assertedSourceClass: "x" }),
        ).not.toBe(base);
        expect(
            deriveRequestDigest({ operations, sourceKind: "assistant", assertedTaintClass: "y" }),
        ).not.toBe(base);
        expect(
            deriveRequestDigest({
                operations,
                sourceKind: "assistant",
                assertedSourceClass: undefined,
            }),
        ).toBe(base);
    });

    test("commits sharing one identity but differing in source_kind carry different digests on the wire", async () => {
        const transport = new FakeTransport().queue(
            commitReply(2, false, spec.object_id),
            commitReply(3, false, spec.object_id),
        );
        const c = client(transport);
        await c.create(spec, { ...intent, sourceKind: "assistant" });
        await c.create(spec, { ...intent, sourceKind: "user" });
        const [first, second] = transport
            .bodies("kernel.commit")
            .map((body) => (body as { intent: Record<string, string> }).intent);
        expect(first?.operation_key).toBe(second?.operation_key);
        expect(first?.request_digest).not.toBe(second?.request_digest);
    });

    test("an omitted source_kind hashes like the explicit default the wire carries", async () => {
        const transport = new FakeTransport().queue(
            commitReply(2, false, spec.object_id),
            commitReply(3, false, spec.object_id),
        );
        const c = client(transport);
        await c.create(spec, intent);
        await c.create(spec, { ...intent, sourceKind: "assistant" });
        const [first, second] = transport
            .bodies("kernel.commit")
            .map((body) => (body as { intent: Record<string, string> }).intent);
        expect(first?.request_digest).toBe(second?.request_digest);
    });

    test("sessions reusing one tool-call id derive distinct keys", () => {
        const parts = { projectRoot: PROJECT, producer: "plugin", actor: "a" };
        expect(deriveOperationKey({ ...parts, operationId: "session-1\u001fcall-1" })).not.toBe(
            deriveOperationKey({ ...parts, operationId: "session-2\u001fcall-1" }),
        );
    });

    test("deriveObjectId joins the fields under the unit separator and pins byte-for-byte", () => {
        // The literal detects changes to hash inputs, separator, field order, or slice.
        expect(deriveObjectId("mem", "a", "b")).toBe("mem_f04cdced9736a69da6103f08a4daaf8c");
        expect(deriveObjectId("dec", "a", "b")).toBe("dec_f04cdced9736a69da6103f08a4daaf8c");
        expect(deriveObjectId("mem", "a\u001fb")).toBe(deriveObjectId("mem", "a", "b"));
        expect(deriveObjectId("mem", "a", "b")).not.toBe(deriveObjectId("mem", "b", "a"));
    });

    test("a separator inside a non-final key field is refused instead of shifting the field boundary", () => {
        // With every field but the last separator-free, the joined bytes parse back to exactly one field list of that arity, so distinct inputs cannot collide. commentlint: allow(JUDGE)
        expect(() => deriveObjectId("mem", "a\u001fb", "c")).toThrow(RangeError);
        expect(() =>
            deriveOperationKey({
                projectRoot: PROJECT,
                producer: "plugin",
                actor: "assistant\u001fsession-1",
                operationId: "call-1",
            }),
        ).toThrow(RangeError);
        expect(() =>
            deriveOperationKey({
                projectRoot: `${PROJECT}\u001f`,
                producer: "plugin",
                actor: "assistant",
                operationId: "call-1",
            }),
        ).toThrow(RangeError);
        expect(() =>
            deriveOperationKey({
                projectRoot: PROJECT,
                producer: "plug\u001fin",
                actor: "assistant",
                operationId: "call-1",
            }),
        ).toThrow(RangeError);
    });

    test("a commit whose actor carries the separator is invalid_input with no transport call", async () => {
        const transport = new FakeTransport().queue(commitReply(2, false, spec.object_id));
        const result = await client(transport).create(spec, {
            ...intent,
            actor: "assistant\u001fsession-1",
        });
        expect(result.state).toEqual({ kind: "invalid", reason: "invalid_input" });
        expect(transport.calls).toHaveLength(0);
    });

    test("a client bound to a root carrying the separator refuses commits but still reads", async () => {
        const transport = new FakeTransport().queue(readReply(1));
        const c = client(transport, true, { projectRoot: `${PROJECT}\u001fx` });
        expect((await c.read({ surface: "auto_inject" })).state).toEqual({ kind: "available" });
        const result = await c.create(spec, intent);
        expect(result.state).toEqual({ kind: "invalid", reason: "invalid_input" });
        expect(transport.bodies("kernel.commit")).toHaveLength(0);
    });

    test("create sends one insert_decision under a derived intent", async () => {
        const transport = new FakeTransport().queue(commitReply(2, false, spec.object_id));
        const c = client(transport);
        const result = await c.create(spec, { ...intent, sourceKind: "assistant" });
        expect(isAvailable(result)).toBe(true);
        const body = transport.bodies("kernel.commit")[0] as Record<string, unknown>;
        expect(body.operations).toEqual([{ op: "insert_decision", spec }]);
        expect(body.tokens).toEqual([]);
        expect(body.source_kind).toBe("assistant");
        const wireIntent = body.intent as Record<string, string>;
        expect(wireIntent.producer).toBe("plugin");
        expect(wireIntent.request_digest).toBe(
            deriveRequestDigest({
                operations: [{ op: "insert_decision", spec }],
                sourceKind: "assistant",
            }),
        );
        expect(wireIntent.cause).toBe(intent.cause);
        expect(wireIntent.operation_key).toBe(
            deriveOperationKey({
                projectRoot: PROJECT,
                producer: "plugin",
                actor: intent.actor,
                operationId: intent.operationId,
            }),
        );
        expect(c.tokens.get(PROJECT, spec.object_id)?.known_as_of).toBe(2);
    });

    test("the free-text cause travels in the intent and never enters the operation key", async () => {
        const transport = new FakeTransport().queue(
            commitReply(2, false, spec.object_id),
            commitReply(3, false, spec.object_id),
        );
        const c = client(transport);
        await c.create(spec, { ...intent, cause: "call-1 reason: superseded" });
        await c.create(spec, { ...intent, cause: "call-1 reason: obsolete" });
        const [first, second] = transport
            .bodies("kernel.commit")
            .map((body) => (body as { intent: Record<string, string> }).intent);
        expect(first?.cause).toBe("call-1 reason: superseded");
        expect(second?.cause).toBe("call-1 reason: obsolete");
        expect(first?.operation_key).toBe(second?.operation_key);
    });

    test("a mutation without a cached token does one ungated explicit_search read first", async () => {
        const transport = new FakeTransport().queue(
            readReply(6, "old-object"),
            commitReply(7, false, "old-object", spec.object_id),
        );
        const result = await client(transport).revise("old-object", spec, intent);
        expect(isAvailable(result)).toBe(true);
        expect(transport.calls.map((call) => call.method)).toEqual([
            "kernel.read",
            "kernel.commit",
        ]);
        const read = transport.bodies("kernel.read")[0];
        expect(read?.surface).toBe("explicit_search");
        expect(read?.gated).toBe(false);
        expect(read?.as_of).toBeNull();
        expect(read?.object_ids).toEqual(["old-object"]);
        expect(transport.bodies("kernel.commit")[0]?.tokens).toEqual([
            { object_id: "old-object", known_as_of: 6 },
        ]);
    });

    test("a cached token skips the pre-read", async () => {
        const transport = new FakeTransport().queue(
            readReply(3, "o1"),
            commitReply(4, false, "o1"),
        );
        const c = client(transport);
        await c.read({ surface: "auto_inject" });
        await c.archive("o1", intent);
        expect(transport.calls.map((call) => call.method)).toEqual([
            "kernel.read",
            "kernel.commit",
        ]);
        expect(transport.bodies("kernel.commit")[0]?.operations).toEqual([
            { op: "retire_decision", object_id: "o1" },
        ]);
        expect(transport.bodies("kernel.commit")[0]?.tokens).toEqual([
            { object_id: "o1", known_as_of: 3 },
        ]);
    });

    test("merge supersedes every object with one survivor in one envelope", async () => {
        const transport = new FakeTransport().queue(
            readReply(3, "a", "b"),
            commitReply(4, false, "a", "b", spec.object_id),
        );
        await client(transport).merge(["a", "b"], spec, intent);
        expect(transport.bodies("kernel.commit")).toHaveLength(1);
        expect(transport.bodies("kernel.commit")[0]?.operations).toEqual([
            { op: "supersede_decision", replaced_object_id: "a", spec },
            { op: "supersede_decision", replaced_object_id: "b", spec },
        ]);
    });

    test("snapshot_diverged on commit drops tokens, re-reads the tip, and commits once more", async () => {
        const transport = new FakeTransport().queue(
            readReply(3, "o1"),
            DIVERGED,
            readReply(8, "o1"),
            commitReply(9, false, "o1"),
        );
        const c = client(transport);
        await c.read({ surface: "auto_inject" });
        const result = await c.archive("o1", intent);
        expect(isAvailable(result)).toBe(true);
        expect(transport.calls.map((call) => call.method)).toEqual([
            "kernel.read",
            "kernel.commit",
            "kernel.read",
            "kernel.commit",
        ]);
        expect(transport.bodies("kernel.commit")[1]?.tokens).toEqual([
            { object_id: "o1", known_as_of: 8 },
        ]);
    });

    test("a second divergence in the same commit returns snapshot_diverged", async () => {
        const transport = new FakeTransport().queue(
            readReply(3, "o1"),
            DIVERGED,
            readReply(8, "o1"),
            DIVERGED,
        );
        const c = client(transport);
        await c.read({ surface: "auto_inject" });
        const result = await c.archive("o1", intent);
        expect(result.state).toEqual({ kind: "unavailable", reason: "snapshot_diverged" });
        expect(transport.calls).toHaveLength(4);
        expect(c.tokens.get(PROJECT, "o1")).toBeUndefined();
    });

    test("a target still absent after the refresh read is never committed", async () => {
        const transport = new FakeTransport().queue(readReply(3, "other"));
        const result = await client(transport).archive("foreign", intent);
        expect(result.state).toEqual({ kind: "conflict", reason: "retracted" });
        expect(transport.bodies("kernel.commit")).toHaveLength(0);
    });

    test("a refresh for more targets than one filter holds reads them in filter-sized batches", async () => {
        // An unfiltered fallback would be subject to the daemon's newest-rows cap, where a live target can simply be past the cut. commentlint: allow(JUDGE)
        const targets = Array.from({ length: MAX_READ_OBJECT_IDS + 1 }, (_, i) => `t${i}`);
        const transport = new FakeTransport().queue(
            readReply(3, ...targets.slice(0, MAX_READ_OBJECT_IDS)),
            readReply(3, ...targets.slice(MAX_READ_OBJECT_IDS)),
            commitReply(4, false, ...targets, spec.object_id),
        );
        const result = await client(transport).merge(targets, spec, intent);
        expect(isAvailable(result)).toBe(true);
        const reads = transport.bodies("kernel.read");
        expect(reads.map((body) => (body.object_ids as string[]).length)).toEqual([
            MAX_READ_OBJECT_IDS,
            1,
        ]);
        expect(transport.bodies("kernel.commit")[0]?.tokens).toHaveLength(targets.length);
    });

    test("a target dropped from a truncated refresh batch is re-read alone before it is judged", async () => {
        // The byte budget keeps a newest-first prefix of a filtered read, so absence in a truncated batch proves nothing; a one-object read always fits. commentlint: allow(JUDGE)
        const transport = new FakeTransport().queue(
            { ...readReply(3, "a"), truncated: true },
            readReply(3, "b"),
            commitReply(4, false, "a", "b", spec.object_id),
        );
        const result = await client(transport).merge(["a", "b"], spec, intent);
        expect(isAvailable(result)).toBe(true);
        expect(transport.bodies("kernel.read").map((body) => body.object_ids)).toEqual([
            ["a", "b"],
            ["b"],
        ]);
        expect(transport.bodies("kernel.commit")[0]?.tokens).toEqual([
            { object_id: "a", known_as_of: 3 },
            { object_id: "b", known_as_of: 3 },
        ]);
    });

    test("a target absent from a complete one-object refresh is retracted", async () => {
        const transport = new FakeTransport().queue(
            { ...readReply(3, "a"), truncated: true },
            readReply(3),
        );
        const result = await client(transport).merge(["a", "b"], spec, intent);
        expect(result.state).toEqual({ kind: "conflict", reason: "retracted" });
        expect(transport.bodies("kernel.commit")).toHaveLength(0);
    });

    test("a truncated one-object refresh with no row is an internal error, not a retraction", async () => {
        const transport = new FakeTransport().queue({ ...readReply(3), truncated: true });
        const result = await client(transport).archive("o1", intent);
        expect(result.state).toEqual({ kind: "invalid", reason: "internal" });
        expect(transport.bodies("kernel.read")).toHaveLength(1);
        expect(transport.bodies("kernel.commit")).toHaveLength(0);
    });

    test("snapshot_diverged with caller-supplied tokens is returned without a second commit", async () => {
        // The caller's tokens are sent verbatim, so dropping the cache cannot change the retry; a second identical send only risks downgrading the definitive state to outcome_unknown. commentlint: allow(JUDGE)
        const transport = new FakeTransport().queue(DIVERGED);
        const result = await client(transport).commit({
            ...intent,
            operations: [{ op: "retire_decision", object_id: "o1" }],
            tokens: [{ object_id: "o1", known_as_of: 99 }],
        });
        expect(result.state).toEqual({ kind: "unavailable", reason: "snapshot_diverged" });
        expect(transport.bodies("kernel.commit")).toHaveLength(1);
    });

    test("the wire deadline_ms is the budget left after the refresh read, not the caller's total", async () => {
        let now = 0;
        const transport = new FakeTransport().queue(
            () => {
                now += 3_000;
                return readReply(6, "o1");
            },
            commitReply(7, false, "o1"),
        );
        const c = client(transport, true, { clock: () => now });
        const result = await c.archive("o1", { ...intent, deadlineMs: 10_000 });
        expect(isAvailable(result)).toBe(true);
        expect(transport.bodies("kernel.commit")[0]?.deadline_ms).toBe(7_000);
    });

    test("the divergence retry sends the remaining budget rather than the original", async () => {
        let now = 0;
        const transport = new FakeTransport().queue(
            readReply(3, "o1"),
            () => {
                now += 4_000;
                return DIVERGED;
            },
            readReply(8, "o1"),
            commitReply(9, false, "o1"),
        );
        const c = client(transport, true, { clock: () => now });
        await c.read({ surface: "auto_inject" });
        const result = await c.archive("o1", { ...intent, deadlineMs: 10_000 });
        expect(isAvailable(result)).toBe(true);
        const commits = transport.bodies("kernel.commit");
        expect(commits[0]?.deadline_ms).toBe(10_000);
        expect(commits[1]?.deadline_ms).toBe(6_000);
    });

    test("no deadline_ms travels when the caller passes none", async () => {
        const transport = new FakeTransport().queue(commitReply(2, false, spec.object_id));
        await client(transport).create(spec, intent);
        expect("deadline_ms" in (transport.bodies("kernel.commit")[0] ?? {})).toBe(false);
    });

    test("a reissued write carries the budget left at the reissue, not the first attempt's", async () => {
        let now = 0;
        const transport = new FakeTransport().queue(
            () => {
                now += 3_000;
                return new HostCallError("outcome_unknown", "deadline", "request_deadline");
            },
            commitReply(5, true, spec.object_id),
        );
        const result = await client(transport, true, { clock: () => now }).create(spec, {
            ...intent,
            deadlineMs: 10_000,
        });
        expect(isAvailable(result)).toBe(true);
        const commits = transport.bodies("kernel.commit");
        expect(commits.map((body) => body.deadline_ms)).toEqual([10_000, 7_000]);
        // Only the budget field differs between the attempts, so the daemon's receipt lookup still sees one identity and digest. commentlint: allow(JUDGE)
        const { deadline_ms: _first, ...first } = commits[0] ?? {};
        const { deadline_ms: _second, ...second } = commits[1] ?? {};
        expect(second).toEqual(first);
    });

    test("a conflict from the daemon passes through", async () => {
        const transport = new FakeTransport().queue(readReply(3, "o1"), {
            state: { kind: "conflict", reason: "known_as_of_advanced" },
        });
        const result = await client(transport).archive("o1", intent);
        expect(result.state).toEqual({ kind: "conflict", reason: "known_as_of_advanced" });
    });
});

describe("kernelMemorySnapshotFrom", () => {
    test("an available read's truncated flag rides the snapshot projection", async () => {
        const transport = new FakeTransport().queue({ ...readReply(9, "o1"), truncated: true });
        const read = await client(transport).read({ surface: "explicit_search" });
        expect(kernelMemorySnapshotFrom(read)).toMatchObject({
            rows: [
                expect.objectContaining({ object: expect.objectContaining({ object_id: "o1" }) }),
            ],
            knownAsOf: 9,
            truncated: true,
        });
    });

    test("a complete read projects truncated false", async () => {
        const transport = new FakeTransport().queue(readReply(9, "o1"));
        const read = await client(transport).read({ surface: "explicit_search" });
        expect(kernelMemorySnapshotFrom(read).truncated).toBe(false);
    });

    test("a non-available read projects no rows and no truncation", async () => {
        const transport = new FakeTransport().queue({
            state: { kind: "unavailable", reason: "store_busy" },
        });
        const read = await client(transport).read({ surface: "explicit_search" });
        expect(kernelMemorySnapshotFrom(read)).toEqual({
            state: { kind: "unavailable", reason: "store_busy" },
            rows: [],
            knownAsOf: null,
        });
    });
});
