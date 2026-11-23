import { describe, expect, it } from "bun:test";
import {
    createReviewClassifierPump,
    ReviewClassifierError,
    type ReviewClassifierExecutor,
    reviewClassifierWork,
} from "./memory-review-classifier";

const LEASE = "0123456789abcdef0123456789abcdef";
const SCOPE = { sessionId: "s1", projectRoot: "/work/project", classifier: "typesafe/jev-latest" };

function work(overrides: Record<string, unknown> = {}) {
    return {
        state: "work",
        lease: LEASE,
        classifier: "typesafe/jev-latest",
        request: JSON.stringify({
            state: { memory: { text: "User stated: Deploy from the release branch." } },
            questions: { changes: { type: "bool" } },
        }),
        max_duration_ms: 10_000,
        ...overrides,
    };
}

/** A daemon stand-in that answers `next` from a queue and records every call. */
function daemon(replies: unknown[]) {
    const calls: Array<{ method: string; body: Record<string, unknown> }> = [];
    return {
        calls,
        client: {
            call: async (args: { method: string; body: unknown }) => {
                calls.push({ method: args.method, body: args.body as Record<string, unknown> });
                if (args.method === "memory.review.classify.submit") return { state: "processed" };
                const reply = replies.shift() ?? { state: "disabled" };
                if (reply instanceof Error) throw reply;
                return reply;
            },
        },
    };
}

describe("reviewClassifierWork", () => {
    it("keeps the members of the request in the order of its text", () => {
        const text =
            '{"state":{"memory":{},"source":[],"later":[]},"questions":{"changes":{"criteria":{"true":"a","false":"b"}},"relation":{},"work_ended":{},"strongest":{}}}';
        const parsed = reviewClassifierWork(work({ request: text }));
        expect(Object.keys(parsed?.state ?? {})).toEqual(["memory", "source", "later"]);
        expect(Object.keys(parsed?.questions ?? {})).toEqual([
            "changes",
            "relation",
            "work_ended",
            "strongest",
        ]);
        expect(JSON.stringify({ state: parsed?.state, questions: parsed?.questions })).toBe(text);
    });

    it("reads a work response and caps the duration it states", () => {
        expect(reviewClassifierWork(work({ max_duration_ms: 60_000 }))).toEqual({
            lease: LEASE,
            classifier: "typesafe/jev-latest",
            state: { memory: { text: "User stated: Deploy from the release branch." } },
            questions: { changes: { type: "bool" } },
            maxDurationMs: 10_000,
        });
    });

    it("refuses a response outside the contract", () => {
        for (const broken of [
            work({ lease: "short" }),
            work({ classifier: "jev" }),
            work({ request: JSON.stringify({ state: [], questions: {} }) }),
            work({ request: JSON.stringify({ state: {} }) }),
            work({ request: { state: {}, questions: {} } }),
            work({ request: "{" }),
            work({
                request: JSON.stringify({ state: {}, questions: {}, pad: "x".repeat(128 * 1024) }),
            }),
            work({ max_duration_ms: 0 }),
            null,
        ]) {
            expect(reviewClassifierWork(broken)).toBeUndefined();
        }
    });
});

