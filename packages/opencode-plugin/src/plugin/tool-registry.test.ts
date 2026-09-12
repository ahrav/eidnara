/// <reference types="bun-types" />

import { afterEach, describe, expect, it } from "bun:test";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
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
const A1_GUIDANCE_SECTION_HEADING = "## 1. System-prompt guidance section";
const DAEMON_GUIDANCE_ASSETS = [
    ["PRIMARY full (reduce=on)", "guidance_primary.txt"],
    ["PRIMARY full (reduce=off)", "guidance_no_reduce.txt"],
    ["PRIMARY light (reduce=on)", "guidance_light_primary.txt"],
    ["PRIMARY light (reduce=off)", "guidance_light_no_reduce.txt"],
] as const;

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

    it("compaction-off tool set = mode-on tool set minus exactly the reduce factory's IDs, with the other tools' fields intact", () => {
        const modeOn = buildRegistry({});
        const modeOff = buildRegistry({ compaction: { enabled: false } as never });

        const onIds = new Set(Object.keys(modeOn));
        const offIds = new Set(Object.keys(modeOff));

        const removed = [...onIds].filter((id) => !offIds.has(id));
        const added = [...offIds].filter((id) => !onIds.has(id));
        expect(removed.sort()).toEqual([...COMPACTION_OFF_REMOVED_TOOL_IDS].sort());
        expect(added).toEqual([]);

        // Every other ctx_* tool stays registered (subject to its own gates).
        expect(modeOff.ctx_reduce).toBeUndefined();
        expect(Object.keys(modeOff).sort()).toEqual(["ctx_memory", "ctx_note", "ctx_search"]);
        // ctx_search still advertises its fields — the reduce factory was
        // skipped, not the search factory.
        const searchSchema = tool.schema.toJSONSchema(
            tool.schema.object(modeOff.ctx_search?.args ?? {}),
        ) as { properties?: Record<string, unknown> };
        expect(Object.keys(searchSchema.properties ?? {})).toContain("query");
    });

    it("compaction { enabled: true } is identical to default (back-compat)", () => {
        const implicit = buildRegistry({});
        const explicit = buildRegistry({ compaction: { enabled: true } as never });
        expect(Object.keys(explicit).sort()).toEqual(Object.keys(implicit).sort());
        expect(Object.keys(explicit)).toContain("ctx_reduce");
    });
});

type GoldenTool = { description: string; parameters: Record<string, unknown> };
type GoldenHashBaseline = { bytes: number; md5: string };
type JsonSchemaNode = {
    type?: string;
    enum?: unknown[];
    items?: JsonSchemaNode;
    properties?: Record<string, JsonSchemaNode>;
};

function readA1GoldenGuidance(document: string): Record<string, string> {
    const guidanceSection = document.slice(
        a1GoldenSectionOffset(document, A1_GUIDANCE_SECTION_HEADING),
        a1GoldenSectionOffset(document, A1_TOOL_SECTION_HEADING),
    );
    return Object.fromEntries(
        [
            ...guidanceSection.matchAll(
                /^### (.+?): \d+ chars, ~\d+ tokens\n\n```markdown\n([\s\S]*?)\n```$/gm,
            ),
        ].map((match) => [match[1], match[2]]),
    );
}

function readA1GoldenHashBaselines(document: string): Record<string, GoldenHashBaseline> {
    const hashSection = document.slice(a1GoldenSectionOffset(document, A1_HASH_BASELINE_HEADING));
    return Object.fromEntries(
        [...hashSection.matchAll(/^\| ([^|]+?) \| (\d+) \| `([0-9a-f]{32})` \|$/gm)].map(
            (match) => [match[1], { bytes: Number(match[2]), md5: match[3] }],
        ),
    );
}

