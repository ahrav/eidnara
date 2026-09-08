/**
 *
 */

import { providerRequestSignal } from "./request-timeout";

interface ModelTest {
    label: string;
    provider: string;
    modelId: string;
}

interface AuthEntry {
    type: string;
    access?: string;
    key?: string;
}

interface MeasureResult {
    systemApi: number | null;
    toolsApi: number | null;
}

const ANTHROPIC_BETA = "oauth-2025-04-20";
const COUNT_URL = "https://api.anthropic.com/v1/messages/count_tokens";

/** OAuth tokens go in `authorization` with the OAuth beta flag; API keys go in `x-api-key`. */
function authHeaders(auth: AuthEntry): Record<string, string> {
    if (auth.type === "oauth" && auth.access) {
        return { authorization: `Bearer ${auth.access}`, "anthropic-beta": ANTHROPIC_BETA };
    }
    if (auth.type === "api" && auth.key) {
        return { "x-api-key": auth.key };
    }
    throw new Error("Anthropic auth must be an OAuth entry with access or an api entry with key");
}

async function callCountTokens(
    body: Record<string, unknown>,
    headers: Record<string, string>,
): Promise<number> {
    const res = await fetch(COUNT_URL, {
        method: "POST",
        headers: {
            ...headers,
            "anthropic-version": "2023-06-01",
            "content-type": "application/json",
            "user-agent": "eidnara-calibration/1.0",
        },
        body: JSON.stringify(body),
        signal: providerRequestSignal(),
    });
    const text = await res.text();
    if (!res.ok) {
        throw new Error(`count_tokens HTTP ${res.status}: ${text.slice(0, 200)}`);
    }
    const json = JSON.parse(text) as { input_tokens?: number; type?: string };
    if (json.type === "error" || typeof json.input_tokens !== "number") {
        throw new Error(`count_tokens error: ${text.slice(0, 200)}`);
    }
    return json.input_tokens;
}

export async function measureAnthropic(
    test: ModelTest,
    auth: AuthEntry,
    systemText: string,
    toolsArray: unknown[],
): Promise<MeasureResult> {
    const headers = authHeaders(auth);

    const systemBody = {
        model: test.modelId,
        system: systemText,
        messages: [{ role: "user", content: "x" }],
    };
    const systemApi = await callCountTokens(systemBody, headers);

    // Tools-only request
    const toolsBody = {
        model: test.modelId,
        tools: toolsArray,
        messages: [{ role: "user", content: "x" }],
    };
    const toolsApi = await callCountTokens(toolsBody, headers);

    const baselineBody = {
        model: test.modelId,
        messages: [{ role: "user", content: "x" }],
    };
    const baseline = await callCountTokens(baselineBody, headers);
    return {
        systemApi: Math.max(0, systemApi - baseline),
        toolsApi: Math.max(0, toolsApi - baseline),
    };
}
