import type { RustModeModuleClient } from "../hooks/context/transform-session-client";
import { TimeoutError, withTimeout } from "./with-timeout";

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
    /** Starts answering classifier work for `scope.projectRoot`; a later scope of the same root replaces the session the calls travel on, and the session's loop of another root ends. */
    start(scope: ReviewClassifierScope): void;
    /** Ends each loop whose calls travel on `sessionId`, once its call in flight has ended. */
    stop(sessionId: string): void;
    /** Settles once every loop that is running has ended on its own. */
    idle(): Promise<void>;
    /** Stops every loop and settles once the calls in flight have ended. */
    close(): Promise<void>;
}

const LEASE_PATTERN = /^[a-f0-9]{32}$/;
/**
 * The longest pause between two `next` calls that found no work. A session's daemon calls travel in
 * order, so each `next` call is one short round trip; a review waits 20 seconds for its answer
 * and the daemon counts a harness present for 120 seconds after a call it answers `work` or
 * `ready`.
 */
const POLL_INTERVAL_MS = 2_000;
/**
 * A short pause after answered work shortens the wait of a request that becomes ready just after
 * an answer. The first pause schedules this fraction of the poll interval. The next six pauses
 * each double the cumulative scheduled pause time, which then equals one poll interval.
 */
const FIRST_REPOLL_FRACTION = 1 / 64;
const CALL_TIMEOUT_MS = 10_000;
/** The largest request text the harness parses. */
const MAX_REQUEST_BYTES = 128 * 1024;
/** The longest classifier call the harness makes, whatever the work states. */
const MAX_DURATION_MS = 10_000;
/** How long past its duration a classifier call may take to settle after its signal aborts. */
const SETTLE_GRACE_MS = 2_000;
/** The largest answers object the daemon takes in a submit (`MAX_ANSWERS_BYTES`, `crates/daemon/src/memory_reviewer/classifier_wire.rs`). */
const MAX_ANSWERS_BYTES = 16 * 1024;
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
 * per project root asks `memory.review.classify.next` for work every few seconds, and at short,
 * doubling pauses for one poll interval after it answers work, runs the
 * classifier through `execute`, and returns the answers or one failure code with
 * `memory.review.classify.submit`. The daemon reads each `next` call it answers `work` or
 * `ready` as the sign that this project's harness has a classifier, so a harness that cannot call one starts no loop and the
 * chat model decides. A `disabled` reply ends the loop: the project names no classifier, or
 * another one. A classifier call reported `cancelled` ends the loop after its report, and a
 * later `start` begins another.
 */
