/** Local exploratory client timing, not a daemon, transport, or end-to-end benchmark.
 * Timing includes run(), request decoding, the fake's recipe construction, and the response
 * encode/parse round trip. Fixture construction, equality checks and memory observations stay
 * outside the timer. The fake also sizes the native suffix response the old wire would have
 * carried, as a byte comparator only; that encoding is never sent.
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

type Operation =
    | { op: "keep"; source: "input" | "previous"; start: number; count: number }
    | { op: "insert"; values: unknown[] };

/**
 * Nominates by `info.id`, confirms by JSON equality, prefers `previous` then `input`, and
 * coalesces adjacent keeps: the same policy the daemon builder applies to its final array.
 */
function buildOperations(
    output: readonly Record<string, unknown>[],
    input: readonly Record<string, unknown>[],
    previous: readonly Record<string, unknown>[] | undefined,
): { operations: Operation[]; usedPrevious: boolean } {
    const idOf = (message: Record<string, unknown>) =>
        (message.info as { id?: unknown } | undefined)?.id;
    const index = (values: readonly Record<string, unknown>[]) => {
        const byId = new Map<unknown, number>();
        values.forEach((value, position) => {
            const id = idOf(value);
            if (typeof id === "string") byId.set(id, byId.has(id) ? -1 : position);
        });
        return byId;
    };
    const inputById = index(input);
    const previousById = previous ? index(previous) : new Map<unknown, number>();
    const cursors = { input: 0, previous: 0 };
    const operations: Operation[] = [];
    let usedPrevious = false;
    const keep = (source: "input" | "previous", position: number) => {
        const last = operations.at(-1);
        if (last?.op === "keep" && last.source === source && last.start + last.count === position)
            last.count += 1;
        else operations.push({ op: "keep", source, start: position, count: 1 });
        cursors[source] = position + 1;
    };
    for (const value of output) {
        const id = idOf(value);
        // Shared references confirm without serializing, as the daemon's `Arc` identity check does.
        let text: string | undefined;
        const candidate = (
            source: "input" | "previous",
            values: readonly Record<string, unknown>[] | undefined,
            byId: Map<unknown, number>,
        ) => {
            const position = byId.get(id);
            if (values === undefined || position === undefined || position < cursors[source])
                return undefined;
            if (values[position] === value) return position;
            text ??= JSON.stringify(value);
            return JSON.stringify(values[position]) === text ? position : undefined;
        };
        const fromPrevious = candidate("previous", previous, previousById);
        if (fromPrevious !== undefined) {
            keep("previous", fromPrevious);
            usedPrevious = true;
            continue;
        }
        const fromInput = candidate("input", input, inputById);
        if (fromInput !== undefined) {
            keep("input", fromInput);
            continue;
        }
        const last = operations.at(-1);
        if (last?.op === "insert") last.values.push(value);
        else operations.push({ op: "insert", values: [value] });
    }
    return { operations, usedPrevious };
}

function echoClient() {
    const stats = {
        requestBytes: 0,
        responseBytes: 0,
        /** Bytes the native suffix response of the old wire would have carried. */
        suffixComparatorBytes: 0,
        pages: 0,
        chunks: 0,
        completed: 0,
        deltas: 0,
    };
    let native: Array<Record<string, unknown>> = [];
    /** The complete native input after tail-delta expansion, per session. */
    const inputs = new Map<string, Array<Record<string, unknown>>>();
    /** The last output each session applied, with the revision that named it. */
    const applied = new Map<string, { revision: string; values: Array<Record<string, unknown>> }>();
    let outputCounter = 0;
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
            const sessionId = request.session_id as string;
            const tailDelta = request.tail_delta as
                | { after: string; native_replace_from: number }
                | undefined;
            const input = tailDelta
                ? [
                      ...(inputs.get(sessionId) ?? []).slice(0, tailDelta.native_replace_from),
                      ...native,
                  ]
                : native;
            inputs.set(sessionId, input);
            const output = input.map((value, position) =>
                position === 0 && (value.info as { id?: string })?.id === "m-1"
                    ? { ...value, benchmarkPublished: true }
                    : value,
            );
            const previous = applied.get(sessionId);
            const eligible =
                previous && request.previous_output_revision === previous.revision
                    ? previous
                    : undefined;
            const built = buildOperations(output, input, eligible?.values);
            outputCounter += 1;
            const outputRevision = `bench-out-${outputCounter}`;
            const response: Record<string, unknown> = {
                base_revision: request.base_revision,
                output_revision: outputRevision,
                ...(built.usedPrevious && eligible
                    ? { previous_output_revision: eligible.revision }
                    : {}),
                operations: built.operations,
            };
            applied.set(sessionId, { revision: outputRevision, values: output });
            // The old wire re-sent the changed suffix (or the whole array) as literals.
            const suffix = tailDelta ? output.slice(tailDelta.native_replace_from) : output;
            stats.suffixComparatorBytes += Buffer.byteLength(
                JSON.stringify(
                    tailDelta
                        ? {
                              native_messages_delta: {
                                  after: tailDelta.after,
                                  replace_from: tailDelta.native_replace_from,
                                  messages: suffix,
                              },
                          }
                        : { native_messages: suffix },
                ),
            );
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
    await transform.run(sessionId, output);
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
        suffixComparatorBytes: stats.suffixComparatorBytes - before.suffixComparatorBytes,
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
            suffix_comparator_bytes_median: quantile(
                samples.map((sample) => sample.suffixComparatorBytes),
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
