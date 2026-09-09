/**
 * Reachability is read from the bundler's metafile rather than from the
 * emitted text, so a claim about what an entry imports is not fooled by a
 * string that happens to appear in a comment or a log line.
 */

import { readFileSync } from "node:fs";
import { posix } from "node:path";
import ts from "typescript";

/** The Pi `build` script's externals; the graph stops at these package boundaries. */
export const BUNDLE_EXTERNALS = [
    "@eidnara/shm-native",
    "@earendil-works/pi-coding-agent",
    "@earendil-works/pi-tui",
] as const;

/** Import specifiers are matched as written, so the adapter may appear with no extension or any executable one. */
export const DATABASE_BINDING =
    /(?:^|\/)(?:node:sqlite|bun:sqlite|better-sqlite3)(?:$|\/)|(?:^|\/)shared\/sqlite(?:\.(?:[cm]?[jt]s|[jt]sx))?$/;

/** Relative specifiers resolve against the importer's directory before matching: `./sqlite` written beside the adapter names it. */
export function resolvesToBinding(importer: string, specifier: string): boolean {
    if (DATABASE_BINDING.test(specifier)) return true;
    if (!specifier.startsWith("./") && !specifier.startsWith("../")) return false;
    return DATABASE_BINDING.test(posix.normalize(posix.join(posix.dirname(importer), specifier)));
}

/** Every executable extension the bundler accepts; the scans must read all of them. */
export const CODE_FILE = /\.(?:[cm]?[jt]s|[jt]sx)$/;

/** A test file under any executable extension; the state-ownership scans exclude these by name. */
export const TEST_FILE = /\.test\.(?:[cm]?[jt]s|[jt]sx)$/;

/**
 * First-party bundle inputs: code goes to every scan, and JSON goes to the literal scans since
 * a bundled JSON value can carry a store path or an operation name the importing code never
 * spells. Anything else is an extension no scan understands, so it is an error rather than a
 * silent skip.
 */
export function firstPartyInputs(graph: Pick<ModuleGraph, "inputs">): {
    code: string[];
    data: string[];
} {
    const code: string[] = [];
    const data: string[] = [];
    for (const input of graph.inputs) {
        // The bundler names inputs relative to its working directory, so a dependency can
        // appear with or without a leading path segment.
        if (input.startsWith("node_modules/") || input.includes("/node_modules/")) continue;
        if (CODE_FILE.test(input)) code.push(input);
        else if (/\.json$/.test(input)) data.push(input);
        else throw new Error(`bundle input ${input} has an extension the source scans do not read`);
    }
    return { code, data };
}

