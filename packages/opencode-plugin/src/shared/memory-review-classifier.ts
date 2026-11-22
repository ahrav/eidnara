import type { RustModeModuleClient } from "../hooks/context/transform-session-client";

/** One stage-1 request of a memory review, as the daemon hands it to the harness. */
export interface ReviewClassifierWork {
    lease: string;
    /** The `provider/model` the harness looks the classifier up by. */
    classifier: string;
    state: Record<string, unknown>;
    questions: Record<string, unknown>;
    maxDurationMs: number;
}

/** Calls the classifier with the harness's own registry and credential and returns its answers by question id. */
export type ReviewClassifierExecutor = (
    work: ReviewClassifierWork,
    signal: AbortSignal,
) => Promise<Record<string, unknown>>;

/** Only these content-free failures cross back into daemon state. */
export class ReviewClassifierError extends Error {
    constructor(readonly code: "provider_unavailable" | "model_failed" | "cancelled") {
        super(`Memory review classifier: ${code}`);
    }
}

export interface ReviewClassifierScope {
    sessionId: string;
    projectRoot: string;
    classifier: string;
}

export interface ReviewClassifierPump {
    /** Starts answering classifier work for `scope.projectRoot`; a later scope of the same root replaces the session the calls travel on. */
    start(scope: ReviewClassifierScope): void;
    /** Settles once every loop that is running has ended on its own. */
    idle(): Promise<void>;
    /** Stops every loop and settles once the calls in flight have ended. */
    close(): Promise<void>;
}

const LEASE_PATTERN = /^[a-f0-9]{32}$/;
/** The daemon holds a `next` request open this long for work to arrive. */
const NEXT_WAIT_MS = 20_000;
/** The transport outlives the daemon's wait. */
const NEXT_TIMEOUT_MS = NEXT_WAIT_MS + 10_000;
const SUBMIT_TIMEOUT_MS = 10_000;
/** The largest request text the harness parses. */
const MAX_REQUEST_BYTES = 128 * 1024;
/** The longest classifier call the harness makes, whatever the work states. */
const MAX_DURATION_MS = 10_000;
const RETRY_DELAY_MS = 5_000;
/** Consecutive failed `next` calls after which a loop ends; the next `start` begins another. */
const MAX_CONSECUTIVE_FAILURES = 5;

function record(value: unknown): Record<string, unknown> | undefined {
    return value !== null && typeof value === "object" && !Array.isArray(value)
        ? (value as Record<string, unknown>)
        : undefined;
}

/** The request text of a work response as the classifier call takes it. Parsing keeps the members of every object in the order of the text, which is the order the daemon chose for the classifier. */
function parsedRequest(text: unknown): Record<string, unknown> | undefined {
    if (typeof text !== "string" || Buffer.byteLength(text) > MAX_REQUEST_BYTES) return undefined;
    try {
        return record(JSON.parse(text));
    } catch {
        return undefined;
    }
}

/** The work of a `next` response in state `work`, or `undefined` when its shape is outside the contract. */
export function reviewClassifierWork(response: unknown): ReviewClassifierWork | undefined {
    const value = record(response);
    const request = parsedRequest(value?.request);
    const state = record(request?.state);
    const questions = record(request?.questions);
    if (
        !value ||
        typeof value.lease !== "string" ||
        !LEASE_PATTERN.test(value.lease) ||
        typeof value.classifier !== "string" ||
        !value.classifier.includes("/") ||
        !state ||
        !questions ||
        typeof value.max_duration_ms !== "number" ||
        !Number.isSafeInteger(value.max_duration_ms) ||
        value.max_duration_ms <= 0
    ) {
        return undefined;
    }
    return {
        lease: value.lease,
        classifier: value.classifier,
        state,
        questions,
        maxDurationMs: Math.min(value.max_duration_ms, MAX_DURATION_MS),
    };
}

