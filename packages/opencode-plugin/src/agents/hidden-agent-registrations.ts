import { buildAllowOnlyPermission } from "./permissions";

// Hidden-agent caps are 40 for sidekick and 8 for the smart-note compiler.
function clampHiddenAgentStepLimit(value: unknown, cap: number): number {
    return typeof value === "number" && Number.isFinite(value) ? Math.min(value, cap) : cap;
}

export const HIDDEN_AGENT_DESCRIPTION_MARKER = "Internal Eidnara";
const HIDDEN_AGENT_DESCRIPTION =
    "Internal Eidnara maintenance agent. Not for general tasks — do not select for user work.";

export interface HiddenAgentRegistration {
    id: string;
    /** OpenCode's task registry excludes primary agents from general subagent routing. */
    mode: "primary";
    /** Hidden agents stay out of the UI picker while remaining directly resolvable. */
    hidden: true;
    /** Description used by OpenCode's automatic name/description-based task router; keep it generic so this hidden agent is not selected for unrelated tasks. */
    description: string;
    prompt: string | undefined;
    allowedTools: readonly string[];
    maxSteps: number;
    overrides?: Record<string, unknown>;
    /** `lockPermissions` drops user `permission` overrides for privacy-critical agents. */
    lockPermissions?: boolean;
}

export function buildHiddenAgentRegistrations(args: {
    smartNoteCompilerPrompt: string | undefined;
    sidekickPrompt: string | undefined;
    sidekickOverrides?: Record<string, unknown>;
}): HiddenAgentRegistration[] {
    return [
        {
            id: "smart-note-compiler",
            mode: "primary",
            hidden: true,
            description: HIDDEN_AGENT_DESCRIPTION,
            prompt: args.smartNoteCompilerPrompt,
            allowedTools: [],
            maxSteps: 8,
            // `lockPermissions` prevents user overrides from granting compiler tools.
            lockPermissions: true,
        },
        {
            id: "sidekick",
            mode: "primary",
            hidden: true,
            description: HIDDEN_AGENT_DESCRIPTION,
            prompt: args.sidekickPrompt,
            allowedTools: ["ctx_search", "aft_outline", "aft_zoom"],
            maxSteps: 40,
            overrides: args.sidekickOverrides,
        },
    ];
}

/**
 * User overrides may lower `steps` and `maxSteps` but cannot raise either above the built-in cap.
 */
export function buildHiddenAgentConfig(
    prompt: string,
    allowedTools: readonly string[],
    maxSteps: number,
    overrides?: Record<string, unknown>,
    agentLabel?: string,
    lockPermissions = false,
    description?: string,
) {
    const {
        permission: overridePermission,
        tools: overrideTools,
        // Destructuring `prompt` and `system` lets locked configs discard them and prevents `...rest` from overwriting `prompt`.
        prompt: overridePrompt,
        system: overrideSystem,
        ...rest
    } = (overrides ?? {}) as {
        permission?: Record<string, unknown>;
        tools?: Record<string, boolean>;
        prompt?: unknown;
        system?: unknown;
        [key: string]: unknown;
    };
    // When `lockPermissions` is true, excluding user `tools`, `prompt`, and `system` overrides prevents users from re-enabling denied tools or replacing configured prompts.
    // A user `tools` override could otherwise re-enable a tool denied by `basePermission`.
    const promptOverrides: Record<string, unknown> = lockPermissions
        ? {}
        : {
              ...(overridePrompt !== undefined ? { prompt: overridePrompt } : {}),
              ...(overrideSystem !== undefined ? { system: overrideSystem } : {}),
          };
    const restOverrides: Record<string, unknown> = lockPermissions
        ? { ...rest, ...promptOverrides }
        : {
              ...rest,
              ...promptOverrides,
              ...(overrideTools !== undefined ? { tools: overrideTools } : {}),
          };
    const basePermission = buildAllowOnlyPermission(allowedTools, agentLabel);
    return {
        prompt,
        // User-supplied `fallback_models` passes through `restOverrides`; no built-in fallback is added.
        ...restOverrides,
        steps: clampHiddenAgentStepLimit(restOverrides.steps, maxSteps),
        maxSteps: clampHiddenAgentStepLimit(restOverrides.maxSteps, maxSteps),
        // `permission` follows `restOverrides` so `restOverrides` cannot override the deny baseline.
        // When `lockPermissions` is true, excluding `overridePermission` preserves the denied tools in `basePermission`.
        permission: {
            ...basePermission,
            ...(lockPermissions ? {} : (overridePermission ?? {})),
        },
        mode: "primary" as const,
        hidden: true,
        ...(description !== undefined ? { description } : {}),
    };
}
