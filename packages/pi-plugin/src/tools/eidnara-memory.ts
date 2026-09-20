import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { resolveProjectRootDirectory } from "@eidnara/opencode/features/context/project-identity";
import { MemoryInputError } from "@eidnara/opencode/shared/kernel-client/anti-memory";
import {
    EIDNARA_MEMORY_DESCRIPTION,
    EIDNARA_MEMORY_TOOL_NAME,
    EIDNARA_MEMORY_UNWRAP_RULES,
    WRITABLE_MEMORY_CATEGORIES,
} from "@eidnara/opencode/tools/eidnara-memory/constants";
import {
    EIDNARA_MEMORY_ACTOR,
    executeEidnaraMemory,
} from "@eidnara/opencode/tools/eidnara-memory/execute";
import {
    EIDNARA_MEMORY_ACTIONS,
    type EidnaraMemoryAction,
    type EidnaraMemoryArgs,
    isEidnaraMemoryMutation,
    type KernelClientResolver,
} from "@eidnara/opencode/tools/eidnara-memory/types";
import {
    assertEidnaraMemoryWriteShape,
    requireTaxonomyCategory,
} from "@eidnara/opencode/tools/eidnara-memory/write-shape";
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
            Type.String({
                enum: EIDNARA_MEMORY_ACTIONS,
                description: "create, get, revise, archive, or merge",
            }),
        ),
        content: Type.Optional(
            Type.String({ description: "Memory content for create/revise/merge" }),
        ),
        category: Type.Optional(
            Type.String({
                enum: WRITABLE_MEMORY_CATEGORIES,
                description: `Memory category for create/revise/merge: ${WRITABLE_MEMORY_CATEGORIES.join(", ")}.`,
            }),
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

type EidnaraMemoryParams = Static<typeof ParamsSchema>;

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

export interface EidnaraMemoryToolDeps {
    /** Resolves the client bound to the calling session and filesystem project root. */
    kernelClient: KernelClientResolver;
    resolveProjectPath: (directory: string) => string | undefined;
}

export function createEidnaraMemoryTool(
    deps: EidnaraMemoryToolDeps,
): ToolDefinition<typeof ParamsSchema> {
    return {
        name: EIDNARA_MEMORY_TOOL_NAME,
        label: "Eidnara: Memory",
        description: EIDNARA_MEMORY_DESCRIPTION,
        parameters: ParamsSchema,
        prepareArguments(raw) {
            if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
                throw new MemoryInputError("eidnara_memory arguments must be an object");
            }
            const params = unwrapImitatedReducedArgs(raw, ["action"], EIDNARA_MEMORY_UNWRAP_RULES);
            // Pi's schema error omits enum choices. Report the domain error before validation.
            if ("category" in params && typeof params.category === "string") {
                requireTaxonomyCategory(params.category);
            }
            // Pi validates the complete schema immediately after argument preparation.
            return params as EidnaraMemoryParams;
        },
        async execute(toolCallId, rawParams, signal, _onUpdate, ctx) {
            try {
                let params = rawParams as EidnaraMemoryParams & EidnaraMemoryArgs;
                params = unwrapImitatedReducedArgs(params, ["action"], EIDNARA_MEMORY_UNWRAP_RULES);
                const rawAction = (params as { action?: unknown }).action;
                if (rawAction === "approve" || rawAction === "enforce") {
                    return err(
                        "Error: approve and enforce are human-host-owned commands, not agent actions.",
                    );
                }
                if (
                    typeof rawAction !== "string" ||
                    !(EIDNARA_MEMORY_ACTIONS as readonly string[]).includes(rawAction)
                ) {
                    return err(
                        `Error: Action '${String(rawAction)}' is not allowed in this context.`,
                    );
                }
                const action = rawAction as EidnaraMemoryAction;
                const args: EidnaraMemoryArgs = { ...params, action };
                assertEidnaraMemoryWriteShape(args);
                const projectIdentity = deps.resolveProjectPath(ctx.cwd);
                if (!projectIdentity) {
                    return err("Error: Could not resolve project identity for memory action.");
                }
                const sessionId = ctx.sessionManager.getSessionId();
                if (!sessionId) {
                    return err("Error: eidnara_memory requires an active session.");
                }
                if (!toolCallId && isEidnaraMemoryMutation(action)) {
                    return err(
                        "Error: eidnara_memory mutation requires a stable tool-call identity.",
                    );
                }
                const client = deps.kernelClient({
                    sessionId,
                    projectRoot: resolveProjectRootDirectory(ctx.cwd),
                });
                const text = await executeEidnaraMemory({
                    client,
                    args,
                    action,
                    identity: { sessionId, toolCallId: toolCallId || "read" },
                    actor: EIDNARA_MEMORY_ACTOR,
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
