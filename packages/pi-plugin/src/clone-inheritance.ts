import { log } from "@eidnara/opencode/shared/logger";
import { isolatePiSessionKernelTokens } from "./kernel-client-pi";

type SessionManagerLike = {
    getSessionId?: () => string | undefined;
    getBranch?: () => unknown;
};

type CloneContextLike = { sessionManager?: SessionManagerLike };
type CloneStartEventLike = { reason?: unknown; previousSessionFile?: unknown };

export interface PiCloneInheritanceDeps {
    writeLog?: (message: string) => void;
}

/* */
export async function handlePiCloneSessionStart(
    event: CloneStartEventLike,
    ctx: CloneContextLike,
    deps: PiCloneInheritanceDeps,
): Promise<boolean> {
    if (event.reason !== "fork") {
        return false;
    }

    const stage = "resolve-destination";
    let destinationSessionId = "unknown";
    const writeLog = deps.writeLog ?? log;
    try {
        const manager = ctx.sessionManager;
        if (typeof manager?.getSessionId !== "function") {
            throw new Error("Pi session manager does not expose getSessionId");
        }
        destinationSessionId = manager.getSessionId() ?? "";
        if (destinationSessionId.length === 0) {
            throw new Error("Pi clone session id is empty");
        }

        // Kernel tokens and `known_as_of` are process state the clone must not share, so an applied copy's first read fetches from tip; a redelivered clone event is a no-op and must not wipe tokens the session already accumulated. commentlint: allow(JUDGE)
        isolatePiSessionKernelTokens(destinationSessionId);
        writeLog(
            `[eidnara][pi] clone-inheritance: isolated kernel tokens dest=${destinationSessionId} reason=fork`,
        );
        return true;
    } catch (error) {
        writeLog(
            `[eidnara][pi] clone-inheritance: failed dest=${destinationSessionId} stage=${stage} error=${error instanceof Error ? error.message : String(error)}`,
        );
        return false;
    }
}
