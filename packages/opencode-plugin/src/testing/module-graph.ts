/**
 * Reachability is read from the bundler's metafile rather than from the
 * emitted text, so a claim about what an entry imports is not fooled by a
 * string that happens to appear in a comment or a log line.
 */

import { readFileSync } from "node:fs";
import ts from "typescript";

/** The Pi `build` script's externals; the graph stops at these package boundaries. */
export const BUNDLE_EXTERNALS = [
    "@eidnara/shm-native",
    "@earendil-works/pi-coding-agent",
    "@earendil-works/pi-tui",
] as const;

/** Import specifiers are matched as written, so the adapter may appear with or without its extension. */
export const DATABASE_BINDING =
    /(?:^|\/)(?:node:sqlite|bun:sqlite|better-sqlite3)(?:$|\/)|(?:^|\/)shared\/sqlite(?:\.ts)?$/;

/** The suffix is unconstrained so a spelling with a hyphen or an interpolation still matches. */
export const OPERATION_LITERAL = /["'`](?:claim|dreamer)\.[^"'`]*["'`]/;

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
        .filter(([, imports]) => imports.some((edge) => DATABASE_BINDING.test(edge)))
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

export interface DatabaseUses {
    /** `opens` contains source lines for `new` expressions whose constructor text contains `Database`, or for every `new` expression under `allConstructions`. */
    opens: string[];
    /** `escapes` contains source lines for value-position identifiers containing `Database` and for binding-module imports that bypass direct constructor matching. */
    escapes: string[];
}

export interface DatabaseUsesOptions {
    /** `allConstructions` reports every `new` expression, so an alias whose text does not match `/Database/` still appears in `opens`. */
    allConstructions?: boolean;
}

/** An alias such as `const DB = Database` evades `new Database(` text matching; the syntax tree reports it as an escape. */
export function databaseUses(
    source: string,
    fileName = "module.ts",
    options: DatabaseUsesOptions = {},
): DatabaseUses {
    const file = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true);
    const lines = source.split("\n");
    const lineOf = (node: ts.Node) =>
        lines[file.getLineAndCharacterOfPosition(node.getStart(file)).line];
    const uses: DatabaseUses = { opens: [], escapes: [] };
    const escapedLines = new Set<number>();
    const recordEscape = (node: ts.Node) => {
        const index = file.getLineAndCharacterOfPosition(node.getStart(file)).line;
        if (escapedLines.has(index)) return;
        escapedLines.add(index);
        uses.escapes.push(lines[index]);
    };
    const isBindingSpecifier = (node: ts.Expression | undefined) =>
        node !== undefined && ts.isStringLiteralLike(node) && DATABASE_BINDING.test(node.text);

    const visit = (node: ts.Node): void => {
        if (
            ts.isNewExpression(node) &&
            (options.allConstructions || /Database/.test(node.expression.getText(file)))
        ) {
            uses.opens.push(lineOf(node));
        }
        if (
            ts.isImportDeclaration(node) &&
            isBindingSpecifier(node.moduleSpecifier) &&
            node.importClause &&
            !node.importClause.isTypeOnly
        ) {
            const clause = node.importClause;
            if (
                clause.name ||
                (clause.namedBindings && ts.isNamespaceImport(clause.namedBindings))
            ) {
                recordEscape(node);
            }
            if (clause.namedBindings && ts.isNamedImports(clause.namedBindings)) {
                // An inline `type` specifier is erased at runtime and cannot alias the constructor.
                for (const element of clause.namedBindings.elements) {
                    if (element.propertyName && !element.isTypeOnly) recordEscape(element);
                }
            }
        }
        if (
            ts.isExportDeclaration(node) &&
            isBindingSpecifier(node.moduleSpecifier) &&
            !node.isTypeOnly
        ) {
            // A type-only re-export is erased at runtime; one runtime specifier in a mixed clause is enough to escape.
            const clause = node.exportClause;
            const exportsValue =
                !clause ||
                !ts.isNamedExports(clause) ||
                clause.elements.some((element) => !element.isTypeOnly);
            if (exportsValue) recordEscape(node);
        }
        if (ts.isCallExpression(node)) {
            const callee = node.expression;
            const isImport = callee.kind === ts.SyntaxKind.ImportKeyword;
            const isRequire = ts.isIdentifier(callee) && callee.text === "require";
            if ((isImport || isRequire) && isBindingSpecifier(node.arguments[0])) {
                recordEscape(node);
            }
        }
        if (ts.isIdentifier(node) && /Database/.test(node.text)) {
            const parent = node.parent;
            // `isPartOfTypeNode` accepts `implements` and rejects `extends`; declaration names and
            // `typeof` operands are type-only positions it does not classify.
            const isTypeUse =
                ts.isPartOfTypeNode(node) ||
                ts.isTypeQueryNode(parent) ||
                ts.isQualifiedName(parent) ||
                ((ts.isTypeAliasDeclaration(parent) ||
                    ts.isInterfaceDeclaration(parent) ||
                    ts.isTypeParameterDeclaration(parent)) &&
                    parent.name === node) ||
                (ts.isExportSpecifier(parent) &&
                    (parent.isTypeOnly || parent.parent.parent.isTypeOnly));
            const isOpen = ts.isNewExpression(parent) && parent.expression === node;
            const isPropertyKey =
                (ts.isPropertyAccessExpression(parent) && parent.name === node) ||
                (ts.isPropertyAssignment(parent) && parent.name === node) ||
                (ts.isPropertySignature(parent) && parent.name === node) ||
                (ts.isBindingElement(parent) && parent.propertyName === node);
            if (!ts.isImportSpecifier(parent) && !isTypeUse && !isOpen && !isPropertyKey) {
                recordEscape(node);
            }
        }
        ts.forEachChild(node, visit);
    };
    visit(file);
    return uses;
}
