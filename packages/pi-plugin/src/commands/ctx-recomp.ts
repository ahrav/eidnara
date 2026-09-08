import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { parseRecompArgs } from "@eidnara/opencode/hooks/context/command-handler";
import {
    COMPACTION_OFF_COMMAND_UNAVAILABLE,
    callDaemonSession,
    type DaemonSessionDeps,
    formatRustOperationMessage,
    operationMessageLevel,
    RECOMP_RANGE_UNSUPPORTED,
    RECOMP_USAGE,
    rustCommandId,
} from "./daemon-session-routes";
import { resolveSessionId, sendCtxStatusMessage } from "./pi-command-utils";

export type RegisterCtxRecompDeps = DaemonSessionDeps;

export function registerCtxRecompCommand(pi: ExtensionAPI, deps: RegisterCtxRecompDeps): void {
    pi.registerCommand("ctx-recomp", {
        description: "Rebuild Eidnara compacted history from the raw Pi session",
        handler: async (args, ctx) => {
            const sessionId = resolveSessionId(ctx);
            if (!sessionId) {
                sendCtxStatusMessage(pi, {
                    title: "/ctx-recomp",
                    text: "## Eidnara Recomp\n\nNo active Pi session is available.",
                    level: "error",
                });
                return;
            }
            if (deps.compactionOff) {
                sendCtxStatusMessage(pi, {
                    title: "/ctx-recomp",
                    text: COMPACTION_OFF_COMMAND_UNAVAILABLE,
                    level: "warning",
                });
                return;
            }

            let result: string;
            const parsedArgs = parseRecompArgs(args);
            if (parsedArgs.kind === "error") {
                result = `## Eidnara Recomp — Invalid Arguments\n\n${parsedArgs.message}`;
            } else if (parsedArgs.kind === "partial") {
                result = `## Eidnara Recomp — Unsupported\n\nRequested range: \`${parsedArgs.range.start}-${parsedArgs.range.end}\`. ${RECOMP_RANGE_UNSUPPORTED}\n\n${RECOMP_USAGE}`;
            } else {
                try {
                    const value = await callDaemonSession(deps, ctx, "session.recomp", {
                        method: "session.recomp",
                        v: 1,
                        session_id: sessionId,
                        command_id: rustCommandId("recomp"),
                    });
                    result = formatRustOperationMessage("recomp", value);
                } catch (error) {
                    result = `## Eidnara Recomp — Failed\n\n${error instanceof Error ? error.message : String(error)}`;
                }
            }
            sendCtxStatusMessage(pi, {
                title: "/ctx-recomp",
                text: result,
                level: operationMessageLevel(result),
            });
        },
    });
}
