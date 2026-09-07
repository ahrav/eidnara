/**
 *
 * Feature and tool code use this token-counting contract without importing the transform runtime hook module.
 * The hook module re-exports the token-counting symbols, preserving existing imports.
 */

import { existsSync, readFileSync, realpathSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { getErrorMessage } from "./error-message";

// Synchronous `require` preserves the `estimateTokens` API and defers both package loads until the first non-empty call.
type TokenizerLike = {
    encode: (text: string, allowedSpecial: string) => number[];
};
type TokenizerConstructor = new (encoding: unknown) => TokenizerLike;

const TOKENIZER_PACKAGE_DIRS = [
    ["@eidnara", "opencode"],
    ["@eidnara", "pi"],
] as const;
let tokenizer: TokenizerLike | undefined;
/** `tokenizerLoadAttempted` prevents `getTokenizer` from retrying a failed bare `require`.
 * `getTokenizer` leaves it unset while `tokenizerLoadPromise` is pending, so an in-flight
 * `preloadTokenizer` still publishes its result.
 * */
let tokenizerLoadAttempted = false;
/** `tokenizerPreloadAttempted` gates `preloadTokenizer`'s asynchronous installed-package search.
 * A failed synchronous `require` does not prevent `preloadTokenizer` from finding an installed package on disk.
 * */
let tokenizerPreloadAttempted = false;
/** `tokenizerPoisoned` is set when a constructed tokenizer fails to encode.
 * After an encode failure produces heuristic estimates, later loads cannot replace the fallback estimator, preventing identical text from alternating between exact and approximate counts.
 * */
let tokenizerPoisoned = false;
let tokenizerLoadPromise: Promise<boolean> | undefined;
/** Each failure cause warns once, so an encode failure after a recovered load is still reported. */
const tokenizerWarningsSent = new Set<"load" | "encode">();

/**
 * Candidate `ai-tokenizer` locations: the OpenCode cache and the runtime entry's ancestors.
 * `process.cwd()` is excluded so a checked-out repository cannot supply the module this process
 * imports.
 */
function tokenizerPackageRoots(): string[] {
    const openCodeCache = join(process.env.XDG_CACHE_HOME ?? join(homedir(), ".cache"), "opencode");
    const candidates: string[] = [];
    for (const packageDir of TOKENIZER_PACKAGE_DIRS) {
        // `tokenizerPackageRoots` prefers the plugin-nested `ai-tokenizer` dependency to a conflicting host-hoisted version.
        candidates.push(
            join(openCodeCache, "node_modules", ...packageDir, "node_modules", "ai-tokenizer"),
        );
    }
    candidates.push(join(openCodeCache, "node_modules", "ai-tokenizer"));

    if (process.argv[1]) {
        let ancestor = dirname(resolve(process.argv[1]));
        while (true) {
            candidates.push(join(ancestor, "node_modules", "ai-tokenizer"));
            const parent = dirname(ancestor);
            if (parent === ancestor) break;
            ancestor = parent;
        }
    }
    return [...new Set(candidates)];
}

function packageImportTarget(value: unknown): string | undefined {
    if (typeof value === "string") return value;
    if (!value || typeof value !== "object") return undefined;
    const conditions = value as Record<string, unknown>;
    return packageImportTarget(conditions.import) ?? packageImportTarget(conditions.default);
}

type TokenizerImportPaths = { tokenizerPath: string; encodingPath: string };

/** Every candidate root whose `package.json` resolves both entry points, in search order. */
function findTokenizerImportPaths(): TokenizerImportPaths[] {
    const found: TokenizerImportPaths[] = [];
    for (const packageRoot of tokenizerPackageRoots()) {
        const packageJsonPath = join(packageRoot, "package.json");
        if (!existsSync(packageJsonPath)) continue;
        try {
            const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8")) as {
                module?: unknown;
                main?: unknown;
                exports?: Record<string, unknown>;
            };
            const tokenizerTarget =
                packageImportTarget(packageJson.exports?.["."]) ??
                (typeof packageJson.module === "string" ? packageJson.module : undefined) ??
                (typeof packageJson.main === "string" ? packageJson.main : undefined);
            const encodingTarget = packageImportTarget(packageJson.exports?.["./encoding/claude"]);
            if (!tokenizerTarget || !encodingTarget) continue;
            found.push({
                tokenizerPath: realpathSync(join(packageRoot, tokenizerTarget)),
                encodingPath: realpathSync(join(packageRoot, encodingTarget)),
            });
        } catch {}
    }
    return found;
}

/**
 * `ai-tokenizer` reads `stringEncoder[piece]` with property access, so `valueOf` resolves to
 * `Object.prototype.valueOf` instead of a rank. A null-prototype table makes that lookup miss;
 * `ai-tokenizer` then falls through to byte-pair merging.
 */
