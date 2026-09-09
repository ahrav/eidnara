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

/** Every executable extension the bundler accepts; the scans must read all of them. */
export const CODE_FILE = /\.(?:[cm]?[jt]s|[jt]sx)$/;

/**
 * First-party bundle inputs that are code go to the scans; JSON is data. Anything else is an
 * extension no scan understands, so it is an error rather than a silent skip.
 */
export function firstPartyCodeInputs(graph: Pick<ModuleGraph, "inputs">): string[] {
    const code: string[] = [];
    for (const input of graph.inputs) {
        // The bundler names inputs relative to its working directory, so a dependency can
        // appear with or without a leading path segment.
        if (input.startsWith("node_modules/") || input.includes("/node_modules/")) continue;
        if (CODE_FILE.test(input)) code.push(input);
        else if (!/\.json$/.test(input)) {
            throw new Error(`bundle input ${input} has an extension the source scans do not read`);
        }
    }
    return code;
}

/** The suffix is unconstrained so a spelling with a hyphen or an interpolation still matches. */
export const OPERATION_LITERAL = /["'`](?:claim|dreamer)\.[^"'`]*["'`]/;

/** File names of the Rust-owned product stores, as a path a module could open, with any non-word suffix such as `-wal` or `?mode=ro`; the scan blanks comments before matching. */
export const PRODUCT_STORE_FILE =
    /["'`/](?:memory\.sqlite|kernel\.sqlite|context\.db|store\.db)(?!\w)/;

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

/** No library or module resolution: the program exists only to report syntactic diagnostics. */
const PARSE_ONLY: ts.CompilerOptions = {
    allowJs: true,
    jsx: ts.JsxEmit.Preserve,
    target: ts.ScriptTarget.Latest,
    noLib: true,
    noResolve: true,
    types: [],
};

/**
 * A syntax error makes the parser recover with a tree that can omit the very construction an
 * audit looks for, so a file that does not parse cleanly is rejected instead of scanned.
 */
export function parseSource(source: string, fileName: string): ts.SourceFile {
    const file = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true);
    const host: ts.CompilerHost = {
        getSourceFile: (name) => (name === fileName ? file : undefined),
        getDefaultLibFileName: () => "lib.d.ts",
        writeFile: () => {},
        getCurrentDirectory: () => "",
        getCanonicalFileName: (name) => name,
        useCaseSensitiveFileNames: () => true,
        getNewLine: () => "\n",
        fileExists: (name) => name === fileName,
        readFile: (name) => (name === fileName ? source : undefined),
    };
    const program = ts.createProgram([fileName], PARSE_ONLY, host);
    const diagnostics = program.getSyntacticDiagnostics(file);
    if (diagnostics.length > 0) {
        const messages = diagnostics
            .map(
                (diagnostic) =>
                    `TS${diagnostic.code}: ${ts.flattenDiagnosticMessageText(diagnostic.messageText, " ")}`,
            )
            .join("; ");
        throw new SyntaxError(`${fileName} does not parse: ${messages}`);
    }
    return file;
}

/**
 * Comments are blanked to spaces, never removed, so the scan reports the source line number
 * of a hit and a closed block comment cannot hide the code that follows it on the same line.
 */
export function withoutComments(
    source: string,
    fileName = "module.ts",
    parsed?: ts.SourceFile,
): string {
    const file = parsed ?? parseSource(source, fileName);
    const ranges = new Map<number, number>();
    const collect = (node: ts.Node): void => {
        for (const range of [
            ...(ts.getLeadingCommentRanges(source, node.getFullStart()) ?? []),
            ...(ts.getTrailingCommentRanges(source, node.getEnd()) ?? []),
        ]) {
            ranges.set(range.pos, range.end);
        }
        ts.forEachChild(node, collect);
    };
    collect(file);
    // A shebang or leading comment before the first statement attaches to the end-of-file token.
    for (const range of ts.getLeadingCommentRanges(source, file.endOfFileToken.getFullStart()) ??
        []) {
        ranges.set(range.pos, range.end);
    }
    let out = source;
    for (const [pos, end] of ranges) {
        const blank = source.slice(pos, end).replace(/[^\n]/g, " ");
        out = out.slice(0, pos) + blank + out.slice(end);
    }
    return out;
}

