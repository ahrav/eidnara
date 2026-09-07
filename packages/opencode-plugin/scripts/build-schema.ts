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
import { EidnaraConfigSchema } from "../src/config/schema/eidnara";

const SCHEMA_ID = "https://raw.githubusercontent.com/ahrav/eidnara/main/assets/eidnara.schema.json";

export function buildSchema(): Record<string, unknown> {
    // The generator uses `io: "input"` so optional and defaulted fields describe accepted JSONC input rather than `.transform` output.
    // (the `.transform` output shape is irrelevant to what a user may write).
    const generated = z.toJSONSchema(EidnaraConfigSchema, {
        target: "draft-7",
        io: "input",
    }) as Record<string, unknown>;

    delete generated.$schema;

    const properties = (generated.properties ?? {}) as Record<string, unknown>;

    // The generated schema allows `$schema` for editor validation and autocomplete although `EidnaraConfigSchema` does not define it.
    if (!("$schema" in properties)) {
        properties.$schema = {
            type: "string",
            description: "JSON Schema reference for editor validation and autocomplete.",
        };
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
