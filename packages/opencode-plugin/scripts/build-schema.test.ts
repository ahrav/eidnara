import { describe, expect, test } from "bun:test";
import * as fs from "node:fs";
import * as path from "node:path";
import { buildSchema } from "./build-schema";

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
});