/** Operation names are string literals, not import edges, so the metafile cannot see them. */
export function operationLiteralHits(
    files: string[],
    pattern: RegExp = OPERATION_LITERAL,
): string[] {
    assertStatelessPattern("operationLiteralHits", pattern);
    const hits: string[] = [];
    for (const file of files) {
        const source = readFileSync(file, "utf8");
        const parsed = parseSource(source, file);
        const lines = new Set<number>();
        for (const [index, line] of withoutComments(source, file, parsed).split("\n").entries()) {
            if (pattern.test(line)) lines.add(index + 1);
        }
        for (const { line, value } of literalStrings(parsed)) {
            if (pattern.test(`"${value}"`)) lines.add(line);
        }
        for (const line of [...lines].sort((a, b) => a - b)) hits.push(`${file}:${line}`);
    }
    return hits;
}

/**
 * The strings a module evaluates to, as the parser cooks them: every string literal and template
 * span with escapes decoded (`"context\u002edb"` is `context.db`), and every `+` expression whose
 * leaves are all string literals folded to its concatenation, inner chains included. Each is
 * reported at its first line.
 */
export function literalStrings(file: ts.SourceFile): { line: number; value: string }[] {
    const folded: { line: number; value: string }[] = [];
    const leafText = (node: ts.Expression): string | undefined => {
        const inner = ts.isParenthesizedExpression(node) ? node.expression : node;
        if (ts.isStringLiteralLike(inner)) return inner.text;
        if (ts.isBinaryExpression(inner) && inner.operatorToken.kind === ts.SyntaxKind.PlusToken) {
            const left = leafText(inner.left);
            const right = leafText(inner.right);
            return left !== undefined && right !== undefined ? left + right : undefined;
        }
        return undefined;
    };
    const lineOf = (node: ts.Node) =>
        file.getLineAndCharacterOfPosition(node.getStart(file)).line + 1;
    const visit = (node: ts.Node): void => {
        if (ts.isStringLiteralLike(node) || ts.isTemplateLiteralToken(node)) {
            folded.push({ line: lineOf(node), value: node.text });
        }
        if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken) {
            const value = leafText(node);
            if (value !== undefined) folded.push({ line: lineOf(node), value });
        }
        ts.forEachChild(node, visit);
    };
    visit(file);
    return folded;
}

export interface DatabaseUses {
    /** `opens` contains source lines for `new` expressions whose constructor text contains `Database`, or for every `new` expression under `allConstructions`. */
    opens: string[];
    /** `escapes` contains source lines for value-position identifiers containing `Database`, for binding-module imports that bypass direct constructor matching, and for dynamic loads whose specifier is not a string literal. */
    escapes: string[];
}

/**
 * Every database-relevant line in the retained OpenCode tree, keyed by path under its `src`:
 * constructor opens, aliases of the constructor, binding imports, and dynamic loads whose
 * specifier is not a string literal. A module absent here has none. The adapter's own load
 * and the tokenizer's file-URL loads are computed on purpose; the path helper only names the
 * harness database's location. The Pi bundle reaches a subset of these same modules.
 */
