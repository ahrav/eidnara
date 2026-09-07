/**
 * One process-wide `exit` listener aborts every registered controller. A listener per plugin
 * instance can trigger `MaxListenersExceededWarning` when enough instances register.
 */

/** Each controller maps to the `abort` listener that prunes it, so unregistering can detach it. */
const controllers = new Map<AbortController, () => void>();
let listenerRegistered = false;

function abortAll(): void {
    for (const controller of controllers.keys()) {
        try {
            controller.abort();
        } catch {
            // Exit handling ignores individual `abort()` failures so remaining controllers are aborted.
        }
    }
}

export function registerExitAbort(controller: AbortController): void {
    if (controller.signal.aborted || controllers.has(controller)) return;
    const prune = (): void => {
        controllers.delete(controller);
    };
    controllers.set(controller, prune);
    controller.signal.addEventListener("abort", prune, { once: true });
    if (listenerRegistered) return;
    listenerRegistered = true;
    process.once("exit", abortAll);
}

export function unregisterExitAbort(controller: AbortController): void {
    const prune = controllers.get(controller);
    if (!prune) return;
    controllers.delete(controller);
    controller.signal.removeEventListener("abort", prune);
}

export function exitAbortRegistrySize(): number {
    return controllers.size;
}
