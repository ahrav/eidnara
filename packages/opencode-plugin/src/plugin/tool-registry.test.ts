/// <reference types="bun-types" />

import { afterEach, describe, expect, it } from "bun:test";
import { type ToolDefinition, tool } from "@opencode-ai/plugin";
import type { EidnaraPluginConfig } from "../config";
import { resetCtxReduceRegisteredGloballyForTest } from "../hooks/context/ctx-reduce-availability";
import {
    A1_HASH_BASELINE_HEADING,
    A1_TOOL_SECTION_HEADING,
    a1GoldenSectionOffset,
    readA1GoldenDocument,
} from "../shared/prompt-surface-a1-golden";
import type { PromptSurfaceRuntime, PromptSurfaceToolId } from "../shared/prompt-surface-runtime";
import {
    ACTIVE_TOOL_IDS,
    createPromptSurfaceRuntime,
    LIGHT_TOOL_DESCRIPTIONS,
} from "../shared/prompt-surface-runtime";
import type { RustToolBackends } from "./rust-tool-backends";
import { createToolRegistry, getCompactionOffRemovedToolIds } from "./tool-registry";

afterEach(() => {
    // The compaction-off override is process-global and boot-resolved; reset
    // to the default-true baseline so a compaction-off test cannot leak a
    // false verdict into a later test in the same bun process.
    resetCtxReduceRegisteredGloballyForTest();
});

/** Tool ids the prompt-surface catalog names but this registry never builds. */
const UNREGISTERED_CATALOG_TOOL_IDS = new Set<string>(["ctx_expand"]);

function registeredCatalogToolIds(): PromptSurfaceToolId[] {
    return ACTIVE_TOOL_IDS.filter((id) => !UNREGISTERED_CATALOG_TOOL_IDS.has(id));
}

const REGISTERED_TOOL_IDS = ["ctx_reduce", "ctx_search", "ctx_note", "ctx_memory"] as const;

function buildRegistry(
    config: Partial<EidnaraPluginConfig>,
    rustToolBackends: RustToolBackends = {},
    promptSurfaceRuntime?: PromptSurfaceRuntime,
    registrationPromptSurface?: EidnaraPluginConfig["prompt_surface"],
): Record<string, ToolDefinition> {
    return createToolRegistry({
        pluginConfig: { enabled: true, ...config } as EidnaraPluginConfig,
        rustToolBackends,
        promptSurfaceRuntime,
        registrationPromptSurface,
    });
}

describe("createToolRegistry — registered tool set", () => {
    it("registers exactly the four daemon-backed ctx_* tools and never ctx_expand", () => {
        const tools = buildRegistry({});
        expect(Object.keys(tools).sort()).toEqual([...REGISTERED_TOOL_IDS].sort());
        expect(tools.ctx_expand).toBeUndefined();
    });

    it("registers nothing when the plugin is disabled", () => {
        expect(buildRegistry({ enabled: false })).toEqual({});
    });

    it("registers ctx_memory regardless of memory.enabled", () => {
        const tools = buildRegistry({ memory: { enabled: false } as never });
        expect(Object.keys(tools)).toContain("ctx_memory");
        expect(Object.keys(tools)).toContain("ctx_search");
    });

    it("advertises only real ctx_* fields", () => {
        const tools = buildRegistry({});
        const expectedFields: Record<string, string[]> = {
            ctx_reduce: ["drop"],
            ctx_note: [
                "action",
                "content",
                "surface_condition",
                "filter",
                "limit",
                "offset",
                "note_id",
            ],
            ctx_search: ["query", "limit", "sources"],
            ctx_memory: [
                "action",
                "content",
                "category",
                "antiMemory",
                "objectId",
                "objectIds",
                "reason",
            ],
        };

        for (const [name, fields] of Object.entries(expectedFields)) {
            const definition = tools[name];
            expect(definition).toBeDefined();
            const jsonSchema = tool.schema.toJSONSchema(
                tool.schema.object(definition?.args ?? {}),
            ) as { properties?: Record<string, unknown> };
            expect(Object.keys(jsonSchema.properties ?? {}).sort()).toEqual([...fields].sort());
            expect(jsonSchema.properties).not.toHaveProperty("reduced");
            expect(jsonSchema.properties).not.toHaveProperty("summary");
        }
    });
});