describe("createReviewClassifierPump", () => {
    it("answers each work item under its lease and stops when the daemon says disabled", async () => {
        const fake = daemon([{ state: "ready" }, work(), { state: "disabled" }]);
        const seen: unknown[] = [];
        const execute: ReviewClassifierExecutor = async (item) => {
            seen.push(item.state);
            return { changes: { type: "bool", probability: 0.03 } };
        };
        const pump = createReviewClassifierPump(fake.client, execute, { pollIntervalMs: 1 });
        pump.start(SCOPE);
        await pump.idle();
        await pump.close();
        expect(seen).toEqual([
            { memory: { text: "User stated: Deploy from the release branch." } },
        ]);
        expect(fake.calls.map((call) => call.method)).toEqual([
            "memory.review.classify.next",
            "memory.review.classify.next",
            "memory.review.classify.submit",
            "memory.review.classify.next",
        ]);
        expect(fake.calls[0]?.body).toEqual({
            method: "memory.review.classify.next",
            session_id: "s1",
            project_root: "/work/project",
            classifier: "typesafe/jev-latest",
        });
        expect(fake.calls[2]?.body).toEqual({
            method: "memory.review.classify.submit",
            session_id: "s1",
            project_root: "/work/project",
            classifier: "typesafe/jev-latest",
            lease: LEASE,
            answers: { changes: { type: "bool", probability: 0.03 } },
        });
    });

    it("reports a failed classifier call as one content-free code", async () => {
        for (const [thrown, code] of [
            [new ReviewClassifierError("provider_unavailable"), "provider_unavailable"],
            [new Error("the provider said something private"), "model_failed"],
        ] as const) {
            const fake = daemon([work(), { state: "disabled" }]);
            const pump = createReviewClassifierPump(fake.client, async () => {
                throw thrown;
            });
            pump.start(SCOPE);
            await pump.idle();
            const submitted = fake.calls.find(
                (call) => call.method === "memory.review.classify.submit",
            );
            expect(submitted?.body.error).toBe(code);
            expect(submitted?.body.answers).toBeUndefined();
            expect(JSON.stringify(submitted?.body)).not.toContain("private");
        }
    });

    it("reports a classifier call that outlasts the work's duration as provider_unavailable", async () => {
        const fake = daemon([work({ max_duration_ms: 5 }), { state: "disabled" }]);
        const pump = createReviewClassifierPump(
            fake.client,
            (_item, signal) =>
                new Promise((_resolve, reject) => {
                    signal.addEventListener("abort", () =>
                        reject(new ReviewClassifierError("cancelled")),
                    );
                }),
        );
        pump.start(SCOPE);
        await pump.idle();
        const submitted = fake.calls.find(
            (call) => call.method === "memory.review.classify.submit",
        );
        expect(submitted?.body.error).toBe("provider_unavailable");
    });

    it("reports answers once, and a submit the daemon does not receive is left to the review's wait", async () => {
        const methods: string[] = [];
        const client = {
            call: async (args: { method: string }) => {
                methods.push(args.method);
                if (args.method === "memory.review.classify.submit")
                    throw new Error("transport closed");
                return methods.length === 1 ? work() : { state: "disabled" };
            },
        };
        const pump = createReviewClassifierPump(client, async () => ({ changes: {} }));
        pump.start(SCOPE);
        await pump.idle();
        expect(methods).toEqual([
            "memory.review.classify.next",
            "memory.review.classify.submit",
            "memory.review.classify.next",
        ]);
    });

    it("waits the poll interval between two next calls that found no work", async () => {
        const fake = daemon(Array.from({ length: 50 }, () => ({ state: "ready" })));
        const pump = createReviewClassifierPump(fake.client, async () => ({}), {
            pollIntervalMs: 40,
        });
        pump.start(SCOPE);
        await new Promise((resolve) => setTimeout(resolve, 100));
        await pump.close();
        expect(fake.calls.length).toBeGreaterThanOrEqual(2);
        expect(fake.calls.length).toBeLessThanOrEqual(4);
    });

    it("closes at once while a next call is in flight or a pause is running", async () => {
        // The transport rejects a call whose signal aborts.
        const client = {
            call: (args: { signal?: AbortSignal }) =>
                new Promise((_resolve, reject) => {
                    args.signal?.addEventListener("abort", () => reject(new Error("aborted")));
                }),
        };
        const inFlight = createReviewClassifierPump(client, async () => ({}));
        inFlight.start(SCOPE);
        const paused = createReviewClassifierPump(
            daemon([{ state: "ready" }]).client,
            async () => ({}),
        );
        paused.start(SCOPE);
        await new Promise((resolve) => setTimeout(resolve, 5));
        const started = Date.now();
        await Promise.all([inFlight.close(), paused.close()]);
        expect(Date.now() - started).toBeLessThan(1_000);
    });

    it("stops the loops of a released session and starts again for the next one", async () => {
        const fake = daemon(Array.from({ length: 400 }, () => ({ state: "ready" })));
        const pump = createReviewClassifierPump(fake.client, async () => ({}), {
            pollIntervalMs: 1,
        });
        pump.start(SCOPE);
        pump.stop("another-session");
        await new Promise((resolve) => setTimeout(resolve, 15));
        expect(fake.calls.length).toBeGreaterThan(1);
        pump.stop(SCOPE.sessionId);
        await pump.idle();
        const settled = fake.calls.length;
        await new Promise((resolve) => setTimeout(resolve, 10));
        expect(fake.calls).toHaveLength(settled);
        // A session that starts while the released one's loop is still stopping takes the loop over.
        pump.start(SCOPE);
        pump.stop(SCOPE.sessionId);
        pump.start({ ...SCOPE, sessionId: "s2" });
        await new Promise((resolve) => setTimeout(resolve, 15));
        expect(fake.calls.length).toBeGreaterThan(settled + 1);
        expect(fake.calls.at(-1)?.body.session_id).toBe("s2");
        await pump.close();
    });

    it("ends a session's loop of the project root it left", async () => {
        const fake = daemon(Array.from({ length: 400 }, () => ({ state: "ready" })));
        const pump = createReviewClassifierPump(fake.client, async () => ({}), {
            pollIntervalMs: 1,
        });
        pump.start(SCOPE);
        pump.start({ ...SCOPE, projectRoot: "/work/other" });
        await new Promise((resolve) => setTimeout(resolve, 15));
        const roots = fake.calls.slice(-3).map((call) => call.body.project_root);
        expect(new Set(roots)).toEqual(new Set(["/work/other"]));
        await pump.close();
    });

    it("answers work that arrives as the pump closes with cancelled, without a classifier call", async () => {
        const methods: string[] = [];
        const errors: unknown[] = [];
        let pump: ReturnType<typeof createReviewClassifierPump> | undefined;
        let closed: Promise<void> | undefined;
        const client = {
            call: async (args: { method: string; body: unknown }) => {
                methods.push(args.method);
                if (args.method === "memory.review.classify.submit") {
                    errors.push((args.body as Record<string, unknown>).error);
                    return { state: "processed" };
                }
                closed = pump?.close();
                return work();
            },
        };
        let executed = false;
        pump = createReviewClassifierPump(client, async () => {
            executed = true;
            return {};
        });
        pump.start(SCOPE);
        await pump.idle();
        await closed;
        expect(executed).toBe(false);
        expect(errors).toEqual(["cancelled"]);
    });

    it("retries a failed next call and gives up after five in a row", async () => {
        const fake = daemon(Array.from({ length: 8 }, () => new Error("unreachable")));
        const pump = createReviewClassifierPump(
            fake.client,
            async () => {
                throw new Error("never called");
            },
            { retryDelayMs: 1 },
        );
        pump.start(SCOPE);
        await pump.idle();
        expect(fake.calls).toHaveLength(5);
        // A later start begins a new loop for the root.
        pump.start(SCOPE);
        await pump.idle();
        expect(fake.calls.length).toBeGreaterThan(5);
        await pump.close();
    });

    it("runs one loop per project root and moves it to the newest session", async () => {
        let release: (value: unknown) => void = () => undefined;
        const held = new Promise((resolve) => {
            release = resolve;
        });
        const sessions: string[] = [];
        const client = {
            call: async (args: { sessionId: string; method: string }) => {
                sessions.push(args.sessionId);
                if (sessions.length === 1) return held;
                return { state: "disabled" };
            },
        };
        const pump = createReviewClassifierPump(client, async () => ({}), {
            pollIntervalMs: 1,
        });
        pump.start(SCOPE);
        pump.start({ ...SCOPE, sessionId: "s2" });
        await new Promise((resolve) => setTimeout(resolve, 5));
        expect(sessions).toEqual(["s1"]);
        release({ state: "ready" });
        await pump.idle();
        expect(sessions).toEqual(["s1", "s2"]);
        await pump.close();
    });
});
