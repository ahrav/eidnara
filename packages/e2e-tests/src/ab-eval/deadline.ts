export type Settled<T> = { timedOut: false; value: T } | { timedOut: true };

export class StillRunningError extends Error {}

/** `within` clears its timer when the race settles, releasing the timer's hold on the process. */
function within<T>(
    work: Promise<T>,
    ms: number,
): Promise<{ done: true; value: T } | { done: false }> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const expiry = new Promise<{ done: false }>((resolve) => {
        timer = setTimeout(() => resolve({ done: false }), ms);
    });
    return Promise.race([work.then((value) => ({ done: true as const, value })), expiry]).finally(
        () => clearTimeout(timer),
    );
}

/**
 * After timeout, `settleWithin` calls `stop.abort()` and waits for `work` to settle. If the drain
 * window expires first, it throws `StillRunningError` while `work` may still be running.
 */
export async function settleWithin<T>(
    work: Promise<T>,
    timeoutMs: number,
    stop: { abort: () => Promise<unknown>; drainMs: number },
): Promise<Settled<T>> {
    const first = await within(work, timeoutMs);
    if (first.done) return { timedOut: false, value: first.value };
    await within(
        stop.abort().catch(() => undefined),
        stop.drainMs,
    );
    const drained = await within(
        work.then(
            () => undefined,
            () => undefined,
        ),
        stop.drainMs,
    );
    if (!drained.done) {
        throw new StillRunningError(`work still running ${stop.drainMs}ms after its abort`);
    }
    return { timedOut: true };
}
