import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { COMPACTION_ENABLED_PATH } from "@eidnara/opencode/config/agent-disable";
import type { RustSessionStatus } from "@eidnara/opencode/plugin/rpc-handlers";
import { describeError } from "@eidnara/opencode/shared/error-message";
import { sessionLog } from "@eidnara/opencode/shared/logger";
import {
    formatTailHygiene,
    resolveTailHygieneStatus,
} from "@eidnara/opencode/shared/tail-hygiene-status";
import { formatWindowDerivationLine } from "@eidnara/opencode/shared/window-geometry";
import { type StatusDialogDeps, showStatusDialog } from "../dialogs/status-dialog";
import { resolvePiWindowGeometry } from "../pi-context-limit";
import {
    callDaemonSession,
    type DaemonSessionDeps,
    formatRustStatusText,
    statusInputTokens,
} from "./daemon-session-routes";
import { resolveSessionId, sendCtxStatusMessage } from "./pi-command-utils";

export type StatusProjectSettings = Omit<StatusDialogDeps, "kernelClient">;

export type RegisterCtxStatusDeps = DaemonSessionDeps &
    Pick<StatusDialogDeps, "kernelClient"> & {
        /** Resolves the invoking context's project settings; the daemon request and the dialog then describe the same project. */
        resolveProjectSettings: (ctx: { cwd: string }) => StatusProjectSettings;
    };

export function registerCtxStatusCommand(pi: ExtensionAPI, deps: RegisterCtxStatusDeps): void {
    pi.registerCommand("ctx-status", {
        description: "Show Eidnara status for the current Pi session",
        handler: async (_args, ctx) => {
            const sessionId = resolveSessionId(ctx);
            if (!sessionId) {
                sendCtxStatusMessage(pi, {
                    title: "/ctx-status",
                    text: "## Eidnara Status\n\nNo active Pi session is available.",
                    level: "error",
                });
                return;
            }
            const dialogDeps: StatusDialogDeps = {
                kernelClient: deps.kernelClient,
                ...deps.resolveProjectSettings(ctx),
            };

            let daemonStatus: RustSessionStatus | null = null;
            let statusError: string | undefined;
            try {
                daemonStatus = (await callDaemonSession(deps, ctx.cwd, "session.status", {
                    method: "session.status",
                    v: 1,
                    session_id: sessionId,
                })) as RustSessionStatus;
            } catch (error) {
                sessionLog(sessionId, "rust session.status failed:", error);
                statusError = error instanceof Error ? error.message : String(error);
            }

            try {
                if (ctx.hasUI) {
                    await showStatusDialog(pi, ctx, dialogDeps, daemonStatus);
                    return;
                }

                const usage = ctx.getContextUsage?.();
                const windowGeometry = resolvePiWindowGeometry({
                    rawContextWindow: usage?.contextWindow ?? ctx.model?.contextWindow,
                    model: ctx.model,
                });
                const tailHygiene = resolveTailHygieneStatus(daemonStatus?.tail_hygiene);
                const lines = ["## Eidnara Status"];
                if (deps.compactionOff) {
                    lines.push(
                        "",
                        `**Compaction:** disabled (${COMPACTION_ENABLED_PATH}: false) — native compaction owns the context window.`,
                    );
                }
                if (daemonStatus) {
                    const value = daemonStatus as Record<string, unknown>;
                    lines.push("", formatRustStatusText(value));
                    if (windowGeometry) {
                        lines.push(
                            `- ${formatWindowDerivationLine(statusInputTokens(value), windowGeometry)}`,
                        );
                    }
                } else {
                    lines.push(
                        "",
                        `Session status is unavailable: ${statusError ?? "no response"}`,
                    );
                }
                if (tailHygiene !== undefined) {
                    lines.push(
                        "",
                        "### Tail Hygiene",
                        `- Reclaimable / eligible: ${formatTailHygiene(tailHygiene)}`,
                        "- Reasoning is excluded from both terms.",
                    );
                }
                sendCtxStatusMessage(
                    pi,
                    { title: "/ctx-status", text: lines.join("\n"), level: "info" },
                    { sessionId, daemonStatus },
                );
            } catch (error) {
                sendCtxStatusMessage(pi, {
                    title: "/ctx-status",
                    text: `## Eidnara Status — Failed\n\n${describeError(error).brief}`,
                    level: "error",
                });
            }
        },
    });
}