function withNullPrototypeStringEncoder(claudeEncoding: unknown): unknown {
    if (!claudeEncoding || typeof claudeEncoding !== "object") return claudeEncoding;
    const encoding = claudeEncoding as { stringEncoder?: unknown };
    const table = encoding.stringEncoder;
    if (!table || typeof table !== "object" || Object.getPrototypeOf(table) === null) {
        return claudeEncoding;
    }
    return { ...encoding, stringEncoder: Object.assign(Object.create(null), table) };
}

function constructTokenizer(tokenizerModule: unknown, claudeEncoding: unknown): TokenizerLike {
    const typedModule = tokenizerModule as {
        default?: TokenizerConstructor;
        Tokenizer?: TokenizerConstructor;
    };
    const Tokenizer = typedModule.default ?? typedModule.Tokenizer;
    if (!Tokenizer) {
        throw new Error("ai-tokenizer does not expose a Tokenizer constructor");
    }
    return new Tokenizer(withNullPrototypeStringEncoder(claudeEncoding));
}

function loadTokenizer(): TokenizerLike {
    // Non-literal specifiers prevent Bun's bundler from folding the Claude vocabulary into the eager chunk.
    const requireFromThisModule = createRequire(import.meta.url);
    return constructTokenizer(
        requireFromThisModule("ai-" + "tokenizer"),
        requireFromThisModule("ai-tokenizer/encoding/" + "claude"),
    );
}

/** A candidate that fails to import or construct does not shadow a later working install. */
async function loadTokenizerFromInstalledPackage(): Promise<TokenizerLike> {
    const candidates = findTokenizerImportPaths();
    if (candidates.length === 0) {
        throw new Error(
            "ai-tokenizer was not found under the OpenCode cache or runtime node_modules roots",
        );
    }
    let lastError: unknown;
    for (const paths of candidates) {
        try {
            const [tokenizerModule, claudeEncoding] = await Promise.all([
                import(pathToFileURL(paths.tokenizerPath).href),
                import(pathToFileURL(paths.encodingPath).href),
            ]);
            return constructTokenizer(tokenizerModule, claudeEncoding);
        } catch (error) {
            lastError = error;
        }
    }
    throw lastError;
}

function warnTokenizerFallback(cause: "load" | "encode", error: unknown): void {
    if (tokenizerWarningsSent.has(cause)) return;
    tokenizerWarningsSent.add(cause);
    const reason = getErrorMessage(error);
    const event =
        cause === "load"
            ? "ai-tokenizer is unavailable"
            : "ai-tokenizer failed to encode and is disabled";
    console.warn(
        `[eidnara] ${event}; using approximate character-based token counts for this process. Token budgets, persisted per-message counts, and protected-tail/compartment boundaries may be less accurate until restart:`,
        reason,
    );
}

export async function preloadTokenizer(): Promise<boolean> {
    if (tokenizer) return true;
    if (tokenizerPoisoned || tokenizerPreloadAttempted) return false;
    if (tokenizerLoadPromise) return tokenizerLoadPromise;

    tokenizerLoadPromise = (async () => {
        try {
            try {
                tokenizer = loadTokenizer();
            } catch {
                tokenizer = await loadTokenizerFromInstalledPackage();
            }
            tokenizerLoadAttempted = true;
            return true;
        } catch (error) {
            tokenizerLoadAttempted = true;
            warnTokenizerFallback("load", error);
            return false;
        } finally {
            tokenizerPreloadAttempted = true;
            tokenizerLoadPromise = undefined;
        }
    })();
    return tokenizerLoadPromise;
}

function getTokenizer(): TokenizerLike | undefined {
    if (tokenizer || tokenizerLoadAttempted) return tokenizer;
    // Do not start a synchronous load while `tokenizerLoadPromise` is set.
    if (tokenizerLoadPromise) return undefined;
    tokenizerLoadAttempted = true;
    try {
        tokenizer = loadTokenizer();
    } catch (error) {
        warnTokenizerFallback("load", error);
    }
    return tokenizer;
}

function estimateTokensHeuristically(text: string): number {
    return Math.ceil(text.length / 3.5);
}

export function estimateTokens(text: string): number {
    if (!text) return 0;
    const activeTokenizer = getTokenizer();
    if (!activeTokenizer) return estimateTokensHeuristically(text);
    try {
        // `estimateTokens` uses `allowedSpecial="all"` so literal special-token strings do not throw.
        return activeTokenizer.encode(text, "all").length;
    } catch (error) {
        // `estimateTokens` must not fail a prompt; after an encode failure, it latches the deterministic fallback for the process.
        tokenizer = undefined;
        tokenizerLoadAttempted = true;
        tokenizerPoisoned = true;
        warnTokenizerFallback("encode", error);
        return estimateTokensHeuristically(text);
    }
}