/** The suffix is unconstrained so a spelling with a hyphen or an interpolation still matches; case is ignored so a handler cannot compare a normalized operation against `"CLAIM.INTENT.STAGE"`. */
export const OPERATION_LITERAL = /["'`](?:claim|dreamer)\.[^"'`]*["'`]/i;

/** Every character the scans' forbidden spellings use; a regex wildcard is tried against each of them. */
const WILDCARD_ALPHABET = [...new Set("claimdreamemorysqlitekernelcontextdbstore.")].join("");

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
        .filter(([path, imports]) => imports.some((edge) => resolvesToBinding(path, edge)))
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
 * span with escapes decoded (`"context\u002edb"` is `context.db`); every `+` expression,
 * template, or array-literal `join` whose leaves are all string literals, folded to the value it
 * produces, inner chains included; and every regular-expression literal reduced to its body
 * without anchors or escapes. Each is reported at its first line.
 */
export function literalStrings(file: ts.SourceFile): { line: number; value: string }[] {
    const folded: { line: number; value: string }[] = [];
    // Bindings whose initializers fold to strings, keyed by name without regard to scope, so a
    // shadowed name keeps every value it is ever given. Array bindings whose elements all fold
    // are kept separately for `parts.join(...)`.
    const bindings = new Map<string, string[]>();
    const arrayBindings = new Map<string, ts.ArrayLiteralExpression>();
    // Every string an expression can evaluate to under the bindings, or `undefined` when a leaf
    // is not foldable. A product past 1024 values fails the scan rather than dropping any.
    const product = (parts: (string[] | undefined)[]): string[] | undefined => {
        let values = [""];
        for (const part of parts) {
            if (part === undefined) return undefined;
            const next: string[] = [];
            for (const head of values) for (const tail of part) next.push(head + tail);
            if (next.length > 1024)
                throw new RangeError("a folded string has more than 1024 values");
            values = [...new Set(next)];
        }
        return values;
    };
    const leafTexts = (node: ts.Expression): string[] | undefined => {
        const inner = ts.isParenthesizedExpression(node) ? node.expression : node;
        if (ts.isStringLiteralLike(inner)) return [inner.text];
        if (ts.isIdentifier(inner)) return bindings.get(inner.text);
        // `flag ? "claim" : "kernel"` evaluates to either branch.
        if (ts.isConditionalExpression(inner)) {
            const whenTrue = leafTexts(inner.whenTrue);
            const whenFalse = leafTexts(inner.whenFalse);
            if (whenTrue === undefined || whenFalse === undefined) return undefined;
            return [...new Set([...whenTrue, ...whenFalse])];
        }
        if (ts.isBinaryExpression(inner) && inner.operatorToken.kind === ts.SyntaxKind.PlusToken) {
            return product([leafTexts(inner.left), leafTexts(inner.right)]);
        }
        if (ts.isTemplateExpression(inner)) {
            const parts: (string[] | undefined)[] = [[inner.head.text]];
            for (const span of inner.templateSpans) {
                parts.push(leafTexts(span.expression), [span.literal.text]);
            }
            return product(parts);
        }
        // `"context".concat(".db")` with a foldable receiver and arguments.
        if (
            ts.isCallExpression(inner) &&
            ts.isPropertyAccessExpression(inner.expression) &&
            inner.expression.name.text === "concat"
        ) {
            return product([inner.expression.expression, ...inner.arguments].map(leafTexts));
        }
        // `"CONTEXT.DB".toLowerCase()` and the other case conversions of a foldable receiver.
        if (
            ts.isCallExpression(inner) &&
            ts.isPropertyAccessExpression(inner.expression) &&
            inner.arguments.length === 0 &&
            /^to(?:Locale)?(?:Lower|Upper)Case$/.test(inner.expression.name.text)
        ) {
            const receiver = leafTexts(inner.expression.expression);
            if (receiver === undefined) return undefined;
            const lower = /Lower/.test(inner.expression.name.text);
            return receiver.map((value) => (lower ? value.toLowerCase() : value.toUpperCase()));
        }
        // `["claim", "intent"].join(".")`, or `parts.join(".")` on a constant array binding,
        // with every element and the separator foldable.
        if (
            ts.isCallExpression(inner) &&
            ts.isPropertyAccessExpression(inner.expression) &&
            inner.expression.name.text === "join" &&
            inner.arguments.length <= 1
        ) {
            const receiver = inner.expression.expression;
            const array = ts.isArrayLiteralExpression(receiver)
                ? receiver
                : ts.isIdentifier(receiver)
                  ? arrayBindings.get(receiver.text)
                  : undefined;
            if (array === undefined) return undefined;
            const separator = inner.arguments[0] ? leafTexts(inner.arguments[0]) : [","];
            const parts: (string[] | undefined)[] = [];
            for (const [index, element] of array.elements.entries()) {
                if (index > 0) parts.push(separator);
                parts.push(leafTexts(element));
            }
            return product(parts);
        }
        return undefined;
    };
    // A regex that matches one exact string is that string with its anchors and escapes removed;
    // the literal-shaped body is what a `/^claim\.intent\.stage$/.test(op)` dispatch compares.
    // A private-use marker prevents expansion from treating escaped `|`, `(`, or `[` as syntax.
    const regexTexts = (body: string): string[] => {
        // A bare `.` or a shorthand class outside a bracket class matches any character of its
        // kind, so it becomes an explicit class over the audited alphabet.
        const wordChars = [...WILDCARD_ALPHABET].filter((ch) => /[\w]/.test(ch)).join("");
        const nonWordChars = [...WILDCARD_ALPHABET].filter((ch) => !/[\w]/.test(ch)).join("");
        const shorthand: Record<string, string> = {
            ".": WILDCARD_ALPHABET,
            "\\w": wordChars,
            "\\W": nonWordChars,
            "\\S": WILDCARD_ALPHABET,
            "\\D": WILDCARD_ALPHABET,
        };
        const withWildcards = body
            .replace(/^\^/, "")
            .replace(/\$$/, "")
            // A lookaround constrains a match without contributing characters to it.
            .replace(/\(\?<?[=!](?:\\.|\[(?:\\.|[^\]])*\]|[^()])*\)/g, "")
            .replace(/\\[pP]\{[^}]*\}|\\.|\[(?:\\.|[^\]])*\]|\./g, (token) => {
                // A Unicode property escape is a wildcard over the alphabet, like `\S`.
                if (/^\\[pP]\{/.test(token)) return `[${WILDCARD_ALPHABET}]`;
                if (token.startsWith("[")) {
                    // Inside a class, `\w` and `\p{..}` contribute alphabet members alongside
                    // the others.
                    return token.replace(/\\[pP]\{[^}]*\}|\\[wWSD]/g, (inner) =>
                        /^\\[pP]/.test(inner) ? WILDCARD_ALPHABET : (shorthand[inner] ?? inner),
                    );
                }
                const members = shorthand[token];
                return members === undefined || members === "" ? token : `[${members}]`;
            });
        const reduced = withWildcards
            .replace(
                /\\x([0-9a-fA-F]{2})|\\u([0-9a-fA-F]{4})|\\u\{([0-9a-fA-F]+)\}/g,
                (_, x, u, b) => `\uE000${String.fromCodePoint(Number.parseInt(x ?? u ?? b, 16))}`,
            )
            .replace(/\\(.)/g, "\uE000$1");
        const out: string[] = [];
        // A pattern with any repetition (`*`, `+`, `{n}`, `{n,m}`) matches a run and is outside
        // the literal-spelling contract; only a repetition-free pattern too large to enumerate
        // fails the scan.
        const repetitionFree = !/(?<!\uE000)[*+{]/.test(reduced);
        const splitUnescaped = (value: string): string[] => value.split(/(?<!\uE000)\|/);
        // `classMembers` marks every member as escaped so expansion treats `-` and `]` literally.
        // A negated class yields every alphabet character it does not exclude.
        const classMembers = (inner: string): string[] | undefined => {
            if (inner.startsWith("^")) {
                const excluded = classMembers(inner.slice(1));
                if (excluded === undefined) return undefined;
                return [...WILDCARD_ALPHABET]
                    .map((char) => `\uE000${char}`)
                    .filter((member) => !excluded.includes(member));
            }
            const members: string[] = [];
            const chars = [...inner];
            for (let index = 0; index < chars.length; index++) {
                let char = chars[index] ?? "";
                if (char === "\uE000") char = chars[++index] ?? "";
                if (chars[index + 1] === "-" && index + 2 < chars.length) {
                    let end = chars[index + 2] ?? "";
                    index += 2;
                    if (end === "\uE000") end = chars[++index] ?? "";
                    const from = char.codePointAt(0) ?? 0;
                    const to = end.codePointAt(0) ?? 0;
                    if (to < from || to - from >= 16) return undefined;
                    for (let point = from; point <= to; point++) {
                        members.push(`\uE000${String.fromCodePoint(point)}`);
                    }
                    continue;
                }
                members.push(`\uE000${char}`);
            }
            return members.length <= 32 ? members : undefined;
        };
        const overflow = Symbol("overflow");
        const pushText = (value: string): void => {
            if (out.length >= 4096) throw overflow;
            out.push(value.replaceAll("\uE000", ""));
        };
        // `?:`, a capture name `?<name>`, and a flag prefix `?i:` are group syntax, not text.
        const groupPrefix = /^\((?:\?(?::|<[A-Za-z_$][\w$]*>|[a-zA-Z-]*:))?/;
        const isQuantifier = (char: string | undefined) =>
            char === "?" || char === "*" || char === "+" || char === "{";
        // `+` and `*` accept any repetition count; one and two, or zero and one, are the counts
        // that can spell a route, so they expand like `{1,2}` and `{0,1}`.
        const bounds = (quantifier: string, low?: string, high?: string): [number, number] =>
            quantifier === "?" || quantifier === "*"
                ? [0, 1]
                : quantifier === "+"
                  ? [1, 2]
                  : [Number(low), Number(high ?? low)];
        const quantifierPattern =
            /(\((?:\?(?::|<[A-Za-z_$][\w$]*>|[a-zA-Z-]*:))?[^()]*\)|\uE000.|\[(?:\uE000.|[^\]\uE000])*\]|[^\uE000\]?*+{}()|])(\?|\*|\+|\{(\d+)(?:,(\d+))?\})/g;
        const choicesOf = (atom: string): number | undefined =>
            atom.startsWith("[")
                ? classMembers(atom.slice(1, -1))?.length
                : atom.startsWith("(")
                  ? splitUnescaped(atom.replace(groupPrefix, "").slice(0, -1)).length
                  : 1;
        const variantsOf = (value: string, match: RegExpExecArray): number | undefined => {
            const [, atom, quantifier, low, high] = match;
            if (match.index > 0 && value.startsWith("\uE000", match.index - 1)) return undefined;
            const [from, to] = bounds(quantifier, low, high);
            const choices = choicesOf(atom);
            if (choices === undefined || to < from || to > 16) return undefined;
            let variants = 0;
            for (let count = from; count <= to; count++) variants += choices ** count;
            return variants;
        };
        const expand = (value: string): void => {
            // A quantified group (`(ab)?`, `(a|b){2}`) belongs to the quantifier rule below.
            const group = /\((?:\?(?::|<[A-Za-z_$][\w$]*>|[a-zA-Z-]*:))?([^()]*)\)(?![*+?{])/.exec(
                value,
            );
            if (group?.index !== undefined && !value.startsWith("\uE000", group.index - 1)) {
                const prefix = value.slice(0, group.index);
                const suffix = value.slice(group.index + group[0].length);
                for (const alternative of splitUnescaped(group[1] ?? "")) {
                    expand(prefix + alternative + suffix);
                }
                return;
            }
            for (const match of value.matchAll(quantifierPattern)) {
                if (variantsOf(value, match) === undefined) continue;
                const [whole, atom, quantifier, low, high] = match;
                const [from, to] = bounds(quantifier, low, high);
                const prefix = value.slice(0, match.index);
                const suffix = value.slice(match.index + whole.length);
                for (let count = from; count <= to; count++) {
                    expand(prefix + atom.repeat(count) + suffix);
                }
                return;
            }
            const classPattern = /\[((?:\uE000.|[^\]\uE000])*)\](?![*+?{])/g;
            for (const cls of value.matchAll(classPattern)) {
                if (cls.index > 0 && value.startsWith("\uE000", cls.index - 1)) continue;
                const members = classMembers(cls[1] ?? "");
                if (members !== undefined) {
                    const prefix = value.slice(0, cls.index);
                    const suffix = value.slice(cls.index + cls[0].length);
                    for (const member of members) expand(prefix + member + suffix);
                    return;
                }
            }
            for (const alternative of splitUnescaped(value)) pushText(alternative);
        };
        // A repetition-free pattern accepts a small set of texts; one too large to enumerate
        // cannot be proved free of a forbidden spelling, so it fails the scan. A pattern with
        // repetition that overflows stays as written, one text per top-level alternative.
        try {
            expand(reduced);
        } catch (error) {
            if (error !== overflow) throw error;
            if (repetitionFree) {
                throw new RangeError(
                    `regex /${body}/ accepts more than 4096 texts and cannot be audited`,
                );
            }
            out.length = 0;
            for (const alternative of splitUnescaped(reduced)) {
                out.push(alternative.replaceAll("\uE000", ""));
            }
        }
        return out;
    };
    const lineOf = (node: ts.Node) =>
        file.getLineAndCharacterOfPosition(node.getStart(file)).line + 1;
    // An `i` flag accepts every casing, including the case-sensitive store names.
    const pushRegex = (node: ts.Node, body: string, flags: string): void => {
        const caseInsensitive = flags.includes("i");
        for (const accepted of regexTexts(body)) {
            folded.push({ line: lineOf(node), value: accepted });
            const lower = accepted.toLowerCase();
            if (caseInsensitive && lower !== accepted) {
                folded.push({ line: lineOf(node), value: lower });
            }
        }
    };
    const isRegExpConstructor = (callee: ts.Expression): boolean =>
        (ts.isIdentifier(callee) && callee.text === "RegExp") ||
        (ts.isPropertyAccessExpression(callee) && callee.name.text === "RegExp");
    const visit = (node: ts.Node): void => {
        if ((ts.isStringLiteralLike(node) || ts.isTemplateLiteralToken(node)) && node.text !== "") {
            folded.push({ line: lineOf(node), value: node.text });
        }
        if (ts.isRegularExpressionLiteral(node)) {
            const close = node.text.lastIndexOf("/");
            pushRegex(node, node.text.slice(1, close), node.text.slice(close + 1));
        }
        // `RegExp(pattern, flags)` behaves like `new RegExp(pattern, flags)`.
        if (
            (ts.isNewExpression(node) || ts.isCallExpression(node)) &&
            isRegExpConstructor(node.expression) &&
            node.arguments?.[0] !== undefined
        ) {
            const bodies = leafTexts(node.arguments[0]) ?? [];
            // Flags the folder cannot evaluate may include `i`, so the body is audited as if
            // they did.
            const flags = node.arguments[1] ? (leafTexts(node.arguments[1]) ?? ["i"]) : [""];
            for (const body of bodies) for (const flag of flags) pushRegex(node, body, flag);
        }
        if (
            (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken) ||
            ts.isTemplateExpression(node) ||
            ts.isConditionalExpression(node) ||
            ts.isCallExpression(node)
        ) {
            for (const value of leafTexts(node) ?? []) folded.push({ line: lineOf(node), value });
        }
        ts.forEachChild(node, visit);
    };
    // Bindings can refer to declarations collected in earlier passes, so collection repeats
    // until no binding gains a value. A shadowing declaration that reads its own name
    // (`const p = p + "x"`) would grow forever, so passes are bounded.
    let bindingValues = 0;
    const bind = (name: ts.Identifier, value: ts.Expression): void => {
        const known = bindings.get(name.text) ?? [];
        const merged = [...new Set([...known, ...(leafTexts(value) ?? [])])];
        if (merged.length > 1024) {
            throw new RangeError(`binding ${name.text} has more than 1024 values`);
        }
        if (merged.length > known.length) {
            bindings.set(name.text, merged);
            bindingValues += merged.length - known.length;
        }
    };
    // `let prefix; prefix = "claim";` binds through an assignment rather than an initializer.
    const collectBindings = (node: ts.Node): void => {
        if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.initializer) {
            bind(node.name, node.initializer);
            if (ts.isArrayLiteralExpression(node.initializer)) {
                arrayBindings.set(node.name.text, node.initializer);
            }
        }
        if (
            ts.isBinaryExpression(node) &&
            node.operatorToken.kind === ts.SyntaxKind.EqualsToken &&
            ts.isIdentifier(node.left)
        ) {
            bind(node.left, node.right);
        }
        ts.forEachChild(node, collectBindings);
    };
    for (let pass = 0, seen = -1; pass < 32 && seen !== bindingValues; pass++) {
        seen = bindingValues;
        collectBindings(file);
    }
    visit(file);
    return folded;
}

export interface DatabaseUses {
    /** `opens` contains source lines for `new` expressions whose constructor text contains `Database`, or for every `new` expression under `allConstructions`. */
    opens: string[];
    /** `openArguments` contains, for each `Database`-named open in order, the source line that binds its first argument in the nearest enclosing scope, the argument's own text when it is not an identifier, or `<parameter>` when a function parameter binds it. */
    openArguments: string[];
    /** `escapes` contains source lines for value-position identifiers containing `Database`, for binding-module imports that bypass direct constructor matching, for `new` expressions whose constructor is not a plain identifier, and for dynamic loads whose specifier is not a string literal. */
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
        openArguments: ["    const dbPath = getOpenCodeDbPath();"],
        escapes: [],
    },
    "hooks/context/read-session-db.ts": {
        opens: ["    const db = new Database(dbPath, { readonly: true });"],
        openArguments: ["    const dbPath = getOpenCodeDbPath();"],
        escapes: [],
    },
    "shared/sqlite.ts": {
        opens: ['    const probe = new Database(":memory:");'],
        openArguments: ['":memory:"'],
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
        openArguments: [],
        escapes: [
            '        requireFromThisModule("ai-" + "tokenizer"),',
            '        requireFromThisModule("ai-tokenizer/encoding/" + "claude"),',
            "                import(pathToFileURL(paths.tokenizerPath).href),",
            "                import(pathToFileURL(paths.encodingPath).href),",
        ],
    },
    "shared/opencode-database-path.ts": {
        opens: [],
        openArguments: [],
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
        openArguments: [],
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
    const uses: DatabaseUses = { opens: [], openArguments: [], escapes: [] };
    // The nearest binding of `name` visible from `from`: a declaration in an enclosing block
    // that precedes the use, or a parameter of an enclosing function.
    const bindingLineOf = (from: ts.Node, name: string): string => {
        for (let scope: ts.Node | undefined = from.parent; scope; scope = scope.parent) {
            if (ts.isBlock(scope) || ts.isSourceFile(scope)) {
                let nearest: ts.Node | undefined;
                for (const statement of scope.statements) {
                    if (statement.getStart(file) >= from.getStart(file)) break;
                    if (ts.isVariableStatement(statement)) {
                        for (const declaration of statement.declarationList.declarations) {
                            if (
                                ts.isIdentifier(declaration.name) &&
                                declaration.name.text === name
                            ) {
                                nearest = declaration;
                            }
                        }
                    }
                }
                if (nearest !== undefined) return lineOf(nearest);
            }
            if (ts.isFunctionLike(scope)) {
                for (const parameter of scope.parameters) {
                    if (ts.isIdentifier(parameter.name) && parameter.name.text === name) {
                        return "<parameter>";
                    }
                }
            }
        }
        return "<unbound>";
    };
    const escapedLines = new Set<number>();
    const recordEscape = (node: ts.Node) => {
        const index = file.getLineAndCharacterOfPosition(node.getStart(file)).line;
        if (escapedLines.has(index)) return;
        escapedLines.add(index);
        uses.escapes.push(lines[index]);
    };
    const isBindingSpecifier = (node: ts.Expression | undefined) =>
        node !== undefined &&
        ts.isStringLiteralLike(node) &&
        resolvesToBinding(fileName, node.text);
    // `const load = createRequire(import.meta.url)` makes `load(...)` a require by another name,
    // and `import { createRequire as makeRequire }` gives the factory another name too.
    const factories = new Set<string>(["createRequire"]);
    const collectFactories = (node: ts.Node): void => {
        if (
            ts.isImportSpecifier(node) &&
            node.propertyName?.text === "createRequire" &&
            !node.isTypeOnly
        ) {
            factories.add(node.name.text);
        }
        // `const make = createRequire;` aliases the factory itself.
        if (
            ts.isVariableDeclaration(node) &&
            ts.isIdentifier(node.name) &&
            node.initializer &&
            factories.has(node.initializer.getText(file).split(".").pop() ?? "")
        ) {
            factories.add(node.name.text);
        }
        // `const { createRequire: make } = mod;` destructures it under another name, and
        // `{ ["createRequire"]: make }` spells the key as a computed string.
        if (ts.isBindingElement(node) && ts.isIdentifier(node.name)) {
            const property = node.propertyName ?? node.name;
            const key = ts.isIdentifier(property)
                ? property.text
                : ts.isStringLiteralLike(property)
                  ? property.text
                  : ts.isComputedPropertyName(property) &&
                      ts.isStringLiteralLike(property.expression)
                    ? property.expression.text
                    : undefined;
            if (key !== undefined && factories.has(key)) factories.add(node.name.text);
        }
        ts.forEachChild(node, collectFactories);
    };
    // `process["getBuiltinModule"]` is the same member as `process.getBuiltinModule`.
    const memberText = (expression: ts.Expression): string =>
        ts.isElementAccessExpression(expression) &&
        ts.isStringLiteralLike(expression.argumentExpression)
            ? `${expression.expression.getText(file)}.${expression.argumentExpression.text}`
            : expression.getText(file);
    const isLoaderMember = (text: string) =>
        text === "module.require" || /(^|\.)getBuiltinModule$/.test(text);
    const loaders = new Set<string>(["require"]);
    // `load = createRequire(...)` makes a loader whether it initializes a declaration or is
    // assigned later; `run = load` aliases one; `load = process.getBuiltinModule` aliases a member.
    const bindLoader = (name: ts.Identifier, value: ts.Expression): void => {
        if (ts.isCallExpression(value)) {
            const factory = value.expression.getText(file).split(".").pop() ?? "";
            if (factories.has(factory)) loaders.add(name.text);
        } else if (ts.isIdentifier(value) && loaders.has(value.text)) {
            loaders.add(name.text);
        } else if (isLoaderMember(memberText(value))) {
            loaders.add(name.text);
        }
    };
    const collectLoaders = (node: ts.Node): void => {
        if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.initializer) {
            bindLoader(node.name, node.initializer);
        }
        if (
            ts.isBinaryExpression(node) &&
            node.operatorToken.kind === ts.SyntaxKind.EqualsToken &&
            ts.isIdentifier(node.left)
        ) {
            bindLoader(node.left, node.right);
        }
        ts.forEachChild(node, collectLoaders);
    };
    // `const Ctor = db.constructor`, `const { constructor: Ctor } = db`, and `Again = Ctor`
    // reach whatever class `db` is without naming it; `new Ctor(...)` is then an escape.
    const constructorAliases = new Set<string>();
    // A computed member (`obj["constr" + "uctor"]`, `obj[key]`) may name `constructor`, so any
    // element access whose key is not a plain literal counts.
    const unwrap = (value: ts.Expression): ts.Expression =>
        ts.isParenthesizedExpression(value) ||
        ts.isAsExpression(value) ||
        ts.isNonNullExpression(value) ||
        ts.isTypeAssertionExpression(value) ||
        ts.isSatisfiesExpression(value)
            ? unwrap(value.expression)
            : value;
    const isConstructorSource = (raw: ts.Expression): boolean => {
        const value = unwrap(raw);
        return (
            (ts.isPropertyAccessExpression(value) && value.name.text === "constructor") ||
            (ts.isElementAccessExpression(value) &&
                (!ts.isStringLiteralLike(value.argumentExpression) ||
                    value.argumentExpression.text === "constructor")) ||
            (ts.isIdentifier(value) && constructorAliases.has(value.text))
        );
    };
    const collectConstructorAliases = (node: ts.Node): void => {
        if (
            ts.isVariableDeclaration(node) &&
            ts.isIdentifier(node.name) &&
            node.initializer &&
            isConstructorSource(node.initializer)
        ) {
            constructorAliases.add(node.name.text);
        }
        if (
            ts.isBinaryExpression(node) &&
            node.operatorToken.kind === ts.SyntaxKind.EqualsToken &&
            ts.isIdentifier(node.left) &&
            isConstructorSource(node.right)
        ) {
            constructorAliases.add(node.left.text);
        }
        if (ts.isBindingElement(node) && ts.isIdentifier(node.name)) {
            const property = node.propertyName ?? node.name;
            // A computed key that is not a plain literal may spell `constructor`.
            const key = ts.isIdentifier(property)
                ? property.text
                : ts.isStringLiteralLike(property)
                  ? property.text
                  : ts.isComputedPropertyName(property) &&
                      ts.isStringLiteralLike(property.expression)
                    ? property.expression.text
                    : "constructor";
            if (key === "constructor") constructorAliases.add(node.name.text);
        }
        ts.forEachChild(node, collectConstructorAliases);
    };
    // Aliases can be declared after their use, so repeat until no set grows.
    for (let size = -1; size !== factories.size + loaders.size + constructorAliases.size; ) {
        size = factories.size + loaders.size + constructorAliases.size;
        collectFactories(file);
        collectLoaders(file);
        collectConstructorAliases(file);
    }
    let namesBinding = options.allConstructions === true;
    const findBinding = (node: ts.Node): void => {
        if (namesBinding) return;
        // A type-only import or export is erased at runtime and binds nothing, whether the
        // `type` marker sits on the declaration or on every specifier in its clause.
        const allTypeOnly = (elements: readonly { isTypeOnly: boolean }[]) =>
            elements.length > 0 && elements.every((element) => element.isTypeOnly);
        if (
            (ts.isImportDeclaration(node) &&
                (node.importClause?.isTypeOnly ||
                    (node.importClause?.namedBindings !== undefined &&
                        !node.importClause.name &&
                        ts.isNamedImports(node.importClause.namedBindings) &&
                        allTypeOnly(node.importClause.namedBindings.elements)))) ||
            (ts.isExportDeclaration(node) &&
                (node.isTypeOnly ||
                    (node.exportClause !== undefined &&
                        ts.isNamedExports(node.exportClause) &&
                        allTypeOnly(node.exportClause.elements)))) ||
            (ts.isImportEqualsDeclaration(node) && node.isTypeOnly)
        ) {
            return;
        }
        if (ts.isStringLiteralLike(node) && resolvesToBinding(fileName, node.text)) {
            namesBinding = true;
            return;
        }
        ts.forEachChild(node, findBinding);
    };
    findBinding(file);

    const visit = (node: ts.Node): void => {
        if (
            ts.isNewExpression(node) &&
            (options.allConstructions || /Database/.test(node.expression.getText(file)))
        ) {
            uses.opens.push(lineOf(node));
        }
        // The argument an open receives is pinned through its binding, so a shadowing
        // declaration between the pinned initializer and the open changes the recorded line.
        if (ts.isNewExpression(node) && /Database/.test(node.expression.getText(file))) {
            const argument = node.arguments?.[0];
            uses.openArguments.push(
                argument === undefined
                    ? "<none>"
                    : ts.isIdentifier(argument)
                      ? bindingLineOf(node, argument.text)
                      : argument.getText(file),
            );
        }
        // `new db.constructor(path)` reopens whatever `db` is; `new Intl.DisplayNames(...)` in
        // a module with no binding specifier cannot reach a database.
        if (
            ts.isNewExpression(node) &&
            ((!ts.isIdentifier(node.expression) &&
                (namesBinding || /\bconstructor\b/.test(node.expression.getText(file)))) ||
                (ts.isIdentifier(node.expression) && constructorAliases.has(node.expression.text)))
        ) {
            recordEscape(node);
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
        // `import native = require("node:sqlite")` binds the whole module under one name, like a
        // namespace import; `import type X = require(...)` is erased at runtime.
        if (
            ts.isImportEqualsDeclaration(node) &&
            !node.isTypeOnly &&
            ts.isExternalModuleReference(node.moduleReference) &&
            isBindingSpecifier(node.moduleReference.expression)
        ) {
            recordEscape(node);
        }
        // `Reflect.construct(ctor, args)` builds an instance with no `new` expression to classify.
        if (
            ts.isCallExpression(node) &&
            memberText(node.expression) === "Reflect.construct" &&
            (namesBinding ||
                node.arguments.some((argument) => {
                    const value = unwrap(argument);
                    return (
                        (ts.isIdentifier(value) && constructorAliases.has(value.text)) ||
                        /Database|constructor/.test(argument.getText(file))
                    );
                }))
        ) {
            recordEscape(node);
        }
        if (ts.isCallExpression(node)) {
            const callee = node.expression;
            const isImport = callee.kind === ts.SyntaxKind.ImportKeyword;
            const isRequire =
                (ts.isIdentifier(callee) && loaders.has(callee.text)) ||
                isLoaderMember(memberText(callee));
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
