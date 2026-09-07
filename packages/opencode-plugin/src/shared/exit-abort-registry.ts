/**
 * One process-wide `exit` listener aborts every registered controller. A listener per plugin
 * instance can trigger `MaxListenersExceededWarning` when enough instances register.
 */

const controllers = new Set<AbortController>();
let listenerRegistered = false;

function abortAll(): void {
    for (const controller of controllers) {
        try {
            controller.abort();
        } catch {
            // Exit handling ignores individual `abort()` failures so remaining controllers are aborted.
        }
    }
}

export function registerExitAbort(controller: AbortController): void {
    if (controller.signal.aborted) return;
    controllers.add(controller);
    controller.signal.addEventListener("abort", () => controllers.delete(controller), {
        once: true,
    });
    if (listenerRegistered) return;
    listenerRegistered = true;
    process.once("exit", abortAll);
}

export function unregisterExitAbort(controller: AbortController): void {
    controllers.delete(controller);
}

export function exitAbortRegistrySize(): number {
    return controllers.size;
}
