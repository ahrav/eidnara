import { afterEach, describe, expect, it } from "bun:test";

import { anthropicAuthHeaders, measureAnthropic } from "./anthropic";

const originalFetch = globalThis.fetch;

afterEach(() => {
    globalThis.fetch = originalFetch;
});

describe("anthropicAuthHeaders", () => {
    it("sends a bearer token and the OAuth beta flag for OAuth auth", () => {
        expect(anthropicAuthHeaders({ type: "oauth", access: "tok" })).toEqual({
            authorization: "Bearer tok",
            "anthropic-beta": "oauth-2025-04-20",
        });
    });

    it("sends x-api-key without the OAuth beta flag for API-key auth", () => {
        expect(anthropicAuthHeaders({ type: "api", key: "sk-test" })).toEqual({
            "x-api-key": "sk-test",
        });
    });

    it("rejects entries with neither an access token nor a key", () => {
        expect(() => anthropicAuthHeaders({ type: "oauth" })).toThrow(/Anthropic auth/);
        expect(() => anthropicAuthHeaders({ type: "api" })).toThrow(/Anthropic auth/);
    });
});

describe("measureAnthropic", () => {
    it("measures with an API key and subtracts the baseline", async () => {
        const seen: Array<Record<string, string>> = [];
        const totals = [30, 50, 10];
        globalThis.fetch = (async (_input: string | URL | Request, init?: RequestInit) => {
            seen.push(init?.headers as Record<string, string>);
            return new Response(JSON.stringify({ input_tokens: totals[seen.length - 1] }), {
                status: 200,
                headers: { "content-type": "application/json" },
            });
        }) as typeof fetch;

        const result = await measureAnthropic(
            {
                label: "anthropic/claude-sonnet-4-5",
                provider: "anthropic",
                modelId: "claude-sonnet-4-5",
            },
            { type: "api", key: "sk-test" },
            "system prompt",
            [{ name: "tool", description: "Tool", input_schema: { type: "object" } }],
        );

        expect(seen).toHaveLength(3);
        for (const headers of seen) {
            expect(headers["x-api-key"]).toBe("sk-test");
            expect(headers.authorization).toBeUndefined();
            expect(headers["anthropic-beta"]).toBeUndefined();
        }
        expect(result).toEqual({ systemApi: 20, toolsApi: 40 });
    });
});
