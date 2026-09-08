import { z } from "zod";

const PermissionValueSchema = z.enum(["ask", "allow", "deny"]);

// A rule is either one action or a map from glob pattern to action.
const PermissionRuleSchema = z.union([
    PermissionValueSchema,
    z.record(z.string(), PermissionValueSchema),
]);

// `z.object` strips unknown keys by default, which would silently drop a
// restriction on any permission not named here (`read`, `task`, `websearch`,
// an MCP tool name, `*`). The catch-all validates those keys instead.
const PermissionSchema = z
    .object({
        edit: PermissionRuleSchema.optional(),
        bash: PermissionRuleSchema.optional(),
        webfetch: PermissionValueSchema.optional(),
        doom_loop: PermissionValueSchema.optional(),
        external_directory: PermissionRuleSchema.optional(),
    })
    .catchall(PermissionRuleSchema)
    .optional();

export const AgentOverrideConfigSchema = z.object({
    model: z.string().optional().describe("Primary model ID (e.g. 'claude-sonnet-4-6')"),
    temperature: z.number().min(0).max(2).optional().describe("Sampling temperature (0-2)"),
    top_p: z.number().min(0).max(1).optional().describe("Nucleus sampling top_p (0-1)"),
    prompt: z.string().optional().describe("Additional system prompt text"),
    tools: z.record(z.string(), z.boolean()).optional().describe("Tool enable/disable overrides"),
    disable: z.boolean().optional().describe("Disable this agent"),
    description: z.string().optional().describe("Agent description"),
    mode: z
        .enum(["subagent", "primary", "all"])
        .optional()
        .describe("Agent mode (subagent, primary, or all)"),
    color: z
        .string()
        .regex(/^#[0-9A-Fa-f]{6}$/)
        .optional()
        .describe("Hex color for the agent (e.g. '#a1b2c3')"),
    maxSteps: z.number().optional().describe("Maximum tool-call steps per invocation"),
    permission: PermissionSchema.describe("Per-tool permission overrides"),
    maxTokens: z.number().optional().describe("Maximum output tokens"),
    variant: z
        .string()
        .optional()
        .describe("OpenCode reasoning variant (e.g. for extended thinking)"),
    fallback_models: z
        .union([z.string(), z.array(z.string())])
        .optional()
        .describe("Fallback model IDs if primary is unavailable"),
});
