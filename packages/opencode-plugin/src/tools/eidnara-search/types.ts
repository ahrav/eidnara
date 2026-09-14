import type { SessionDirectoryResolver } from "../../hooks/context/session-directory";
import type { KernelClientResolver } from "../eidnara-memory/types";
import type { ImitatedReducedArgs } from "../unwrap-imitated-reduced-args";

/** `memory` is the one searchable source: the project memories served by the memory daemon. */
export type EidnaraSearchSource = "memory";

export interface EidnaraSearchArgs extends ImitatedReducedArgs {
    query?: string;
    limit?: number;
    /** Restrict search to specific sources. Omit to search all; [] searches none. */
    sources?: EidnaraSearchSource[];
}

export interface EidnaraSearchToolDeps {
    kernelClient: KernelClientResolver;
    /**
     * Resolve the project identity for the session's directory at call time.
     * OpenCode's top-level `ctx.directory` reflects the launch directory, not the session's working directory.
     */
    resolveProjectPath: (directory: string) => string | undefined;
    /** Pins the same route root used by every daemon call for one session. */
    resolveSessionDirectory?: SessionDirectoryResolver;
}
