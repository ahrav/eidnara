/**
 *
 * The script measures drift between local ai-tokenizer estimates and provider input-token counts.
 * The script uses the production system prompt and tool definitions.
 * The script sends requests directly to each provider using credentials from `<XDG_DATA_HOME>/opencode/auth.json`.
 * The script does not depend on a running OpenCode process.
 *
 * The script sends a system-only request containing the production system prompt and a minimal user message.
 * The script sends a tools-only request containing the production tools array and a minimal user message.
 * The script reads the provider's input-token count from its usage field.
 * The providers subtract a minimal-user baseline request so `api` counts cover only the system prompt or the tools.
 * Raw mode counts with the `claude` encoding because `src/shared/token-estimator.ts` counts with that encoding for every model, so `ratio_raw` applies directly to production counts.
 * SDK mode counts with the model's own encoding and subtracts the SDK count of the same minimal-user baseline so `local_sdk` and `api` measure the same component.
 *
 * The script emits per-model raw and SDK ratios for system prompts and tools.
 * A run with `--only` or `--providers` merges its measurements into the existing `results.json` by label; an unfiltered run replaces the file.
 * The process exits nonzero when any measurement fails.
 *
 * Usage:
 *   bun run packages/opencode-plugin/scripts/calibrate-tokenizer/index.ts
 *   bun run packages/opencode-plugin/scripts/calibrate-tokenizer/index.ts --only anthropic/claude-opus-4-7
 *   bun run packages/opencode-plugin/scripts/calibrate-tokenizer/index.ts --providers anthropic,openai
 */
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import Tokenizer, { models as aiTokenizerModels } from "ai-tokenizer";
import * as cl100kEncoding from "ai-tokenizer/encoding/cl100k_base";
import * as claudeEncoding from "ai-tokenizer/encoding/claude";
import * as o200kEncoding from "ai-tokenizer/encoding/o200k_base";
import * as p50kEncoding from "ai-tokenizer/encoding/p50k_base";
import { count as sdkCount } from "ai-tokenizer/sdk";

import { getDataDir } from "../../src/shared/data-path";
import { measureAnthropic } from "./providers/anthropic";
import { measureOpenAICodex } from "./providers/openai-codex";
import { measureOpenAICompatible } from "./providers/openai-compatible";

interface AuthFile {
    [provider: string]:
        | { type: "oauth"; access: string; refresh?: string; expires?: number }
        | { type: "api"; key: string };
}

interface ModelTest {
    label: string;
    provider: string;
    modelId: string;
    tokenizerKey: string | null;
}

interface ModelTestSet {
    tests: ModelTest[];
}

interface MeasurementResult {
    label: string;
    provider: string;
    modelId: string;
    tokenizerKey: string | null;
    systemTokens: {
        local_raw: number;
        local_sdk: number | null;
        api: number | null;
        ratio_raw: number | null;
        ratio_sdk: number | null;
    };
    toolsTokens: {
        local_raw: number;
        local_sdk: number | null;
        api: number | null;
        ratio_raw: number | null;
        ratio_sdk: number | null;
    };
    error: string | null;
    durationMs: number;
}

const ENCODINGS = {
    cl100k_base: cl100kEncoding,
    claude: claudeEncoding,
    o200k_base: o200kEncoding,
    p50k_base: p50kEncoding,
};

// biome-ignore lint/suspicious/noExplicitAny: ai-tokenizer model types cannot represent this cast
const ALL_MODELS = aiTokenizerModels as unknown as Record<string, any>;

/**
 * The Codex backend requires the `ChatGPT-Account-Id` header to route requests to the authenticated account.
 */
function extractCodexAccountId(accessToken: string): string | undefined {
    try {
        const parts = accessToken.split(".");
        if (parts.length !== 3) return undefined;
        const payload = parts[1];
        if (!payload) return undefined;
        const padded = payload + "=".repeat((4 - (payload.length % 4)) % 4);
        const decoded = Buffer.from(padded, "base64").toString("utf-8");
        const claims = JSON.parse(decoded) as Record<string, unknown>;
        const auth = claims["https://api.openai.com/auth"] as Record<string, unknown> | undefined;
        return auth?.chatgpt_account_id as string | undefined;
    } catch {
        return undefined;
    }
}

function pickEncoding(tokenizerKey: string | null): unknown {
    if (!tokenizerKey) return claudeEncoding;
    const m = ALL_MODELS[tokenizerKey];
    if (!m) return claudeEncoding;
    const enc = ENCODINGS[m.encoding as keyof typeof ENCODINGS];
    return enc ?? claudeEncoding;
}

/** Baseline request whose SDK count is subtracted to isolate the system and tools token counts. */
const BASELINE_USER_MESSAGE = { role: "user" as const, content: "x" };

/** Raw counts use the `claude` encoding because `estimateTokens` constructs it for every model. */
// biome-ignore lint/suspicious/noExplicitAny: ai-tokenizer's encoding modules do not satisfy its constructor type
const rawTokenizer = new Tokenizer(claudeEncoding as any);

