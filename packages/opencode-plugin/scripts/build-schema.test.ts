import { describe, expect, test } from "bun:test";
import * as fs from "node:fs";
import * as path from "node:path";
import { isValidLanguageCode } from "../src/agents/language-directive";
import { PROMPT_SURFACE_MODEL_KEY_PATTERN } from "../src/shared/prompt-surface";
import { buildSchema, languageCodePattern } from "./build-schema";

/**
 *
 */
describe("eidnara JSON schema", () => {
    // Same resolution as `main()` in build-schema.ts: repository root, then assets/.
    const rootDir = path.resolve(import.meta.dir, "..", "..", "..");
    const schemaPath = path.join(rootDir, "assets", "eidnara.schema.json");

    test("committed schema matches generator output (run `bun packages/opencode-plugin/scripts/build-schema.ts` if this fails)", () => {
        const committed = fs.readFileSync(schemaPath, "utf-8");
        const regenerated = `${JSON.stringify(buildSchema(), null, 2)}\n`;
        expect(committed).toBe(regenerated);
    });

    test("every top-level Zod config key appears in the schema", async () => {
        const { EidnaraConfigSchema } = await import("../src/config/schema/eidnara");
        // EidnaraConfigSchema wraps its object shape in `.transform()`.
        const def: any = (EidnaraConfigSchema as any)._def ?? (EidnaraConfigSchema as any).def;
        const inner: any = def?.innerType ?? def?.schema ?? EidnaraConfigSchema;
        const shape =
            (inner as any).shape ?? (inner as any)._def?.shape ?? (inner as any).def?.shape;
        const zodKeys =
            typeof shape === "function" ? Object.keys(shape()) : Object.keys(shape ?? {});

        const schema = JSON.parse(fs.readFileSync(schemaPath, "utf-8")) as {
            properties: Record<string, unknown>;
        };
        const schemaKeys = new Set(Object.keys(schema.properties));

        const missing = zodKeys.filter((k) => !schemaKeys.has(k));
        expect(missing).toEqual([]);
    });

    test("experimental is not a published schema property", () => {
        // The in-memory migration relocates `experimental.*` keys, so the
        // published schema must not document the legacy container.
        const schema = JSON.parse(fs.readFileSync(schemaPath, "utf-8")) as {
            properties: Record<string, unknown>;
        };
        expect(schema.properties.experimental).toBeUndefined();
    });

    test("the published language pattern accepts exactly the codes the runtime accepts", () => {
        const pattern = new RegExp(languageCodePattern());
        let mismatches = 0;
        for (let first = 97; first <= 122; first++) {
            for (let second = 97; second <= 122; second++) {
                const code = String.fromCharCode(first) + String.fromCharCode(second);
                for (const variant of [code, code.toUpperCase(), ` ${code} `]) {
                    if (pattern.test(variant) !== isValidLanguageCode(variant)) mismatches++;
                }
            }
        }
        expect(mismatches).toBe(0);
        expect(pattern.test("zz")).toBe(false);
        expect(pattern.test("english")).toBe(false);
        expect(pattern.test(" TR ")).toBe(true);
    });

    test("loader-only top-level keys are published so the closed root accepts them", () => {
        const schema = buildSchema() as {
            additionalProperties: boolean;
            properties: {
                $schema: { type: string };
                disabled_hooks: { type: string; items: { type: string } };
                command: {
                    type: string;
                    additionalProperties: {
                        required: string[];
                        properties: Record<string, unknown>;
                    };
                };
            };
        };

        expect(schema.additionalProperties).toBe(false);
        expect(schema.properties.$schema.type).toBe("string");
        expect(schema.properties.disabled_hooks).toMatchObject({
            type: "array",
            items: { type: "string" },
        });
        expect(schema.properties.command.type).toBe("object");
        expect(schema.properties.command.additionalProperties.required).toEqual(["template"]);
        expect(Object.keys(schema.properties.command.additionalProperties.properties)).toEqual([
            "template",
            "description",
            "agent",
            "model",
            "subtask",
        ]);
    });

    test("runtime string constraints are published as JSON Schema patterns", () => {
        // `z.toJSONSchema` drops `.refine` callbacks and publishes `.trim().min(1)` as a bare
        // `minLength: 1`, so these constraints must stay expressed as `.regex` checks or
        // editors accept values the loader rejects.
        const schema = buildSchema() as {
            properties: {
                language: { pattern?: string };
                mural: { properties: { model: { pattern?: string; minLength?: number } } };
                models: {
                    properties: { window_overlay_path: { pattern?: string; minLength?: number } };
                };
                subc: {
                    properties: { connection_file: { pattern?: string; minLength?: number } };
                };
                pi: {
                    properties: {
                        subagent_extensions: { items: { pattern?: string; minLength?: number } };
                    };
                };
                prompt_surface: {
                    properties: {
                        models: { propertyNames: { pattern?: string } };
                        guidance_override_path: { pattern?: string };
                        tool_descriptions: {
                            propertyNames: { pattern?: string };
                            additionalProperties: { pattern?: string };
                        };
                    };
                };
            };
        };
        const promptSurface = schema.properties.prompt_surface.properties;

        expect(schema.properties.language.pattern).toBe(languageCodePattern());
        expect(promptSurface.models.propertyNames.pattern).toBe(
            PROMPT_SURFACE_MODEL_KEY_PATTERN.source,
        );
        expect(promptSurface.guidance_override_path.pattern).toBe("\\S");
        expect(promptSurface.tool_descriptions.propertyNames.pattern).toBe("\\S");
        expect(promptSurface.tool_descriptions.additionalProperties.pattern).toBe("\\S");

        // Trimmed-then-non-empty strings: `minLength: 1` alone would admit "   ".
        for (const field of [
            schema.properties.mural.properties.model,
            schema.properties.models.properties.window_overlay_path,
            schema.properties.subc.properties.connection_file,
            schema.properties.pi.properties.subagent_extensions.items,
        ]) {
            expect(field.pattern).toBe("\\S");
            expect(field.minLength).toBeUndefined();
        }
    });
});
