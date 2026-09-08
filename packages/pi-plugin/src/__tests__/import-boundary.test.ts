import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const packageRoot = join(import.meta.dir, "..", "..");

describe("import boundary", () => {
    test("biome rejects bare @eidnara/opencode and @eidnara/pi specifiers", () => {
        const biome = JSON.parse(readFileSync(join(packageRoot, "biome.json"), "utf8")) as {
            linter: {
                rules: {
                    style: {
                        noRestrictedImports: {
                            level: string;
                            options: { paths: Record<string, string> };
                        };
                    };
                };
            };
        };
        const rule = biome.linter.rules.style.noRestrictedImports;
        expect(rule.level).toBe("error");
        expect(Object.keys(rule.options.paths).sort()).toEqual([
            "@eidnara/opencode",
            "@eidnara/pi",
        ]);
    });

    test("the @eidnara/opencode/* alias points at the OpenCode package source", () => {
        const tsconfig = JSON.parse(readFileSync(join(packageRoot, "tsconfig.json"), "utf8")) as {
            compilerOptions: { paths: Record<string, string[]>; noEmit: boolean };
        };
        expect(tsconfig.compilerOptions.paths["@eidnara/opencode/*"]).toEqual([
            "../opencode-plugin/src/*",
        ]);
        expect(tsconfig.compilerOptions.noEmit).toBe(true);
    });
});