function localCounts(
    systemText: string,
    toolsArray: unknown[],
    tokenizerKey: string | null,
): {
    systemRaw: number;
    systemSdk: number | null;
    toolsRaw: number;
    toolsSdk: number | null;
    sdkError: string | null;
} {
    const systemRaw = rawTokenizer.count(systemText);
    const toolsRaw = rawTokenizer.count(JSON.stringify(toolsArray));

    let systemSdk: number | null = null;
    let toolsSdk: number | null = null;
    let sdkError: string | null = null;

    if (tokenizerKey && !ALL_MODELS[tokenizerKey]) {
        sdkError = `tokenizer key ${tokenizerKey} is not in ai-tokenizer's model catalog`;
    } else if (tokenizerKey) {
        const m = ALL_MODELS[tokenizerKey];
        // biome-ignore lint/suspicious/noExplicitAny: ai-tokenizer's SDK `Tokenizer` type does not accept the encoding-constructed instance
        const sdkTokenizer = new Tokenizer(pickEncoding(tokenizerKey) as any) as any;
        try {
            const baselineSdk = sdkCount({
                tokenizer: sdkTokenizer,
                model: m,
                messages: [BASELINE_USER_MESSAGE],
            }).total;
            const sysResult = sdkCount({
                tokenizer: sdkTokenizer,
                model: m,
                messages: [{ role: "system", content: systemText }, BASELINE_USER_MESSAGE],
            });
            const tools = (toolsArray as Array<Record<string, unknown>>).map((t) => ({
                type: "function" as const,
                name: t.name as string,
                description: t.description as string,
                inputSchema: t.input_schema as Record<string, unknown>,
            }));
            const toolsResult = sdkCount({
                tokenizer: sdkTokenizer,
                model: m,
                messages: [BASELINE_USER_MESSAGE],
                tools,
            });
            systemSdk = Math.max(0, sysResult.total - baselineSdk);
            toolsSdk = Math.max(0, toolsResult.total - baselineSdk);
        } catch (e) {
            systemSdk = null;
            toolsSdk = null;
            sdkError = `sdk count failed for ${tokenizerKey}: ${e instanceof Error ? e.message : String(e)}`;
        }
    }

    return { systemRaw, systemSdk, toolsRaw, toolsSdk, sdkError };
}

async function measureOne(
    test: ModelTest,
    auth: AuthFile,
    systemText: string,
    toolsArray: unknown[],
): Promise<MeasurementResult> {
    const start = Date.now();
    const local = localCounts(systemText, toolsArray, test.tokenizerKey);
    let systemApi: number | null = null;
    let toolsApi: number | null = null;
    let error: string | null = null;
    try {
        const authEntry = auth[test.provider];
        if (!authEntry) throw new Error(`No auth for provider ${test.provider}`);

        // `extractCodexAccountId` reads `chatgpt_account_id` from the `https://api.openai.com/auth` JWT claim.
        const useCodex =
            test.provider === "openai" && authEntry.type === "oauth" && !!authEntry.access;
        let measurements: { systemApi: number | null; toolsApi: number | null };
        if (test.provider === "anthropic") {
            measurements = await measureAnthropic(test, authEntry, systemText, toolsArray);
        } else if (useCodex) {
            const accountId = extractCodexAccountId(authEntry.access);
            measurements = await measureOpenAICodex(
                test,
                { type: "oauth", access: authEntry.access, accountId },
                systemText,
                toolsArray,
            );
        } else {
            measurements = await measureOpenAICompatible(test, authEntry, systemText, toolsArray);
        }
        systemApi = measurements.systemApi;
        toolsApi = measurements.toolsApi;
    } catch (e) {
        error = e instanceof Error ? e.message : String(e);
    }
    if (local.sdkError) {
        error = error ? `${error}; ${local.sdkError}` : local.sdkError;
    }

    const durationMs = Date.now() - start;
    return {
        label: test.label,
        provider: test.provider,
        modelId: test.modelId,
        tokenizerKey: test.tokenizerKey,
        systemTokens: {
            local_raw: local.systemRaw,
            local_sdk: local.systemSdk,
            api: systemApi,
            ratio_raw: systemApi != null ? +(systemApi / local.systemRaw).toFixed(3) : null,
            ratio_sdk:
                systemApi != null && local.systemSdk
                    ? +(systemApi / local.systemSdk).toFixed(3)
                    : null,
        },
        toolsTokens: {
            local_raw: local.toolsRaw,
            local_sdk: local.toolsSdk,
            api: toolsApi,
            ratio_raw: toolsApi != null ? +(toolsApi / local.toolsRaw).toFixed(3) : null,
            ratio_sdk:
                toolsApi != null && local.toolsSdk ? +(toolsApi / local.toolsSdk).toFixed(3) : null,
        },
        error,
        durationMs,
    };
}

const USAGE = `Usage: bun run packages/opencode-plugin/scripts/calibrate-tokenizer/index.ts [--only <label|modelId>] [--providers <a,b,...>]`;

/**
 * Merging by label keeps a one-model recalibration from deleting every other
 * model's stored measurement.
 */
