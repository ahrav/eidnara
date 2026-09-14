/**
 *
 * packages/plugin/src/hooks/context/command-handler.ts#executeAugmentation):
 * The command sends the preparing notification without including it in the LLM prompt.
 *      `<context_researcher-augmentation>` block.
 * The augmented user message starts the next turn.
 *
 * `pi.sendUserMessage(content)` queues the augmented prompt for the next turn.
 * `pi.sendUserMessage(content)` queues the augmented prompt as the next turn.
 * If the context_researcher subprocess fails, Pi sends the original prompt unaugmented.
 *
 * The augmentation is sent as a new user message rather than mutating a cached prefix.
 * Each `<context_researcher-augmentation>` invocation creates a one-shot user turn.
 * The augmentation does not persist as a prefix change.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { withContentLanguageDirective } from "@eidnara/opencode/agents/language-directive";
import {
    CONTEXT_RESEARCHER_SYSTEM_PROMPT,
    isEmptyContextResearcherResult,
    stripThinkingBlocks,
} from "@eidnara/opencode/features/context/context-researcher/core";
import { resolveProjectIdentityForSession } from "@eidnara/opencode/features/context/project-identity";
import { log, sessionLog } from "@eidnara/opencode/shared/logger";

import { PiSubagentRunner } from "../subagent-runner";

/**
 *
 */
export interface PiContextResearcherConfig {
    /** The `model` value must use `provider/model` form, such as `anthropic/claude-haiku-4-5`. */
    model: string;
    /* */
    systemPrompt?: string;
    /* */
    timeoutMs?: number;
    /** `thinking_level` sets Pi's `--thinking <level>` value for the context_researcher subagent. */
    thinking_level?: string;
    /** `fallbackModels` are tried after the primary context_researcher model. */
    fallbackModels?: readonly string[];
    language?: string;
    /** User-level configuration can allow sessions started exactly in the canonical home directory. */
    allowHomeProject?: boolean;
}

type ResolveContextResearcherConfig = (ctx: {
    cwd: string;
}) => PiContextResearcherConfig | undefined;

/**
 *
 */
export function registerCtxAugCommand(
    pi: ExtensionAPI,
    config: PiContextResearcherConfig | undefined | ResolveContextResearcherConfig,
): void {
    const runner = new PiSubagentRunner();

    pi.registerCommand("eidnara-aug", {
        description: "Augment your prompt with relevant project context (context_researcher)",
        handler: async (args, ctx) => {
            const prompt = args.trim();

            // The session label uses the branch's last entry ID to correlate logs.
            const branch = ctx.sessionManager.getBranch();
            const lastEntryId = branch.length > 0 ? branch[branch.length - 1]?.id : "unknown";
            const sessionLabel = `pi-session-${lastEntryId}`;
            const currentConfig = typeof config === "function" ? config(ctx) : config;

            if (!currentConfig) {
                ctx.ui.notify(
                    "/eidnara-aug: ContextResearcher is not configured. Add `context_researcher.model` to your eidnara.jsonc to enable this command.",
                    "warning",
                );
                return;
            }

            if (prompt.length === 0) {
                ctx.ui.notify(
                    "/eidnara-aug: Usage `/eidnara-aug <your prompt>` — provide a prompt to augment with project memory context.",
                    "info",
                );
                return;
            }

            // `ctx.hasUI` gates the progress notification; augmentation still runs without a UI.
            if (ctx.hasUI) {
                ctx.ui.notify(
                    "🔍 Preparing augmentation… 2-10s depending on your context_researcher provider.",
                    "info",
                );
            }

            sessionLog(sessionLabel, "/eidnara-aug: spawning context_researcher", {
                model: currentConfig.model,
            });

            const projectIdentity = resolveProjectIdentityForSession(
                ctx.cwd,
                currentConfig.allowHomeProject,
            );
            if (!projectIdentity) {
                sessionLog(
                    sessionLabel,
                    "Error: Could not resolve project identity for context_researcher.",
                );
                if (ctx.hasUI) {
                    ctx.ui.notify(
                        "/eidnara-aug: this directory has no project identity (sessions started in the home directory are excluded unless user-level config sets `allow_home_project`). Run from a project directory, or send the prompt without /eidnara-aug.",
                        "warning",
                    );
                    return;
                }
                // Without a UI, `notify` is a no-op, so send the original prompt unless the user
                // aborted; `sendUserMessage` after an abort would start a new turn.
                if (ctx.signal?.aborted) {
                    sessionLog(
                        sessionLabel,
                        "/eidnara-aug: aborted before context_researcher spawn; prompt not sent",
                    );
                    return;
                }
                sessionLog(
                    sessionLabel,
                    "/eidnara-aug: no project identity and no UI; sending prompt without augmentation",
                );
                pi.sendUserMessage(prompt);
                return;
            }
            sessionLog(sessionLabel, "/eidnara-aug: project identity", projectIdentity);

            const result = await runner.run({
                agent: "context-researcher",
                systemPrompt: withContentLanguageDirective(
                    currentConfig.systemPrompt ?? CONTEXT_RESEARCHER_SYSTEM_PROMPT,
                    currentConfig.language,
                ),
                userMessage: prompt,
                model: currentConfig.model,
                fallbackModels: currentConfig.fallbackModels,
                timeoutMs: currentConfig.timeoutMs ?? 30_000,
                cwd: ctx.cwd,
                signal: ctx.signal,
                thinkingLevel: currentConfig.thinking_level,
                accountingSessionId: sessionLabel,
                accountingSubagent: "context-researcher",
            });

            if (!result.ok) {
                if (result.reason === "abort") {
                    // `ctx.signal` aborts when the user stops the agent; `sendUserMessage` here would start a new turn.
                    sessionLog(
                        sessionLabel,
                        "/eidnara-aug: context_researcher aborted; prompt not sent",
                    );
                    if (ctx.hasUI) {
                        ctx.ui.notify("/eidnara-aug: cancelled. Prompt not sent.", "info");
                    }
                    return;
                }
                // If the context_researcher subprocess fails, Pi sends the original prompt unaugmented.
                // only).
                log(
                    `[eidnara][pi] /eidnara-aug: context_researcher failed (${result.reason}): ${result.error}`,
                );
                if (ctx.hasUI) {
                    ctx.ui.notify(
                        `/eidnara-aug: context_researcher failed (${result.reason}). Sending prompt without augmentation.`,
                        "warning",
                    );
                }
                pi.sendUserMessage(prompt);
                return;
            }

            const context_researcherText = stripThinkingBlocks(result.assistantText);
            sessionLog(
                sessionLabel,
                `/eidnara-aug: context_researcher returned ${context_researcherText.length} chars in ${result.durationMs}ms`,
            );

            if (isEmptyContextResearcherResult(context_researcherText)) {
                pi.sendUserMessage(prompt);
                return;
            }

            const augmentedPrompt = `${prompt}\n\n<context_researcher-augmentation>\n${context_researcherText}\n</context_researcher-augmentation>`;
            pi.sendUserMessage(augmentedPrompt);
        },
    });
}
