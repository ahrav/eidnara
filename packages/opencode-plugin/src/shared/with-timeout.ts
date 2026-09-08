/** Limits OpenCode SDK reads so a slow host cannot hold a hook open indefinitely. */
export const HOST_SDK_READ_TIMEOUT_MS = 2_000;

/** The wrapped promise did not settle within the deadline; its underlying operation may still complete. */
export class TimeoutError extends Error {
    constructor(message: string) {
        super(message);
        this.name = "TimeoutError";
    }
}

/**
 * Rejects with a `TimeoutError` when `promise` has not settled within `timeoutMs`.
 * The timer is cleared as soon as either side settles, so a fast success leaves no live handle behind.
 */
export async function withTimeout<T>(
    promise: Promise<T>,
    timeoutMs: number,
    message: string,
): Promise<T> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new TimeoutError(message)), timeoutMs);
    });
    try {
        return await Promise.race([promise, timeout]);
    } finally {
        if (timer !== undefined) clearTimeout(timer);
    }
}
