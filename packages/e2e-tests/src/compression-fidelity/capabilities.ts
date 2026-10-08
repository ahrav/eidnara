export const PINNED_EIDNARA_TOOLS = [
    "eidnara_memory",
    "eidnara_note",
    "eidnara_reduce",
    "eidnara_search",
] as const;
export const PINNED_SEARCH_SOURCES = ["memory"] as const;

const EXACT_EXPANSION = /expand|exact|source_read|transcript|native_source/i;

export interface CapabilitySurface {
    eidnaraTools: string[];
    searchSources: string[];
    exactExpansionTools: string[];
    piTransformAvailable: boolean;
}

interface WireTool {
    name?: unknown;
    input_schema?: {
        properties?: { sources?: { items?: { enum?: unknown } } };
    };
}

export function surfaceOfRequest(
    body: { tools?: unknown },
    piTransformAvailable: boolean,
): CapabilitySurface {
    const tools = (Array.isArray(body.tools) ? body.tools : []) as WireTool[];
    const names = tools.map((tool) => (typeof tool.name === "string" ? tool.name : ""));
    const search = tools.find((tool) => tool.name === "eidnara_search");
    const sources = search?.input_schema?.properties?.sources?.items?.enum;
    return {
        eidnaraTools: names.filter((name) => name.startsWith("eidnara_")).sort(),
        searchSources: Array.isArray(sources) ? sources.map(String).sort() : [],
        exactExpansionTools: names.filter((name) => EXACT_EXPANSION.test(name)),
        piTransformAvailable,
    };
}

export function capabilityDrift(surface: CapabilitySurface): string[] {
    const drift: string[] = [];
    if (surface.eidnaraTools.join() !== [...PINNED_EIDNARA_TOOLS].join()) {
        drift.push(`eidnara tools ${surface.eidnaraTools.join(",") || "none"}`);
    }
    if (surface.searchSources.join() !== [...PINNED_SEARCH_SOURCES].join()) {
        drift.push(`eidnara_search sources ${surface.searchSources.join(",") || "none"}`);
    }
    if (surface.exactExpansionTools.length > 0) {
        drift.push(`exact expansion ${surface.exactExpansionTools.join(",")}`);
    }
    if (!surface.piTransformAvailable) drift.push("pi transform unavailable");
    return drift;
}