export function createReviewClassifierPump(
    client: Pick<RustModeModuleClient, "call">,
    execute: ReviewClassifierExecutor,
    options: { retryDelayMs?: number; pollIntervalMs?: number; settleGraceMs?: number } = {},
): ReviewClassifierPump {
    interface Loop {
        scope: ReviewClassifierScope;
        stopped: boolean;
        done: Promise<void>;
    }
    const closing = new AbortController();
    const closed = new Promise<void>((resolve) => {
        closing.signal.addEventListener("abort", () => resolve(), { once: true });
    });
    const loops = new Map<string, Loop>();
    const retryDelayMs = options.retryDelayMs ?? RETRY_DELAY_MS;
    const pollIntervalMs = options.pollIntervalMs ?? POLL_INTERVAL_MS;
    const settleGraceMs = options.settleGraceMs ?? SETTLE_GRACE_MS;

    /** Settles after `ms`, or at once when the pump closes. */
    const pause = (ms: number) =>
        new Promise<void>((resolve) => {
            if (closing.signal.aborted) return resolve();
            const wake = () => {
                clearTimeout(timer);
                closing.signal.removeEventListener("abort", wake);
                resolve();
            };
            const timer = setTimeout(wake, ms);
            closing.signal.addEventListener("abort", wake, { once: true });
        });

    /** Runs one classifier call and reports its outcome. A call that ignored its abort signal is returned still running, in an object so that awaiting the answer leaves the call unawaited. */
    async function answer(
        loop: Loop,
        work: ReviewClassifierWork,
    ): Promise<{ call: Promise<unknown> } | undefined> {
        let running: Promise<unknown> | undefined;
        let stuck: { call: Promise<unknown> } | undefined;
        const controller = new AbortController();
        const cancel = () => controller.abort();
        closing.signal.addEventListener("abort", cancel, { once: true });
        let timedOut = false;
        const timer = setTimeout(() => {
            timedOut = true;
            cancel();
        }, work.maxDurationMs);
        let outcome: Record<string, unknown>;
        try {
            if (closing.signal.aborted) throw new ReviewClassifierError("cancelled");
            running = execute(work, controller.signal);
            const answers = record(
                await withTimeout(
                    running,
                    work.maxDurationMs + settleGraceMs,
                    "memory review classifier call",
                ),
            );
            // The pump reports `provider_unavailable` when the classifier returns a result after the work's `max_duration_ms` deadline.
            if (timedOut) throw new ReviewClassifierError("provider_unavailable");
            // The daemon takes an answers object whose JSON text fits `MAX_ANSWERS_BYTES`. A classifier result of another shape or size fails the call with `model_failed`.
            if (!answers || Buffer.byteLength(JSON.stringify(answers)) > MAX_ANSWERS_BYTES) {
                throw new ReviewClassifierError("model_failed");
            }
            outcome = { answers };
        } catch (error) {
            if (error instanceof TimeoutError && running) stuck = { call: running };
            const code = closing.signal.aborted
                ? "cancelled"
                : timedOut
                  ? "provider_unavailable"
                  : error instanceof ReviewClassifierError
                    ? error.code
                    : "model_failed";
            if (code === "cancelled") loop.stopped = true;
            outcome = { error: code };
        } finally {
            clearTimeout(timer);
            closing.signal.removeEventListener("abort", cancel);
        }
        // The lease belongs to the project, so the report travels on the session the loop serves when the call has ended. A report the daemon does not receive leaves the review to the end of its own wait.
        const scope = loop.scope;
        await client
            .call({
                sessionId: scope.sessionId,
                projectRoot: scope.projectRoot,
                method: "memory.review.classify.submit",
                timeoutMs: CALL_TIMEOUT_MS,
                body: {
                    method: "memory.review.classify.submit",
                    session_id: scope.sessionId,
                    project_root: scope.projectRoot,
                    classifier: scope.classifier,
                    lease: work.lease,
                    ...outcome,
                },
            })
            .catch(() => undefined);
        return stuck;
    }

    async function run(loop: Loop): Promise<void> {
        let failures = 0;
        // `sinceWork` tracks the cumulative scheduled pauses after an answer; at `pollIntervalMs` the loop pauses the whole interval between calls.
        let sinceWork = pollIntervalMs;
        try {
            while (!closing.signal.aborted && !loop.stopped) {
                const scope = loop.scope;
                let response: unknown;
                try {
                    response = await client.call({
                        sessionId: scope.sessionId,
                        projectRoot: scope.projectRoot,
                        method: "memory.review.classify.next",
                        timeoutMs: CALL_TIMEOUT_MS,
                        signal: closing.signal,
                        body: {
                            method: "memory.review.classify.next",
                            session_id: scope.sessionId,
                            project_root: scope.projectRoot,
                            classifier: scope.classifier,
                        },
                    });
                } catch {
                    if (closing.signal.aborted) return;
                    failures += 1;
                    if (failures >= MAX_CONSECUTIVE_FAILURES) {
                        // A session that started while this call was pending took the loop over; it goes on for that session.
                        if (loop.scope === scope) return;
                        failures = 0;
                        continue;
                    }
                    await pause(retryDelayMs);
                    continue;
                }
                failures = 0;
                const state = record(response)?.state;
                if (state === "ready") {
                    // After the first pause, six doublings bring `sinceWork` to `pollIntervalMs`, which selects the full interval for the next pause.
                    const wait =
                        sinceWork < pollIntervalMs
                            ? Math.max(sinceWork, pollIntervalMs * FIRST_REPOLL_FRACTION)
                            : pollIntervalMs;
                    await pause(wait);
                    sinceWork = Math.min(sinceWork + wait, pollIntervalMs);
                    continue;
                }
                if (state !== "work") {
                    if (loop.scope === scope) return;
                    continue;
                }
                const work = reviewClassifierWork(response);
                // Work outside the contract, and work that arrives for a session released during the call, stays unanswered; its review's wait ends the attempt.
                if (!work || loop.stopped) continue;
                const stuck = await answer(loop, work);
                sinceWork = 0;
                // A call that ignored its abort signal is still running in the harness. The loop waits for it, so a project has one such call at a time.
                if (stuck) await Promise.race([stuck.call.catch(() => undefined), closed]);
            }
        } finally {
            if (loops.get(loop.scope.projectRoot) === loop) loops.delete(loop.scope.projectRoot);
        }
    }

    return {
        start(scope) {
            if (closing.signal.aborted) return;
            // A session answers for the project it is open on, so its loop of a root it left ends.
            for (const loop of loops.values()) {
                if (
                    loop.scope.sessionId === scope.sessionId &&
                    loop.scope.projectRoot !== scope.projectRoot
                )
                    loop.stopped = true;
            }
            const running = loops.get(scope.projectRoot);
            if (running) {
                running.scope = scope;
                running.stopped = false;
                return;
            }
            const loop: Loop = { scope, stopped: false, done: Promise.resolve() };
            loops.set(scope.projectRoot, loop);
            loop.done = run(loop);
        },
        stop(sessionId) {
            for (const loop of loops.values()) {
                if (loop.scope.sessionId === sessionId) loop.stopped = true;
            }
        },
        async idle() {
            while (loops.size > 0) await Promise.all([...loops.values()].map((loop) => loop.done));
        },
        async close() {
            closing.abort();
            await this.idle();
        },
    };
}
