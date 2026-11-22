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
        request: {
            state: { memory: { text: "User stated: Deploy from the release branch." } },
            questions: { changes: { type: "bool" } },
        },
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
            work({ request: { state: [], questions: {} } }),
            work({ request: { state: {} } }),
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
        const pump = createReviewClassifierPump(fake.client, execute);
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
            wait_ms: 20_000,
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
        await new Promise((resolve) => setTimeout(resolve, 60));
        expect(fake.calls).toHaveLength(5);
        // A later start begins a new loop for the root.
        pump.start(SCOPE);
        await new Promise((resolve) => setTimeout(resolve, 20));
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
        const pump = createReviewClassifierPump(client, async () => ({}));
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
