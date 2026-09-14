import type { SessionDirectoryResolver } from "../../hooks/context/session-directory";
import type { KernelClientResolver } from "../../shared/kernel-client";
import type { AntiMemoryPayload } from "../../shared/kernel-client/anti-memory";
import type { ImitatedReducedArgs } from "../unwrap-imitated-reduced-args";

export const EIDNARA_MEMORY_ACTIONS = ["create", "get", "revise", "archive", "merge"] as const;

export type EidnaraMemoryAction = (typeof EIDNARA_MEMORY_ACTIONS)[number];

const EIDNARA_MEMORY_READ_ACTIONS: ReadonlySet<EidnaraMemoryAction> = new Set(["get"]);

/** Whether `action` commits through the kernel rather than reading from it. */
export function isEidnaraMemoryMutation(action: EidnaraMemoryAction): boolean {
    return !EIDNARA_MEMORY_READ_ACTIONS.has(action);
}

export interface EidnaraMemoryArgs extends ImitatedReducedArgs {
    action?: EidnaraMemoryAction;
    content?: string;
    category?: string;
    antiMemory?: AntiMemoryPayload;
    /** The one target of `revise` or `archive`. */
    objectId?: string;
    /** `get` targets; for `merge`, the objects folded into one survivor. */
    objectIds?: string[];
    reason?: string;
}

export type { KernelClientResolver };

export interface EidnaraMemoryToolDeps {
    kernelClient: KernelClientResolver;
    resolveProjectPath: (directory: string) => string | undefined;
    /** Pins the same route root used by every daemon call for one session. */
    resolveSessionDirectory?: SessionDirectoryResolver;
    allowedActions?: EidnaraMemoryAction[];
}
