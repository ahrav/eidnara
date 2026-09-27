import { applyEdits, type JSONPath, modify } from "jsonc-parser";

import { parseConfigJsonc } from "../shared/jsonc-parser";
import { isRecord } from "../shared/record-type-guard";

export type ConfigAdmission = { status: "admitted" } | { status: "unresolved"; reason: string };

const CHAIN_MODEL_KEYS = ["module_model", "model"] as const;
const CHAIN_FALLBACK_KEYS = ["module_fallback_models", "fallback_models"] as const;
const AUTHORITY_BLOCKS: readonly string[] = ["history_summarizer", "compaction"];

function hasVariableReference(text: string): boolean {
    return text.includes("{env:") || text.includes("{file:");
}

function literalChainModel(value: unknown): string | undefined {
    if (typeof value !== "string") return undefined;
    const model = value.trim();
    return model === "" || hasVariableReference(model) ? undefined : model;
}

export function normalizeSummarizerChain(block: unknown): string[] {
    if (!isRecord(block)) return [];
    const moduleSelected = literalChainModel(block.module_model) !== undefined;
    const primary = moduleSelected ? block.module_model : block.model;
    const fallbacks = moduleSelected ? block.module_fallback_models : block.fallback_models;
    const values = [primary, ...(Array.isArray(fallbacks) ? fallbacks : [fallbacks])];
    const chain = values
        .map(literalChainModel)
        .filter((model): model is string => model !== undefined);
    return [...new Set(chain)];
}

function writtenTierRejections(written: unknown): string[] {
    if (!isRecord(written)) return ["the user tier is not a JSON object"];
    const rejections: string[] = [];
    const blockKeys = AUTHORITY_BLOCKS.flatMap((block) => {
        const value = written[block];
        return isRecord(value) ? Object.keys(value) : [];
    });
    if ([...Object.keys(written), ...blockKeys].some(hasVariableReference)) {
        rejections.push("a user-tier key holds a {env:} or {file:} reference");
    }
    for (const block of AUTHORITY_BLOCKS) {
        if (block in written && !isRecord(written[block])) {
            rejections.push(`${block} in the user tier is not an object`);
        }
    }
    const compaction = written.compaction;
    if (
        isRecord(compaction) &&
        "enabled" in compaction &&
        typeof compaction.enabled !== "boolean"
    ) {
        rejections.push("compaction.enabled is not a literal boolean");
    }
    const summarizer = written.history_summarizer;
    if (!isRecord(summarizer)) return rejections;
    for (const key of CHAIN_MODEL_KEYS) {
        if (key in summarizer && typeof summarizer[key] !== "string") {
            rejections.push(`history_summarizer.${key} is not a string`);
        }
    }
    for (const key of CHAIN_FALLBACK_KEYS) {
        const value = summarizer[key];
        const valid =
            value === undefined ||
            typeof value === "string" ||
            (Array.isArray(value) && value.every((item) => typeof item === "string"));
        if (!valid) {
            rejections.push(`history_summarizer.${key} is not a string or an array of strings`);
        }
    }
    return rejections;
}

function excludedChainPaths(written: unknown): JSONPath[] {
    const summarizer = isRecord(written) ? written.history_summarizer : undefined;
    if (!isRecord(summarizer)) return [];
    const paths: JSONPath[] = [];
    for (const key of [...CHAIN_MODEL_KEYS, ...CHAIN_FALLBACK_KEYS]) {
        const value = summarizer[key];
        if (typeof value === "string" && literalChainModel(value) === undefined) {
            paths.push(["history_summarizer", key]);
        } else if (Array.isArray(value)) {
            for (let index = value.length - 1; index >= 0; index--) {
                const item = value[index];
                if (typeof item === "string" && literalChainModel(item) === undefined) {
                    paths.push(["history_summarizer", key, index]);
                }
            }
        }
    }
    return paths;
}

export function screenUserTier(rawText: string): {
    text: string;
    rejections: string[];
    warnings: string[];
} {
    let written: unknown;
    try {
        written = parseConfigJsonc(rawText);
    } catch {
        return {
            text: rawText,
            rejections: ["the user tier is not valid JSONC before variable substitution"],
            warnings: [],
        };
    }
    let text = rawText;
    const warnings: string[] = [];
    for (const path of excludedChainPaths(written)) {
        text = applyEdits(text, modify(text, path, undefined, {}));
        warnings.push(
            `Ignoring a history_summarizer.${String(path[1])} value: summarizer chain keys take a literal, non-blank model id without {env:} or {file:} references.`,
        );
    }
    return { text, rejections: writtenTierRejections(written), warnings };
}

export function rejectedAuthorityKeys(paths: readonly (readonly PropertyKey[])[]): string[] {
    return paths
        .filter((path) => path.length > 1 && AUTHORITY_BLOCKS.includes(String(path[0])))
        .map((path) => `a prototype-pollution key inside ${String(path[0])}`);
}
