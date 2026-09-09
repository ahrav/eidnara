import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { resolveProjectRootDirectory } from "@eidnara/opencode/features/context/project-identity";
import { MemoryInputError } from "@eidnara/opencode/shared/kernel-client/anti-memory";
import {
    CTX_MEMORY_DESCRIPTION,
    CTX_MEMORY_TOOL_NAME,
    CTX_MEMORY_UNWRAP_RULES,
    WRITABLE_MEMORY_CATEGORIES,
} from "@eidnara/opencode/tools/ctx-memory/constants";
import { CTX_MEMORY_ACTOR, executeCtxMemory } from "@eidnara/opencode/tools/ctx-memory/execute";
import {
    CTX_MEMORY_ACTIONS,
    type CtxMemoryAction,
    type CtxMemoryArgs,
    isCtxMemoryMutation,
    type KernelClientResolver,
} from "@eidnara/opencode/tools/ctx-memory/types";
import { assertCtxMemoryWriteShape } from "@eidnara/opencode/tools/ctx-memory/write-shape";
import { unwrapImitatedReducedArgs } from "@eidnara/opencode/tools/unwrap-imitated-reduced-args";
import { type Static, Type } from "typebox";

const AntiMemorySchema = Type.Object(
    {
        trigger: Type.String(),
        rejectedStrategy: Type.String(),
        rejectionReason: Type.String(),
        saferAlternative: Type.Optional(Type.Union([Type.String(), Type.Null()])),
        preconditions: Type.Optional(Type.Union([Type.String(), Type.Null()])),
        attemptedApproach: Type.Optional(Type.Union([Type.String(), Type.Null()])),
        observedFailure: Type.Optional(Type.Union([Type.String(), Type.Null()])),
        rootCause: Type.Optional(Type.Union([Type.String(), Type.Null()])),
        recovery: Type.Optional(Type.Union([Type.String(), Type.Null()])),
        nonApplicableWhen: Type.Optional(Type.Union([Type.String(), Type.Null()])),
        expiresAt: Type.Optional(
            Type.Union([Type.Number(), Type.Null()], {
                description:
                    "Epoch ms after which the warning stops surfacing; omitted writes default to 90 days out",
            }),
        ),
    },
    {
        description:
            "Rejected-approach payload. Required with category REJECTED_APPROACH, and content must be omitted; invalid with any other category.",
    },
);

const ParamsSchema = Type.Object(
    {
        action: Type.Optional(
            Type.Union(
                CTX_MEMORY_ACTIONS.map((action) => Type.Literal(action)),
                {
                    description: "create, get, revise, archive, or merge",
                },
            ),
        ),
        content: Type.Optional(
            Type.String({ description: "Memory content for create/revise/merge" }),
        ),
        category: Type.Optional(
            Type.Union(
                WRITABLE_MEMORY_CATEGORIES.map((category) => Type.Literal(category)),
                {
                    description: "Memory category for create/revise/merge",
                },
            ),
        ),
        antiMemory: Type.Optional(AntiMemorySchema),
        objectId: Type.Optional(Type.String({ description: "Object id for revise/archive" })),
        objectIds: Type.Optional(
            Type.Array(Type.String(), {
                maxItems: 20,
                description: "Object ids for get, or the objects merge folds into one survivor",
            }),
        ),
        reason: Type.Optional(Type.String({ description: "Lifecycle-change reason" })),
    },
    { additionalProperties: true },
);

type CtxMemoryParams = Static<typeof ParamsSchema>;

function ok(text: string) {
    return { content: [{ type: "text" as const, text }], details: undefined };
}

function err(text: string) {
    return {
        content: [{ type: "text" as const, text }],
        details: undefined,
        isError: true,
    };
}

export interface CtxMemoryToolDeps {
    /** Resolves the client bound to the calling session and filesystem project root. */
    kernelClient: KernelClientResolver;
    resolveProjectPath: (directory: string) => string | undefined;
}

export function createCtxMemoryTool(deps: CtxMemoryToolDeps): ToolDefinition<typeof ParamsSchema> {
    return {
        name: CTX_MEMORY_TOOL_NAME,
        label: "Eidnara: Memory",
        description: CTX_MEMORY_DESCRIPTION,
        parameters: ParamsSchema,
        async execute(toolCallId, rawParams, signal, _onUpdate, ctx) {
            try {
                let params = rawParams as CtxMemoryParams & CtxMemoryArgs;
                params = unwrapImitatedReducedArgs(params, ["action"], CTX_MEMORY_UNWRAP_RULES);
                const rawAction = (params as { action?: unknown }).action;
                if (rawAction === "approve" || rawAction === "enforce") {
                    return err(
                        "Error: approve and enforce are human-host-owned commands, not agent actions.",
                    );
                }
                if (
                    typeof rawAction !== "string" ||
                    !(CTX_MEMORY_ACTIONS as readonly string[]).includes(rawAction)
                ) {
                    return err(
                        `Error: Action '${String(rawAction)}' is not allowed in this context.`,
                    );
                }
                const action = rawAction as CtxMemoryAction;
                const args: CtxMemoryArgs = { ...params, action };
                assertCtxMemoryWriteShape(args);
                const projectIdentity = deps.resolveProjectPath(ctx.cwd);
                if (!projectIdentity) {
                    return err("Error: Could not resolve project identity for memory action.");
                }
                const sessionId = ctx.sessionManager.getSessionId();
                if (!sessionId) {
                    return err("Error: ctx_memory requires an active session.");
                }
                if (!toolCallId && isCtxMemoryMutation(action)) {
                    return err("Error: ctx_memory mutation requires a stable tool-call identity.");
                }
                const client = deps.kernelClient({
                    sessionId,
                    projectRoot: resolveProjectRootDirectory(ctx.cwd),
                });
                const text = await executeCtxMemory({
                    client,
                    args,
                    action,
                    identity: { sessionId, toolCallId: toolCallId || "read" },
                    actor: CTX_MEMORY_ACTOR,
                    ...(signal ? { signal } : {}),
                });
                return text.startsWith("Error:") ? err(text) : ok(text);
            } catch (error) {
                if (error instanceof MemoryInputError) {
                    return err(`Error: ${error.message}`);
                }
                throw error;
            }
        },
    };
}
