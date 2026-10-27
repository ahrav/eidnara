import type { SessionDirectoryResolver } from "../../hooks/context/session-directory";
import type { KernelClientResolver } from "../../shared/kernel-client";
import type { AntiMemoryPayload } from "../../shared/kernel-client/anti-memory";
import type { ImitatedReducedArgs } from "../unwrap-imitated-reduced-args";

export const EIDNARA_MEMORY_ACTIONS = [
    "create",
    "get",
    "revise",
    "archive",
    "merge",
    "dispute",
    "candidates",
] as const;

export type EidnaraMemoryAction = (typeof EIDNARA_MEMORY_ACTIONS)[number];

/** The actions both harnesses expose unless a caller names others: `dispute` is admitted only by an explicit allowlist. */
export const EIDNARA_MEMORY_DEFAULT_ACTIONS: readonly EidnaraMemoryAction[] =
    EIDNARA_MEMORY_ACTIONS.filter((action) => action !== "dispute");

/** The events a `dispute` files; both must cite evidence. */
export const EIDNARA_MEMORY_DISPUTE_EVENTS = ["mark_disputed", "contradict"] as const;

const EIDNARA_MEMORY_READ_ACTIONS: ReadonlySet<EidnaraMemoryAction> = new Set(["get", "candidates"]);

/** Whether `action` commits through the kernel rather than reading from it. */
export function isEidnaraMemoryMutation(action: EidnaraMemoryAction): boolean {
    return !EIDNARA_MEMORY_READ_ACTIONS.has(action);
}

export interface EidnaraMemoryArgs extends ImitatedReducedArgs {
    action?: EidnaraMemoryAction;
    content?: string;
    category?: string;
    antiMemory?: AntiMemoryPayload;
    /** The one target of `revise`, `archive`, `dispute`, or `candidates`. */
    objectId?: string;
    /** The `next` cursor of a prior `candidates` page. */
    after?: string | null;
    /** The `candidates` page size, 1 to 64. */
    limit?: number;
    /** The `dispute` event: one of `EIDNARA_MEMORY_DISPUTE_EVENTS`. */
    event?: string;
    /** A live Kernel evidence id the `dispute` cites. */
    evidenceId?: string;
    /** A repository path the `dispute` cites, with `gitObjectId`. */
    path?: string;
    gitObjectId?: string;
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
