import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import {
    COMPACTION_OFF_COMMAND_UNAVAILABLE,
    callDaemonSession,
    type DaemonSessionDeps,
} from "./daemon-session-routes";
import { resolveSessionId, sendCtxStatusMessage } from "./pi-command-utils";

export type RegisterCtxFlushDeps = DaemonSessionDeps;

export function registerCtxFlushCommand(pi: ExtensionAPI, deps: RegisterCtxFlushDeps): void {
    pi.registerCommand("ctx-flush", {
        description: "Force pending Eidnara drops to materialize on the next provider call",
        handler: async (_args, ctx) => {
            const sessionId = resolveSessionId(ctx);
            if (!sessionId) {
                sendCtxStatusMessage(pi, {
                    title: "/ctx-flush",
                    text: "## /ctx-flush\n\nNo active Pi session is available.",
                    level: "error",
                });
                return;
            }
            if (deps.compactionOff) {
                sendCtxStatusMessage(pi, {
                    title: "/ctx-flush",
                    text: COMPACTION_OFF_COMMAND_UNAVAILABLE,
                    level: "warning",
                });
                return;
            }

            let result: string;
            try {
                const value = await callDaemonSession(deps, ctx, "session.flush", {
                    method: "session.flush",
                    v: 1,
                    session_id: sessionId,
                });
                result =
                    value.armed === false
                        ? "No pending operations to flush."
                        : "Flushed: Changes take effect on next message.";
            } catch (error) {
                result = `Error: Failed to flush context operations. ${error instanceof Error ? error.message : String(error)}`;
            }
            sendCtxStatusMessage(
                pi,
                {
                    title: "/ctx-flush",
                    text: `## /ctx-flush\n\n${result}`,
                    level: result.startsWith("Error:") ? "error" : "success",
                },
                { sessionId, result },
            );
        },
    });
}