/**
 * Answers the daemon's stage-1 classifier work for each project a session is open on. One loop
 * per project root long-polls `memory.review.classify.next`, runs the classifier through
 * `execute`, and returns the answers or one failure code with `memory.review.classify.submit`.
 * The daemon reads each `next` call as the sign that this project's harness has a classifier,
 * so a harness that cannot call one starts no loop and the chat model decides. A `disabled`
 * reply ends the loop: the project names no classifier, or another one.
 */
export function createReviewClassifierPump(
    client: Pick<RustModeModuleClient, "call">,
    execute: ReviewClassifierExecutor,
    options: { retryDelayMs?: number } = {},
): ReviewClassifierPump {
    const closing = new AbortController();
    const loops = new Map<string, { scope: ReviewClassifierScope; done: Promise<void> }>();
    const retryDelayMs = options.retryDelayMs ?? RETRY_DELAY_MS;

    const pause = (ms: number) =>
        new Promise<void>((resolve) => {
            const timer = setTimeout(resolve, ms);
            closing.signal.addEventListener(
                "abort",
                () => {
                    clearTimeout(timer);
                    resolve();
                },
                { once: true },
            );
        });

    async function answer(scope: ReviewClassifierScope, work: ReviewClassifierWork): Promise<void> {
        const submit = (extra: Record<string, unknown>) =>
            client.call({
                sessionId: scope.sessionId,
                projectRoot: scope.projectRoot,
                method: "memory.review.classify.submit",
                timeoutMs: SUBMIT_TIMEOUT_MS,
                body: {
                    method: "memory.review.classify.submit",
                    session_id: scope.sessionId,
                    project_root: scope.projectRoot,
                    classifier: scope.classifier,
                    lease: work.lease,
                    ...extra,
                },
            });
        const controller = new AbortController();
        const cancel = () => controller.abort();
        closing.signal.addEventListener("abort", cancel, { once: true });
        const timer = setTimeout(cancel, work.maxDurationMs);
        try {
            const answers = await execute(work, controller.signal);
            await submit({ answers });
        } catch (error) {
            const code = closing.signal.aborted
                ? "cancelled"
                : error instanceof ReviewClassifierError
                  ? error.code
                  : controller.signal.aborted
                    ? "provider_unavailable"
                    : "model_failed";
            // A lost failure report leaves a lease that lapses on its own.
            await submit({ error: code }).catch(() => undefined);
        } finally {
            clearTimeout(timer);
            closing.signal.removeEventListener("abort", cancel);
        }
    }

    async function run(projectRoot: string): Promise<void> {
        let failures = 0;
        try {
            while (!closing.signal.aborted) {
                const scope = loops.get(projectRoot)?.scope;
                if (!scope) return;
                let response: unknown;
                try {
                    response = await client.call({
                        sessionId: scope.sessionId,
                        projectRoot: scope.projectRoot,
                        method: "memory.review.classify.next",
                        timeoutMs: NEXT_TIMEOUT_MS,
                        signal: closing.signal,
                        body: {
                            method: "memory.review.classify.next",
                            session_id: scope.sessionId,
                            project_root: scope.projectRoot,
                            classifier: scope.classifier,
                            wait_ms: NEXT_WAIT_MS,
                        },
                    });
                } catch {
                    failures += 1;
                    if (failures >= MAX_CONSECUTIVE_FAILURES) return;
                    await pause(retryDelayMs);
                    continue;
                }
                failures = 0;
                const state = record(response)?.state;
                if (state === "ready") continue;
                if (state !== "work") return;
                const work = reviewClassifierWork(response);
                // Work outside the contract is left to its lease.
                if (work) await answer(scope, work);
            }
        } finally {
            loops.delete(projectRoot);
        }
    }

    return {
        start(scope) {
            if (closing.signal.aborted) return;
            const running = loops.get(scope.projectRoot);
            if (running) {
                running.scope = scope;
                return;
            }
            const entry = { scope, done: Promise.resolve() };
            loops.set(scope.projectRoot, entry);
            entry.done = run(scope.projectRoot);
        },
        async idle() {
            while (loops.size > 0)
                await Promise.all([...loops.values()].map((entry) => entry.done));
        },
        async close() {
            closing.abort();
            await this.idle();
        },
    };
}
