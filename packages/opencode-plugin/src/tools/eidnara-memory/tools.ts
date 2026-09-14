import { type ToolDefinition, tool } from "@opencode-ai/plugin";
import { CONTEXT_RESEARCHER_AGENT } from "../../agents/context-researcher";
import { resolveProjectRootDirectory } from "../../features/context/project-identity";
import { toolCallIdFromContext } from "../../plugin/rust-tool-backends";
import { MemoryInputError } from "../../shared/kernel-client/anti-memory";
import { unwrapImitatedReducedArgs } from "../unwrap-imitated-reduced-args";
import {
    EIDNARA_MEMORY_DESCRIPTION,
    EIDNARA_MEMORY_TOOL_NAME,
    EIDNARA_MEMORY_UNWRAP_RULES,
    WRITABLE_MEMORY_CATEGORIES,
} from "./constants";
import { EIDNARA_MEMORY_ACTOR, executeEidnaraMemory } from "./execute";
import {
    EIDNARA_MEMORY_ACTIONS,
    type EidnaraMemoryAction,
    type EidnaraMemoryArgs,
    type EidnaraMemoryToolDeps,
    isEidnaraMemoryMutation,
} from "./types";
import { assertEidnaraMemoryWriteShape } from "./write-shape";

export { EIDNARA_MEMORY_LIGHT_DESCRIPTION } from "../light-descriptions";

const antiMemoryShape = {
    trigger: tool.schema.string(),
    rejectedStrategy: tool.schema.string(),
    rejectionReason: tool.schema.string(),
    saferAlternative: tool.schema.string().nullable().optional(),
    preconditions: tool.schema.string().nullable().optional(),
    attemptedApproach: tool.schema.string().nullable().optional(),
    observedFailure: tool.schema.string().nullable().optional(),
    rootCause: tool.schema.string().nullable().optional(),
    recovery: tool.schema.string().nullable().optional(),
    nonApplicableWhen: tool.schema.string().nullable().optional(),
    expiresAt: tool.schema
        .number()
        .nullable()
        .optional()
        .describe(
            "Epoch ms after which the warning stops surfacing; omitted writes default to 90 days out",
        ),
};

const eidnaraMemoryArgsShape = {
    action: tool.schema
        .enum([...EIDNARA_MEMORY_ACTIONS])
        .optional()
        .describe("create, get, revise, archive, or merge"),
    content: tool.schema.string().optional().describe("Memory content for create/revise/merge"),
    category: tool.schema
        .enum([...WRITABLE_MEMORY_CATEGORIES])
        .optional()
        .describe("Memory category for create/revise/merge"),
    antiMemory: tool.schema
        .object(antiMemoryShape)
        .optional()
        .describe(
            "Rejected-approach payload. Required with category REJECTED_APPROACH, and content must be omitted; invalid with any other category.",
        ),
    objectId: tool.schema.string().optional().describe("Object id for revise/archive"),
    objectIds: tool.schema
        .array(tool.schema.string())
        .max(20)
        .optional()
        .describe("Object ids for get, or the objects merge folds into one survivor"),
    reason: tool.schema.string().optional().describe("Lifecycle-change reason"),
};

const eidnaraMemoryArgsSchema = tool.schema.object(eidnaraMemoryArgsShape).passthrough();

/** An omitted `allowedActions` admits every action; an explicit empty list admits none, so a caller whose computed allowlist is empty exposes no operation. */
function allowedActions(deps: EidnaraMemoryToolDeps): readonly EidnaraMemoryAction[] {
    return deps.allowedActions ?? EIDNARA_MEMORY_ACTIONS;
}

function createEidnaraMemoryTool(deps: EidnaraMemoryToolDeps): ToolDefinition {
    const primaryActions = allowedActions(deps);
    return tool({
        description: EIDNARA_MEMORY_DESCRIPTION,
        args: eidnaraMemoryArgsShape,
        async execute(rawArgs: EidnaraMemoryArgs, toolContext) {
            try {
                const parsed = eidnaraMemoryArgsSchema.safeParse(rawArgs);
                let args = (parsed.success ? parsed.data : rawArgs) as EidnaraMemoryArgs;
                args = unwrapImitatedReducedArgs(args, ["action"], EIDNARA_MEMORY_UNWRAP_RULES);
                const rawAction = (args as { action?: unknown }).action;
                if (rawAction === "approve" || rawAction === "enforce") {
                    return "Error: approve and enforce are human-host-owned commands, not agent actions.";
                }
                if (toolContext.agent === CONTEXT_RESEARCHER_AGENT) {
                    return "Error: eidnara_memory is not available to the context_researcher agent.";
                }
                if (
                    typeof rawAction !== "string" ||
                    !EIDNARA_MEMORY_ACTIONS.includes(rawAction as EidnaraMemoryAction) ||
                    !primaryActions.includes(rawAction as EidnaraMemoryAction)
                ) {
                    return `Error: Action '${String(rawAction)}' is not allowed in this context.`;
                }
                const action = rawAction as EidnaraMemoryAction;
                args.action = action;
                assertEidnaraMemoryWriteShape(args);
                const directory = deps.resolveSessionDirectory
                    ? await deps.resolveSessionDirectory(
                          toolContext.sessionID,
                          toolContext.directory,
                      )
                    : toolContext.directory;
                const projectIdentity = deps.resolveProjectPath(directory);
                if (!projectIdentity) {
                    return "Error: Could not resolve project identity for memory action.";
                }
                const toolCallId = toolCallIdFromContext(toolContext);
                if (!toolCallId && isEidnaraMemoryMutation(action)) {
                    return "Error: eidnara_memory mutation requires a stable tool-call identity.";
                }
                const client = deps.kernelClient({
                    sessionId: toolContext.sessionID,
                    projectRoot: resolveProjectRootDirectory(directory),
                });
                return await executeEidnaraMemory({
                    client,
                    args,
                    action,
                    identity: {
                        sessionId: toolContext.sessionID,
                        toolCallId: toolCallId ?? "read",
                    },
                    actor: EIDNARA_MEMORY_ACTOR,
                    ...(toolContext.abort ? { signal: toolContext.abort } : {}),
                });
            } catch (error) {
                if (error instanceof MemoryInputError) {
                    return `Error: ${error.message}`;
                }
                throw error;
            }
        },
    });
}

export function createEidnaraMemoryTools(
    deps: EidnaraMemoryToolDeps,
): Record<string, ToolDefinition> {
    return { [EIDNARA_MEMORY_TOOL_NAME]: createEidnaraMemoryTool(deps) };
}