describe("createToolRegistry — compaction-off mode (#266 S4)", () => {
    // Assert the complete removed-ID set so newly gated reduce tools require an explicit expectation.
    const COMPACTION_OFF_REMOVED_TOOL_IDS = getCompactionOffRemovedToolIds();

    it("compaction-off tool set = mode-on tool set minus exactly the reduce factory's IDs", () => {
        const modeOn = buildRegistry({});
        const modeOff = buildRegistry({ compaction: { enabled: false } as never });

        const onIds = new Set(Object.keys(modeOn));
        const offIds = new Set(Object.keys(modeOff));

        const removed = [...onIds].filter((id) => !offIds.has(id));
        const added = [...offIds].filter((id) => !onIds.has(id));
        expect(removed.sort()).toEqual([...COMPACTION_OFF_REMOVED_TOOL_IDS].sort());
        expect(added).toEqual([]);

        // Every other ctx_* tool stays registered (subject to its own gates).
        for (const id of ["ctx_search", "ctx_note", "ctx_memory"]) {
            expect(offIds.has(id)).toBe(true);
        }
    });

    it("compaction-on (default) registers ctx_reduce", () => {
        const tools = buildRegistry({});
        expect(Object.keys(tools)).toContain("ctx_reduce");
    });

    it("compaction { enabled: true } is identical to default (back-compat)", () => {
        const implicit = buildRegistry({});
        const explicit = buildRegistry({ compaction: { enabled: true } as never });
        expect(Object.keys(explicit).sort()).toEqual(Object.keys(implicit).sort());
        expect(Object.keys(explicit)).toContain("ctx_reduce");
    });

    it("compaction-off omits exactly ctx_reduce and keeps the other tools' fields", () => {
        const tools = buildRegistry({ compaction: { enabled: false } as never });
        expect(tools.ctx_reduce).toBeUndefined();
        expect(Object.keys(tools).sort()).toEqual(["ctx_memory", "ctx_note", "ctx_search"]);
        // ctx_search still advertises its fields — the reduce factory was
        // skipped, not the search factory.
        const searchSchema = tool.schema.toJSONSchema(
            tool.schema.object(tools.ctx_search?.args ?? {}),
        ) as { properties?: Record<string, unknown> };
        expect(Object.keys(searchSchema.properties ?? {})).toContain("query");
    });
});

type GoldenTool = { description: string; parameters: Record<string, unknown> };

