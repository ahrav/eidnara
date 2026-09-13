/** Local exploratory client timing, not a daemon, transport, or end-to-end benchmark.
 * Timing includes run(), request decoding and the fake native response encode/parse round trip.
 * Fixture construction, equality checks and memory observations stay outside the timer.
 */
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { cpus, hostname, release } from "node:os";
import { MODULE_ITEM_CONTINUATION_KEY } from "../src/hooks/context/module-wire";
import { setRawMessageProvider } from "../src/hooks/context/read-session-chunk";
import type { RawMessage } from "../src/hooks/context/read-session-raw";
import {
    createRustModeTransform,
    type RustModeModuleClient,
    type RustModeTransformDeps,
} from "../src/hooks/context/rust-mode-transform";
import type { MessageLike } from "../src/hooks/context/tag-content-primitives";
import { BoundedSessionMap } from "../src/shared/bounded-session-map";
import { serializedJsonText } from "../src/shared/host-client/serialized-json-body";

interface Options {
    samples: number;
    warmup: number;
    messages: number;
    bytes: number;
    json?: string;
}

function parseOptions(argv: readonly string[]): Options {
    const options: Options = { samples: 30, warmup: 3, messages: 1_000, bytes: 2_048 };
    for (let index = 0; index < argv.length; index += 2) {
        const flag = argv[index];
        const value = argv[index + 1];
        assert(value, `missing value for ${flag}`);
        if (flag === "--json") options.json = value;
        else {
            const key = flag.slice(2);
            assert(
                ["samples", "warmup", "messages", "bytes"].includes(key),
                `unknown flag ${flag}`,
            );
            const minimum = key === "warmup" ? 0 : 1;
            assert(
                Number.isSafeInteger(Number(value)) && Number(value) >= minimum,
                `invalid ${flag}`,
            );
            options[key as "samples" | "warmup" | "messages" | "bytes"] = Number(value);
        }
    }
    return options;
}

function fixture(sessionId: string, count: number, bytes: number) {
    const rows = Array.from({ length: count }, (_, index) => ({
        id: `m-${index + 1}`,
        timeCreated: index + 1,
        contributesOrdinal: true as const,
        hasValidInfo: true as const,
    }));
    const unregister = setRawMessageProvider(sessionId, {
        readMessages: () => rows as unknown as RawMessage[],
        readMessageOrdinalPage: (after, limit) =>
            rows
                .filter(
                    (row) =>
                        !after ||
                        row.timeCreated > after.timeCreated ||
                        (row.timeCreated === after.timeCreated && row.id > after.id),
                )
                .slice(0, limit),
        getStoredMessageCount: () => rows.length,
    });
    const words = "alpha beta gamma delta epsilon zeta eta theta ";
    const text = words.repeat(Math.ceil(bytes / words.length)).slice(0, bytes);
    const messages: MessageLike[] = rows.map((row, index) => ({
        info: {
            id: row.id,
            role: index % 2 === 0 ? "user" : "assistant",
            sessionID: sessionId,
            ...(index % 2 === 1 ? { providerID: "anthropic", modelID: "claude-sonnet-4" } : {}),
        },
        parts: [{ type: "text", text }],
    }));
    return { messages, unregister };
}

function makeDeps(): RustModeTransformDeps {
    return {
        client: {
            app: { agents: async () => ({ data: [] }) },
            session: { get: async () => ({ data: { directory: "/tmp/project" } }) },
        } as never,
        contextUsageMap: new BoundedSessionMap(8),
        protectedTags: 4,
        clearReasoningAge: 50,
        cacheTtl: "5m",
        directory: "/tmp/project",
        sessionDirectoryBySession: new Map(),
        isSubagentSession: () => false,
        systemPromptHashFor: () => "",
    };
}

function echoClient() {
    const stats = {
        requestBytes: 0,
        responseBytes: 0,
        pages: 0,
        chunks: 0,
        completed: 0,
        deltas: 0,
    };
    let native: Array<Record<string, unknown>> = [];
    let chunks: string[] = [];
    let chunkTotal = 0;
    let pageId: unknown;
    let pageTotal: unknown;
    let pageIndex = 0;
    const client: RustModeModuleClient = {
        call: async ({ method, body }) => {
            if (method !== "transform") return { ok: true };
            const text = serializedJsonText(body);
            assert(text !== undefined, "expected authoritative serialized request");
            const request = JSON.parse(text);
            stats.requestBytes += Buffer.byteLength(text);
            stats.pages += 1;
            if (request.transform_page_id !== undefined) {
                if (pageIndex === 0) {
                    pageId = request.transform_page_id;
                    pageTotal = request.transform_page_total;
                }
                assert.equal(request.transform_page_id, pageId);
                assert.equal(request.transform_page_total, pageTotal);
                assert.equal(request.transform_page_index, pageIndex++);
                assert.equal(request.transform_page_complete, pageIndex === pageTotal);
            }
            for (const value of request.native_messages) {
                const marker = value[MODULE_ITEM_CONTINUATION_KEY];
                if (marker) {
                    assert.equal(marker.field, "native_messages");
                    assert.equal(marker.item_index, native.length);
                    assert.equal(marker.chunk_index, chunks.length);
                    if (chunks.length === 0) chunkTotal = marker.chunk_total;
                    assert.equal(marker.chunk_total, chunkTotal);
                    chunks.push(value.chunk);
                    stats.chunks += 1;
                    if (chunks.length === chunkTotal) {
                        native.push(JSON.parse(chunks.join("")));
                        chunks = [];
                    }
                } else {
                    assert.equal(chunks.length, 0);
                    native.push(value);
                }
            }
            if (request.transform_page_complete === false) {
                stats.responseBytes += Buffer.byteLength(JSON.stringify({ staged: true }));
                return { staged: true };
            }
            assert.equal(chunks.length, 0, "unfinished native continuation");
            if ((native[0]?.info as { id?: string })?.id === "m-1") {
                native[0] = { ...native[0], benchmarkPublished: true };
            }
            const tailDelta = request.tail_delta as
                | { after: string; native_replace_from: number }
                | undefined;
            const response: Record<string, unknown> = tailDelta
                ? {
                      native_messages_delta: {
                          after: tailDelta.after,
                          replace_from: tailDelta.native_replace_from,
                          messages: native,
                      },
                  }
                : { native_messages: native };
            const encoded = JSON.stringify(response);
            stats.responseBytes += Buffer.byteLength(encoded);
            stats.completed += 1;
            stats.deltas += Number(tailDelta !== undefined);
            native = [];
            pageIndex = 0;
            return JSON.parse(encoded);
        },
    };
    return { client, stats };
}

