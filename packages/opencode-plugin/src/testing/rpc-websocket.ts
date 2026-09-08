/**
 * Provides WebSocket fixtures for tests that drive `EidnaraRpcServer` from a real client socket.
 * Bun's global `WebSocket` accepts custom headers, which the bearer-token upgrade path needs.
 */

/** Opens a socket to `/ws`, sending the token in the `Authorization` header or, with `legacyQueryAuth`, the query string. */
export async function openRpcSocket(
    port: number,
    token: string,
    legacyQueryAuth = false,
): Promise<WebSocket> {
    const ws = legacyQueryAuth
        ? new WebSocket(`ws://127.0.0.1:${port}/ws?token=${encodeURIComponent(token)}`)
        : new WebSocket(`ws://127.0.0.1:${port}/ws`, {
              headers: { Authorization: `Bearer ${token}` },
          });
    await new Promise<void>((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error("socket open timed out")), 2_000);
        ws.addEventListener(
            "open",
            () => {
                clearTimeout(timeout);
                resolve();
            },
            { once: true },
        );
        ws.addEventListener(
            "error",
            () => {
                clearTimeout(timeout);
                reject(new Error("socket open failed"));
            },
            { once: true },
        );
    });
    return ws;
}

/** Resolves with the first JSON frame that satisfies `predicate`; frames that are not JSON are skipped. */
export function waitForJsonMessage<T extends { type?: string }>(
    ws: WebSocket,
    predicate: (message: T) => boolean,
    timeoutMs = 2_000,
): Promise<T> {
    return new Promise((resolve, reject) => {
        const timeout = setTimeout(() => {
            ws.removeEventListener("message", onMessage);
            reject(new Error("socket message timed out"));
        }, timeoutMs);
        const onMessage = (event: MessageEvent) => {
            let message: T;
            try {
                message = JSON.parse(String(event.data)) as T;
            } catch {
                return;
            }
            if (!predicate(message)) return;
            clearTimeout(timeout);
            ws.removeEventListener("message", onMessage);
            resolve(message);
        };
        ws.addEventListener("message", onMessage);
    });
}

/** Polls `condition` every 25 ms and rejects with `label` once `timeoutMs` elapses. */
export async function waitFor(
    condition: () => boolean,
    label: string,
    timeoutMs = 2_000,
): Promise<void> {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
        if (condition()) return;
        await new Promise((resolve) => setTimeout(resolve, 25));
    }
    throw new Error(`Timed out waiting for ${label}`);
}