export const RETAINED_DATABASE_USES: Record<string, DatabaseUses> = {
    "features/context/compaction-marker.ts": {
        opens: ["    const db = new Database(dbPath);"],
        escapes: [],
    },
    "hooks/context/read-session-db.ts": {
        opens: ["    const db = new Database(dbPath, { readonly: true });"],
        escapes: [],
    },
    "shared/sqlite.ts": {
        opens: ['    const probe = new Database(":memory:");'],
        escapes: [
            "    return (await import(",
            "const DatabaseImpl: typeof BetterSqlite3 = isBun",
            "    ? buildBunSqliteDatabaseClass(sqliteModule.Database)",
            "    : buildNodeSqliteDatabaseClass(sqliteModule.DatabaseSync);",
            "export function buildBunSqliteDatabaseClass(BunDatabase: any): typeof BetterSqlite3 {",
            "    class BunSqliteDatabase extends BunDatabase {",
            "    return BunSqliteDatabase as unknown as typeof BetterSqlite3;",
            "export function buildNodeSqliteDatabaseClass(DatabaseSync: any): typeof BetterSqlite3 {",
            "    class NodeSqliteDatabase extends DatabaseSync {",
            "    return NodeSqliteDatabase as unknown as typeof BetterSqlite3;",
            "export const Database: typeof BetterSqlite3 = DatabaseImpl;",
        ],
    },
    "shared/token-estimator.ts": {
        opens: [],
        escapes: [
            '        requireFromThisModule("ai-" + "tokenizer"),',
            '        requireFromThisModule("ai-tokenizer/encoding/" + "claude"),',
            "                import(pathToFileURL(paths.tokenizerPath).href),",
            "                import(pathToFileURL(paths.encodingPath).href),",
        ],
    },
    "shared/opencode-database-path.ts": {
        opens: [],
        escapes: [
            "function listDatabaseFiles(dirPath: string, filePrefix: string): string[] {",
            "export function resolveOpenCodeDatabaseCandidates(dataDir: string = getDataDir()): string[] {",
            '        ...listDatabaseFiles(opencodeRoot, "opencode"),',
            '        ...listDatabaseFiles(storageRoot, ""),',
            "export function resolveOpenCodeDatabasePath(dataDir: string = getDataDir()): string {",
            "    return resolveOpenCodeDatabaseCandidates(dataDir)[0];",
        ],
    },
    "tui/entry.mjs": {
        opens: [],
        escapes: ["    await import(runtimeProbe);"],
    },
};

export interface DatabaseUsesOptions {
    /** `allConstructions` reports every `new` expression, so an alias whose text does not match `/Database/` still appears in `opens`. */
    allConstructions?: boolean;
}

function isAmbient(node: ts.Node): boolean {
    for (let current: ts.Node | undefined = node; current; current = current.parent) {
        if (
            ts.canHaveModifiers(current) &&
            ts.getModifiers(current)?.some((m) => m.kind === ts.SyntaxKind.DeclareKeyword)
        ) {
            return true;
        }
    }
    return false;
}

/** An alias such as `const DB = Database` evades `new Database(` text matching; the syntax tree reports it as an escape. */
export function databaseUses(
    source: string,
    fileName = "module.ts",
    options: DatabaseUsesOptions = {},
): DatabaseUses {
    const file = parseSource(source, fileName);
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
    // `const load = createRequire(import.meta.url)` makes `load(...)` a require by another name.
    const loaders = new Set<string>(["require"]);
    const collectLoaders = (node: ts.Node): void => {
        if (
            ts.isVariableDeclaration(node) &&
            ts.isIdentifier(node.name) &&
            node.initializer &&
            ts.isCallExpression(node.initializer) &&
            /(^|\.)createRequire$/.test(node.initializer.expression.getText(file))
        ) {
            loaders.add(node.name.text);
        }
        ts.forEachChild(node, collectLoaders);
    };
    collectLoaders(file);

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
            const isRequire =
                (ts.isIdentifier(callee) && loaders.has(callee.text)) ||
                callee.getText(file) === "module.require";
            // Non-string-literal specifiers cannot be checked against the binding pattern, so they escape.
            const specifier = node.arguments[0];
            const unresolved = specifier !== undefined && !ts.isStringLiteralLike(specifier);
            if ((isImport || isRequire) && (unresolved || isBindingSpecifier(specifier))) {
                recordEscape(node);
            }
        }
        if (ts.isIdentifier(node) && /Database/.test(node.text)) {
            const parent = node.parent;
            // `isPartOfTypeNode` accepts `implements` and rejects `extends`; declaration names,
            // `typeof` operands, and ambient declarations are type-only positions it does not
            // classify. An ambient declaration emits no value, in a `.d.ts` file or under `declare`.
            const isTypeUse =
                ts.isPartOfTypeNode(node) ||
                file.isDeclarationFile ||
                isAmbient(node) ||
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