function sha256(value: string | Buffer): string {
    return createHash("sha256").update(value).digest("hex");
}

async function timePass(
    sessionId: string,
    transform: ReturnType<typeof createRustModeTransform>,
    messages: MessageLike[],
    stats: ReturnType<typeof echoClient>["stats"],
    warm: boolean,
) {
    const before = { ...stats };
    const inputJson = JSON.stringify(messages);
    const expected = JSON.parse(inputJson);
    expected[0].benchmarkPublished = true;
    const output = { messages: [...messages] as unknown[] };
    const hostArray = output.messages;
    const startedAt = performance.now();
    await transform.run(sessionId, messages, output);
    const passMs = performance.now() - startedAt;
    assert.equal(stats.completed - before.completed, 1, "missing completion or unexpected retry");
    assert.equal(stats.deltas - before.deltas, Number(warm), "unexpected full/delta path");
    assert.equal(output.messages, hostArray, "host array identity changed");
    assert.deepEqual(output.messages, expected, "approved output was not published exactly");
    assert.equal(JSON.stringify(messages), inputJson, "source mutated");
    assert.equal(transform.getState(sessionId).consecutiveFailures, 0);
    return {
        passMs,
        inputSha256: sha256(inputJson),
        requestBytes: stats.requestBytes - before.requestBytes,
        responseBytes: stats.responseBytes - before.responseBytes,
        pages: stats.pages - before.pages,
        chunks: stats.chunks - before.chunks,
        deltas: stats.deltas - before.deltas,
        memory: process.memoryUsage(),
    };
}

async function runCase(
    name: string,
    options: Options,
    messageCount: number,
    bytes: number,
    warm: boolean,
) {
    const observations = [];
    for (let sample = 0; sample < options.warmup + options.samples; sample += 1) {
        const sessionId = `bench-${name}-${sample}`;
        const { messages, unregister } = fixture(sessionId, messageCount + Number(warm), bytes);
        const { client, stats } = echoClient();
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        try {
            const base = messages.slice(0, messageCount);
            const prime = warm
                ? await timePass(sessionId, transform, base, stats, false)
                : undefined;
            observations.push({
                warmup: sample < options.warmup,
                prime,
                ...(await timePass(sessionId, transform, messages, stats, warm)),
            });
        } finally {
            transform.clearSession(sessionId);
            unregister();
        }
    }
    return { name, messageCount, bytes, warm, observations };
}

function quantile(values: number[], q: number): number {
    const sorted = [...values].sort((a, b) => a - b);
    const position = (sorted.length - 1) * q;
    const lower = Math.floor(position);
    const upper = Math.ceil(position);
    return sorted[lower] + (sorted[upper] - sorted[lower]) * (position - lower);
}

async function main(): Promise<void> {
    const options = parseOptions(process.argv.slice(2));
    const results = [];
    for (const large of [true, false]) {
        for (const warm of [false, true]) {
            const name = `${large ? "large" : "small"}-${warm ? "warm-append" : "cold"}`;
            const count = large ? options.messages : 8;
            const bytes = large ? options.bytes : 256;
            results.push(await runCase(name, options, count, bytes, warm));
        }
    }
    const report = results.map(({ name, observations }) => {
        const samples = observations.filter((sample) => !sample.warmup);
        const pass = samples.map((sample) => sample.passMs);
        return {
            name,
            samples: samples.length,
            pass_median_ms: Number(quantile(pass, 0.5).toFixed(3)),
            pass_p95_ms: Number(quantile(pass, 0.95).toFixed(3)),
            pass_min_ms: Number(Math.min(...pass).toFixed(3)),
            response_bytes_median: quantile(
                samples.map((sample) => sample.responseBytes),
                0.5,
            ),
        };
    });
    const evidence = {
        scope: "local exploratory; not U5 acceptance or end-to-end evidence",
        options,
        artifact: {
            revision: process.env.BENCH_SOURCE_REVISION ?? "unidentified-workspace",
            harnessSha256: sha256(readFileSync(import.meta.path)),
            lockSha256: sha256(readFileSync(new URL("../../../bun.lock", import.meta.url))),
        },
        runtime: {
            bun: Bun.version,
            executable: process.execPath,
            platform: process.platform,
            arch: process.arch,
            kernel: release(),
            host: hostname(),
            cpu: cpus()[0]?.model,
        },
        schedule: "sequential cases and instances; one process; no independent paired samples",
        report,
        results,
    };
    console.table(report);
    if (options.json) writeFileSync(options.json, `${JSON.stringify(evidence, null, 2)}\n`);
}

await main();
