import { type ToolDefinition, tool } from "@opencode-ai/plugin";
import { EIDNARA_SEARCH_DESCRIPTION, EIDNARA_SEARCH_TOOL_NAME } from "./constants";
import { executeEidnaraSearch } from "./execute";
import type { EidnaraSearchArgs, EidnaraSearchToolDeps } from "./types";

export { EIDNARA_SEARCH_LIGHT_DESCRIPTION } from "../light-descriptions";
export {
    type EidnaraSearchCallContext,
    type EidnaraSearchExecution,
    executeEidnaraSearch,
} from "./execute";

const eidnaraSearchArgsShape = {
    query: tool.schema
        .string()
        .optional()
        .describe(
            "Search query. Matches project memories served by the memory daemon. A query made only of memory object ids (mem_<32hex>) resolves those memories directly.",
        ),
    limit: tool.schema.number().optional().describe("Maximum results to return (default: 10)"),
    sources: tool.schema
        .array(tool.schema.enum(["memory"]))
        .optional()
        .describe(
            'Optional. Restrict to specific sources. ["memory"] searches the project memories served by the memory daemon. Omit for all enabled sources; pass [] to search no sources.',
        ),
};
// The tool definition exposes only the documented argument shape to the model
// Callers may still send extra arguments.
// `passthrough()` lets `execute()` receive fields that the model cannot see in the argument schema.
const eidnaraSearchArgsSchema = tool.schema.object(eidnaraSearchArgsShape).passthrough();

function createEidnaraSearchTool(deps: EidnaraSearchToolDeps): ToolDefinition {
    return tool({
        description: EIDNARA_SEARCH_DESCRIPTION,
        args: eidnaraSearchArgsShape,
        async execute(rawArgs: EidnaraSearchArgs, toolContext) {
            const parsedArgs = eidnaraSearchArgsSchema.safeParse(rawArgs);
            const args = (parsedArgs.success ? parsedArgs.data : rawArgs) as EidnaraSearchArgs;
            const execution = await executeEidnaraSearch(deps, args, toolContext);
            return execution.text;
        },
    });
}

export function createEidnaraSearchTools(
    deps: EidnaraSearchToolDeps,
): Record<string, ToolDefinition> {
    return {
        [EIDNARA_SEARCH_TOOL_NAME]: createEidnaraSearchTool(deps),
    };
}