function mergeResults(
    outPath: string,
    fresh: MeasurementResult[],
    filtered: boolean,
): MeasurementResult[] {
    if (!filtered || !existsSync(outPath)) return fresh;
    const existing = JSON.parse(readFileSync(outPath, "utf-8")) as MeasurementResult[];
    const freshByLabel = new Map(fresh.map((r) => [r.label, r]));
    const merged = existing.map((r) => freshByLabel.get(r.label) ?? r);
    const existingLabels = new Set(existing.map((r) => r.label));
    for (const r of fresh) {
        if (!existingLabels.has(r.label)) merged.push(r);
    }
    return merged;
}

/**
 * Unknown arguments abort before credentials load because a mistyped selector
 * would otherwise run the full paid sweep against every provider.
 */
function parseArgs(): { only: string | null; providers: string[] | null } {
    const args = process.argv.slice(2);
    let only: string | null = null;
    let providers: string[] | null = null;
    for (let i = 0; i < args.length; i++) {
        const arg = args[i];
        if (arg === "--only") {
            const v = args[++i];
            if (!v) throw new Error(`--only requires a value\n${USAGE}`);
            only = v;
        } else if (arg === "--providers") {
            const v = args[++i];
            if (!v) throw new Error(`--providers requires a value\n${USAGE}`);
            providers = v
                .split(",")
                .map((s) => s.trim())
                .filter(Boolean);
            if (providers.length === 0) throw new Error(`--providers is empty\n${USAGE}`);
        } else {
            throw new Error(`Unknown argument: ${arg}\n${USAGE}`);
        }
    }
    return { only, providers };
}

async function main(): Promise<void> {
    const { only, providers } = parseArgs();
    const here = fileURLToPath(new URL(".", import.meta.url));
    const systemText = readFileSync(join(here, "fixture-system.txt"), "utf-8");
    const toolsArray = JSON.parse(
        readFileSync(join(here, "fixture-tools.json"), "utf-8"),
    ) as unknown[];
    const testSet = JSON.parse(readFileSync(join(here, "models.json"), "utf-8")) as ModelTestSet;

    let tests = testSet.tests;
    if (only) tests = tests.filter((t) => t.label === only || t.modelId === only);
    if (providers) tests = tests.filter((t) => providers.includes(t.provider));
    if (tests.length === 0) {
        throw new Error(
            `No models match --only=${only ?? "-"} --providers=${providers?.join(",") ?? "-"}; models.json has ${testSet.tests.length} entries`,
        );
    }

    const auth = JSON.parse(
        readFileSync(join(getDataDir(), "opencode", "auth.json"), "utf-8"),
    ) as AuthFile;

    console.log(
        `Calibration harness: ${tests.length} models, system=${systemText.length} chars, tools=${toolsArray.length} (${JSON.stringify(toolsArray).length} chars)`,
    );
    console.log("");

    const results: MeasurementResult[] = [];
    for (const test of tests) {
        process.stdout.write(`  ${test.label.padEnd(45, " ")} ... `);
        const r = await measureOne(test, auth, systemText, toolsArray);
        if (r.error) {
            process.stdout.write(`ERROR (${r.durationMs}ms) ${r.error.slice(0, 80)}\n`);
        } else {
            process.stdout.write(
                `system=${r.systemTokens.api ?? "—"} (raw ${r.systemTokens.local_raw}, ratio ${r.systemTokens.ratio_raw ?? "—"}x), tools=${r.toolsTokens.api ?? "—"} (raw ${r.toolsTokens.local_raw}, ratio ${r.toolsTokens.ratio_raw ?? "—"}x), ${r.durationMs}ms\n`,
            );
        }
        results.push(r);
    }

    const outPath = join(here, "results.json");
    writeFileSync(
        outPath,
        JSON.stringify(
            mergeResults(outPath, results, only !== null || providers !== null),
            null,
            2,
        ),
        "utf-8",
    );
    console.log(`\nWrote ${outPath}`);

    // Summary
    console.log("\n=== Summary ===");
    console.log(
        "model".padEnd(45, " "),
        "sys.raw->api",
        " | ",
        "sys.sdk->api",
        " | ",
        "tools.raw->api",
        " | ",
        "tools.sdk->api",
    );
    for (const r of results) {
        if (r.error) {
            console.log(r.label.padEnd(45, " "), "ERROR:", r.error.slice(0, 60));
            continue;
        }
        console.log(
            r.label.padEnd(45, " "),
            (r.systemTokens.ratio_raw ?? "—").toString().padStart(11, " "),
            " | ",
            (r.systemTokens.ratio_sdk ?? "—").toString().padStart(11, " "),
            " | ",
            (r.toolsTokens.ratio_raw ?? "—").toString().padStart(13, " "),
            " | ",
            (r.toolsTokens.ratio_sdk ?? "—").toString().padStart(13, " "),
        );
    }

    const failed = results.filter((r) => r.error !== null).length;
    if (failed > 0) {
        console.error(`\n${failed} of ${results.length} measurements failed; see results.json`);
        process.exitCode = 1;
    }
}

main().catch((err) => {
    console.error("Harness failed:", err);
    process.exit(1);
});
