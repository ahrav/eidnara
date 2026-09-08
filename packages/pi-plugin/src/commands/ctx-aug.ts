/**
 *
 * packages/plugin/src/hooks/context/command-handler.ts#executeAugmentation):
 * The command sends the preparing notification without including it in the LLM prompt.
 *      `<sidekick-augmentation>` block.
 * The augmented user message starts the next turn.
 *
 * `pi.sendUserMessage(content)` queues the augmented prompt for the next turn.
 * `pi.sendUserMessage(content)` queues the augmented prompt as the next turn.
 * If the sidekick subprocess fails, Pi sends the original prompt unaugmented.
 *
 * The augmentation is sent as a new user message rather than mutating a cached prefix.
 * Each `<sidekick-augmentation>` invocation creates a one-shot user turn.
 * The augmentation does not persist as a prefix change.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { withContentLanguageDirective } from "@eidnara/opencode/agents/language-directive";
import { resolveProjectIdentityForSession } from "@eidnara/opencode/features/context/project-identity";
import {
    isEmptySidekickResult,
    SIDEKICK_SYSTEM_PROMPT,
    stripThinkingBlocks,
} from "@eidnara/opencode/features/context/sidekick/core";
import { log, sessionLog } from "@eidnara/opencode/shared/logger";

import { PiSubagentRunner } from "../subagent-runner";

/**
 *
 */
export interface PiSidekickConfig {
    /** The `model` value must use `provider/model` form, such as `anthropic/claude-haiku-4-5`. */
    model: string;
    /* */
    systemPrompt?: string;
    /* */
    timeoutMs?: number;
    /** `thinking_level` sets Pi's `--thinking <level>` value for the sidekick subagent. */
    thinking_level?: string;
    /** `fallbackModels` are tried after the primary sidekick model. */
    fallbackModels?: readonly string[];
    language?: string;
    /** User-level configuration can allow sessions started exactly in the canonical home directory. */
    allowHomeProject?: boolean;
}

type ResolveSidekickConfig = (ctx: { cwd: string }) => PiSidekickConfig | undefined;

/**
 *
 */
export function registerCtxAugCommand(
    pi: ExtensionAPI,
    config: PiSidekickConfig | undefined | ResolveSidekickConfig,
): void {
    const runner = new PiSubagentRunner();

    pi.registerCommand("ctx-aug", {
        description: "Augment your prompt with relevant project context (sidekick)",
        handler: async (args, ctx) => {
            const prompt = args.trim();

            // The session label uses the branch's last entry ID to correlate logs.
            const branch = ctx.sessionManager.getBranch();
            const lastEntryId = branch.length > 0 ? branch[branch.length - 1]?.id : "unknown";
            const sessionLabel = `pi-session-${lastEntryId}`;
            const currentConfig = typeof config === "function" ? config(ctx) : config;

            if (!currentConfig) {
                ctx.ui.notify(
                    "/ctx-aug: Sidekick is not configured. Add `sidekick.model` to your eidnara.jsonc to enable this command.",
                    "warning",
                );
                return;
            }

            if (prompt.length === 0) {
                ctx.ui.notify(
                    "/ctx-aug: Usage `/ctx-aug <your prompt>` — provide a prompt to augment with project memory context.",
                    "info",
                );
                return;
            }

            // `ctx.hasUI` gates the progress notification; augmentation still runs without a UI.
            if (ctx.hasUI) {
                ctx.ui.notify(
                    "🔍 Preparing augmentation… 2-10s depending on your sidekick provider.",
                    "info",
                );
            }

            sessionLog(sessionLabel, "/ctx-aug: spawning sidekick", {
                model: currentConfig.model,
            });

            const projectIdentity = resolveProjectIdentityForSession(
                ctx.cwd,
                currentConfig.allowHomeProject,
            );
            if (!projectIdentity) {
                sessionLog(sessionLabel, "Error: Could not resolve project identity for sidekick.");
                return;
            }
            sessionLog(sessionLabel, "/ctx-aug: project identity", projectIdentity);

            const result = await runner.run({
                agent: "sidekick",
                systemPrompt: withContentLanguageDirective(
                    currentConfig.systemPrompt ?? SIDEKICK_SYSTEM_PROMPT,
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
                accountingSubagent: "sidekick",
            });

            if (!result.ok) {
                log(`[eidnara][pi] /ctx-aug: sidekick failed (${result.reason}): ${result.error}`);
                // An abort is the user stopping this command; sending the prompt would start the very turn they cancelled.
                if (result.reason === "abort") return;
                // Any other sidekick failure sends the original prompt unaugmented.
                if (ctx.hasUI) {
                    ctx.ui.notify(
                        `/ctx-aug: sidekick failed (${result.reason}). Sending prompt without augmentation.`,
                        "warning",
                    );
                }
                pi.sendUserMessage(prompt);
                return;
            }

            const sidekickText = stripThinkingBlocks(result.assistantText);
            sessionLog(
                sessionLabel,
                `/ctx-aug: sidekick returned ${sidekickText.length} chars in ${result.durationMs}ms`,
            );

            if (isEmptySidekickResult(sidekickText)) {
                pi.sendUserMessage(prompt);
                return;
            }

            const augmentedPrompt = `${prompt}\n\n<sidekick-augmentation>\n${sidekickText}\n</sidekick-augmentation>`;
            pi.sendUserMessage(augmentedPrompt);
        },
    });
}
