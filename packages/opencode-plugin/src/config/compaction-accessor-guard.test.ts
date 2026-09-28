import { describe, expect, it } from "bun:test";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, relative, resolve } from "node:path";

// isCompactionEnabled (config/agent-disable.ts) is the ONLY non-schema reader
// Gate sites must use `isCompactionEnabled` rather than read `compaction.enabled` directly.
// Reject direct `compaction.enabled` and `compaction?.enabled` reads outside `ALLOWED_READERS`.
// (packages/cli/src/lib/migration-import-guard.test.ts).
//
// `eidnara.ts` defines `compaction.enabled` and is excluded from this check.

const REPOSITORY_ROOT = resolve(import.meta.dir, "../../../..");
const SOURCE_ROOTS = ["packages/cli/src", "packages/opencode-plugin/src", "packages/pi-plugin/src"];

const ALLOWED_READERS = new Set<string>([
    // `agent-disable.ts` is the permitted non-schema reader.
    "packages/opencode-plugin/src/config/agent-disable.ts",
    // `eidnara.ts` defines `compaction.enabled`.
    "packages/opencode-plugin/src/config/schema/eidnara.ts",
    // `project-security.ts` only names `compaction.enabled` in a warning while deleting a raw project-tier key.
    "packages/opencode-plugin/src/config/project-security.ts",
    "packages/opencode-plugin/src/config/fold-authority.ts",
    // OMP's own setting key appears only as an external CLI string literal;
    // `omp-helpers.ts`, `setup-omp.ts`, and `doctor-omp.ts` never read Eidnara's parsed compaction config.
    "packages/cli/src/lib/omp-helpers.ts",
    "packages/cli/src/commands/setup-omp.ts",
    "packages/cli/src/commands/doctor-omp.ts",
]);

function sourceFiles(directory: string): string[] {
    const result: string[] = [];
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
        const path = join(directory, entry.name);
        if (entry.isDirectory()) {
            result.push(...sourceFiles(path));
        } else if (/\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name)) {
            result.push(path);
        }
    }
    return result;
}

// `COMPACTION_ENABLED_READ` matches `compaction.enabled` and `compaction?.enabled`, including string literals.
// False positives are allow-listed only when they do not read Eidnara's parsed config path.
// config path.
// `COMPACTION_ENABLED_READ` excludes `compaction_mode_record`, `isCompactionEnabled`, and schema `.object({ enabled: ... })`.
const COMPACTION_ENABLED_READ = /\bcompaction\??\s*\.\s*enabled\b(?!_)/;

describe("compaction.enabled accessor exclusivity (issue #266)", () => {
    it("no non-schema source file reads compaction.enabled directly", () => {
        const offenders: string[] = [];
        for (const root of SOURCE_ROOTS) {
            // A root that has not landed yet contributes no readers.
            if (!existsSync(join(REPOSITORY_ROOT, root))) continue;
            for (const path of sourceFiles(resolve(REPOSITORY_ROOT, root))) {
                const relativePath = relative(REPOSITORY_ROOT, path);
                if (ALLOWED_READERS.has(relativePath)) continue;
                const source = readFileSync(path, "utf8");
                if (COMPACTION_ENABLED_READ.test(source)) {
                    offenders.push(relativePath);
                }
            }
        }
        expect(offenders).toEqual([]);
    });

    it("isCompactionEnabled requires a summarizer chain and a compaction setting that is not false", async () => {
        const { isCompactionEnabled } = await import("../config/agent-disable");
        const history_summarizer = { model: "anthropic/claude-haiku-4-5" };
        expect(isCompactionEnabled({ history_summarizer })).toBe(true);
        expect(isCompactionEnabled({ compaction: {}, history_summarizer })).toBe(true);
        expect(isCompactionEnabled({ compaction: { enabled: true }, history_summarizer })).toBe(
            true,
        );
        expect(isCompactionEnabled({ compaction: { enabled: false }, history_summarizer })).toBe(
            false,
        );
        expect(isCompactionEnabled({ compaction: null, history_summarizer })).toBe(true);
        expect(isCompactionEnabled({})).toBe(false);
        expect(isCompactionEnabled({ compaction: { enabled: true } })).toBe(false);
        expect(isCompactionEnabled({ history_summarizer: { model: "  " } })).toBe(false);
    });
});
