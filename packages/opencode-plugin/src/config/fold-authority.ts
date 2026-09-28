import { visit } from "jsonc-parser";

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

const CHAIN_KEYS: readonly string[] = [...CHAIN_MODEL_KEYS, ...CHAIN_FALLBACK_KEYS];

function excludedChainValues(rawText: string): { offset: number; length: number; key: string }[] {
    const spans: { offset: number; length: number; key: string }[] = [];
    visit(rawText, {
        onLiteralValue: (value, offset, length, _line, _column, pathSupplier) => {
            const [block, key, index, ...rest] = pathSupplier();
            const chainValue =
                block === "history_summarizer" &&
                CHAIN_KEYS.includes(String(key)) &&
                (index === undefined || typeof index === "number") &&
                rest.length === 0;
            if (chainValue && typeof value === "string" && literalChainModel(value) === undefined) {
                spans.push({ offset, length, key: String(key) });
            }
        },
    });
    return spans;
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
    for (const { offset, length, key } of excludedChainValues(rawText).reverse()) {
        text = `${text.slice(0, offset)}""${text.slice(offset + length)}`;
        warnings.push(
            `Ignoring a history_summarizer.${key} value: summarizer chain keys take a literal, non-blank model id without {env:} or {file:} references.`,
        );
    }
    return { text, rejections: writtenTierRejections(written), warnings };
}

export function rejectedAuthorityKeys(paths: readonly (readonly PropertyKey[])[]): string[] {
    return paths
        .filter((path) => path.length === 2 && AUTHORITY_BLOCKS.includes(String(path[0])))
        .map((path) => `a prototype-pollution key inside ${String(path[0])}`);
}

/** A refused tier still contributes its settings outside `AUTHORITY_BLOCKS`. */
export function withoutAuthorityBlocks(written: Record<string, unknown>): Record<string, unknown> {
    return Object.fromEntries(
        Object.entries(written).filter(([key]) => !AUTHORITY_BLOCKS.includes(key)),
    );
}

/** Unresolved configuration leaves folding to the host's native compaction. */
export function withdrawUnresolvedFoldAuthority(
    config: { compaction?: { enabled?: boolean } },
    admission: ConfigAdmission,
): string[] {
    if (admission.status === "admitted") return [];
    config.compaction = { ...config.compaction, enabled: false };
    return [
        `configuration unresolved (${admission.reason}); Eidnara leaves folding to the host's native compaction until the configuration is fixed.`,
    ];
}

export type FoldAuthority =
    | { kind: "eidnara"; reason: string }
    | { kind: "native"; reason: string }
    | { kind: "unresolved"; reason: string };

export function foldAuthorityOf(load: {
    config: { enabled?: unknown; compaction?: unknown; history_summarizer?: unknown };
    admission: ConfigAdmission;
}): FoldAuthority {
    if (load.admission.status === "unresolved") {
        return { kind: "unresolved", reason: load.admission.reason };
    }
    if (load.config.enabled === false) {
        return { kind: "native", reason: "Eidnara is disabled (`enabled: false`)" };
    }
    const chain = normalizeSummarizerChain(load.config.history_summarizer);
    if (chain.length === 0) {
        return { kind: "native", reason: "no summarizer model is configured" };
    }
    if (isRecord(load.config.compaction) && load.config.compaction.enabled === false) {
        return { kind: "native", reason: "Eidnara compaction is turned off" };
    }
    return { kind: "eidnara", reason: `summarizer chain: ${chain.join(", ")}` };
}

export function describeFoldAuthority(authority: FoldAuthority): string {
    switch (authority.kind) {
        case "eidnara":
            return `Eidnara folds (${authority.reason})`;
        case "native":
            return `OpenCode's native compaction folds (${authority.reason})`;
        case "unresolved":
            return `unresolved (${authority.reason})`;
    }
}
