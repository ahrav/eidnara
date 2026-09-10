import { describe, expect, test } from "bun:test";
import { readFileSync, statSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";

import packageJson from "../../package.json";

/**
 * Relative imports reachable from `./tui` must be included by `package.json` `files` because npm loads its source from the installed tarball.
 * Type-only imports are walked because `./tui`'s `types` export targets `src/tui/index.tsx`.
 */
const PACKAGE_ROOT = resolve(import.meta.dir, "..", "..");

const ENTRY_POINTS = ["src/tui/entry.mjs", "src/tui/index.tsx", "src/tui-compiled/index.tsx"];

const RESOLVE_EXTENSIONS = [".ts", ".tsx", ".mjs", ".js", ".json"];

/**
 * Each alternative captures its specifier in a distinct group. The `from`
 * alternative refuses to cross a quote, backtick, or semicolon before `from`,
 * so a string literal mentioning `from "..."` cannot masquerade as an import;
 * line-anchoring keeps `* import ... from "..."` lines inside doc comments out.
 *
 * Global regexes retain `lastIndex` across nested scans, so the regex is built per call.
 */
function importSpecifiers(source: string): string[] {
    const pattern = new RegExp(
        [
            String.raw`^\s*(?:import|export)\b[^'"\x60;]*?\bfrom\s*["']([^"']+)["']`,
            String.raw`^\s*import\s*["']([^"']+)["']`,
            String.raw`\bimport\s*\(\s*["']([^"']+)["']\s*\)`,
        ].join("|"),
        "gm",
    );
    const specifiers: string[] = [];
    for (const match of source.matchAll(pattern)) {
        const specifier = match[1] ?? match[2] ?? match[3];
        if (specifier !== undefined) specifiers.push(specifier);
    }
    return specifiers;
}

function isRelativeSpecifier(specifier: string): boolean {
    return specifier.startsWith("./") || specifier.startsWith("../");
}

function isFile(candidate: string): boolean {
    return statSync(candidate, { throwIfNoEntry: false })?.isFile() ?? false;
}

/** Returning `null` records dangling imports instead of dropping graph edges. */
function resolveRelativeImport(importer: string, specifier: string): string | null {
    const base = resolve(dirname(importer), specifier);
    if (isFile(base)) return base;
    for (const extension of RESOLVE_EXTENSIONS) {
        const withExtension = base + extension;
        if (isFile(withExtension)) return withExtension;
    }
    for (const extension of RESOLVE_EXTENSIONS) {
        const indexFile = join(base, `index${extension}`);
        if (isFile(indexFile)) return indexFile;
    }
    return null;
}

function packageRelative(file: string): string {
    return relative(PACKAGE_ROOT, file).split("\\").join("/");
}

function isInsidePackage(file: string): boolean {
    const rel = relative(PACKAGE_ROOT, file);
    return rel !== "" && !rel.startsWith("..") && !isAbsolute(rel);
}

/**
 * npm's `files` mixes literal include paths with `!`-prefixed exclusion globs. The exclusions
 * here use only `**` and `*`, which map onto a regular expression directly.
 */
function exclusionPattern(entry: string): RegExp {
    const source = entry
        .slice(1)
        .split("**/")
        .map((segment) => segment.split("*").map(escapeRegExp).join("[^/]*"))
        .join("(?:.*/)?");
    return new RegExp(`^${source}(?:/|$)`);
}

function escapeRegExp(text: string): string {
    return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function includeEntries(files: readonly string[]): string[] {
    return files.filter((entry) => !entry.startsWith("!"));
}

function excludePatterns(files: readonly string[]): RegExp[] {
    return files.filter((entry) => entry.startsWith("!")).map(exclusionPattern);
}

/** npm ships `package.json` regardless of `files`. */
function isShipped(rel: string, files: readonly string[]): boolean {
    if (rel === "package.json") return true;
    if (excludePatterns(files).some((pattern) => pattern.test(rel))) return false;
    return includeEntries(files).some((entry) => rel === entry || rel.startsWith(`${entry}/`));
}

type ImportGraph = {
    /** Package-relative paths of every visited file. */
    reached: Set<string>;
    /** Unresolved relative specifiers as `importer -> specifier`. */
    dangling: string[];
};

function walkImportGraph(entryPoints: readonly string[]): ImportGraph {
    const reached = new Set<string>();
    const dangling: string[] = [];
    const pending = entryPoints.map((entry) => join(PACKAGE_ROOT, entry));

    while (pending.length > 0) {
        const file = pending.pop() as string;
        const rel = packageRelative(file);
        if (reached.has(rel)) continue;
        reached.add(rel);
        // JSON carries data, not imports; scanning it would treat embedded text as code.
        if (file.endsWith(".json")) continue;

        for (const specifier of importSpecifiers(readFileSync(file, "utf8"))) {
            if (!isRelativeSpecifier(specifier)) continue;
            const resolved = resolveRelativeImport(file, specifier);
            if (resolved === null) {
                dangling.push(`${rel} -> ${specifier}`);
                continue;
            }
            pending.push(resolved);
        }
    }

    return { reached, dangling };
}

describe("packed TUI import graph", () => {
    const graph = walkImportGraph(ENTRY_POINTS);

    test("every entry point exists and every relative import resolves", () => {
        for (const entry of ENTRY_POINTS) {
            expect(isFile(join(PACKAGE_ROOT, entry)), entry).toBe(true);
        }
        expect(graph.dangling).toEqual([]);
    });

    test("the walk reaches modules several hops from the entry points", () => {
        // `walkImportGraph` matching no imports would pass the shipping check vacuously.
        expect(graph.reached).toContain("package.json");
        expect(graph.reached).toContain("src/tui/data/session-rpc.ts");
        expect(graph.reached).toContain("src/shared/data-path.ts");
        expect(graph.reached).toContain("src/tui-compiled/data/session-rpc.ts");
    });

    test("`files` include entries are literal paths and exclusions use only `*` globs", () => {
        // `isShipped` matches include entries as literal paths and exclusions through
        // `exclusionPattern`, which understands `**` and `*` but no other glob syntax.
        const globIncludes = includeEntries(packageJson.files).filter((entry) =>
            /[*?[\]{}!]/.test(entry),
        );
        expect(globIncludes).toEqual([]);
        const unsupportedExclusions = packageJson.files.filter(
            (entry) => entry.startsWith("!") && /[?[\]{}]/.test(entry),
        );
        expect(unsupportedExclusions).toEqual([]);
        expect(isShipped("src/shared/data-path.test.ts", packageJson.files)).toBe(false);
        expect(isShipped("src/shared/__tests__/fixture.ts", packageJson.files)).toBe(false);
        expect(isShipped("src/shared/data-path.ts", packageJson.files)).toBe(true);
    });

    test("every reachable module is inside the package and covered by `files`", () => {
        const outsidePackage: string[] = [];
        const unshipped: string[] = [];
        for (const rel of graph.reached) {
            const absolute = resolve(PACKAGE_ROOT, rel);
            if (!isInsidePackage(absolute)) {
                outsidePackage.push(rel);
            } else if (!isShipped(rel, packageJson.files)) {
                unshipped.push(rel);
            }
        }
        expect(outsidePackage, "reachable from the TUI but outside the package").toEqual([]);
        expect(unshipped, "reachable from the TUI but not in package.json `files`").toEqual([]);
    });
});
