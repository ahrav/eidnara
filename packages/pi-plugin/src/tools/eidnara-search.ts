import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { EIDNARA_SEARCH_DESCRIPTION } from "@eidnara/opencode/tools/eidnara-search/constants";
import { executeEidnaraSearch } from "@eidnara/opencode/tools/eidnara-search/execute";
import { normalizeEidnaraSearchArgs } from "@eidnara/opencode/tools/eidnara-search/query-input";
import type { EidnaraSearchToolDeps } from "@eidnara/opencode/tools/eidnara-search/types";
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

type EidnaraSearchParams = Static<typeof ParamsSchema>;

export type { EidnaraSearchToolDeps };

export function createEidnaraSearchTool(
    deps: EidnaraSearchToolDeps,
): ToolDefinition<typeof ParamsSchema> {
    return {
        name: "eidnara_search",
        label: "Eidnara: Search",
        description: EIDNARA_SEARCH_DESCRIPTION,
        parameters: ParamsSchema,
        async execute(_toolCallId, params: EidnaraSearchParams, signal, _onUpdate, ctx) {
            params = normalizeEidnaraSearchArgs(params);
            const execution = await executeEidnaraSearch(deps, params, {
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
