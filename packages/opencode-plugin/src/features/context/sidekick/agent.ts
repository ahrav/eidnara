import { withContentLanguageDirective } from "../../../agents/language-directive";
import { SIDEKICK_AGENT } from "../../../agents/sidekick";
import type { SidekickConfig } from "../../../config/schema/eidnara";
import {
    childSessionMessagesFetcher,
    createChildSession,
    deleteChildSession,
} from "../../../hooks/context/child-session-spawn";
import type { PluginContext } from "../../../plugin/types";
import * as shared from "../../../shared";
import { extractLatestAssistantText } from "../../../shared/assistant-message-extractor";
import { shouldKeepSubagents } from "../../../shared/keep-subagents";
import { log, sessionLog } from "../../../shared/logger";
import { resolveFallbackChain } from "../../../shared/resolve-fallbacks";
import { SIDEKICK_SYSTEM_PROMPT, stripThinkingBlocks } from "./core";

export { SIDEKICK_SYSTEM_PROMPT };

export async function runSidekick(deps: {
    client: PluginContext["client"];
    sessionId?: string;
    projectPath: string;
    userMessage: string;
    config: SidekickConfig;
    sessionDirectory?: string;
    language?: string;
}): Promise<string | null> {
    const fallbackModels = resolveFallbackChain(deps.config.fallback_models);
    let agentSessionId: string | null = null;
    try {
        const createResponse = await createChildSession({
            client: deps.client,
            parentSessionId: deps.sessionId,
            title: "eidnara-sidekick",
            directory: deps.sessionDirectory ?? deps.projectPath,
        });
        const createdSession = shared.normalizeSDKResponse(
            createResponse,
            null as { id?: string } | null,
            { preferResponseOnMissingData: true },
        );
        agentSessionId = typeof createdSession?.id === "string" ? createdSession.id : null;
        if (!agentSessionId) {
            throw new Error("Sidekick could not create its child session.");
        }
        const childSessionId = agentSessionId;

        const systemPrompt = withContentLanguageDirective(
            deps.config.system_prompt?.trim() ||
                deps.config.prompt?.trim() ||
                SIDEKICK_SYSTEM_PROMPT,
            deps.language,
        );

        const sidekickRun = await shared.promptSyncWithValidatedOutputRetry(
            deps.client,
            {
                path: { id: childSessionId },
                query: { directory: deps.sessionDirectory ?? deps.projectPath },
                body: {
                    agent: SIDEKICK_AGENT,
                    system: systemPrompt,
                    // synthetic: true hides the sidekick prompt from the TUI subagent
                    // pane while still delivering it to the model.
                    parts: [{ type: "text", text: deps.userMessage, synthetic: true }],
                },
            },
            {
                timeoutMs: deps.config.timeout_ms,
                fallbackModels,
                callContext: "sidekick",
                fetchOutput: childSessionMessagesFetcher(
                    deps.client,
                    childSessionId,
                    deps.sessionDirectory ?? deps.projectPath,
                    50,
                ),
                validateOutput: (messages) => {
                    const taskResult = extractLatestAssistantText(messages);
                    if (!taskResult) {
                        throw new Error("Sidekick returned no assistant output.");
                    }
                    const finalText = stripThinkingBlocks(taskResult);
                    if (finalText.length === 0) {
                        throw new Error("Sidekick returned no assistant output.");
                    }
                    return finalText;
                },
            },
        );

        return sidekickRun.validated;
    } catch (error) {
        if (deps.sessionId) {
            sessionLog(deps.sessionId, "sidekick failed:", error);
        } else {
            log("[eidnara] sidekick failed:", error);
        }
        return null;
    } finally {
        if (agentSessionId && !shouldKeepSubagents()) {
            await deleteChildSession(deps.client, agentSessionId).catch((error: unknown) => {
                log("[eidnara] failed to delete sidekick child session:", error);
            });
        }
    }
}
