#!/usr/bin/env bun
/**
 *
 * `EidnaraConfigSchema` in `src/config/schema/eidnara.ts` is the source of truth for this JSON Schema.
 *
 * This script generates the output from `EidnaraConfigSchema`, including its `.describe(...)` calls; do not hand-edit the output.
 *
 * Output: assets/eidnara.schema.json
 */

import * as path from "node:path";
import { z } from "zod";
import { isValidLanguageCode } from "../src/agents/language-directive";
import { EidnaraConfigSchema, LanguageCodeSchema } from "../src/config/schema/eidnara";

const SCHEMA_ID = "https://raw.githubusercontent.com/ahrav/eidnara/main/assets/eidnara.schema.json";

/**
 * The runtime check consults ICU, which JSON Schema cannot call, so the accepted set is expanded
 * from the same predicate into a case-insensitive alternation that keeps the runtime's
 * whitespace handling.
 */
export function languageCodePattern(): string {
    const alternatives: string[] = [];
    for (let first = 97; first <= 122; first++) {
        for (let second = 97; second <= 122; second++) {
            const code = String.fromCharCode(first) + String.fromCharCode(second);
            if (!isValidLanguageCode(code)) continue;
            const [a, b] = code;
            alternatives.push(`[${a}${a.toUpperCase()}][${b}${b.toUpperCase()}]`);
        }
    }
    return `^\\s*(?:${alternatives.join("|")})\\s*$`;
}

/**
 * The loader reads these top-level keys from raw configuration, not `EidnaraConfigSchema`.
 * The root is closed with `additionalProperties: false`, so publishing each loader-only key
 * prevents editors from rejecting configuration that the loader accepts.
 */
const LOADER_ONLY_PROPERTIES: Record<string, unknown> = {
    $schema: {
        type: "string",
        description: "JSON Schema reference for editor validation and autocomplete.",
    },
    disabled_hooks: {
        type: "array",
        items: { type: "string" },
        description:
            "Hook IDs to disable. User and project values are union-merged, so a project can only add to the user's list.",
    },
    command: {
        type: "object",
        description: "Custom slash commands keyed by command name.",
        additionalProperties: {
            type: "object",
            properties: {
                template: { type: "string", description: "Prompt template for the command." },
                description: { type: "string", description: "Command description." },
                agent: { type: "string", description: "Agent that runs the command." },
                model: { type: "string", description: "Model override for the command." },
                subtask: {
                    type: "boolean",
                    description: "Run the command as a subtask.",
                },
            },
            required: ["template"],
            additionalProperties: false,
        },
    },
};

export function buildSchema(): Record<string, unknown> {
    // The generator uses `io: "input"` so optional and defaulted fields describe accepted JSONC input rather than `.transform` output.
    // (the `.transform` output shape is irrelevant to what a user may write).
    const generated = z.toJSONSchema(EidnaraConfigSchema, {
        target: "draft-7",
        io: "input",
        override: (ctx) => {
            if (ctx.zodSchema === LanguageCodeSchema) {
                ctx.jsonSchema.pattern = languageCodePattern();
            }
        },
    }) as Record<string, unknown>;

    delete generated.$schema;

    const properties = (generated.properties ?? {}) as Record<string, unknown>;

    for (const [key, definition] of Object.entries(LOADER_ONLY_PROPERTIES)) {
        if (!(key in properties)) {
            properties[key] = definition;
        }
    }

    return {
        $schema: "http://json-schema.org/draft-07/schema#",
        $id: SCHEMA_ID,
        title: "Eidnara Configuration",
        description:
            "Configuration schema for the @eidnara/opencode plugin. Place as .eidnara/eidnara.jsonc in your project root or eidnara.jsonc under ~/.config/eidnara/.",
        ...generated,
        properties,
        additionalProperties: false,
    };
}

async function main() {
    const rootDir = path.resolve(import.meta.dir, "..", "..", "..");
    const assetsDir = path.join(rootDir, "assets");
    const outputPath = path.join(assetsDir, "eidnara.schema.json");

    const fs = await import("node:fs");
    if (!fs.existsSync(assetsDir)) {
        fs.mkdirSync(assetsDir, { recursive: true });
    }

    const schema = buildSchema();
    await Bun.write(outputPath, `${JSON.stringify(schema, null, 2)}\n`);
    console.log(`✓ JSON Schema generated: ${outputPath}`);
}

if (import.meta.main) {
    void main();
}
