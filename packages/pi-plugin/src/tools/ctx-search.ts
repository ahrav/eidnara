import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { CTX_SEARCH_DESCRIPTION } from "@eidnara/opencode/tools/ctx-search/constants";
import { executeCtxSearch } from "@eidnara/opencode/tools/ctx-search/execute";
import { normalizeCtxSearchArgs } from "@eidnara/opencode/tools/ctx-search/query-input";
import type { CtxSearchToolDeps } from "@eidnara/opencode/tools/ctx-search/types";
import { type Static, Type } from "typebox";

const ParamsSchema = Type.Object(
    {
        query: Type.Optional(
            Type.String({
                description:
                    "Search query. Matches project memories served by the memory daemon. A query made only of memory object ids (mem_<32hex>) resolves those memories directly.",
            }),
        ),
        limit: Type.Optional(
            Type.Number({
                description: "Maximum results to return (default: 10)",
            }),
        ),
        sources: Type.Optional(
            Type.Array(Type.Union([Type.Literal("memory")]), {
                description:
                    'Optional. Restrict to specific sources. ["memory"] searches the project memories served by the memory daemon. Omit for all enabled sources; pass [] to search no sources.',
            }),
        ),
    },
    { additionalProperties: true },
);

type CtxSearchParams = Static<typeof ParamsSchema>;

export type { CtxSearchToolDeps };

export function createCtxSearchTool(deps: CtxSearchToolDeps): ToolDefinition<typeof ParamsSchema> {
    return {
        name: "ctx_search",
        label: "Eidnara: Search",
        description: CTX_SEARCH_DESCRIPTION,
        parameters: ParamsSchema,
        async execute(_toolCallId, params: CtxSearchParams, signal, _onUpdate, ctx) {
            params = normalizeCtxSearchArgs(params);
            const execution = await executeCtxSearch(deps, params, {
                sessionID: ctx.sessionManager.getSessionId(),
                directory: ctx.cwd,
                ...(signal ? { abort: signal } : {}),
            });
            return {
                content: [{ type: "text", text: execution.text }],
                details: undefined,
                ...(execution.status === "invalid" ? { isError: true } : {}),
            };
        },
    };
}
