import { CONTEXT_RESEARCHER_AGENT } from "../../../agents/context-researcher";
import { withContentLanguageDirective } from "../../../agents/language-directive";
import type { ContextResearcherConfig } from "../../../config/schema/eidnara";
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
import {
    CONTEXT_RESEARCHER_SYSTEM_PROMPT,
    isEmptyContextResearcherResult,
    stripThinkingBlocks,
} from "./core";

export { CONTEXT_RESEARCHER_SYSTEM_PROMPT };

export async function runContextResearcher(deps: {
    client: PluginContext["client"];
    sessionId?: string;
    projectPath: string;
    userMessage: string;
    config: ContextResearcherConfig;
    sessionDirectory?: string;
    language?: string;
}): Promise<string | null> {
    const fallbackModels = resolveFallbackChain(deps.config.fallback_models);
    let agentSessionId: string | null = null;
    try {
        const createResponse = await createChildSession({
            client: deps.client,
            parentSessionId: deps.sessionId,
            title: "eidnara-context_researcher",
            directory: deps.sessionDirectory ?? deps.projectPath,
        });
        const createdSession = shared.normalizeSDKResponse(
            createResponse,
            null as { id?: string } | null,
            { preferResponseOnMissingData: true },
        );
        agentSessionId = typeof createdSession?.id === "string" ? createdSession.id : null;
        if (!agentSessionId) {
            throw new Error("ContextResearcher could not create its child session.");
        }
        const childSessionId = agentSessionId;

        const systemPrompt = withContentLanguageDirective(
            deps.config.system_prompt?.trim() ||
                deps.config.prompt?.trim() ||
                CONTEXT_RESEARCHER_SYSTEM_PROMPT,
            deps.language,
        );

        const context_researcherRun = await shared.promptSyncWithValidatedOutputRetry(
            deps.client,
            {
                path: { id: childSessionId },
                query: { directory: deps.sessionDirectory ?? deps.projectPath },
                body: {
                    agent: CONTEXT_RESEARCHER_AGENT,
                    system: systemPrompt,
                    // synthetic: true hides the context_researcher prompt from the TUI subagent
                    // pane while still delivering it to the model.
                    parts: [{ type: "text", text: deps.userMessage, synthetic: true }],
                },
            },
            {
                timeoutMs: deps.config.timeout_ms,
                fallbackModels,
                callContext: "context-researcher",
                fetchOutput: childSessionMessagesFetcher(
                    deps.client,
                    childSessionId,
                    deps.sessionDirectory ?? deps.projectPath,
                    50,
                ),
                validateOutput: (messages) => {
                    const taskResult = extractLatestAssistantText(messages);
                    if (!taskResult) {
                        throw new Error("ContextResearcher returned no assistant output.");
                    }
                    const finalText = stripThinkingBlocks(taskResult);
                    if (finalText.length === 0) {
                        throw new Error("ContextResearcher returned no assistant output.");
                    }
                    return finalText;
                },
            },
        );

        // The no-result sentinel is a valid completion, not a failure, so it is filtered
        // after validation rather than thrown into the fallback-model retry.
        if (isEmptyContextResearcherResult(context_researcherRun.validated)) {
            return null;
        }
        return context_researcherRun.validated;
    } catch (error) {
        if (deps.sessionId) {
            sessionLog(deps.sessionId, "context_researcher failed:", error);
        } else {
            log("[eidnara] context_researcher failed:", error);
        }
        return null;
    } finally {
        if (agentSessionId && !shouldKeepSubagents()) {
            await deleteChildSession(deps.client, agentSessionId).catch((error: unknown) => {
                log("[eidnara] failed to delete context_researcher child session:", error);
            });
        }
    }
}
