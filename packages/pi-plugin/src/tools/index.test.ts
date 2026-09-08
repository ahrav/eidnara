import { describe, expect, it } from "bun:test";
import {
    A1_HASH_BASELINE_HEADING,
    A1_TOOL_SECTION_HEADING,
    a1GoldenSectionOffset,
    readA1GoldenDocument,
} from "@eidnara/opencode/shared/prompt-surface-a1-golden";
import {
    createPromptSurfaceRuntime,
    LIGHT_TOOL_DESCRIPTIONS,
} from "@eidnara/opencode/shared/prompt-surface-runtime";
import { fakeKernelResolver } from "../__tests__/test-utils";
import { registerEidnaraTools } from "./index";

const kernelClient = fakeKernelResolver().kernelClient;
const baseOptions = { kernelClient, rustToolBackends: {} };

describe("registerEidnaraTools", () => {
    it("can omit ctx_memory for retrieval-only sidekick subagents", () => {
        const registered: string[] = [];
        const commands: string[] = [];
        const pi = {
            registerTool: (tool: { name: string }) => {
                registered.push(tool.name);
            },
            registerCommand: (name: string) => {
                commands.push(name);
            },
        } as never;

        registerEidnaraTools(pi, {
            ...baseOptions,
            memoryToolEnabled: false,
            sessionScopedToolsDisabled: true,
            todowriteCommandEnabled: false,
        });

        expect(registered).toEqual(["ctx_search", "todowrite"]);
        expect(commands).not.toContain("todos");
    });

    it("removes only ctx_reduce in compaction-off mode", () => {
        const registered: string[] = [];
        const pi = {
            registerTool: (tool: { name: string }) => registered.push(tool.name),
            registerCommand: () => undefined,
        } as never;
        registerEidnaraTools(pi, { ...baseOptions, compactionOff: true });

        expect(registered).toEqual(["ctx_search", "ctx_memory", "ctx_note", "todowrite"]);
    });

    it("registers exactly the daemon-backed tool set by default", () => {
        const registered: string[] = [];
        const pi = {
            registerTool: (tool: { name: string }) => registered.push(tool.name),
            registerCommand: () => undefined,
        } as never;
        registerEidnaraTools(pi, baseOptions);

        expect(registered).toEqual([
            "ctx_search",
            "ctx_memory",
            "ctx_note",
            "todowrite",
            "ctx_reduce",
        ]);
    });

    it("advertises only real ctx_* fields and allows additional properties", () => {
        const registered = new Map<
            string,
            {
                name: string;
                parameters: {
                    properties?: Record<string, unknown>;
                    additionalProperties?: unknown;
                };
            }
        >();
        const pi = {
            registerTool: (tool: {
                name: string;
                parameters: {
                    properties?: Record<string, unknown>;
                    additionalProperties?: unknown;
                };
            }) => registered.set(tool.name, tool),
            registerCommand: () => undefined,
        } as never;

        registerEidnaraTools(pi, baseOptions);

        const expectedFields: Record<string, string[]> = {
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
            ctx_note: [
                "action",
                "content",
                "surface_condition",
                "note_id",
                "filter",
                "limit",
                "offset",
            ],
            ctx_reduce: ["drop"],
        };
        for (const [name, fields] of Object.entries(expectedFields)) {
            const definition = registered.get(name);
            expect(definition).toBeDefined();
            expect(Object.keys(definition?.parameters.properties ?? {}).sort()).toEqual(
                [...fields].sort(),
            );
            expect(definition?.parameters.properties).not.toHaveProperty("reduced");
            expect(definition?.parameters.properties).not.toHaveProperty("summary");
            expect(definition?.parameters.additionalProperties).toBe(true);
        }
    });

    it("registered ctx_note resolves the project identity from the invocation cwd", async () => {
        const registered = new Map<string, { execute: (...args: never[]) => unknown }>();
        const pi = {
            registerTool: (tool: { name: string; execute: (...args: never[]) => unknown }) => {
                registered.set(tool.name, tool);
            },
            registerCommand: () => undefined,
        } as never;
        const requests: Array<{ memoryProject: string; projectRoot: string }> = [];

        registerEidnaraTools(pi, {
            kernelClient,
            rustToolBackends: {
                note: async (request) => {
                    requests.push(request);
                    return "Saved session note #1.";
                },
            },
            resolveProjectIdentity: (ctx) =>
                ctx.cwd === "/tmp/project-b" ? "git:project-b" : undefined,
        });

        const noteTool = registered.get("ctx_note");
        expect(noteTool).toBeDefined();
        const result = await noteTool?.execute(
            "call-1" as never,
            { action: "write", content: "Project B note" } as never,
            new AbortController().signal as never,
            undefined as never,
            {
                cwd: "/tmp/project-b",
                sessionManager: { getSessionId: () => "ses-tool-cd" },
            } as never,
        );

        expect((result as { isError?: boolean } | undefined)?.isError).toBeUndefined();
        expect(requests).toEqual([
            expect.objectContaining({
                memoryProject: "git:project-b",
                projectRoot: "/tmp/project-b",
            }),
        ]);
    });

    it("registers todowrite and /todos by default", () => {
        const registered: string[] = [];
        const commands: string[] = [];
        const pi = {
            registerTool: (tool: { name: string }) => registered.push(tool.name),
            registerCommand: (name: string) => commands.push(name),
        } as never;

        registerEidnaraTools(pi, baseOptions);

        expect(registered).toContain("todowrite");
        expect(commands).toContain("todos");
    });

    it("omits todowrite and /todos when todowrite is disabled", () => {
        const registered: string[] = [];
        const commands: string[] = [];
        const pi = {
            registerTool: (tool: { name: string }) => registered.push(tool.name),
            registerCommand: (name: string) => commands.push(name),
        } as never;

        registerEidnaraTools(pi, { ...baseOptions, todowriteEnabled: false });

        expect(registered).toContain("ctx_search");
        expect(registered).not.toContain("todowrite");
        expect(commands).not.toContain("todos");
    });

    it("can keep /todos off for lean subagent entries", () => {
        const registered: string[] = [];
        const commands: string[] = [];
        const pi = {
            registerTool: (tool: { name: string }) => registered.push(tool.name),
            registerCommand: (name: string) => commands.push(name),
        } as never;

        registerEidnaraTools(pi, { ...baseOptions, todowriteCommandEnabled: false });

        expect(registered).toContain("todowrite");
        expect(commands).not.toContain("todos");
    });
});

