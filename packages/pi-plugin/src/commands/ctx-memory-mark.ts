import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { resolveProjectRootDirectory } from "@eidnara/opencode/features/context/project-identity";
import type {
    DispositionPreview,
    KernelClientResolver,
} from "@eidnara/opencode/shared/kernel-client";
import {
    formatMemoryMarkConfirmation,
    formatMemoryMarkOutcome,
    MEMORY_MARK_COMMAND,
    MEMORY_MARK_DESCRIPTION,
    parseMemoryMarkArgs,
    runMemoryMarkCommand,
} from "@eidnara/opencode/shared/memory-mark-command";
import { resolveSessionId, sendCtxStatusMessage } from "./pi-command-utils";

export const PI_MEMORY_MARK_ACTOR = "user:pi";

export interface RegisterCtxMemoryMarkDeps {
    kernelClient: KernelClientResolver;
}

export function registerCtxMemoryMarkCommand(
    pi: ExtensionAPI,
    deps: RegisterCtxMemoryMarkDeps,
): void {
    const title = `/${MEMORY_MARK_COMMAND}`;
    pi.registerCommand(MEMORY_MARK_COMMAND, {
        description: MEMORY_MARK_DESCRIPTION,
        handler: async (args, ctx) => {
            const sessionId = resolveSessionId(ctx);
            if (!sessionId) {
                sendCtxStatusMessage(pi, {
                    title,
                    text: `## ${title}\n\nNo active Pi session is available.`,
                    level: "error",
                });
                return;
            }
            const parsed = parseMemoryMarkArgs(args ?? "");
            if (!parsed.ok) {
                sendCtxStatusMessage(pi, {
                    title,
                    text: `## Eidnara Memory — Invalid Arguments\n\n${parsed.message}`,
                    level: "error",
                });
                return;
            }
            const projectRoot = resolveProjectRootDirectory(ctx.cwd);
            // Pi invalidates a command's `ctx` when its session is replaced or reloaded, so reading it throws once the invoking session is gone; a commit after that would reopen the closed session's daemon route.
            const isCancelled = (): boolean => {
                try {
                    return resolveSessionId(ctx) !== sessionId;
                } catch {
                    return true;
                }
            };
            const outcome = await runMemoryMarkCommand({
                client: deps.kernelClient({ sessionId, projectRoot }),
                sessionId,
                actor: PI_MEMORY_MARK_ACTOR,
                args: parsed.args,
                isCancelled,
                // Without a dialog the runner asks for the confirm flag instead.
                ...(ctx.hasUI
                    ? {
                          confirm: (preview: DispositionPreview) =>
                              ctx.ui.confirm(
                                  `Apply ${preview.event} to ${preview.object_id}?`,
                                  formatMemoryMarkConfirmation(preview),
                              ),
                      }
                    : {}),
            });
            // The session that asked is gone, so there is no transcript to answer into.
            if (isCancelled()) return;
            sendCtxStatusMessage(
                pi,
                {
                    title,
                    text: formatMemoryMarkOutcome(outcome, parsed.args),
                    level:
                        outcome.kind === "applied"
                            ? "success"
                            : outcome.kind === "refused"
                              ? "error"
                              : "warning",
                },
                { sessionId, outcome },
            );
        },
    });
}
