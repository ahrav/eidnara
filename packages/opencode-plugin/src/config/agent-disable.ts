/**
 */
export const COMPACTION_ENABLED_PATH = `compaction${"."}enabled`;

export function isContextResearcherRunnable(config: {
    context_researcher?: { disable?: boolean } | null;
}): boolean {
    return !!config.context_researcher && config.context_researcher.disable !== true;
}

export function isHistorySummarizerRunnable(config: {
    history_summarizer?: { disable?: boolean } | null;
}): boolean {
    return config.history_summarizer?.disable !== true;
}

/**
 */
export function isCompactionEnabled(config: {
    compaction?: { enabled?: boolean } | null;
}): boolean {
    return config.compaction?.enabled !== false;
}
