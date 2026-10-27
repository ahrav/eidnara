import { type ToolDefinition, tool } from "@opencode-ai/plugin";
import { CONTEXT_RESEARCHER_AGENT } from "../../agents/context-researcher";
import { resolveProjectRootDirectory } from "../../features/context/project-identity";
import { toolCallIdFromContext } from "../../plugin/rust-tool-backends";
import { MemoryInputError } from "../../shared/kernel-client/anti-memory";
import { unwrapImitatedReducedArgs } from "../unwrap-imitated-reduced-args";
import {
    actionList,
    EIDNARA_MEMORY_TOOL_NAME,
    EIDNARA_MEMORY_UNWRAP_RULES,
    eidnaraMemoryDescription,
    WRITABLE_MEMORY_CATEGORIES,
} from "./constants";
import { EIDNARA_MEMORY_ACTOR, executeEidnaraMemory } from "./execute";
import {
    EIDNARA_MEMORY_ACTIONS,
    EIDNARA_MEMORY_DEFAULT_ACTIONS,
    EIDNARA_MEMORY_DISPUTE_EVENTS,
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

/** The `action` field lists exactly the actions the tool admits, so a model never sees one it cannot run. */
function actionField(actions: readonly EidnaraMemoryAction[]) {
    const [first, ...rest] = actions;
    if (first === undefined) {
        return tool.schema.never().optional().describe("no action is available");
    }
    return tool.schema
        .enum([first, ...rest])
        .optional()
        .describe(actionList(actions));
}

const eidnaraMemoryArgsShape = {
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
    objectId: tool.schema.string().optional().describe("Object id for revise/archive/candidates"),
    objectIds: tool.schema
        .array(tool.schema.string())
        .max(20)
        .optional()
        .describe("Object ids for get, or the objects merge folds into one survivor"),
    reason: tool.schema.string().optional().describe("Lifecycle-change reason"),
};

/** The fields only `dispute` reads; exposed exactly when the allowlist admits it. */
const disputeArgsShape = {
    objectId: tool.schema
        .string()
        .optional()
        .describe("Object id for revise/archive/dispute/candidates"),
    event: tool.schema
        .enum([...EIDNARA_MEMORY_DISPUTE_EVENTS])
        .optional()
        .describe("dispute event: mark_disputed or contradict"),
    evidenceId: tool.schema.string().optional().describe("Live evidence id a dispute cites"),
    path: tool.schema
        .string()
        .optional()
        .describe("Relative repository path a dispute cites, with gitObjectId"),
    gitObjectId: tool.schema
        .string()
        .optional()
        .describe("40- or 64-character lowercase hex git object id for path"),
};

/** The paging fields only `candidates` reads; exposed exactly when the allowlist admits it. */
const candidatesArgsShape = {
    after: tool.schema
        .string()
        .nullable()
        .optional()
        .describe("The next cursor of a prior candidates page"),
    limit: tool.schema
        .number()
        .int()
        .min(1)
        .max(64)
        .optional()
        .describe("candidates page size, 1 to 64"),
};

const eidnaraMemoryArgsSchema = tool.schema
    .object({ ...eidnaraMemoryArgsShape, ...candidatesArgsShape, ...disputeArgsShape })
    .passthrough();

/** An omitted `allowedActions` admits the default actions, which exclude `dispute`; an explicit empty list admits none, so a caller whose computed allowlist is empty exposes no operation. */
function allowedActions(deps: EidnaraMemoryToolDeps): readonly EidnaraMemoryAction[] {
    return deps.allowedActions ?? EIDNARA_MEMORY_DEFAULT_ACTIONS;
}

function createEidnaraMemoryTool(deps: EidnaraMemoryToolDeps): ToolDefinition {
    const primaryActions = allowedActions(deps);
    return tool({
        description: eidnaraMemoryDescription(primaryActions),
        args: {
            ...(eidnaraMemoryArgsShape as typeof eidnaraMemoryArgsShape &
                typeof candidatesArgsShape &
                typeof disputeArgsShape),
            ...(primaryActions.includes("candidates") ? candidatesArgsShape : {}),
            ...(primaryActions.includes("dispute") ? disputeArgsShape : {}),
            action: actionField(primaryActions),
        },
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
                    disputeAdmitted: primaryActions.includes("dispute"),
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
