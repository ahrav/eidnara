import { unwrapImitatedReducedArgs } from "../unwrap-imitated-reduced-args";
import { type ExplicitQueryPreparation, prepareExplicitQuery } from "./bounds";
import type { EidnaraSearchArgs } from "./types";

export function normalizeEidnaraSearchArgs(rawArgs: EidnaraSearchArgs): EidnaraSearchArgs {
    return unwrapImitatedReducedArgs(rawArgs, ["query"], {
        query: "string",
        limit: "number",
        sources: {
            type: "array",
            items: "string",
            maxItems: 5,
            values: ["memory"],
        },
    });
}

/**
 * */
export function prepareQueryFromNormalizedArgs(args: EidnaraSearchArgs): ExplicitQueryPreparation {
    return prepareExplicitQuery(typeof args.query === "string" ? args.query : "");
}

/**
 * */
export function extractEidnaraSearchQueryInput(
    rawArgs: EidnaraSearchArgs,
): ExplicitQueryPreparation {
    return prepareQueryFromNormalizedArgs(normalizeEidnaraSearchArgs(rawArgs));
}
