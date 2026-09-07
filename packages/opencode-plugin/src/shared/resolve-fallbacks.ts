/**
 * Canonicalization through `parseProviderModel` collapses equivalent provider/model
 * spellings before `dedupe`, so one pair is attempted once.
 */
export function resolveFallbackChain(
    userFallbacks: readonly string[] | string | undefined,
): string[] {
    const userList = normalizeUserFallbacks(userFallbacks);
    const canonical: string[] = [];
    for (const spec of userList) {
        const parsed = parseProviderModel(spec);
        if (parsed) canonical.push(`${parsed.providerID}/${parsed.modelID}`);
    }
    return dedupe(canonical);
}

function normalizeUserFallbacks(userFallbacks: readonly string[] | string | undefined): string[] {
    if (!userFallbacks) return [];
    if (typeof userFallbacks === "string") {
        const trimmed = userFallbacks.trim();
        return trimmed ? [trimmed] : [];
    }
    return userFallbacks.map((s) => s.trim()).filter((s) => s.length > 0);
}

function dedupe(list: string[]): string[] {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const item of list) {
        if (seen.has(item)) continue;
        seen.add(item);
        out.push(item);
    }
    return out;
}

/**
 * parseProviderModel splits at the first `/`; modelID may contain `/`.
 * (e.g. `lemonade/GLM-4.7-Flash-GGUF/main`).
 */
export function parseProviderModel(spec: string): { providerID: string; modelID: string } | null {
    const slash = spec.indexOf("/");
    if (slash < 0) return null;
    const providerID = spec.slice(0, slash).trim();
    const modelID = spec.slice(slash + 1).trim();
    if (providerID.length === 0 || modelID.length === 0) return null;
    return { providerID, modelID };
}

/**
 * `client.session.prompt` body.
 */
export function modelBodyField(spec: string | undefined): {
    model?: { providerID: string; modelID: string };
} {
    if (!spec) return {};
    const parsed = parseProviderModel(spec);
    return parsed ? { model: parsed } : {};
}