function readA1GoldenTools(): Record<string, GoldenTool> {
    const document = readA1GoldenDocument();
    const toolSection = document.slice(
        a1GoldenSectionOffset(document, A1_TOOL_SECTION_HEADING),
        a1GoldenSectionOffset(document, A1_HASH_BASELINE_HEADING),
    );
    const headings = [...toolSection.matchAll(/^### (ctx_[a-z_]+) —.*$/gm)];
    return Object.fromEntries(
        headings.map((heading, index) => {
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
        }),
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

describe("A1 prompt-surface golden", () => {
    it("matches current daemon guidance assets and their hash baselines", () => {
        const document = readA1GoldenDocument();
        const guidance = readA1GoldenGuidance(document);
        const baselines = readA1GoldenHashBaselines(document);
        const expectedVariants = DAEMON_GUIDANCE_ASSETS.map(([variant]) => variant);

        expect(Object.keys(guidance)).toEqual(expectedVariants);
        expect(Object.keys(baselines)).toEqual(expectedVariants);
        for (const [variant, asset] of DAEMON_GUIDANCE_ASSETS) {
            const source = readFileSync(
                join(import.meta.dir, "../../../../crates/daemon/assets", asset),
                "utf8",
            );
            expect(
                source.endsWith("\n") && !source.endsWith("\n\n"),
                `${asset} must end with exactly one source newline`,
            ).toBe(true);
            const composedGuidance = [source.slice(0, -1)].join("\n");

            expect(guidance[variant]).toBe(composedGuidance);
            expect(baselines[variant]).toEqual({
                bytes: Buffer.byteLength(composedGuidance),
                md5: createHash("md5").update(composedGuidance, "utf8").digest("hex"),
            });
        }
    });

    it("keeps guidance within the registered memory address and search-source contracts", () => {
        const registry = buildRegistry({});
        const memory = registry.ctx_memory;
        const search = registry.ctx_search;
        expect(memory).toBeDefined();
        expect(search).toBeDefined();

        const memorySchema = tool.schema.toJSONSchema(
            tool.schema.object(memory?.args ?? {}),
        ) as JsonSchemaNode;
        expect(memorySchema.properties?.objectId?.type).toBe("string");
        expect(memorySchema.properties?.objectIds?.type).toBe("array");
        expect(memorySchema.properties?.objectIds?.items?.type).toBe("string");

        const objectIdShape = memory?.description.match(/mem_<32hex>/)?.[0];
        expect(objectIdShape).toBe("mem_<32hex>");

        const searchSchema = tool.schema.toJSONSchema(
            tool.schema.object(search?.args ?? {}),
        ) as JsonSchemaNode;
        const searchSources = searchSchema.properties?.sources?.items?.enum;
        expect(searchSources).toEqual(["memory"]);

        for (const [, asset] of DAEMON_GUIDANCE_ASSETS) {
            const guidance = readFileSync(
                join(import.meta.dir, "../../../../crates/daemon/assets", asset),
                "utf8",
            );
            expect(guidance).toContain(`\`${objectIdShape}\``);
            expect(guidance).toContain("`objectId`/`objectIds`");
            expect(guidance).toMatch(/No other ID form exists\./);

            // `<project-memory>` lines render kernel object IDs, so the guidance must
            // present them as direct `objectId` handles.
            const projectMemoryLine = guidance
                .split("\n")
                .find((line) => line.includes("`<project-memory>`") && line.includes("ID"));
            expect(projectMemoryLine).toContain("`mem_<32hex>`");
            expect(projectMemoryLine).toMatch(/use them directly/);

            const sourceScopeLine = guidance
                .split("\n")
                .find((line) => line.includes("`sources` permits only `memory`"));
            expect(sourceScopeLine).toContain("only `memory`");
            expect(sourceScopeLine).toMatch(/notes or (summarized history|summaries)/);
            expect(sourceScopeLine).toContain("`ctx_expand`");
            expect(sourceScopeLine).toContain("When `ctx_expand` is registered");
            expect(sourceScopeLine).toContain("`## start-end · date · title`");
            expect(sourceScopeLine).toContain("`<session-history>`");
        }
    });
});

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
