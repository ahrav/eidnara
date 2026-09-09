/**
 * Reachability is read from the bundler's metafile rather than from the
 * emitted text, so a claim about what an entry imports is not fooled by a
 * string that happens to appear in a comment or a log line.
 */

import { readFileSync } from "node:fs";

/** The Pi `build` script's externals; the graph stops at these package boundaries. */
export const BUNDLE_EXTERNALS = [
    "@eidnara/shm-native",
    "@earendil-works/pi-coding-agent",
    "@earendil-works/pi-tui",
] as const;

/** Import specifiers are matched as written, so the adapter may appear with or without its extension. */
export const DATABASE_BINDING =
    /(?:^|\/)(?:node:sqlite|bun:sqlite|better-sqlite3)(?:$|\/)|(?:^|\/)shared\/sqlite(?:\.ts)?$/;

export const OPERATION_LITERAL = /["'`](?:claim|dreamer)\.[A-Za-z_][A-Za-z0-9_.]*["'`]/;

export interface ModuleGraph {
    /** Every source module in the bundle, as the bundler names it relative to the working directory. */
    inputs: string[];
    /** Every specifier an input imports that stays external, since those never appear in `inputs`. */
    externals: string[];
    /** Each input's import specifiers as written, bundled and external alike. */
    imports: Record<string, string[]>;
    /** The bundled text, for claims about emitted bytes such as which externals stay external. */
    text: string;
}

export interface MetafileInput {
    imports?: { path?: string; external?: boolean }[];
}

export function graphFromMetafileInputs(
    inputs: Record<string, MetafileInput>,
): Omit<ModuleGraph, "text"> {
    const externals = new Set<string>();
    const imports: Record<string, string[]> = {};
    for (const [path, input] of Object.entries(inputs)) {
        imports[path] = [];
        for (const edge of input.imports ?? []) {
            if (typeof edge.path !== "string") continue;
            imports[path].push(edge.path);
            if (edge.external === true) externals.add(edge.path);
        }
    }
    return { inputs: Object.keys(inputs), externals: [...externals], imports };
}

export async function bundleModuleGraph(entry: string): Promise<ModuleGraph> {
    const result = await Bun.build({
        entrypoints: [entry],
        target: "node",
        format: "esm",
        external: [...BUNDLE_EXTERNALS],
        metafile: true,
    });
    if (!result.success) {
        throw new Error(`bundle of ${entry} failed: ${result.logs.map(String).join("\n")}`);
    }
    if (result.outputs.length === 0) throw new Error(`bundle of ${entry} produced no output`);
    // The runtime hands the metafile back as an object; the type declares a JSON string.
    const raw: unknown = result.metafile;
    const metafile = (typeof raw === "string" ? JSON.parse(raw) : raw) as
        | { inputs?: Record<string, MetafileInput> }
        | undefined;
    if (!metafile?.inputs) throw new Error(`bundle of ${entry} produced no metafile`);
    const texts = await Promise.all(result.outputs.map((output) => output.text()));
    return { ...graphFromMetafileInputs(metafile.inputs), text: texts.join("\n") };
}

/**
 * `RegExp.prototype.test` advances `lastIndex` for `g` and `y` patterns, so later inputs
 * can be skipped. A skipped match would make a boundary proof pass vacuously.
 */
function assertStatelessPattern(caller: string, pattern: RegExp): void {
    if (pattern.global || pattern.sticky) {
        throw new TypeError(
            `${caller}: pattern must not use the g or y flag (got /${pattern.source}/${pattern.flags})`,
        );
    }
}

/** Module and external-specifier paths matching `pattern`. */
export function reachableModules(graph: ModuleGraph, pattern: RegExp): string[] {
    assertStatelessPattern("reachableModules", pattern);
    return [...graph.inputs, ...graph.externals].filter((path) => pattern.test(path));
}

export function databaseBinders(graph: Pick<ModuleGraph, "imports">): string[] {
    return Object.entries(graph.imports)
        .filter(
            ([path, imports]) =>
                !DATABASE_BINDING.test(path) && imports.some((edge) => DATABASE_BINDING.test(edge)),
        )
        .map(([path]) => path)
        .sort();
}

/** Operation names are string literals, not import edges, so the metafile cannot see them. */
export function operationLiteralHits(
    files: string[],
    pattern: RegExp = OPERATION_LITERAL,
): string[] {
    assertStatelessPattern("operationLiteralHits", pattern);
    const hits: string[] = [];
    for (const file of files) {
        const lines = readFileSync(file, "utf8").split("\n");
        for (const [index, line] of lines.entries()) {
            if (pattern.test(line)) hits.push(`${file}:${index + 1}`);
        }
    }
    return hits;
}