type RegisteredPromptTool = {
    name: string;
    description: string;
    parameters: { properties?: Record<string, unknown> };
};

function readA1GoldenTools(): Record<
    string,
    { description: string; parameters: Record<string, unknown> }
> {
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

/** Tool ids the prompt-surface catalog names but neither adapter builds. */
const UNREGISTERED_CATALOG_TOOL_IDS = new Set<string>(["ctx_expand"]);

function captureRegisteredTools(
    options: Parameters<typeof registerEidnaraTools>[1],
): Map<string, RegisteredPromptTool> {
    const registered = new Map<string, RegisteredPromptTool>();
    const pi = {
        registerTool: (tool: RegisteredPromptTool) => registered.set(tool.name, tool),
        registerCommand: () => undefined,
    } as never;
    registerEidnaraTools(pi, options);
    return registered;
}

describe("registerEidnaraTools — prompt-surface registration", () => {
    it("matches the A1 golden for no config and explicit full", () => {
        const golden = readA1GoldenTools();
        const implicit = captureRegisteredTools(baseOptions);
        const explicit = captureRegisteredTools({
            ...baseOptions,
            promptSurface: { default: "full" },
        });
        const implicitIds = [...implicit.keys()].filter((id) => id.startsWith("ctx_"));
        const explicitIds = [...explicit.keys()].filter((id) => id.startsWith("ctx_"));

        expect(implicitIds.sort()).toEqual(Object.keys(golden).sort());
        expect(explicitIds.sort()).toEqual(Object.keys(golden).sort());
        for (const [toolId, expected] of Object.entries(golden)) {
            expect(implicit.get(toolId)?.description).toBe(expected.description);
            expect(explicit.get(toolId)?.description).toBe(expected.description);
            expect(explicit.get(toolId)?.parameters).toEqual(implicit.get(toolId)?.parameters);
            expect(Object.keys(implicit.get(toolId)?.parameters.properties ?? {}).sort()).toEqual(
                Object.keys(expected.parameters).sort(),
            );
        }
    });

    it("registers built-in light descriptions without changing parameter schemas", () => {
        const full = captureRegisteredTools(baseOptions);
        const light = captureRegisteredTools({
            ...baseOptions,
            promptSurface: { default: "light" },
        });
        const registeredIds = [...full.keys()].filter((id) => id.startsWith("ctx_"));
        expect(registeredIds.sort()).toEqual([
            "ctx_memory",
            "ctx_note",
            "ctx_reduce",
            "ctx_search",
        ]);
        for (const toolId of registeredIds) {
            expect(light.get(toolId)?.description).toBe(
                LIGHT_TOOL_DESCRIPTIONS[toolId as keyof typeof LIGHT_TOOL_DESCRIPTIONS],
            );
            expect(light.get(toolId)?.parameters).toEqual(full.get(toolId)?.parameters);
        }
    });

    it("applies top-level overrides once without changing parameter schemas", () => {
        const warnings: string[] = [];
        const baseline = captureRegisteredTools(baseOptions);
        const runtime = createPromptSurfaceRuntime({
            userConfigDirectory: process.cwd(),
            warn: (warning) => warnings.push(warning),
        });
        const overridden = captureRegisteredTools({
            ...baseOptions,
            promptSurface: {
                default: "full",
                models: { "provider/model": "light" },
                tool_descriptions: { ctx_search: "Pi custom search surface" },
            },
            promptSurfaceRuntime: runtime,
        });

        expect(overridden.get("ctx_search")?.description).toBe("Pi custom search surface");
        expect(overridden.get("ctx_reduce")?.description).toBe(
            baseline.get("ctx_reduce")?.description,
        );
        for (const toolId of [...baseline.keys()].filter((id) => id.startsWith("ctx_"))) {
            expect(overridden.get(toolId)?.parameters).toEqual(baseline.get(toolId)?.parameters);
        }
        expect(warnings).toEqual([]);
    });
});