function readA1GoldenTools(): Record<string, GoldenTool> {
    const document = readA1GoldenDocument();
    const toolSection = document.slice(
        a1GoldenSectionOffset(document, A1_TOOL_SECTION_HEADING),
        a1GoldenSectionOffset(document, A1_HASH_BASELINE_HEADING),
    );
    const headings = [...toolSection.matchAll(/^### (ctx_[a-z_]+) —.*$/gm)];
    return Object.fromEntries(
        headings
            .map((heading, index) => {
                const start = (heading.index ?? 0) + heading[0].length;
                const end = headings[index + 1]?.index ?? toolSection.length;
                const body = toolSection.slice(start, end);
                const description = body.match(/\*\*Description:\*\*\s+```\n([\s\S]*?)\n```/)?.[1];
                const parameters = body.match(
                    /\*\*Parameters \(JSON Schema per parameter, as serialized to the provider\):\*\*\s+```json\n([\s\S]*?)\n```/,
                )?.[1];
                if (description === undefined || parameters === undefined) {
                    throw new Error(`Malformed A1 golden tool section: ${heading[1]}`);
                }
                return [
                    heading[1],
                    {
                        description,
                        parameters: JSON.parse(parameters) as Record<string, unknown>,
                    },
                ] as const;
            })
            .filter(([toolId]) => !UNREGISTERED_CATALOG_TOOL_IDS.has(toolId)),
    );
}

function providerParameters(definition: ToolDefinition): Record<string, unknown> {
    return Object.fromEntries(
        Object.entries(definition.args ?? {}).map(([name, schema]) => {
            const serializable = schema as { _zod?: { toJSONSchema?: () => unknown } };
            return [
                name,
                serializable._zod?.toJSONSchema
                    ? serializable._zod.toJSONSchema()
                    : "<no toJSONSchema>",
            ];
        }),
    );
}

describe("createToolRegistry — prompt-surface registration", () => {
    it("links canonical prompt-surface IDs to light descriptions and registration", () => {
        const registeredCtxToolIds = Object.keys(buildRegistry({})).filter((id) =>
            id.startsWith("ctx_"),
        );
        const canonicalIds = new Set<string>(registeredCatalogToolIds());
        const registeredIds = new Set(registeredCtxToolIds);
        const missing = [...canonicalIds].filter((id) => !registeredIds.has(id));
        const extra = [...registeredIds].filter((id) => !canonicalIds.has(id));
        if (missing.length > 0 || extra.length > 0) {
            throw new Error(
                [
                    "Prompt-surface tool registry drifted from ACTIVE_TOOL_IDS.",
                    `Missing: ${missing.join(", ") || "none"}.`,
                    `Extra: ${extra.join(", ") || "none"}.`,
                    "If this is a new ctx_* tool, also review the Rust prompt-surface list in crates/daemon/src/prompt_surface.rs; cross-language drift is intentionally checked separately.",
                ].join(" "),
            );
        }

        for (const id of registeredCatalogToolIds()) {
            expect(Object.hasOwn(LIGHT_TOOL_DESCRIPTIONS, id)).toBe(true);
            expect(LIGHT_TOOL_DESCRIPTIONS[id].trim().length).toBeGreaterThan(0);
        }
    });

    // The golden includes `ctx_*` descriptions and fields that the tool modules do not emit:
    // ctx_search sources beyond `memory`, a ctx_memory `list` action and `limit` field, and
    // the `packages/plugin/` path in ctx_note. commentlint: allow(JUDGE)
    it("matches the A1 golden for no config and explicit full", () => {
        const golden = readA1GoldenTools();
        const implicit = buildRegistry({});
        const explicit = buildRegistry({
            prompt_surface: { default: "full" },
        } as Partial<EidnaraPluginConfig>);

        expect(Object.keys(implicit).sort()).toEqual(Object.keys(golden).sort());
        expect(Object.keys(explicit).sort()).toEqual(Object.keys(golden).sort());
        for (const [toolId, expected] of Object.entries(golden)) {
            expect(implicit[toolId]?.description).toBe(expected.description);
            expect(explicit[toolId]?.description).toBe(expected.description);
            expect(providerParameters(implicit[toolId])).toEqual(expected.parameters);
            expect(providerParameters(explicit[toolId])).toEqual(expected.parameters);
        }
    });

    it("applies only top-level user descriptions and ignores model routes", () => {
        const warnings: string[] = [];
        const runtime = createPromptSurfaceRuntime({
            userConfigDirectory: process.cwd(),
            warn: (warning) => warnings.push(warning),
        });
        const baseline = buildRegistry({});
        const overridden = buildRegistry(
            {
                prompt_surface: {
                    default: "full",
                    models: { "provider/model": "light" },
                    tool_descriptions: { ctx_search: "Custom search surface" },
                },
            } as Partial<EidnaraPluginConfig>,
            undefined,
            runtime,
        );

        expect(overridden.ctx_search.description).toBe("Custom search surface");
        expect(overridden.ctx_reduce.description).toBe(baseline.ctx_reduce.description);
        for (const toolId of Object.keys(baseline)) {
            expect(providerParameters(overridden[toolId])).toEqual(
                providerParameters(baseline[toolId]),
            );
        }
        expect(warnings).toEqual([]);
    });

    it("registers the built-in light descriptions without changing schemas", () => {
        const warnings: string[] = [];
        const runtime = createPromptSurfaceRuntime({
            userConfigDirectory: process.cwd(),
            warn: (warning) => warnings.push(warning),
        });
        const full = buildRegistry({});
        const light = buildRegistry(
            { prompt_surface: { default: "light" } } as Partial<EidnaraPluginConfig>,
            undefined,
            runtime,
        );

        for (const toolId of Object.keys(LIGHT_TOOL_DESCRIPTIONS)) {
            if (UNREGISTERED_CATALOG_TOOL_IDS.has(toolId)) continue;
            expect(light[toolId]?.description).toBe(
                LIGHT_TOOL_DESCRIPTIONS[toolId as keyof typeof LIGHT_TOOL_DESCRIPTIONS],
            );
            expect(providerParameters(light[toolId])).toEqual(providerParameters(full[toolId]));
        }
        expect(warnings).toEqual([]);
    });
});

describe("createToolRegistry — user-owned registration default", () => {
    it("does not let a project-routed default select process-scoped tool text", () => {
        const warnings: string[] = [];
        const runtime = createPromptSurfaceRuntime({
            userConfigDirectory: process.cwd(),
            warn: (warning) => warnings.push(warning),
        });
        const registry = buildRegistry(
            {
                prompt_surface: {
                    default: "light",
                    tool_descriptions: { ctx_search: "User-owned search description" },
                },
            } as Partial<EidnaraPluginConfig>,
            undefined,
            runtime,
            {
                default: "full",
                tool_descriptions: { ctx_search: "User-owned search description" },
            },
        );

        expect(registry.ctx_search.description).toBe("User-owned search description");
        expect(warnings).toEqual([]);
    });
});
