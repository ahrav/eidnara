import { afterEach, describe, expect, it, mock, spyOn } from "bun:test";
import { mkdirSync, mkdtempSync, renameSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { BoundedSessionMap } from "../../shared/bounded-session-map";
import * as logger from "../../shared/logger";
import { promptSurfaceConfigIdentity } from "../../shared/prompt-surface";
import { Database } from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";
import { deriveWindowGeometry } from "../../shared/window-geometry";
import { MODULE_PAGE_MAX_BYTES } from "./module-wire";
import { setRawMessageProvider } from "./read-session-chunk";
import { closeReadOnlySessionDb } from "./read-session-db";
import type { RawMessage } from "./read-session-raw";
import {
    __rustModeTransformTest,
    applyNativeMessagesVerbatim,
    createRustModeTransform,
    type RustModeModuleClient,
    type RustModeTransformDeps,
} from "./rust-mode-transform";
import type { MessageLike } from "./tag-content-primitives";

const unregisters: Array<() => void> = [];
const availabilityDataHomes: string[] = [];
const originalXdgDataHome = process.env.XDG_DATA_HOME;

afterEach(() => {
    closeReadOnlySessionDb();
    for (const unregister of unregisters.splice(0)) unregister();
    for (const dataHome of availabilityDataHomes.splice(0)) {
        rmSync(dataHome, { recursive: true, force: true });
    }
    if (originalXdgDataHome === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = originalXdgDataHome;
});

type RawRow = { id: string; timeCreated: number; contributesOrdinal: true; hasValidInfo: true };

function rawRows(count: number): RawRow[] {
    return Array.from({ length: count }, (_, index) => ({
        id: `m-${index + 1}`,
        timeCreated: index + 1,
        contributesOrdinal: true,
        hasValidInfo: true,
    }));
}

function installRawRows(sessionId: string, rows: RawRow[]): void {
    unregisters.push(
        setRawMessageProvider(sessionId, {
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
        }),
    );
}

function rowMessages(
    sessionId: string,
    rows: RawRow[],
    text = (row: RawRow) => `message ${row.id}`,
): MessageLike[] {
    return rows.map((row) => ({
        info: { id: row.id, role: "user", sessionID: sessionId },
        parts: [{ type: "text", text: text(row) }],
    }));
}

const makeMessages = (sessionId: string): MessageLike[] =>
    rowMessages(sessionId, rawRows(1), () => "hello");

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

type RecordedCall = { method: string; body: unknown; generationSensitive: boolean | undefined };

function recordingClient(
    respond: (body: Record<string, unknown>, index: number) => unknown,
    respondDisposition?: (method: string, body: Record<string, unknown>) => unknown,
): {
    client: RustModeModuleClient;
    bodies: Record<string, unknown>[];
    calls: RecordedCall[];
} {
    const bodies: Record<string, unknown>[] = [];
    const calls: RecordedCall[] = [];
    const client: RustModeModuleClient = {
        call: async ({ method, body, generationSensitive }) => {
            calls.push({ method, body, generationSensitive });
            if (method !== "transform") {
                return (
                    respondDisposition?.(method, body as Record<string, unknown>) ?? { ok: true }
                );
            }
            const request = body as Record<string, unknown>;
            bodies.push(request);
            return respond(request, bodies.length - 1);
        },
    };
    return { client, bodies, calls };
}

function installAvailabilityDb(sessionId: string, firstUserTools?: Record<string, unknown>): void {
    const dataHome = mkdtempSync(join(tmpdir(), "rust-mode-availability-"));
    availabilityDataHomes.push(dataHome);
    const dbPath = join(dataHome, "opencode", "opencode.db");
    mkdirSync(dirname(dbPath), { recursive: true });
    const opencodeDb = new Database(dbPath);
    opencodeDb.exec(`
        CREATE TABLE message (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL,
            data TEXT NOT NULL
        );
    `);
    if (firstUserTools !== undefined) {
        opencodeDb
            .prepare(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
            )
            .run(
                "availability-user",
                sessionId,
                1,
                1,
                JSON.stringify({ id: "availability-user", role: "user", tools: firstUserTools }),
            );
    }
    closeQuietly(opencodeDb);
    process.env.XDG_DATA_HOME = dataHome;
}

function sessionLogs(
    spy: { mock: { calls: Array<[string, string, unknown?]> } },
    sessionId: string,
): string[] {
    return spy.mock.calls
        .filter(([loggedSession]) => loggedSession === sessionId)
        .map(([, message]) => message);
}

const bodyDefaults = {
    input: [] as unknown[],
    nativeMessages: [] as unknown[],
    passInputs: {},
    usage: {},
    modelKey: null,
    providerId: null,
    systemPromptHash: "",
    midTurn: false,
};

describe("Rust mode transform request", () => {
    it("transports S1 geometry on the transform wire and omits it when unresolved", () => {
        const geometry = __rustModeTransformTest.transformGeometryForWire(
            deriveWindowGeometry(
                "openai-codex",
                "gpt-5.6-sol",
                { context: 400_000, input: 272_000, output: 128_000 },
                { outputReserveOverride: 16_384, harness: "opencode" },
            ),
        );
        expect(geometry).toEqual({
            usable_soft: 255_616,
            usable_hard: 368_000,
            derivation:
                "s1-shared/context-output/context=272000/output=16384/mode=shared_upfront/usable-hard=368000",
        });
        const withGeometry = __rustModeTransformTest.buildTransformBody({
            ...bodyDefaults,
            sessionId: "geometry-wire",
            usage: { context_limit_tokens: 128_000 },
            geometry,
        });
        expect(withGeometry.usage).toEqual({ context_limit_tokens: 128_000 });
        expect(withGeometry.geometry).toEqual(geometry);
        const withoutGeometry = __rustModeTransformTest.buildTransformBody({
            ...bodyDefaults,
            sessionId: "plain-wire",
        });
        expect("geometry" in withoutGeometry).toBe(false);
    });

    it("sends the daemon request shape with neutral values for host-only fields", () => {
        const body = __rustModeTransformTest.buildTransformBody({
            ...bodyDefaults,
            sessionId: "shape-wire",
            input: [{ mid: "m-1" }],
            nativeMessages: [{ info: { id: "m-1" } }],
            passInputs: {
                history_budget_tokens: 42_000,
                caveman_enabled: true,
                caveman_min_chars: 240,
            },
            modelKey: "anthropic/opus",
            providerId: "anthropic",
            systemPromptHash: "abc",
            fullArrayFingerprint: "fp",
        });
        expect(body).toMatchObject({
            method: "transform",
            kind: "transform",
            v: 2,
            serializer_profile: "opencode-aisdk",
            serve_native: true,
            render_config: "provider:anthropic|model:anthropic/opus|system:abc",
            upgrade_state: "",
            channel2_nudge_state: "",
            emergency_recovery_armed: false,
            emergency_recovery_no_head_escape: false,
            full_array_fingerprint: "fp",
            prompt_surface_preset: "full",
            history_budget_tokens: 42_000,
            caveman_enabled: true,
            caveman_min_chars: 240,
            messages: [{ mid: "m-1" }],
        });
        for (const absent of ["pass_inputs", "mural", "detected_context_limit", "tail_delta"]) {
            expect(absent in body).toBe(false);
        }
    });

    it("emits discriminating pass and stage logs from ordinary Rust transforms", async () => {
        const sessionId = `rust-log-fence-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const responses = [
            {
                decision: "HARD",
                materialize_reason: "first_render",
                served_from: "transform",
                timings: { handler_total: 5, total: 4, native_cache_encoded_messages: 1 },
            },
            {
                decision: "SOFT+",
                served_from: "cache",
                timings: { handler_total: 3, total: 2, native_cache_reused_messages: 1 },
            },
        ];
        const { client } = recordingClient((_body, index) => ({
            ...responses[index],
            native_messages: makeMessages(sessionId),
        }));
        const logSpy = spyOn(logger, "sessionLog").mockImplementation(() => {});
        try {
            const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
            for (let index = 0; index < 2; index += 1) {
                const messages = makeMessages(sessionId);
                await transform.run(sessionId, messages, { messages: [...messages] });
            }
            const logged = sessionLogs(logSpy, sessionId);
            const passLines = logged.filter((message) => message.startsWith("rust pass:"));
            expect(passLines).toHaveLength(2);
            expect(passLines[0]).toContain("decision=HARD");
            expect(passLines[0]).toContain("served_from=transform");
            expect(passLines[0]).toMatch(
                /reason=first_render .* stages=prefix_guard:[\d.]+ ordinal_resolve:[\d.]+ clone:[\d.]+ wire_build:[\d.]+ wire_messages:1 transport:[\d.]+ transport_pages:1 transport_bytes:\d+ apply:[\d.]+ other:[\d.]+$/,
            );
            expect(passLines[1]).toContain("decision=SOFT+");
            expect(passLines[1]).toContain("served_from=cache");
            expect(logged.some((message) => message.startsWith("rust module stages:"))).toBe(true);
        } finally {
            logSpy.mockRestore();
        }
    });

    it("sends the full array first and a tail delta on the next pass", async () => {
        const sessionId = `rust-seed-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const native = [{ role: "user", parts: [{ type: "text", text: "module output" }] }];
        const { client, bodies, calls } = recordingClient(() => ({ native_messages: native }));
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const messages = makeMessages(sessionId);
        const output = { messages: messages as unknown[] };

        await transform.run(sessionId, messages, output);
        expect(calls.map((call) => call.method)).toEqual(["transform"]);
        const first = bodies[0]!;
        expect(first).toMatchObject({
            serve_native: true,
            tool_present: true,
            todo_tool_present: true,
            cache_ttl: "5m",
            auto_search_enabled: true,
            auto_search_score_threshold: 0.6,
            auto_search_min_prompt_chars: 20,
            prompt_surface_preset: "full",
            prompt_surface_model_key: null,
            prompt_surface_config_identity: promptSurfaceConfigIdentity(undefined),
            prompt_surface_tool_descriptions: {},
        });
        expect(first.native_messages).toBe(messages);
        expect(Array.isArray(first.messages)).toBe(true);
        expect("pass_inputs" in first).toBe(false);
        expect(output.messages).toEqual(native);

        const secondInput = makeMessages(sessionId);
        const secondOutput = { messages: secondInput as unknown[] };
        await transform.run(sessionId, secondInput, secondOutput);
        expect(calls.map((call) => call.method)).toEqual(["transform", "transform"]);
        expect(bodies[1]?.tail_delta).toEqual({
            after: first.full_array_fingerprint,
            replace_from: 1,
            native_replace_from: 1,
        });
        expect((bodies[1]?.messages as unknown[]).length).toBe(0);
        expect((bodies[1]?.native_messages as unknown[]).length).toBe(0);
        expect(secondOutput.messages).toEqual(native);
    });

    it("sends canonical model identity with the model-routed prompt preset and overrides", async () => {
        const sessionId = `rust-prompt-surface-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient(() => ({
            native_messages: makeMessages(sessionId),
        }));
        const deps = makeDeps();
        deps.promptSurface = {
            default: "full",
            models: { "openai/gpt-5.6-sol": "light" },
            guidance_override_path: "trusted-guidance.md",
            tool_descriptions: { ctx_search: "Search the project memory index." },
        };
        deps.promptSurfaceRuntime = {
            resolveRegistration: () => ({
                preset: "full",
                descriptionFor: (_toolId, fullDescription) => fullDescription,
            }),
            resolveGuidance: () => ({
                preset: "light",
                primaryOverride: "## Eidnara\n\nTrusted user guidance.",
            }),
        };
        const transform = createRustModeTransform(deps, { moduleClient: client });
        const messages = makeMessages(sessionId);
        (messages[0]!.info as { model?: { providerID: string; modelID: string } }).model = {
            providerID: "openai-codex",
            modelID: "gpt-5.6-sol",
        };

        await transform.run(sessionId, messages, { messages: messages as unknown[] });

        expect(bodies[0]?.render_config).toContain("model:openai/gpt-5.6-sol");
        expect(bodies[0]).toMatchObject({
            model_key: "openai/gpt-5.6-sol",
            provider_id: "openai-codex",
            prompt_surface_preset: "light",
            prompt_surface_model_key: "openai/gpt-5.6-sol",
            prompt_surface_config_identity: promptSurfaceConfigIdentity(deps.promptSurface),
            prompt_surface_tool_descriptions: { ctx_search: "Search the project memory index." },
            prompt_surface_guidance_override: "## Eidnara\n\nTrusted user guidance.",
        });
    });

    it("sends fail-closed tool verdicts while availability remains provisional", async () => {
        const sessionId = `rust-availability-provisional-${Date.now()}`;
        installAvailabilityDb(sessionId);
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient(() => ({ native_messages: [] }));
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const messages: MessageLike[] = [
            {
                info: { id: "m-1", role: "assistant", sessionID: sessionId },
                parts: [{ type: "text", text: "assistant" }],
            },
        ];

        await transform.run(sessionId, messages, { messages: messages as unknown[] });

        expect(bodies).toHaveLength(1);
        expect(bodies[0]?.tool_present).toBe(false);
        expect(bodies[0]?.todo_tool_present).toBe(false);
    });

    it("seeds the ctx_reduce verdict from the live message array before the first user row persists", async () => {
        const sessionId = `rust-ctx-reduce-from-messages-${Date.now()}`;
        installAvailabilityDb(sessionId);
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient(() => ({ native_messages: [] }));
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const messages = makeMessages(sessionId);
        (messages[0]!.info as { tools?: Record<string, boolean> }).tools = { ctx_reduce: true };

        await transform.run(sessionId, messages, { messages: messages as unknown[] });

        expect(bodies).toHaveLength(1);
        expect(bodies[0]?.tool_present).toBe(true);
    });

    it("omits usage when the host holds no context-usage sample", async () => {
        const sessionId = `rust-usage-absent-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient(() => ({ native_messages: [] }));
        const deps = makeDeps();
        const transform = createRustModeTransform(deps, { moduleClient: client });

        const first = makeMessages(sessionId);
        await transform.run(sessionId, first, { messages: [...first] });
        expect("usage" in bodies[0]!).toBe(false);

        deps.contextUsageMap.set(sessionId, {
            usage: { percentage: 50, inputTokens: 64_000 },
            updatedAt: Date.now(),
            lastResponseTime: Date.now(),
            hasUsageTokens: true,
        });
        const second = makeMessages(sessionId);
        await transform.run(sessionId, second, { messages: [...second] });
        expect(bodies[1]?.usage).toEqual({
            input_tokens: 64_000,
            limit: 128_000,
            current_total_input_tokens: 64_000,
            context_limit_tokens: 128_000,
        });
    });

    it("sends the combined todowrite map and live-permission verdict", async () => {
        const sessionId = `rust-todo-permission-denied-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient(() => ({ native_messages: [] }));
        const deps = makeDeps();
        const agents = mock(async () => ({
            data: [{ name: "build", permission: { todowrite: "deny" } }],
        }));
        deps.client = {
            app: { agents },
            session: {
                get: async () => ({ data: { agent: "build", directory: "/tmp/project" } }),
            },
        } as never;
        const transform = createRustModeTransform(deps, { moduleClient: client });
        const messages = makeMessages(sessionId);
        (messages[0]!.info as { tools?: Record<string, boolean> }).tools = {};
        (messages[0]!.info as { agent?: string }).agent = "build";

        await transform.run(sessionId, messages, { messages: messages as unknown[] });

        expect(agents).toHaveBeenCalledTimes(1);
        expect(bodies[0]?.todo_tool_present).toBe(false);
    });

    it("sends todowrite absent when an empty-cache permission read never settles", async () => {
        const sessionId = `rust-todo-permission-hang-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient(() => ({ native_messages: [] }));
        const deps = makeDeps();
        const agents = mock(() => new Promise<never>(() => {}));
        deps.client = {
            app: { agents },
            session: {
                get: async () => ({ data: { agent: "build", directory: "/tmp/project" } }),
            },
        } as never;
        const transform = createRustModeTransform(deps, { moduleClient: client });
        const messages = makeMessages(sessionId);
        (messages[0]!.info as { tools?: Record<string, boolean> }).tools = {};
        (messages[0]!.info as { agent?: string }).agent = "build";

        const startedAt = performance.now();
        await transform.run(sessionId, messages, { messages: messages as unknown[] });
        const elapsedMs = performance.now() - startedAt;

        expect(agents).toHaveBeenCalledTimes(1);
        expect(bodies).toHaveLength(1);
        expect(bodies[0]?.todo_tool_present).toBe(false);
        expect(elapsedMs).toBeGreaterThanOrEqual(1_500);
        expect(elapsedMs).toBeLessThan(10_000);
    });

    it("logs a synthetic-turn cascade once after three consecutive synthetic turns", async () => {
        const sessionId = `rust-synthetic-cascade-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient(() => ({
            native_messages: [{ role: "assistant", parts: [] }],
        }));
        const logSpy = spyOn(logger, "sessionLog").mockImplementation(() => {});
        try {
            const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
            for (let turn = 1; turn <= 4; turn += 1) {
                const input = [
                    ...makeMessages(sessionId),
                    {
                        info: { id: `synthetic-${turn}`, role: "user", sessionID: sessionId },
                        parts: [{ type: "text", text: "synthetic turn", synthetic: true }],
                    },
                ];
                await transform.run(sessionId, input, { messages: input });
            }
            expect(transform.getState(sessionId).syntheticTurnCount).toBe(4);
            expect(bodies).toHaveLength(4);
            expect(
                sessionLogs(logSpy, sessionId).filter((message) =>
                    message.startsWith("rust synthetic-turn cascade: 3 consecutive"),
                ),
            ).toHaveLength(1);

            const realInput = makeMessages(sessionId);
            await transform.run(sessionId, realInput, { messages: realInput });
            expect(transform.getState(sessionId).syntheticTurnCount).toBe(0);
            expect(transform.getState(sessionId).syntheticCascadeLogged).toBe(false);
        } finally {
            logSpy.mockRestore();
        }
    });
});

describe("Rust mode transform transport", () => {
    it("re-pages every transform payload after need_full_sync", async () => {
        const sessionId = `rust-repage-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const messages = makeMessages(sessionId);
        messages[0]!.parts = [{ type: "text", text: "x".repeat(600_000) }];
        const native = [{ role: "assistant", parts: [] }];
        let retryStarted = false;
        const { client, bodies } = recordingClient((page) => {
            if (!retryStarted && page.transform_page_complete === true) {
                retryStarted = true;
                return { status: "need_full_sync" };
            }
            return { native_messages: native };
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const output = { messages: messages as unknown[] };
        await transform.run(sessionId, messages, output);

        expect(new Set(bodies.map((body) => body.transform_page_id)).size).toBe(2);
        expect(bodies.length).toBeGreaterThan(2);
        expect(
            bodies.every((body) =>
                [
                    "transform_page_id",
                    "transform_generation",
                    "transform_page_index",
                    "transform_page_total",
                    "transform_page_complete",
                    "transform_page_digest",
                ].every((field) => field in body),
            ),
        ).toBe(true);
        expect(bodies.at(-1)?.tool_present).toBe(true);
        expect(bodies.at(-1)?.todo_tool_present).toBe(true);
        expect(output.messages).toEqual(native);
    });

    it("restarts a paged transform series after an attempt mismatch", async () => {
        const sessionId = `rust-series-restart-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const messages = makeMessages(sessionId);
        messages[0]!.parts = [{ type: "text", text: "x".repeat(600_000) }];
        const native = [{ role: "assistant", parts: [] }];
        let failedPageId: unknown;
        const { client, bodies } = recordingClient((page) => {
            if (page.transform_page_index === 1 && failedPageId === undefined) {
                failedPageId = page.transform_page_id;
                throw Object.assign(
                    new Error("transform page generation or envelope changed during collection"),
                    { code: "authority_transform_page_attempt_mismatch" },
                );
            }
            return page.transform_page_complete === true
                ? { decision: "HARD", served_from: "transform", native_messages: native }
                : { staged: true };
        });
        const logSpy = spyOn(logger, "sessionLog").mockImplementation(() => {});
        try {
            const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
            const output = { messages: messages as unknown[] };
            await transform.run(sessionId, messages, output);

            const seriesStarts = bodies.filter((page) => page.transform_page_index === 0);
            expect(seriesStarts).toHaveLength(2);
            expect(new Set(seriesStarts.map((page) => page.transform_page_id)).size).toBe(2);
            expect(failedPageId).toBe(seriesStarts[0]?.transform_page_id);
            expect(output.messages).toEqual(native);
            expect(sessionLogs(logSpy, sessionId)).toContain(
                `transform_series_restart reason=attempt_mismatch pages=${seriesStarts[0]?.transform_page_total} at_page=1`,
            );
        } finally {
            logSpy.mockRestore();
        }
    });

    it("restarts a paged transform series after a mid-series reconnect", async () => {
        const sessionId = `rust-series-reconnect-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const messages = makeMessages(sessionId);
        messages[0]!.parts = [{ type: "text", text: "x".repeat(600_000) }];
        const native = [{ role: "assistant", parts: [] }];
        let reconnectReported = false;
        const { client, bodies, calls } = recordingClient((page) => {
            if (page.transform_page_index === 1 && !reconnectReported) {
                reconnectReported = true;
                return {
                    transport_status: "connection_generation_changed",
                    previous_generation: 3,
                    current_generation: 4,
                };
            }
            return page.transform_page_complete === true
                ? { decision: "HARD", served_from: "transform", native_messages: native }
                : { staged: true };
        });
        const logSpy = spyOn(logger, "sessionLog").mockImplementation(() => {});
        try {
            const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
            const output = { messages: messages as unknown[] };
            await transform.run(sessionId, messages, output);

            const seriesStarts = bodies.filter((body) => body.transform_page_index === 0);
            expect(seriesStarts).toHaveLength(2);
            expect(new Set(seriesStarts.map((body) => body.transform_page_id)).size).toBe(2);
            const continuation = calls.find(
                (call) => (call.body as Record<string, unknown>).transform_page_index === 1,
            );
            expect(continuation?.generationSensitive).toBe(true);
            expect(output.messages).toEqual(native);
            expect(sessionLogs(logSpy, sessionId)).toContain(
                `transform_series_restart reason=reconnect pages=${seriesStarts[0]?.transform_page_total} at_page=1`,
            );
        } finally {
            logSpy.mockRestore();
        }
    });

    it("clears Rust state, wire caches, and the transport route for a deleted session", async () => {
        const sessionId = `rust-clear-session-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const deleteSession = mock(async () => {});
        const closeSession = mock(() => {});
        const { client, bodies } = recordingClient(() => ({
            decision: "PASSTHROUGH",
            native_messages: [],
        }));
        client.deleteSession = deleteSession;
        client.closeSession = closeSession;
        const deps = makeDeps();
        deps.sessionDirectoryBySession?.set(sessionId, "/session/root-b");
        const transform = createRustModeTransform(deps, { moduleClient: client });
        for (let pass = 0; pass < 2; pass += 1) {
            const input = makeMessages(sessionId);
            await transform.run(sessionId, input, { messages: [...input] });
        }
        expect(bodies[1]?.tail_delta).toBeDefined();

        transform.clearSession(sessionId);
        await Bun.sleep(0);
        expect(deleteSession).toHaveBeenCalledWith(sessionId, "/session/root-b");
        expect(closeSession).toHaveBeenCalledWith(sessionId);
        const afterClear = makeMessages(sessionId);
        await transform.run(sessionId, afterClear, { messages: [...afterClear] });

        expect(bodies[2]?.tail_delta).toBeUndefined();
        expect(bodies[2]?.messages).toHaveLength(1);
        expect(transform.getState(sessionId).passCount).toBe(1);
    });

    it("cancels an active wrapup route before daemon deletion and closes the cleanup route", async () => {
        const sessionId = `rust-clear-active-wrapup-${Date.now()}`;
        const events: string[] = [];
        let activeRoute: "wrapup" | "delete" | null = "wrapup";
        const deleteSession = mock(async () => {
            events.push("delete");
            if (activeRoute === "wrapup") throw new Error("active wrapup still owns the route");
            activeRoute = "delete";
        });
        const closeSession = mock(() => {
            events.push(`close:${activeRoute ?? "none"}`);
            activeRoute = null;
        });
        const { client } = recordingClient(() => ({ native_messages: [] }));
        client.deleteSession = deleteSession;
        client.closeSession = closeSession;
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });

        transform.clearSession(sessionId);
        await Bun.sleep(0);

        expect(events).toEqual(["close:wrapup", "delete", "close:delete"]);
        expect(activeRoute).toBeNull();
    });

    it("serves the input unchanged without sending when the session is cleared during preflight", async () => {
        const sessionId = `rust-cleared-during-preflight-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        let releaseDirectoryRead: (() => void) | undefined;
        const deps = makeDeps();
        deps.client = {
            session: {
                get: () =>
                    new Promise<{ data: { directory: string } }>((resolve) => {
                        releaseDirectoryRead = () =>
                            resolve({ data: { directory: "/tmp/project" } });
                    }),
            },
        } as never;
        const deleteSession = mock(async () => {});
        const { client, calls } = recordingClient(() => ({ native_messages: [] }));
        client.deleteSession = deleteSession;
        const logSpy = spyOn(logger, "sessionLog").mockImplementation(() => {});
        try {
            const transform = createRustModeTransform(deps, { moduleClient: client });
            const messages = makeMessages(sessionId);
            const output = { messages: [...messages] as unknown[] };
            const pass = transform.run(sessionId, messages, output);
            while (releaseDirectoryRead === undefined) await Bun.sleep(0);
            transform.clearSession(sessionId);
            releaseDirectoryRead?.();
            await pass;

            expect(calls.filter((call) => call.method === "transform")).toHaveLength(0);
            expect(output.messages).toEqual(messages);
            expect(transform.getState(sessionId).consecutiveFailures).toBe(0);
            expect(
                sessionLogs(logSpy, sessionId).some((message) =>
                    message.includes("was cleared during the pass"),
                ),
            ).toBe(true);
        } finally {
            logSpy.mockRestore();
        }
    });

    it("deletes the daemon session by its known directory when no pass has run in this process", async () => {
        const sessionId = `rust-clear-before-pass-${Date.now()}`;
        const deleteSession = mock(async () => {});
        const closeSession = mock(() => {});
        const { client } = recordingClient(() => ({ native_messages: [] }));
        client.deleteSession = deleteSession;
        client.closeSession = closeSession;
        const deps = makeDeps();
        deps.sessionDirectoryBySession?.set(sessionId, "/session/root-c");
        const transform = createRustModeTransform(deps, { moduleClient: client });

        transform.clearSession(sessionId);
        await Bun.sleep(0);
        expect(deleteSession).toHaveBeenCalledWith(sessionId, "/session/root-c");
        expect(closeSession).toHaveBeenCalledWith(sessionId);

        const other = `${sessionId}-launch-dir`;
        transform.clearSession(other);
        await Bun.sleep(0);
        expect(deleteSession).toHaveBeenCalledWith(other, "/tmp/project");
    });

    it("forces a full send after invalidateWireState", async () => {
        const sessionId = `rust-invalidate-wire-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient(() => ({ native_messages: [] }));
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        for (let pass = 0; pass < 2; pass += 1) {
            const input = makeMessages(sessionId);
            await transform.run(sessionId, input, { messages: [...input] });
        }
        expect(bodies[1]?.tail_delta).toBeDefined();

        transform.invalidateWireState(sessionId);
        expect(transform.getState(sessionId).forceFullWire).toBe(true);
        const input = makeMessages(sessionId);
        await transform.run(sessionId, input, { messages: [...input] });

        expect(bodies[2]?.tail_delta).toBeUndefined();
        expect(bodies[2]?.messages).toHaveLength(1);
        expect(bodies[2]?.native_messages).toEqual(input);
        expect(transform.getState(sessionId).forceFullWire).toBe(false);
        expect(transform.getState(sessionId).passCount).toBe(3);
    });

    it("discards the pass cache when invalidateWireState lands while the daemon call is in flight", async () => {
        const sessionId = `rust-invalidate-in-flight-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        let release: (() => void) | undefined;
        const bodies: Record<string, unknown>[] = [];
        const client: RustModeModuleClient = {
            call: async ({ body }) => {
                bodies.push(body as Record<string, unknown>);
                if (bodies.length === 2) {
                    await new Promise<void>((resolve) => {
                        release = resolve;
                    });
                }
                return { native_messages: [] };
            },
        };
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const first = makeMessages(sessionId);
        await transform.run(sessionId, first, { messages: [...first] });

        const second = makeMessages(sessionId);
        const inFlight = transform.run(sessionId, second, { messages: [...second] });
        while (release === undefined) await Bun.sleep(0);
        expect(bodies[1]?.tail_delta).toBeDefined();
        transform.invalidateWireState(sessionId);
        release?.();
        await inFlight;

        expect(transform.getState(sessionId).forceFullWire).toBe(true);
        const third = makeMessages(sessionId);
        await transform.run(sessionId, third, { messages: [...third] });
        expect(bodies[2]?.tail_delta).toBeUndefined();
        expect(bodies[2]?.native_messages).toEqual(third);
        expect(transform.getState(sessionId).forceFullWire).toBe(false);
    });

    it("supersedes an older pass that finishes preflight after a newer pass starts", async () => {
        const sessionId = `rust-overlapping-preflight-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        let releaseDirectoryRead: (() => void) | undefined;
        const deps = makeDeps();
        deps.client = {
            session: {
                get: () =>
                    new Promise<{ data: { directory: string } }>((resolve) => {
                        releaseDirectoryRead = () =>
                            resolve({ data: { directory: "/tmp/project" } });
                    }),
            },
        } as never;
        deps.sessionMetadataReadStateBySession = new Map();
        const { client, bodies } = recordingClient((request) => ({
            native_messages: request.native_messages,
        }));
        const transform = createRustModeTransform(deps, { moduleClient: client });
        const firstInput = makeMessages(sessionId);
        const firstOutput = { messages: [...firstInput] as unknown[] };
        const first = transform.run(sessionId, firstInput, firstOutput);
        while (releaseDirectoryRead === undefined) await Bun.sleep(0);
        const secondInput = makeMessages(sessionId);
        const secondOutput = { messages: [...secondInput] as unknown[] };
        const second = transform.run(sessionId, secondInput, secondOutput);

        releaseDirectoryRead();
        await Promise.all([first, second]);

        expect(bodies).toHaveLength(1);
        expect(firstOutput.messages).toEqual(firstInput);
        expect(secondOutput.messages).toEqual(secondInput);
        expect(transform.getState(sessionId).passCount).toBe(2);
        expect(transform.getState(sessionId).failureCount).toBe(0);
    });

    it("nacks note deliveries when a pass is superseded while its transform response is pending", async () => {
        const sessionId = `rust-overlapping-response-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        let releaseFirstResponse:
            | ((value: {
                  native_messages: unknown[];
                  note_deliveries: Array<{ transform_pass_id: string }>;
              }) => void)
            | undefined;
        const { client, calls } = recordingClient((_request, index) => {
            if (index > 0) return { native_messages: [] };
            return new Promise<{
                native_messages: unknown[];
                note_deliveries: Array<{ transform_pass_id: string }>;
            }>((resolve) => {
                releaseFirstResponse = resolve;
            });
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const firstInput = makeMessages(sessionId);
        const firstOutput = { messages: [...firstInput] as unknown[] };
        const first = transform.run(sessionId, firstInput, firstOutput);
        while (releaseFirstResponse === undefined) await Bun.sleep(0);
        const secondInput = makeMessages(sessionId);
        await transform.run(sessionId, secondInput, { messages: [...secondInput] });

        releaseFirstResponse({
            native_messages: [{ info: { id: "superseded" }, parts: [] }],
            note_deliveries: [{ transform_pass_id: "pass-superseded" }],
        });
        await first;

        expect(firstOutput.messages).toEqual(firstInput);
        expect(calls.map((call) => call.method)).toEqual([
            "transform",
            "transform",
            "transform.nack",
        ]);
        expect(transform.getState(sessionId).failureCount).toBe(0);
    });

    it("does not resurrect a wire cache for a session cleared while its pass is in flight", async () => {
        const sessionId = `rust-clear-in-flight-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        let release: (() => void) | undefined;
        const bodies: Record<string, unknown>[] = [];
        const client: RustModeModuleClient = {
            call: async ({ body }) => {
                bodies.push(body as Record<string, unknown>);
                if (bodies.length === 2) {
                    await new Promise<void>((resolve) => {
                        release = resolve;
                    });
                }
                return { native_messages: [] };
            },
        };
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const first = makeMessages(sessionId);
        await transform.run(sessionId, first, { messages: [...first] });

        const second = makeMessages(sessionId);
        const inFlight = transform.run(sessionId, second, { messages: [...second] });
        while (release === undefined) await Bun.sleep(0);
        transform.clearSession(sessionId);
        release?.();
        await inFlight;

        const third = makeMessages(sessionId);
        await transform.run(sessionId, third, { messages: [...third] });
        expect(bodies[2]?.tail_delta).toBeUndefined();
        expect(bodies[2]?.native_messages).toEqual(third);
        expect(transform.getState(sessionId).passCount).toBe(1);
    });

    it("re-primes persisted ordinals after the continuation base when the memo is reset", async () => {
        const sessionId = `rust-continuation-reprime-${Date.now()}`;
        installRawRows(sessionId, rawRows(2));
        const { client, bodies } = recordingClient(() => ({
            native_messages: [],
            ordinal_continuation_base: 10,
        }));
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const ordinalsOf = (body: Record<string, unknown> | undefined): number[] =>
            (body?.messages as Array<{ ordinal: number }>).map((message) => message.ordinal);

        const first = rowMessages(sessionId, rawRows(2));
        await transform.run(sessionId, first, { messages: [...first] });
        expect(ordinalsOf(bodies[0])).toEqual([1, 2]);
        expect(transform.getState(sessionId).ordinalContinuationBase).toBe(10);
        expect(transform.getState(sessionId).idOrdinalMemo.get("m-2")).toBe(12);

        transform.invalidateWireState(sessionId);
        const second = rowMessages(sessionId, rawRows(2));
        await transform.run(sessionId, second, { messages: [...second] });
        expect(bodies[1]?.tail_delta).toBeUndefined();
        expect(ordinalsOf(bodies[1])).toEqual([11, 12]);
        expect(transform.getState(sessionId).idOrdinalMemo.get("m-1")).toBe(11);
        expect(transform.getState(sessionId).consecutiveFailures).toBe(0);
    });

    it("evicts the least recently used session's wire cache and sends its next pass in full", async () => {
        const capacity = __rustModeTransformTest.WIRE_CACHE_SESSION_CAPACITY;
        const stamp = Date.now();
        const sessionIdAt = (index: number): string => `rust-wire-cache-lru-${stamp}-${index}`;
        const bodiesBySession = new Map<string, Record<string, unknown>[]>();
        const client: RustModeModuleClient = {
            call: async ({ sessionId, body }) => {
                const list = bodiesBySession.get(sessionId) ?? [];
                list.push(body as Record<string, unknown>);
                bodiesBySession.set(sessionId, list);
                return { native_messages: [] };
            },
        };
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        for (let index = 0; index <= capacity; index += 1) {
            const sessionId = sessionIdAt(index);
            installRawRows(sessionId, rawRows(1));
            const input = makeMessages(sessionId);
            await transform.run(sessionId, input, { messages: [...input] });
        }

        const newest = sessionIdAt(capacity);
        const newestInput = makeMessages(newest);
        await transform.run(newest, newestInput, { messages: [...newestInput] });
        expect(bodiesBySession.get(newest)?.[1]?.tail_delta).toBeDefined();

        const evicted = sessionIdAt(0);
        const evictedInput = makeMessages(evicted);
        await transform.run(evicted, evictedInput, { messages: [...evictedInput] });
        expect(bodiesBySession.get(evicted)?.[1]?.tail_delta).toBeUndefined();
        expect(bodiesBySession.get(evicted)?.[1]?.native_messages).toEqual(evictedInput);
    });

    it("keeps a multi-frame tail delta paged instead of rebuilding the full wire", async () => {
        const sessionId = `rust-wire-paged-delta-${Date.now()}`;
        const rows = rawRows(3);
        installRawRows(sessionId, rows);
        const { client, bodies } = recordingClient(() => ({
            decision: "PASSTHROUGH",
            native_messages: [],
        }));
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const buildMessages = () =>
            rowMessages(sessionId, rows, (row) =>
                row.id === "m-4" ? `large delta ${"x".repeat(350 * 1024)}` : `message ${row.id}`,
            );

        const initial = buildMessages();
        await transform.run(sessionId, initial, { messages: [...initial] });
        rows.push({ id: "m-4", timeCreated: 4, contributesOrdinal: true, hasValidInfo: true });
        const appended = buildMessages();
        await transform.run(sessionId, appended, { messages: [...appended] });

        const deltaPages = bodies.filter((body) => "transform_page_id" in body);
        expect(deltaPages.length).toBeGreaterThan(1);
        expect(
            deltaPages.every(
                (page) => Buffer.byteLength(JSON.stringify(page)) <= MODULE_PAGE_MAX_BYTES,
            ),
        ).toBe(true);
        expect(deltaPages.at(-1)!.tail_delta).toEqual({
            after: bodies[0]?.full_array_fingerprint,
            replace_from: 2,
            native_replace_from: 2,
        });
        const pagedWire = JSON.stringify(deltaPages);
        expect(pagedWire).not.toContain("message m-1");
        expect(pagedWire).not.toContain("message m-2");
        expect(pagedWire).toContain("message m-3");
        expect(pagedWire).toContain("large delta");
    });
});

describe("native output delta", () => {
    it("applies a native_messages_delta in place and acks its note deliveries", async () => {
        const sessionId = `rust-native-delta-ack-${Date.now()}`;
        const rows = rawRows(1);
        installRawRows(sessionId, rows);
        const first = [
            { info: { id: "m0" }, parts: [{ type: "text", text: "stable" }] },
            { info: { id: "m-1" }, parts: [{ type: "text", text: "old" }] },
        ];
        const suffix = [
            { info: { id: "m-1" }, parts: [{ type: "text", text: "new" }] },
            { info: { id: "m-2" }, parts: [{ type: "text", text: "tail" }] },
        ];
        const { client, bodies, calls } = recordingClient((_request, index) =>
            index === 0
                ? { native_messages: first }
                : {
                      native_messages_delta: {
                          after: bodies[0]?.full_array_fingerprint,
                          replace_from: 1,
                          messages: suffix,
                      },
                      note_deliveries: [{ transform_pass_id: "pass-1" }],
                  },
        );
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const initial = rowMessages(sessionId, rows);
        await transform.run(sessionId, initial, { messages: [...initial] });
        rows.push({ id: "m-2", timeCreated: 2, contributesOrdinal: true, hasValidInfo: true });
        const appended = rowMessages(sessionId, rows);
        const output = { messages: [...appended] as unknown[] };
        await transform.run(sessionId, appended, output);

        expect(output.messages).toEqual([first[0], ...suffix]);
        expect(output.messages[0]).toBe(first[0]);
        expect(calls.filter((call) => call.method !== "transform")).toEqual([
            {
                method: "transform.ack",
                generationSensitive: undefined,
                body: {
                    method: "transform.ack",
                    v: 1,
                    session_id: sessionId,
                    transform_pass_id: "pass-1",
                },
            },
        ]);
    });

    it("attempts every ack sequentially before reporting aggregate failures", async () => {
        const sessionId = `rust-note-ack-isolation-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const ackFailure = new Error("first ack failed");
        let activeCalls = 0;
        let maxActiveCalls = 0;
        const native = [{ role: "assistant", parts: [] }];
        const { client, calls } = recordingClient(
            () => ({
                native_messages: native,
                note_deliveries: [
                    { transform_pass_id: "pass-1" },
                    { transform_pass_id: "pass-2" },
                    { transform_pass_id: "pass-3" },
                ],
            }),
            async (_method, body) => {
                activeCalls += 1;
                maxActiveCalls = Math.max(maxActiveCalls, activeCalls);
                try {
                    await Bun.sleep(0);
                    if (body.transform_pass_id === "pass-1") throw ackFailure;
                    return { ok: true };
                } finally {
                    activeCalls -= 1;
                }
            },
        );
        const logSpy = spyOn(logger, "sessionLog").mockImplementation(() => {});
        try {
            const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
            const input = makeMessages(sessionId);
            const output = { messages: [...input] as unknown[] };

            await transform.run(sessionId, input, output);

            expect(
                calls
                    .filter((call) => call.method === "transform.ack")
                    .map((call) => (call.body as Record<string, unknown>).transform_pass_id),
            ).toEqual(["pass-1", "pass-2", "pass-3"]);
            expect(maxActiveCalls).toBe(1);
            expect(output.messages).toEqual(native);
            expect(transform.getState(sessionId).failureCount).toBe(0);
            const aggregate = logSpy.mock.calls.find(
                ([, message]) => message === "rust note delivery ack failed (will retry):",
            )?.[2];
            expect(aggregate).toBeInstanceOf(AggregateError);
            expect((aggregate as AggregateError).errors).toEqual([ackFailure]);
        } finally {
            logSpy.mockRestore();
        }
    });

    it("keeps applied note output when a newer pass starts during the ack", async () => {
        const sessionId = `rust-note-ack-superseded-${Date.now()}`;
        installRawRows(sessionId, rawRows(3));
        let releaseAck: (() => void) | undefined;
        const firstNative = [{ info: { id: "first-applied" }, parts: [] }];
        const secondNative = [
            { info: { id: "second-applied-1" }, parts: [] },
            { info: { id: "second-applied-2" }, parts: [] },
        ];
        const { client, bodies } = recordingClient(
            (_request, index) =>
                index === 0
                    ? {
                          native_messages: firstNative,
                          note_deliveries: [{ transform_pass_id: "pass-first" }],
                      }
                    : { native_messages: secondNative },
            async (method) => {
                if (method === "transform.ack") {
                    await new Promise<void>((resolve) => {
                        releaseAck = resolve;
                    });
                }
                return { ok: true };
            },
        );
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const firstInput = rowMessages(sessionId, rawRows(1));
        const firstOutput = { messages: [...firstInput] as unknown[] };
        const first = transform.run(sessionId, firstInput, firstOutput);
        while (releaseAck === undefined) await Bun.sleep(0);
        const secondInput = rowMessages(sessionId, rawRows(2));
        const secondOutput = { messages: [...secondInput] as unknown[] };
        const second = transform.run(sessionId, secondInput, secondOutput);
        await second;
        releaseAck();
        await first;
        const thirdInput = rowMessages(sessionId, rawRows(3));
        await transform.run(sessionId, thirdInput, { messages: [...thirdInput] });

        expect(firstOutput.messages).toEqual(firstNative);
        expect(secondOutput.messages).toEqual(secondNative);
        expect(
            (bodies[2]?.tail_delta as { native_replace_from?: number } | undefined)
                ?.native_replace_from,
        ).toBe(1);
        expect(transform.getState(sessionId).failureCount).toBe(0);
    });

    it("disposes each duplicate delivery pass ID only once", async () => {
        const sessionId = `rust-note-delivery-dedup-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const { client, calls } = recordingClient(() => ({
            native_messages: [],
            note_deliveries: [
                { transform_pass_id: "pass-1" },
                { transform_pass_id: "pass-1" },
                { transform_pass_id: "pass-2" },
                { transform_pass_id: "pass-1" },
            ],
        }));
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const input = makeMessages(sessionId);

        await transform.run(sessionId, input, { messages: [...input] });

        expect(
            calls
                .filter((call) => call.method === "transform.ack")
                .map((call) => (call.body as Record<string, unknown>).transform_pass_id),
        ).toEqual(["pass-1", "pass-2"]);
    });

    it("attempts every nack without replacing the original apply error", async () => {
        const sessionId = `rust-note-nack-isolation-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const nackFailure = new Error("first nack failed");
        const { client, calls } = recordingClient(
            () => ({
                boundary_id: "m-1#0",
                native_messages: [{ role: "assistant", parts: [] }],
                note_deliveries: [{ transform_pass_id: "pass-1" }, { transform_pass_id: "pass-2" }],
            }),
            (_method, body) => {
                if (body.transform_pass_id === "pass-1") throw nackFailure;
                return { ok: true };
            },
        );
        const logSpy = spyOn(logger, "sessionLog").mockImplementation(() => {});
        try {
            const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
            const input = makeMessages(sessionId);
            const output = { messages: [...input] as unknown[] };

            await transform.run(sessionId, input, output);

            expect(calls.map((call) => call.method)).toEqual([
                "transform",
                "transform.nack",
                "transform.nack",
            ]);
            expect(output.messages).toEqual(input);
            expect(transform.getState(sessionId).failureCount).toBe(1);
            const aggregate = logSpy.mock.calls.find(
                ([, message]) => message === "rust note delivery nack failed (ignored):",
            )?.[2];
            expect(aggregate).toBeInstanceOf(AggregateError);
            expect((aggregate as AggregateError).errors).toEqual([nackFailure]);
            const applyError = logSpy.mock.calls.find(([, message]) =>
                message.startsWith("rust transform failed; serving the input unchanged:"),
            )?.[2];
            expect(applyError).toBeInstanceOf(Error);
            expect(applyError).not.toBeInstanceOf(AggregateError);
            expect((applyError as Error).message).toContain("wire invariant failed");
        } finally {
            logSpy.mockRestore();
        }
    });

    it("nacks discarded delivery IDs and acks only IDs from the applied retry response", async () => {
        const sessionId = `rust-note-delivery-retry-union-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const native = [{ role: "assistant", parts: [] }];
        const { client, bodies, calls } = recordingClient((_request, index) =>
            index === 0
                ? {
                      status: "ok",
                      served_from: "transform",
                      note_deliveries: [
                          { transform_pass_id: "pass-initial" },
                          { transform_pass_id: "pass-shared" },
                      ],
                  }
                : {
                      native_messages: native,
                      note_deliveries: [
                          { transform_pass_id: "pass-shared" },
                          { transform_pass_id: "pass-retry" },
                      ],
                  },
        );
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const input = makeMessages(sessionId);
        const output = { messages: [...input] as unknown[] };

        await transform.run(sessionId, input, output);

        expect(bodies).toHaveLength(2);
        expect(output.messages).toEqual(native);
        expect(
            calls
                .filter((call) => call.method === "transform.nack")
                .map((call) => (call.body as Record<string, unknown>).transform_pass_id),
        ).toEqual(["pass-initial"]);
        expect(
            calls
                .filter((call) => call.method === "transform.ack")
                .map((call) => (call.body as Record<string, unknown>).transform_pass_id),
        ).toEqual(["pass-shared", "pass-retry"]);
    });

    it("nacks initial and retry delivery IDs when the full retry still cannot be applied", async () => {
        const sessionId = `rust-note-delivery-retry-failure-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const { client, calls } = recordingClient((_request, index) =>
            index === 0
                ? {
                      status: "ok",
                      served_from: "transform",
                      note_deliveries: [{ transform_pass_id: "pass-initial" }],
                  }
                : {
                      status: "need_full_sync",
                      note_deliveries: [{ transform_pass_id: "pass-retry" }],
                  },
        );
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const input = makeMessages(sessionId);
        const output = { messages: [...input] as unknown[] };

        await transform.run(sessionId, input, output);

        expect(output.messages).toEqual(input);
        expect(
            calls
                .filter((call) => call.method === "transform.nack")
                .map((call) => (call.body as Record<string, unknown>).transform_pass_id),
        ).toEqual(["pass-initial", "pass-retry"]);
        expect(calls.some((call) => call.method === "transform.ack")).toBe(false);
    });

    it("nacks note deliveries and serves the input unchanged when the boundary lacks a synthetic m0", async () => {
        const sessionId = `rust-wire-invariant-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const input = makeMessages(sessionId);
        const { client, calls } = recordingClient(() => ({
            action: "CACHE_HIT",
            served_from: "transform",
            boundary_id: "m-1#0",
            note_deliveries: [{ transform_pass_id: "pass-1" }],
            native_messages: [
                {
                    info: { role: "user", sessionID: sessionId },
                    parts: [{ type: "text", text: "not marked synthetic" }],
                },
            ],
        }));
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const output = { messages: input as unknown[] };

        await transform.run(sessionId, input, output);

        expect(output.messages).toBe(input);
        expect(output.messages[0]).toEqual(input[0]);
        expect(calls.map((call) => call.method)).toEqual(["transform", "transform.nack"]);
        expect(transform.getState(sessionId).failureCount).toBe(1);
    });

    it("retries with full arrays when a delta response omits native content", async () => {
        const sessionId = `rust-native-omission-retry-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        let healedNative: unknown[] = [];
        const { client, bodies } = recordingClient((request, index) => {
            if (index === 0) return { native_messages: structuredClone(request.native_messages) };
            if (request.tail_delta) return { status: "ok", served_from: "transform" };
            return { native_messages: structuredClone(healedNative) };
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const initial = makeMessages(sessionId);
        await transform.run(sessionId, initial, { messages: [...initial] });

        const changed = structuredClone(initial);
        changed[0]!.parts = [{ type: "text", text: "changed after warm prime" }];
        healedNative = structuredClone(changed);
        const output = { messages: [...changed] as unknown[] };
        const logSpy = spyOn(logger, "sessionLog").mockImplementation(() => {});
        try {
            await transform.run(sessionId, changed, output);
            // The retry pass builds two bodies (the delta and its full replacement); the send itself is transport time, not wire-build time.
            const stageLogs = sessionLogs(logSpy, sessionId);
            expect(
                stageLogs.filter((message) => message.includes("stage=rust.wire_build")),
            ).toHaveLength(2);
            expect(
                stageLogs.filter((message) => message.includes("stage=rust.transport")),
            ).toHaveLength(2);
        } finally {
            logSpy.mockRestore();
        }

        expect(bodies).toHaveLength(3);
        expect(bodies[1]?.tail_delta).toEqual({
            after: bodies[0]?.full_array_fingerprint,
            replace_from: 0,
            native_replace_from: 0,
        });
        expect(bodies[2]?.tail_delta).toBeUndefined();
        expect(bodies[2]?.native_messages).toEqual(changed);
        expect(output.messages).toEqual(healedNative);
        expect(transform.getState(sessionId).consecutiveFailures).toBe(0);
    });

    it("reconstructs the exact acknowledged prefix plus replacement suffix", () => {
        const previous = [
            { info: { id: "m0" }, parts: [{ type: "text", text: "stable" }] },
            { info: { id: "m1" }, parts: [{ type: "text", text: "old" }] },
        ];
        const suffix = [
            { info: { id: "m1" }, parts: [{ type: "text", text: "new" }] },
            { info: { id: "m2" }, parts: [{ type: "text", text: "tail" }] },
        ];
        const output = { messages: [] as unknown[] };

        const applied = applyNativeMessagesVerbatim(
            output,
            { native_messages_delta: { after: "fp-before", replace_from: 1, messages: suffix } },
            { messages: previous, fingerprint: "fp-before" },
        );

        expect(applied).toEqual([previous[0], ...suffix]);
        expect(output.messages).toEqual(applied);
        expect(applied[0]).toBe(previous[0]);
    });

    it("rejects a delta whose prefix fingerprint is not acknowledged", () => {
        expect(() =>
            applyNativeMessagesVerbatim(
                { messages: [] },
                { native_messages_delta: { after: "stale", replace_from: 1, messages: [] } },
                { messages: [{ info: { id: "m0" } }], fingerprint: "current" },
            ),
        ).toThrow("did not match the acknowledged output");
    });
});

describe("delta prefix-mutation guard", () => {
    it("in-place mutation of an older message forces a full send instead of a delta", async () => {
        const sessionId = `rust-prefix-guard-${Date.now()}`;
        const rows = rawRows(4);
        installRawRows(sessionId, rows);
        let moduleNativeSnapshot: unknown[] = [];
        const { client, bodies } = recordingClient((request) => {
            const suffix = request.native_messages as unknown[];
            const delta = request.tail_delta as { native_replace_from?: unknown } | undefined;
            moduleNativeSnapshot =
                delta && typeof delta.native_replace_from === "number"
                    ? [
                          ...moduleNativeSnapshot.slice(0, delta.native_replace_from),
                          ...structuredClone(suffix),
                      ]
                    : structuredClone(suffix);
            return { native_messages: structuredClone(moduleNativeSnapshot) };
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const buildMessages = (count: number, mutatePrefix = false): MessageLike[] =>
            rowMessages(sessionId, rows.slice(0, count), (row) =>
                mutatePrefix && row.id === "m-1" ? "MESSAGE m-1" : `message ${row.id}`,
            );

        const first = buildMessages(3);
        await transform.run(sessionId, first, { messages: [...first] });
        const appended = buildMessages(4);
        await transform.run(sessionId, appended, { messages: [...appended] });
        expect(bodies).toHaveLength(2);
        expect(bodies[1]?.tail_delta).toEqual({
            after: expect.any(String),
            replace_from: expect.any(Number),
            native_replace_from: expect.any(Number),
        });

        const mutated = buildMessages(4, true);
        expect(__rustModeTransformTest.messageContentSnapshot(mutated[0]).signature).not.toBe(
            __rustModeTransformTest.messageContentSnapshot(appended[0]).signature,
        );
        const mutatedOutput = { messages: [...mutated] as unknown[] };
        await transform.run(sessionId, mutated, mutatedOutput);
        expect(bodies).toHaveLength(3);
        expect(bodies[2]?.tail_delta).toBeUndefined();
        expect(bodies[2]?.messages).toHaveLength(4);
        expect(bodies[2]?.native_messages).toEqual(mutated);
        expect(JSON.stringify(bodies[2]?.messages)).toContain("MESSAGE m-1");
        expect(mutatedOutput.messages).toEqual(mutated);

        const stableOutput = { messages: [...mutated] as unknown[] };
        await transform.run(sessionId, mutated, stableOutput);
        expect(bodies).toHaveLength(4);
        expect(bodies[3]?.tail_delta).toEqual({
            after: bodies[2]?.full_array_fingerprint,
            replace_from: 4,
            native_replace_from: 4,
        });
        expect(bodies[3]?.messages).toEqual([]);
        expect(bodies[3]?.native_messages).toEqual([]);
        expect(stableOutput.messages).toEqual(mutatedOutput.messages);
    });

    it("carries terminal visibility across an empty delta so a later append resends a mutated terminal", async () => {
        const sessionId = `rust-empty-delta-visibility-${Date.now()}`;
        const rows = rawRows(2);
        installRawRows(sessionId, rows);
        const { client, bodies } = recordingClient(() => ({ native_messages: [] }));
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const buildMessages = (count: number, mutateTerminal = false): MessageLike[] =>
            rowMessages(sessionId, rows.slice(0, count), (row) =>
                mutateTerminal && row.id === "m-2" ? "MESSAGE m-2" : `message ${row.id}`,
            );

        const first = buildMessages(2);
        await transform.run(sessionId, first, { messages: [...first] });
        const unchanged = buildMessages(2);
        await transform.run(sessionId, unchanged, { messages: [...unchanged] });
        expect(bodies[1]?.tail_delta).toEqual({
            after: bodies[0]?.full_array_fingerprint,
            replace_from: 2,
            native_replace_from: 2,
        });

        rows.push({ id: "m-3", timeCreated: 3, contributesOrdinal: true, hasValidInfo: true });
        const appended = buildMessages(3, true);
        await transform.run(sessionId, appended, { messages: [...appended] });
        expect(bodies).toHaveLength(3);
        expect(bodies[2]?.tail_delta).toEqual({
            after: bodies[1]?.full_array_fingerprint,
            replace_from: 1,
            native_replace_from: 1,
        });
        expect(bodies[2]?.native_messages).toEqual(appended.slice(1));
        expect(JSON.stringify(bodies[2]?.messages)).toContain("MESSAGE m-2");
    });

    it("resends a wire-invisible former terminal that was mutated while a message was appended", async () => {
        const sessionId = `rust-invisible-terminal-append-${Date.now()}`;
        const rows = rawRows(2);
        installRawRows(sessionId, rows);
        const { client, bodies } = recordingClient(() => ({ native_messages: [] }));
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const buildMessages = (count: number, summaryText: string): MessageLike[] =>
            rows.slice(0, count).map((row) =>
                row.id === "m-2"
                    ? {
                          info: {
                              id: row.id,
                              role: "assistant",
                              sessionID: sessionId,
                              summary: true,
                              finish: "stop",
                          },
                          parts: [{ type: "text", text: summaryText }],
                      }
                    : {
                          info: { id: row.id, role: "user", sessionID: sessionId },
                          parts: [{ type: "text", text: `message ${row.id}` }],
                      },
            );

        const first = buildMessages(2, "summary v1");
        await transform.run(sessionId, first, { messages: [...first] });
        expect(bodies[0]?.messages).toHaveLength(1);

        rows.push({ id: "m-3", timeCreated: 3, contributesOrdinal: true, hasValidInfo: true });
        const appended = buildMessages(3, "summary v2");
        await transform.run(sessionId, appended, { messages: [...appended] });
        expect(bodies).toHaveLength(2);
        expect(bodies[1]?.tail_delta).toEqual({
            after: bodies[0]?.full_array_fingerprint,
            replace_from: 1,
            native_replace_from: 1,
        });
        expect(bodies[1]?.native_messages).toEqual(appended.slice(1));
        expect(JSON.stringify(bodies[1]?.native_messages)).toContain("summary v2");
    });

    it("recovers after a queued user message is mutated in place and the module rejects twice", async () => {
        const sessionId = `rust-tail-mutation-recovery-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const initial = makeMessages(sessionId);
        const mutated = rowMessages(
            sessionId,
            rawRows(1),
            () => "<system-reminder>queued user message was wrapped in place</system-reminder>",
        );
        const recovered = rowMessages(sessionId, rawRows(1), () => "module recovered");
        const { client, bodies } = recordingClient((_request, index) => {
            if (index === 1 || index === 2) throw new Error("CK message block identity drift");
            return { native_messages: recovered };
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });

        await transform.run(sessionId, initial, { messages: initial as unknown[] });
        for (let retry = 0; retry < 2; retry += 1) {
            const output = { messages: mutated as unknown[] };
            await transform.run(sessionId, mutated, output);
            expect(output.messages).toBe(mutated);
        }
        expect(transform.getState(sessionId).consecutiveFailures).toBe(2);

        const recoveredOutput = { messages: mutated as unknown[] };
        await transform.run(sessionId, mutated, recoveredOutput);
        expect(bodies).toHaveLength(4);
        expect(recoveredOutput.messages).toEqual(recovered);
        expect(transform.getState(sessionId).consecutiveFailures).toBe(0);
    });

    it("reads mid-turn state through the hook across warm reads and same-path replacement", async () => {
        const sessionId = "session-db-hook";
        installAvailabilityDb(sessionId, {});
        const dataHome = process.env.XDG_DATA_HOME!;
        const dbPath = join(dataHome, "opencode", "opencode.db");
        const writer = new Database(dbPath);
        writer.exec(
            "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, data TEXT)",
        );
        writer
            .prepare("INSERT INTO message VALUES (?, ?, ?, ?, ?)")
            .run(
                "assistant",
                sessionId,
                100,
                100,
                JSON.stringify({ role: "assistant", finish: "stop", time: { completed: 100 } }),
            );
        writer
            .prepare("INSERT INTO message VALUES (?, ?, ?, ?, ?)")
            .run("new-user", sessionId, 200, 200, '{"role":"user"}');
        writer
            .prepare("INSERT INTO part VALUES (?, ?, ?, ?)")
            .run("part", "new-user", sessionId, '{"type":"text","text":"prompt","ignored":true}');
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient(() => ({
            native_messages: makeMessages(sessionId),
        }));
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        try {
            for (let pass = 0; pass < 3; pass++) {
                const messages = makeMessages(sessionId);
                await transform.run(sessionId, messages, { messages: [...messages] });
                if (pass === 1)
                    writer.exec(`UPDATE part SET data = '{"type":"text","text":"prompt"}'`);
            }
        } finally {
            writer.close();
        }
        expect(bodies.map((body) => body.mid_turn)).toEqual([false, false, true]);

        installAvailabilityDb(sessionId);
        const replacementPath = join(process.env.XDG_DATA_HOME!, "opencode", "opencode.db");
        const replacement = new Database(replacementPath);
        replacement.exec(
            "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, data TEXT)",
        );
        replacement.close();
        renameSync(replacementPath, dbPath);
        process.env.XDG_DATA_HOME = dataHome;
        const messages = makeMessages(sessionId);
        await transform.run(sessionId, messages, { messages: [...messages] });
        expect(bodies).toHaveLength(4);
        expect(bodies[3].mid_turn).toBe(false);
    });
});
