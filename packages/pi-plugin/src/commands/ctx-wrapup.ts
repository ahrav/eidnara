import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { parseWrapupArgs } from "@eidnara/opencode/hooks/context/command-handler";
import { MAX_WRAPUP_REQUEST_BUDGET_MS } from "@eidnara/opencode/hooks/context/module-transport";
import {
    COMPACTION_OFF_COMMAND_UNAVAILABLE,
    callDaemonSession,
    type DaemonSessionDeps,
    formatRustOperationMessage,
    operationMessageLevel,
    rustCommandId,
} from "./daemon-session-routes";
import { resolveSessionId, sendCtxStatusMessage } from "./pi-command-utils";

export type RegisterCtxWrapupDeps = DaemonSessionDeps;

export function registerCtxWrapupCommand(pi: ExtensionAPI, deps: RegisterCtxWrapupDeps): void {
    pi.registerCommand("ctx-wrapup", {
        description: "Compact older Eidnara history while keeping the newest messages raw",
        handler: async (args, ctx) => {
            const sessionId = resolveSessionId(ctx);
            if (!sessionId) {
                sendCtxStatusMessage(pi, {
                    title: "/ctx-wrapup",
                    text: "## Eidnara Wrapup\n\nNo active Pi session is available.",
                    level: "error",
                });
                return;
            }
            if (deps.compactionOff) {
                sendCtxStatusMessage(pi, {
                    title: "/ctx-wrapup",
                    text: COMPACTION_OFF_COMMAND_UNAVAILABLE,
                    level: "warning",
                });
                return;
            }

            let result: string;
            const parsed = parseWrapupArgs(args);
            if (!parsed.ok) {
                result = `## Eidnara Wrapup — Invalid Arguments\n\n${parsed.message}`;
            } else {
                sendCtxStatusMessage(pi, {
                    title: "/ctx-wrapup",
                    text: "## Eidnara Wrapup\n\nStarting wrapup…",
                    level: "info",
                });
                try {
                    const value = await callDaemonSession(
                        deps,
                        "session.wrapup",
                        {
                            method: "session.wrapup",
                            v: 1,
                            session_id: sessionId,
                            keep: parsed.messagesToKeep,
                            command_id: rustCommandId("wrapup"),
                        },
                        MAX_WRAPUP_REQUEST_BUDGET_MS,
                    );
                    result = formatRustOperationMessage("wrapup", value);
                } catch (error) {
                    result = `## Eidnara Wrapup — Failed\n\n${error instanceof Error ? error.message : String(error)}`;
                }
            }
            sendCtxStatusMessage(pi, {
                title: "/ctx-wrapup",
                text: result,
                level: operationMessageLevel(result),
            });
        },
    });
}
