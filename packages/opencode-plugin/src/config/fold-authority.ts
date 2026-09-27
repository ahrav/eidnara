import { parseConfigJsonc } from "../shared/jsonc-parser";
import { isRecord } from "../shared/record-type-guard";

export type ConfigAdmission = { status: "admitted" } | { status: "unresolved"; reason: string };

const CHAIN_MODEL_KEYS = ["module_model", "model"] as const;
const CHAIN_FALLBACK_KEYS = ["module_fallback_models", "fallback_models"] as const;
const AUTHORITY_BLOCKS = ["history_summarizer", "compaction"] as const;

function literalChainModel(value: unknown): string | undefined {
    if (typeof value !== "string") return undefined;
    const model = value.trim();
    if (model === "" || model.includes("{env:") || model.includes("{file:")) return undefined;
    return model;
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

function screenUserTier(
    written: unknown,
    substituted: Record<string, unknown>,
): { rejections: string[]; warnings: string[] } {
    const rejections: string[] = [];
    const warnings: string[] = [];
    if (!isRecord(written)) {
        return { rejections: ["the user tier is not a JSON object"], warnings };
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
    if (!isRecord(summarizer)) return { rejections, warnings };
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
        if (!valid)
            rejections.push(`history_summarizer.${key} is not a string or an array of strings`);
    }

    const target = substituted.history_summarizer;
    if (!isRecord(target)) return { rejections, warnings };
    const excluded = (key: string) =>
        warnings.push(
            `Ignoring a history_summarizer.${key} value: summarizer chain keys take a literal, non-blank model id without {env:} or {file:} references.`,
        );
    for (const key of [...CHAIN_MODEL_KEYS, ...CHAIN_FALLBACK_KEYS]) {
        const value = summarizer[key];
        if (typeof value === "string" && literalChainModel(value) === undefined) {
            excluded(key);
            delete target[key];
        } else if (Array.isArray(value)) {
            const kept = value.filter(
                (item) => typeof item !== "string" || literalChainModel(item) !== undefined,
            );
            if (kept.length !== value.length) {
                excluded(key);
                target[key] = kept;
            }
        }
    }
    return { rejections, warnings };
}

export function screenUserTierText(
    rawText: string,
    substituted: Record<string, unknown>,
): { rejections: string[]; warnings: string[] } {
    let written: unknown;
    try {
        written = parseConfigJsonc(rawText);
    } catch {
        return {
            rejections: ["the user tier is not valid JSONC before variable substitution"],
            warnings: [],
        };
    }
    return screenUserTier(written, substituted);
}
