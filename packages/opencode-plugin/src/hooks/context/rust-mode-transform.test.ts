import { afterEach, describe, expect, it, mock, spyOn } from "bun:test";
import { mkdirSync, mkdtempSync, renameSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { BoundedSessionMap } from "../../shared/bounded-session-map";
import { serializedJsonText } from "../../shared/host-client/serialized-json-body";
import * as logger from "../../shared/logger";
import { promptSurfaceConfigIdentity } from "../../shared/prompt-surface";
import { Database } from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";
import { deriveWindowGeometry } from "../../shared/window-geometry";
import * as editRecipe from "./edit-recipe";
import * as eventResolvers from "./event-resolvers";
import {
    DEFAULT_CONTEXT_LIMIT,
    resolveContextLimit,
    resolveTrustedContextLimit,
} from "./event-resolvers";
import { chargeInvocation } from "./invocation-budget";
import {
    MODULE_ORDINAL_PAGE_SIZE,
    MODULE_PAGE_MAX_BYTES,
    ORDINAL_ENTRY_RETAINED_BYTES,
} from "./module-wire";
import { setRawMessageProvider } from "./read-session-chunk";
import { closeReadOnlySessionDb } from "./read-session-db";
import type { RawMessage } from "./read-session-raw";
import {
    __rustModeTransformTest,
    createRustModeTransform,
    type RustModeModuleClient,
    type RustModeTransformDeps,
} from "./rust-mode-transform";
import type { MessageLike } from "./tag-content-primitives";
import {
    defaultTransformCaptureAdmission,
    inspectReferenceableMessages,
    TransformCaptureAdmission,
} from "./transform-capture";

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
    expect(defaultTransformCaptureAdmission.activePasses).toBe(0);
    expect(defaultTransformCaptureAdmission.chargedBytes).toBe(0);
});

type RawRow = {
    id: string;
    timeCreated: number;
    contributesOrdinal: true;
    hasValidInfo: true;
};

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

let recipeOutputCounter = 0;

/**
 * A daemon answer that inserts `output` whole, bound to the request's base revision. Tests that
 * exercise `previous` keeps build their operations by hand.
 */
function recipeResponse(
    request: Record<string, unknown>,
    output: readonly unknown[],
    extra: Record<string, unknown> = {},
): Record<string, unknown> {
    recipeOutputCounter += 1;
    return {
        ...extra,
        base_revision: request.base_revision,
        output_revision: `out-${recipeOutputCounter}`,
        operations: output.length === 0 ? [] : [{ op: "insert", values: output }],
    };
}

/** For a response resolved after the fact: binds the recipe to the most recent transform body. */
function recipeForLast(
    bodies: readonly Record<string, unknown>[],
    output: readonly unknown[],
    extra: Record<string, unknown> = {},
): Record<string, unknown> {
    const request = bodies.at(-1);
    if (!request) throw new Error("no transform body was recorded");
    return recipeResponse(request, output, extra);
}

type RecordedCall = {
    method: string;
    body: unknown;
    generationSensitive: boolean | undefined;
    signal?: AbortSignal;
};

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
        call: async ({ method, body, generationSensitive, signal }) => {
            calls.push({ method, body, generationSensitive, signal });
            if (method !== "transform") {
                return (
                    respondDisposition?.(method, body as Record<string, unknown>) ?? {
                        ok: true,
                    }
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
                JSON.stringify({
                    id: "availability-user",
                    role: "user",
                    tools: firstUserTools,
                }),
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
                terse_text_compression_enabled: true,
                terse_text_compression_min_chars: 240,
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
            terse_text_compression_enabled: true,
            terse_text_compression_min_chars: 240,
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
                timings: {
                    handler_total: 5,
                    total: 4,
                    native_cache_encoded_messages: 1,
                },
            },
            {
                decision: "SOFT+",
                served_from: "cache",
                timings: {
                    handler_total: 3,
                    total: 2,
                    native_cache_reused_messages: 1,
                },
            },
        ];
        const { client } = recordingClient((request, index) =>
            recipeResponse(request, makeMessages(sessionId), responses[index]),
        );
        const logSpy = spyOn(logger.sessionLog, "debug");
        try {
            const transform = createRustModeTransform(makeDeps(), {
                moduleClient: client,
            });
            for (let index = 0; index < 2; index += 1) {
                const messages = makeMessages(sessionId);
                await transform.run(sessionId, { messages: [...messages] });
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
        const { client, bodies, calls } = recordingClient((request) =>
            recipeResponse(request, native),
        );
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const messages = makeMessages(sessionId);
        const output = { messages: messages as unknown[] };

        await transform.run(sessionId, output);
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
        // The carried text is the wire contract; `output.messages` is rewritten in place after the send.
        const firstWire = JSON.parse(serializedJsonText(first)!) as Record<string, unknown>;
        expect(firstWire.native_messages).toEqual(makeMessages(sessionId));
        expect(Array.isArray(first.messages)).toBe(true);
        expect("pass_inputs" in first).toBe(false);
        expect(output.messages).toEqual(native);

        const secondInput = makeMessages(sessionId);
        const secondOutput = { messages: secondInput as unknown[] };
        await transform.run(sessionId, secondOutput);
        expect(calls.map((call) => call.method)).toEqual(["transform", "transform"]);
        expect(bodies[1]?.tail_delta).toEqual({
            after: first.full_array_fingerprint,
            replace_from: 1,
            native_replace_from: 1,
        });
        expect(bodies[1]?.messages).toEqual([]);
        expect(bodies[1]?.native_messages).toEqual([]);
        expect(secondOutput.messages).toEqual(native);
    });

    it("sends canonical model identity with the model-routed prompt preset and overrides", async () => {
        const sessionId = `rust-prompt-surface-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient((request) =>
            recipeResponse(request, makeMessages(sessionId)),
        );
        const deps = makeDeps();
        deps.promptSurface = {
            default: "full",
            models: { "openai/gpt-5.6-sol": "light" },
            guidance_override_path: "trusted-guidance.md",
            tool_descriptions: { eidnara_search: "Search the project memory index." },
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

        await transform.run(sessionId, { messages: messages as unknown[] });

        expect(bodies[0]?.render_config).toContain("model:openai/gpt-5.6-sol");
        expect(bodies[0]).toMatchObject({
            model_key: "openai/gpt-5.6-sol",
            provider_id: "openai-codex",
            prompt_surface_preset: "light",
            prompt_surface_model_key: "openai/gpt-5.6-sol",
            prompt_surface_config_identity: promptSurfaceConfigIdentity(deps.promptSurface),
            prompt_surface_tool_descriptions: {
                eidnara_search: "Search the project memory index.",
            },
            prompt_surface_guidance_override: "## Eidnara\n\nTrusted user guidance.",
        });
    });

    it("sends fail-closed tool verdicts while availability remains provisional", async () => {
        const sessionId = `rust-availability-provisional-${Date.now()}`;
        installAvailabilityDb(sessionId);
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient((request) => recipeResponse(request, []));
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const messages: MessageLike[] = [
            {
                info: { id: "m-1", role: "assistant", sessionID: sessionId },
                parts: [{ type: "text", text: "assistant" }],
            },
        ];

        await transform.run(sessionId, { messages: messages as unknown[] });

        expect(bodies).toHaveLength(1);
        expect(bodies[0]?.tool_present).toBe(false);
        expect(bodies[0]?.todo_tool_present).toBe(false);
    });

    it("seeds the eidnara_reduce verdict from the live message array before the first user row persists", async () => {
        const sessionId = `rust-eidnara-reduce-from-messages-${Date.now()}`;
        installAvailabilityDb(sessionId);
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient((request) => recipeResponse(request, []));
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const messages = makeMessages(sessionId);
        (messages[0]!.info as { tools?: Record<string, boolean> }).tools = {
            eidnara_reduce: true,
        };

        await transform.run(sessionId, { messages: messages as unknown[] });

        expect(bodies).toHaveLength(1);
        expect(bodies[0]?.tool_present).toBe(true);
    });

    it("omits usage when the host holds no context-usage sample", async () => {
        const sessionId = `rust-usage-absent-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient((request) => recipeResponse(request, []));
        const deps = makeDeps();
        const transform = createRustModeTransform(deps, { moduleClient: client });

        const first = makeMessages(sessionId);
        await transform.run(sessionId, { messages: [...first] });
        expect("usage" in bodies[0]!).toBe(false);

        deps.contextUsageMap.set(sessionId, {
            usage: { percentage: 50, inputTokens: 64_000 },
            updatedAt: Date.now(),
            lastResponseTime: Date.now(),
            hasUsageTokens: true,
        });
        const second = makeMessages(sessionId);
        await transform.run(sessionId, { messages: [...second] });
        expect(bodies[1]?.usage).toEqual({
            input_tokens: 64_000,
            limit: 128_000,
            current_total_input_tokens: 64_000,
            context_limit_tokens: 128_000,
        });
    });

    const GATED_MODEL = { providerID: "eidnara-test", modelID: "gated-window" };

    /** Reports `limit` as the models.dev limit for `GATED_MODEL`, the one source the invocation gate reads. */
    function trustContextLimit(limit: number) {
        return spyOn(eventResolvers, "resolveTrustedContextLimit").mockImplementation(
            (providerID, modelID) =>
                providerID === GATED_MODEL.providerID && modelID === GATED_MODEL.modelID
                    ? limit
                    : undefined,
        );
    }

    function gatedMessages(sessionId: string): MessageLike[] {
        const messages = makeMessages(sessionId);
        (messages[0]!.info as Record<string, unknown>).model = GATED_MODEL;
        return messages;
    }

    for (const fits of [true, false]) {
        it(`${fits ? "publishes when the whole invocation fits the context limit with headroom" : "declines the pass when the whole invocation exceeds the context limit"} and leaves the host array intact`, async () => {
            const sessionId = `rust-invocation-budget-${fits}-${Date.now()}`;
            installAvailabilityDb(sessionId, {});
            installRawRows(sessionId, rawRows(1));
            const messages = gatedMessages(sessionId);
            const candidate = [...messages, ...makeMessages(sessionId)];
            const charged = chargeInvocation(
                candidate.map((entry) => editRecipe.canonicalJsonLength(entry)),
                { headroomPermille: 250, profile: "opencode-heuristic" },
            ).chargedTokens;
            const { client, calls } = recordingClient((request) =>
                recipeResponse(request, candidate),
            );
            const deps = makeDeps();
            const limit = fits ? charged : charged - 1;
            const limitSpy = trustContextLimit(limit);
            // The usage sample inverts to the opposite verdict, so the gate is proven to read the trusted limit.
            const invertedLimit = fits ? charged - 1 : charged + 1_000;
            deps.contextUsageMap.set(sessionId, {
                usage: { percentage: 50, inputTokens: invertedLimit / 2 },
                updatedAt: Date.now(),
                lastResponseTime: Date.now(),
                hasUsageTokens: true,
                model: GATED_MODEL,
            });
            const transform = createRustModeTransform(deps, { moduleClient: client });
            const member = messages[0];
            const output = { messages: [...messages] as unknown[] };
            const array = output.messages;
            const logSpy = spyOn(logger.sessionLog, "warn");
            try {
                await transform.run(sessionId, output);
                expect(output.messages).toBe(array);
                expect(calls).toHaveLength(1);
                if (fits) {
                    expect(output.messages).toEqual(candidate);
                } else {
                    expect(output.messages).toHaveLength(1);
                    expect(output.messages[0]).toBe(member);
                    expect(
                        sessionLogs(logSpy, sessionId).some((line) =>
                            line.includes(
                                `pass declined: invocation_budget (${charged} charged tokens over ${limit}, growing from`,
                            ),
                        ),
                    ).toBe(true);
                }
                expect(transform.getState(sessionId).failureCount).toBe(0);
            } finally {
                logSpy.mockRestore();
                limitSpy.mockRestore();
            }
        });
    }

    it("publishes a candidate over the context limit when it is no larger than the incoming surface", async () => {
        const sessionId = `rust-invocation-shrinks-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const messages = gatedMessages(sessionId);
        const candidate = makeMessages(sessionId);
        const { client } = recordingClient((request) => recipeResponse(request, candidate));
        const deps = makeDeps();
        const limitSpy = trustContextLimit(1);
        deps.contextUsageMap.set(sessionId, {
            usage: { percentage: 50, inputTokens: 1 },
            updatedAt: Date.now(),
            lastResponseTime: Date.now(),
            hasUsageTokens: true,
            model: GATED_MODEL,
        });
        const transform = createRustModeTransform(deps, { moduleClient: client });
        const output = { messages: [...messages] as unknown[] };
        try {
            await transform.run(sessionId, output);
        } finally {
            limitSpy.mockRestore();
        }
        expect(output.messages).toEqual(candidate);
    });

    it("gates nothing for a model models.dev cannot name, although its usage sample inverts to the 128k default", async () => {
        const sessionId = `rust-invocation-untrusted-limit-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const model = { providerID: "unknown-provider", modelID: "unknown-model-xyz" };
        expect(resolveTrustedContextLimit(model.providerID, model.modelID)).toBeUndefined();
        // Every producer of `usage.percentage` divides by `resolveContextLimit`, which is the 128k default for this model, so inverting the sample recovers a limit the host never reported.
        const inputTokens = 64_000;
        const percentage =
            (inputTokens / resolveContextLimit(model.providerID, model.modelID)) * 100;
        expect(Math.round(inputTokens / (percentage / 100))).toBe(DEFAULT_CONTEXT_LIMIT);
        const messages = makeMessages(sessionId);
        (messages[0]!.info as Record<string, unknown>).model = model;
        // 128k tokens at 3.5 chars per token under 25% headroom is about 358k canonical characters; this candidate grows past it.
        const candidate = [
            ...messages,
            ...rowMessages(sessionId, rawRows(1), () => "x".repeat(400_000)),
        ];
        expect(
            chargeInvocation(
                candidate.map((entry) => editRecipe.canonicalJsonLength(entry)),
                { headroomPermille: 250, profile: "opencode-heuristic" },
            ).chargedTokens,
        ).toBeGreaterThan(DEFAULT_CONTEXT_LIMIT);
        const { client } = recordingClient((request) => recipeResponse(request, candidate));
        const deps = makeDeps();
        deps.contextUsageMap.set(sessionId, {
            usage: { percentage, inputTokens },
            updatedAt: Date.now(),
            lastResponseTime: Date.now(),
            hasUsageTokens: true,
            model,
        });
        const transform = createRustModeTransform(deps, { moduleClient: client });
        const output = { messages: [...messages] as unknown[] };
        const logSpy = spyOn(logger.sessionLog, "warn");
        try {
            await transform.run(sessionId, output);
            expect(
                sessionLogs(logSpy, sessionId).filter((line) =>
                    line.includes("pass declined: invocation_budget"),
                ),
            ).toEqual([]);
            expect(output.messages).toEqual(candidate);
            expect(transform.getState(sessionId).failureCount).toBe(0);
        } finally {
            logSpy.mockRestore();
        }
    });

    it("sends the combined todowrite map and live-permission verdict", async () => {
        const sessionId = `rust-todo-permission-denied-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient((request) => recipeResponse(request, []));
        const deps = makeDeps();
        const agents = mock(async () => ({
            data: [{ name: "build", permission: { todowrite: "deny" } }],
        }));
        deps.client = {
            app: { agents },
            session: {
                get: async () => ({
                    data: { agent: "build", directory: "/tmp/project" },
                }),
            },
        } as never;
        const transform = createRustModeTransform(deps, { moduleClient: client });
        const messages = makeMessages(sessionId);
        (messages[0]!.info as { tools?: Record<string, boolean> }).tools = {};
        (messages[0]!.info as { agent?: string }).agent = "build";

        await transform.run(sessionId, { messages: messages as unknown[] });

        expect(agents).toHaveBeenCalledTimes(1);
        expect(bodies[0]?.todo_tool_present).toBe(false);
    });

    it("sends todowrite absent when an empty-cache permission read never settles", async () => {
        const sessionId = `rust-todo-permission-hang-${Date.now()}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient((request) => recipeResponse(request, []));
        const deps = makeDeps();
        const agents = mock(() => new Promise<never>(() => {}));
        deps.client = {
            app: { agents },
            session: {
                get: async () => ({
                    data: { agent: "build", directory: "/tmp/project" },
                }),
            },
        } as never;
        const transform = createRustModeTransform(deps, { moduleClient: client });
        const messages = makeMessages(sessionId);
        (messages[0]!.info as { tools?: Record<string, boolean> }).tools = {};
        (messages[0]!.info as { agent?: string }).agent = "build";

        const startedAt = performance.now();
        await transform.run(sessionId, { messages: messages as unknown[] });
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
        const { client, bodies } = recordingClient((request) =>
            recipeResponse(request, [{ role: "assistant", parts: [] }]),
        );
        const logSpy = spyOn(logger.sessionLog, "warn");
        try {
            const transform = createRustModeTransform(makeDeps(), {
                moduleClient: client,
            });
            for (let turn = 1; turn <= 4; turn += 1) {
                const input = [
                    ...makeMessages(sessionId),
                    {
                        info: {
                            id: `synthetic-${turn}`,
                            role: "user",
                            sessionID: sessionId,
                        },
                        parts: [{ type: "text", text: "synthetic turn", synthetic: true }],
                    },
                ];
                await transform.run(sessionId, { messages: input });
            }
            expect(transform.getState(sessionId).syntheticTurnCount).toBe(4);
            expect(bodies).toHaveLength(4);
            expect(
                sessionLogs(logSpy, sessionId).filter((message) =>
                    message.startsWith("rust synthetic-turn cascade: 3 consecutive"),
                ),
            ).toHaveLength(1);

            const realInput = makeMessages(sessionId);
            await transform.run(sessionId, { messages: realInput });
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
            return page.transform_page_complete === false
                ? { staged: true }
                : recipeResponse(page, native);
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const output = { messages: messages as unknown[] };
        await transform.run(sessionId, output);

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

    for (const mismatch of ["thrown-code", "returned-code", "message"] as const) {
        it(`restarts a paged transform series after a ${mismatch} attempt mismatch`, async () => {
            const sessionId = `rust-series-restart-${mismatch}-${Date.now()}`;
            installAvailabilityDb(sessionId, {});
            installRawRows(sessionId, rawRows(1));
            const messages = makeMessages(sessionId);
            messages[0]!.parts = [{ type: "text", text: "x".repeat(600_000) }];
            const native = [{ role: "assistant", parts: [] }];
            let failedPageId: unknown;
            const { client, bodies } = recordingClient((page) => {
                if (page.transform_page_index === 1 && failedPageId === undefined) {
                    failedPageId = page.transform_page_id;
                    if (mismatch === "returned-code")
                        return { code: "authority_transform_page_attempt_mismatch" };
                    if (mismatch === "message")
                        throw new Error("module error: authority_transform_page_attempt_mismatch");
                    throw Object.assign(
                        new Error(
                            "transform page generation or envelope changed during collection",
                        ),
                        { code: "authority_transform_page_attempt_mismatch" },
                    );
                }
                return page.transform_page_complete === true
                    ? recipeResponse(page, native, {
                          decision: "HARD",
                          served_from: "transform",
                      })
                    : { staged: true };
            });
            const logSpy = spyOn(logger.sessionLog, "warn");
            try {
                const transform = createRustModeTransform(makeDeps(), {
                    moduleClient: client,
                });
                const output = { messages: messages as unknown[] };
                await transform.run(sessionId, output);

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
    }

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
                ? recipeResponse(page, native, {
                      decision: "HARD",
                      served_from: "transform",
                  })
                : { staged: true };
        });
        const logSpy = spyOn(logger.sessionLog, "warn");
        try {
            const transform = createRustModeTransform(makeDeps(), {
                moduleClient: client,
            });
            const output = { messages: messages as unknown[] };
            await transform.run(sessionId, output);

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
        const { client, bodies } = recordingClient((request) =>
            recipeResponse(request, [], { decision: "PASSTHROUGH" }),
        );
        client.deleteSession = deleteSession;
        client.closeSession = closeSession;
        const deps = makeDeps();
        deps.sessionDirectoryBySession?.set(sessionId, "/session/root-b");
        const transform = createRustModeTransform(deps, { moduleClient: client });
        for (let pass = 0; pass < 2; pass += 1) {
            const input = makeMessages(sessionId);
            await transform.run(sessionId, { messages: [...input] });
        }
        expect(bodies[1]?.tail_delta).toBeDefined();

        transform.clearSession(sessionId);
        await Bun.sleep(0);
        expect(deleteSession).toHaveBeenCalledWith(sessionId, "/session/root-b");
        expect(closeSession).toHaveBeenCalledWith(sessionId);
        const afterClear = makeMessages(sessionId);
        await transform.run(sessionId, { messages: [...afterClear] });

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
        const { client } = recordingClient((request) => recipeResponse(request, []));
        client.deleteSession = deleteSession;
        client.closeSession = closeSession;
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });

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
        const { client, calls } = recordingClient((request) => recipeResponse(request, []));
        client.deleteSession = deleteSession;
        const logSpy = spyOn(logger.sessionLog, "debug");
        try {
            const transform = createRustModeTransform(deps, { moduleClient: client });
            const messages = makeMessages(sessionId);
            const output = { messages: [...messages] as unknown[] };
            const pass = transform.run(sessionId, output);
            while (releaseDirectoryRead === undefined) await Bun.sleep(0);
            transform.clearSession(sessionId);
            releaseDirectoryRead?.();
            await pass;

            expect(calls.filter((call) => call.method === "transform")).toHaveLength(0);
            expect(output.messages).toEqual(messages);
            expect(transform.getState(sessionId).consecutiveFailures).toBe(0);
            expect(
                sessionLogs(logSpy, sessionId).some((message) =>
                    message.includes("pass declined: cleared"),
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
        const { client } = recordingClient((request) => recipeResponse(request, []));
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

    it("forces a full send after a dispatched delta pass is source-declined", async () => {
        const sessionId = `rust-decline-after-dispatch-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        let live: unknown[] | undefined;
        const { client, bodies, calls } = recordingClient((request) => {
            // The daemon committed its snapshot for this request; the host replaces a member before the response is applied.
            if (live) live[0] = makeMessages(sessionId)[0];
            return recipeResponse(request, []);
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        for (let pass = 0; pass < 2; pass += 1) {
            await transform.run(sessionId, {
                messages: [...makeMessages(sessionId)],
            });
        }
        expect(bodies[1]?.tail_delta).toBeDefined();

        live = [...makeMessages(sessionId)];
        const original = live[0];
        await transform.run(sessionId, { messages: live });
        expect(bodies[2]?.tail_delta).toBeDefined();
        expect(live[0]).not.toBe(original);
        expect(calls.filter((call) => call.method === "transform").length).toBe(3);
        expect(transform.getState(sessionId).consecutiveFailures).toBe(0);
        expect(transform.getState(sessionId).forceFullWire).toBe(true);

        live = undefined;
        const next = makeMessages(sessionId);
        await transform.run(sessionId, { messages: [...next] });
        expect(bodies[3]?.tail_delta).toBeUndefined();
        expect(bodies[3]?.native_messages).toEqual(next);
        expect(transform.getState(sessionId).forceFullWire).toBe(false);
    });

    it("forces a full send after invalidateWireState", async () => {
        const sessionId = `rust-invalidate-wire-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient((request) => recipeResponse(request, []));
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        for (let pass = 0; pass < 2; pass += 1) {
            const input = makeMessages(sessionId);
            await transform.run(sessionId, { messages: [...input] });
        }
        expect(bodies[1]?.tail_delta).toBeDefined();

        transform.invalidateWireState(sessionId);
        expect(transform.getState(sessionId).forceFullWire).toBe(true);
        const input = makeMessages(sessionId);
        await transform.run(sessionId, { messages: [...input] });

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
                const request = body as Record<string, unknown>;
                bodies.push(request);
                if (bodies.length === 2) {
                    await new Promise<void>((resolve) => {
                        release = resolve;
                    });
                }
                return recipeResponse(request, []);
            },
        };
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const first = makeMessages(sessionId);
        await transform.run(sessionId, { messages: [...first] });

        const second = makeMessages(sessionId);
        const inFlight = transform.run(sessionId, { messages: [...second] });
        while (release === undefined) await Bun.sleep(0);
        expect(bodies[1]?.tail_delta).toBeDefined();
        transform.invalidateWireState(sessionId);
        release?.();
        await inFlight;

        expect(transform.getState(sessionId).forceFullWire).toBe(true);
        const third = makeMessages(sessionId);
        await transform.run(sessionId, { messages: [...third] });
        expect(bodies[2]?.tail_delta).toBeUndefined();
        expect(bodies[2]?.native_messages).toEqual(third);
        expect(transform.getState(sessionId).forceFullWire).toBe(false);
    });

    it("supersedes an older pass that finishes preflight after a newer pass starts", async () => {
        const sessionId = `rust-overlapping-preflight-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const started = Promise.withResolvers<void>();
        const directory = Promise.withResolvers<{ data: { directory: string } }>();
        const deps = makeDeps();
        deps.client = {
            session: {
                get: () => {
                    started.resolve();
                    return directory.promise;
                },
            },
        } as never;
        deps.sessionMetadataReadStateBySession = new Map();
        const { client, bodies } = recordingClient((request) =>
            recipeResponse(request, request.native_messages as unknown[]),
        );
        const transform = createRustModeTransform(deps, { moduleClient: client });
        const firstInput = makeMessages(sessionId);
        const firstOutput = { messages: [...firstInput] as unknown[] };
        const firstArray = firstOutput.messages;
        const firstMember = firstInput[0];
        const first = transform.run(sessionId, firstOutput);
        await Promise.race([started.promise, first]);
        const secondInput = makeMessages(sessionId);
        const secondOutput = { messages: [...secondInput] as unknown[] };
        const secondArray = secondOutput.messages;
        const secondMember = secondInput[0];
        const second = transform.run(sessionId, secondOutput);

        directory.resolve({ data: { directory: "/tmp/project" } });
        await Promise.all([first, second]);

        // The newer call declines while the older owner holds the session slot, and the older
        // owner is superseded before it can send: neither dispatches or publishes.
        expect(bodies).toHaveLength(0);
        expect(firstOutput.messages).toBe(firstArray);
        expect(firstOutput.messages[0]).toBe(firstMember);
        expect(secondOutput.messages).toBe(secondArray);
        expect(secondOutput.messages[0]).toBe(secondMember);
        // Only the admitted owner counts as a pass; the declined call never entered execution.
        expect(transform.getState(sessionId).passCount).toBe(1);
        expect(transform.getState(sessionId).failureCount).toBe(0);
        expect(transform.getState(sessionId).consecutiveFailures).toBe(0);
    });

    it("nacks note deliveries when a pass is superseded while its transform response is pending", async () => {
        const sessionId = `rust-overlapping-response-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const started = Promise.withResolvers<void>();
        const response = Promise.withResolvers<unknown>();
        const { client, bodies, calls } = recordingClient((request, index) => {
            if (index > 0) return recipeResponse(request, []);
            started.resolve();
            return response.promise;
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const firstInput = makeMessages(sessionId);
        const firstOutput = { messages: [...firstInput] as unknown[] };
        const firstArray = firstOutput.messages;
        const firstMember = firstInput[0];
        const first = transform.run(sessionId, firstOutput);
        await Promise.race([started.promise, first]);
        expect(calls).toHaveLength(1);
        const secondInput = makeMessages(sessionId);
        const secondOutput = { messages: [...secondInput] };
        const secondArray = secondOutput.messages;
        const secondMember = secondInput[0];
        await transform.run(sessionId, secondOutput);

        response.resolve(
            recipeForLast(bodies, [{ info: { id: "superseded" }, parts: [] }], {
                note_deliveries: [{ transform_pass_id: "pass-superseded" }],
            }),
        );
        await first;

        expect(firstOutput.messages).toBe(firstArray);
        expect(firstOutput.messages[0]).toBe(firstMember);
        expect(secondOutput.messages).toBe(secondArray);
        expect(secondOutput.messages[0]).toBe(secondMember);
        // The newer call declined without dispatch; the superseded owner nacks its own deliveries.
        expect(calls.map((call) => call.method)).toEqual(["transform", "transform.nack"]);
        expect(transform.getState(sessionId).failureCount).toBe(0);
    });

    it("does not resurrect a wire cache for a session cleared while its pass is in flight", async () => {
        const sessionId = `rust-clear-in-flight-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const started = Promise.withResolvers<void>();
        const release = Promise.withResolvers<void>();
        const bodies: Record<string, unknown>[] = [];
        const client: RustModeModuleClient = {
            call: async ({ body }) => {
                const request = body as Record<string, unknown>;
                bodies.push(request);
                if (bodies.length === 2) {
                    started.resolve();
                    await release.promise;
                }
                return recipeResponse(request, []);
            },
        };
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const first = makeMessages(sessionId);
        await transform.run(sessionId, { messages: [...first] });

        const second = makeMessages(sessionId);
        const output = { messages: [...second] };
        const array = output.messages;
        const member = second[0];
        const inFlight = transform.run(sessionId, output);
        await Promise.race([started.promise, inFlight]);
        expect(bodies).toHaveLength(2);
        transform.clearSession(sessionId);
        release.resolve();
        await inFlight;
        expect(output.messages).toBe(array);
        expect(output.messages[0]).toBe(member);

        const third = makeMessages(sessionId);
        await transform.run(sessionId, { messages: [...third] });
        expect(bodies[2]?.tail_delta).toBeUndefined();
        expect(bodies[2]?.native_messages).toEqual(third);
        expect(transform.getState(sessionId).passCount).toBe(1);
    });

    it("re-primes persisted ordinals after the continuation base when the memo is reset", async () => {
        const sessionId = `rust-continuation-reprime-${Date.now()}`;
        installRawRows(sessionId, rawRows(2));
        const { client, bodies } = recordingClient((request) =>
            recipeResponse(request, [], { ordinal_continuation_base: 10 }),
        );
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const ordinalsOf = (body: Record<string, unknown> | undefined): number[] => {
            if (!body) throw new Error("missing recorded transform request");
            return (body.messages as Array<{ ordinal: number }>).map((message) => message.ordinal);
        };

        const first = rowMessages(sessionId, rawRows(2));
        await transform.run(sessionId, { messages: [...first] });
        expect(ordinalsOf(bodies[0])).toEqual([1, 2]);
        const shiftedMemo = transform.getState(sessionId).ordinals;
        expect(shiftedMemo).toEqual({
            generation: 0,
            memoGeneration: 0,
            entries: new Map([
                ["m-1", 11],
                ["m-2", 12],
            ]),
            anchor: { timeCreated: 2, id: "m-2" },
            storedCount: 2,
            canonicalCount: 12,
            continuationBase: 10,
        });

        await transform.run(sessionId, { messages: [...first] });
        expect(bodies[1]?.tail_delta).toBeDefined();
        expect(transform.getState(sessionId).ordinals).toEqual(shiftedMemo);

        shiftedMemo.continuationBase = 100;
        shiftedMemo.canonicalCount = 102;
        shiftedMemo.entries.clear();
        expect(transform.getState(sessionId).ordinals.continuationBase).toBe(10);
        expect(transform.getState(sessionId).ordinals.canonicalCount).toBe(12);
        expect(transform.getState(sessionId).ordinals.entries.get("m-2")).toBe(12);

        transform.invalidateWireState(sessionId);
        expect(transform.getState(sessionId).ordinals).toEqual({
            generation: 0,
            memoGeneration: 0,
            entries: new Map(),
            anchor: null,
            storedCount: null,
            canonicalCount: 0,
            continuationBase: 10,
        });
        const second = rowMessages(sessionId, rawRows(2));
        await transform.run(sessionId, { messages: [...second] });
        expect(bodies[2]?.tail_delta).toBeUndefined();
        expect(ordinalsOf(bodies[2])).toEqual([11, 12]);
        expect(transform.getState(sessionId).ordinals.entries.get("m-1")).toBe(11);
        expect(transform.getState(sessionId).consecutiveFailures).toBe(0);

        transform.clearSession(sessionId);
        expect(transform.getState(sessionId).ordinals).toEqual({
            generation: 0,
            memoGeneration: 0,
            entries: new Map(),
        });
    });

    it("discards a partly shifted ordinal memo before host publication when shifting throws", async () => {
        const sessionId = "rust-continuation-shift-failure";
        installRawRows(sessionId, rawRows(2));
        const { client, calls } = recordingClient((request, index) => ({
            ...recipeResponse(request, []),
            ...(index > 0
                ? {
                      ordinal_continuation_base: 10,
                      note_deliveries: [{ transform_pass_id: `shift-${index}` }],
                  }
                : {}),
        }));
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const messages = rowMessages(sessionId, rawRows(2));
        await transform.run(sessionId, { messages: [...messages] });
        const priorMemo = transform.getState(sessionId).ordinals;
        const output = { messages: [...messages] };
        const array = output.messages;
        const set = Map.prototype.set;
        let shiftFailed = false;
        const setSpy = spyOn(Map.prototype, "set").mockImplementation(function (
            this: Map<unknown, unknown>,
            key,
            value,
        ) {
            if (key === "m-2" && value === 12) {
                shiftFailed = true;
                throw new Error("ordinal shift failed");
            }
            return set.call(this, key, value);
        });
        try {
            await transform.run(sessionId, output);
        } finally {
            setSpy.mockRestore();
        }
        expect(shiftFailed).toBe(true);
        // The failed pass republishes the first pass's empty output; nothing was appended since.
        expect(output.messages).toBe(array);
        expect(output.messages).toHaveLength(0);
        expect(transform.getState(sessionId).ordinals).toEqual(priorMemo);
        expect(calls.map((call) => call.method)).toEqual([
            "transform",
            "transform",
            "transform.nack",
        ]);
        expect(calls[2]?.body).toMatchObject({ transform_pass_id: "shift-1" });
        const retry = { messages: [...messages] };
        await transform.run(sessionId, retry);
        expect(retry.messages).toHaveLength(0);
        expect(transform.getState(sessionId).ordinals.entries).toEqual(
            new Map([
                ["m-1", 11],
                ["m-2", 12],
            ]),
        );
        expect(calls.at(-1)?.method).toBe("transform.ack");
        expect(calls.at(-1)?.body).toMatchObject({ transform_pass_id: "shift-2" });
    });

    it("rejects ordinal continuation overflow before publication and recovers on a valid response", async () => {
        const sessionId = "rust-continuation-overflow";
        installRawRows(sessionId, rawRows(1));
        const { client, calls } = recordingClient((request, index) => ({
            ...recipeResponse(request, []),
            ordinal_continuation_base: index === 0 ? Number.MAX_SAFE_INTEGER : 10,
            note_deliveries: [{ transform_pass_id: `overflow-${index}` }],
        }));
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const input = makeMessages(sessionId);
        const output = { messages: [...input] as unknown[] };
        const array = output.messages;
        await transform.run(sessionId, output);
        expect(output.messages).toBe(array);
        expect(output.messages[0]).toBe(input[0]);
        expect(transform.getState(sessionId).ordinals.entries.size).toBe(0);
        expect(transform.getState(sessionId).failureCount).toBe(1);
        expect(calls.map((call) => call.method)).toEqual(["transform", "transform.nack"]);
        await transform.run(sessionId, output);
        expect(output.messages).toHaveLength(0);
        expect(transform.getState(sessionId).ordinals.entries.get("m-1")).toBe(11);
        expect(calls.at(-1)?.method).toBe("transform.ack");
    });

    it("evicts the least recently used session's wire cache and sends its next pass in full", async () => {
        const capacity = __rustModeTransformTest.WIRE_CACHE_SESSION_CAPACITY;
        const stamp = Date.now();
        const sessionIdAt = (index: number): string => `rust-wire-cache-lru-${stamp}-${index}`;
        const bodiesBySession = new Map<string, Record<string, unknown>[]>();
        const client: RustModeModuleClient = {
            call: async ({ sessionId, body }) => {
                const request = body as Record<string, unknown>;
                const list = bodiesBySession.get(sessionId) ?? [];
                list.push(request);
                bodiesBySession.set(sessionId, list);
                return recipeResponse(request, []);
            },
        };
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        for (let index = 0; index <= capacity; index += 1) {
            const sessionId = sessionIdAt(index);
            installRawRows(sessionId, rawRows(1));
            const input = makeMessages(sessionId);
            await transform.run(sessionId, { messages: [...input] });
        }

        const newest = sessionIdAt(capacity);
        const newestInput = makeMessages(newest);
        await transform.run(newest, { messages: [...newestInput] });
        expect(bodiesBySession.get(newest)?.[1]?.tail_delta).toBeDefined();

        const evicted = sessionIdAt(0);
        const evictedInput = makeMessages(evicted);
        await transform.run(evicted, { messages: [...evictedInput] });
        expect(bodiesBySession.get(evicted)?.[1]?.tail_delta).toBeUndefined();
        expect(bodiesBySession.get(evicted)?.[1]?.native_messages).toEqual(evictedInput);
    });

    it("keeps a multi-frame tail delta paged instead of rebuilding the full wire", async () => {
        const sessionId = `rust-wire-paged-delta-${Date.now()}`;
        const rows = rawRows(3);
        installRawRows(sessionId, rows);
        const { client, bodies } = recordingClient((request) =>
            recipeResponse(request, [], { decision: "PASSTHROUGH" }),
        );
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const buildMessages = () =>
            rowMessages(sessionId, rows, (row) =>
                row.id === "m-4" ? `large delta ${"x".repeat(350 * 1024)}` : `message ${row.id}`,
            );

        const initial = buildMessages();
        await transform.run(sessionId, { messages: [...initial] });
        rows.push({
            id: "m-4",
            timeCreated: 4,
            contributesOrdinal: true,
            hasValidInfo: true,
        });
        const appended = buildMessages();
        await transform.run(sessionId, { messages: [...appended] });

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

describe("recipe application", () => {
    for (const mutationTime of ["before request", "pending response"] as const) {
        it(`rejects mutated retained output ${mutationTime}`, async () => {
            const sessionId = `retained-mutation-${mutationTime}-${Date.now()}`;
            installRawRows(sessionId, rawRows(1));
            const original = makeMessages(sessionId);
            const fresh = structuredClone(original);
            let detachedFirst: unknown;
            const mutate = () => {
                original[0].parts[0].text = "mutated".repeat(1024);
            };
            const { client, bodies, calls } = recordingClient(async (body, index) => {
                const request = JSON.parse(JSON.stringify(body));
                if (index === 0) detachedFirst = request.native_messages;
                if (index === 1 && mutationTime === "pending response") {
                    await Promise.resolve();
                    mutate();
                }
                return {
                    status: "ok",
                    base_revision: request.base_revision,
                    output_revision: `retained-${index}`,
                    ...(request.previous_output_revision
                        ? { previous_output_revision: request.previous_output_revision }
                        : {}),
                    operations: [
                        {
                            op: "keep",
                            source: request.previous_output_revision ? "previous" : "input",
                            start: 0,
                            count: 1,
                        },
                    ],
                    ...(index === 1
                        ? { note_deliveries: [{ transform_pass_id: "mutation-pass" }] }
                        : {}),
                };
            });
            const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
            const first = { messages: [...original] as unknown[] };
            await transform.run(sessionId, first);
            expect(first.messages[0]).toBe(original[0]);
            if (mutationTime === "before request") mutate();
            const second = { messages: [...fresh] as unknown[] };
            await transform.run(sessionId, second);
            expect(bodies).toHaveLength(2);
            expect(second.messages).toEqual(detachedFirst);
            expect(second.messages[0]).toBe(fresh[0]);
            if (mutationTime === "before request") {
                expect(bodies[1].previous_output_revision).toBeUndefined();
                expect(calls.at(-1)?.method).toBe("transform.ack");
            } else {
                expect(bodies[1].previous_output_revision).toBe("retained-0");
                expect(calls.at(-1)?.method).toBe("transform.nack");
            }
        });
    }

    it("keeps from the applied previous output and the submitted input, then acks its note deliveries", async () => {
        const sessionId = `rust-native-delta-ack-${Date.now()}`;
        const rows = rawRows(1);
        installRawRows(sessionId, rows);
        const first = [
            { info: { id: "m0" }, parts: [{ type: "text", text: "stable" }] },
            { info: { id: "m-1" }, parts: [{ type: "text", text: "old" }] },
        ];
        const inserted = {
            info: { id: "m-1" },
            parts: [{ type: "text", text: "new" }],
        };
        let firstOutputRevision: unknown;
        const { client, bodies, calls } = recordingClient((request, index) => {
            if (index === 0) {
                const response = recipeResponse(request, first);
                firstOutputRevision = response.output_revision;
                return response;
            }
            return {
                base_revision: request.base_revision,
                output_revision: "out-second",
                previous_output_revision: request.previous_output_revision,
                operations: [
                    { op: "keep", source: "previous", start: 0, count: 1 },
                    { op: "insert", values: [inserted] },
                    { op: "keep", source: "input", start: 1, count: 1 },
                ],
                note_deliveries: [{ transform_pass_id: "pass-1" }],
            };
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const initial = rowMessages(sessionId, rows);
        await transform.run(sessionId, { messages: [...initial] });
        rows.push({
            id: "m-2",
            timeCreated: 2,
            contributesOrdinal: true,
            hasValidInfo: true,
        });
        const appended = rowMessages(sessionId, rows);
        const output = { messages: [...appended] as unknown[] };
        await transform.run(sessionId, output);

        // The second request advertises the first pass's applied output as its previous source.
        expect(typeof firstOutputRevision).toBe("string");
        expect(bodies[1]?.previous_output_revision).toBe(firstOutputRevision);
        expect(output.messages).toEqual([first[0], inserted, appended[1]]);
        expect(output.messages[0]).toBe(first[0]);
        expect(output.messages[2]).toBe(appended[1]);
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
            (request) => ({
                ...recipeResponse(request, native),
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
        const logSpy = spyOn(logger.sessionLog, "warn");
        try {
            const transform = createRustModeTransform(makeDeps(), {
                moduleClient: client,
            });
            const input = makeMessages(sessionId);
            const output = { messages: [...input] as unknown[] };

            await transform.run(sessionId, output);

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
            (request, index) =>
                index === 0
                    ? {
                          ...recipeResponse(request, firstNative),
                          note_deliveries: [{ transform_pass_id: "pass-first" }],
                      }
                    : recipeResponse(request, secondNative),
            async (method) => {
                if (method === "transform.ack") {
                    await new Promise<void>((resolve) => {
                        releaseAck = resolve;
                    });
                }
                return { ok: true };
            },
        );
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const firstInput = rowMessages(sessionId, rawRows(1));
        const firstOutput = { messages: [...firstInput] as unknown[] };
        const first = transform.run(sessionId, firstOutput);
        while (releaseAck === undefined) await Bun.sleep(0);
        const secondInput = rowMessages(sessionId, rawRows(2));
        const secondOutput = { messages: [...secondInput] as unknown[] };
        const second = transform.run(sessionId, secondOutput);
        await second;
        releaseAck();
        await first;
        const thirdInput = rowMessages(sessionId, rawRows(3));
        await transform.run(sessionId, { messages: [...thirdInput] });

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
        const { client, calls } = recordingClient((request) => ({
            ...recipeResponse(request, []),
            note_deliveries: [
                { transform_pass_id: "pass-1" },
                { transform_pass_id: "pass-1" },
                { transform_pass_id: "pass-2" },
                { transform_pass_id: "pass-1" },
            ],
        }));
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const input = makeMessages(sessionId);

        await transform.run(sessionId, { messages: [...input] });

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
            (request) => ({
                boundary_id: "m-1#0",
                ...recipeResponse(request, [{ role: "assistant", parts: [] }]),
                note_deliveries: [{ transform_pass_id: "pass-1" }, { transform_pass_id: "pass-2" }],
            }),
            (_method, body) => {
                if (body.transform_pass_id === "pass-1") throw nackFailure;
                return { ok: true };
            },
        );
        const logSpy = spyOn(logger.sessionLog, "warn");
        try {
            const transform = createRustModeTransform(makeDeps(), {
                moduleClient: client,
            });
            const input = makeMessages(sessionId);
            const output = { messages: [...input] as unknown[] };

            await transform.run(sessionId, output);

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

    it("nacks every delivery of an ok response that carries no recipe and does not retry", async () => {
        const sessionId = `rust-note-delivery-no-recipe-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const { client, bodies, calls } = recordingClient(() => ({
            status: "ok",
            served_from: "transform",
            note_deliveries: [
                { transform_pass_id: "pass-initial" },
                { transform_pass_id: "pass-shared" },
            ],
        }));
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const input = makeMessages(sessionId);
        const output = { messages: [...input] as unknown[] };

        await transform.run(sessionId, output);

        // A response without operations is an invalid recipe, not a request for another format.
        expect(bodies).toHaveLength(1);
        expect(output.messages).toEqual(input);
        expect(
            calls
                .filter((call) => call.method === "transform.nack")
                .map((call) => (call.body as Record<string, unknown>).transform_pass_id),
        ).toEqual(["pass-initial", "pass-shared"]);
        expect(calls.some((call) => call.method === "transform.ack")).toBe(false);
        expect(transform.getState(sessionId).failureCount).toBe(1);
    });

    it("nacks initial and retry delivery IDs when the full retry still cannot be applied", async () => {
        const sessionId = `rust-note-delivery-retry-failure-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const { client, calls } = recordingClient((request, index) =>
            index === 0
                ? {
                      status: "need_full_sync",
                      note_deliveries: [{ transform_pass_id: "pass-initial" }],
                  }
                : {
                      ...recipeResponse(request, []),
                      base_revision: "not-the-retry-base",
                      note_deliveries: [{ transform_pass_id: "pass-retry" }],
                  },
        );
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const input = makeMessages(sessionId);
        const output = { messages: [...input] as unknown[] };

        await transform.run(sessionId, output);

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
        const { client, calls } = recordingClient((request) =>
            recipeResponse(
                request,
                [
                    {
                        info: { role: "user", sessionID: sessionId },
                        parts: [{ type: "text", text: "not marked synthetic" }],
                    },
                ],
                {
                    action: "CACHE_HIT",
                    served_from: "transform",
                    boundary_id: "m-1#0",
                    note_deliveries: [{ transform_pass_id: "pass-1" }],
                },
            ),
        );
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const output = { messages: input as unknown[] };

        await transform.run(sessionId, output);

        expect(output.messages).toBe(input);
        expect(output.messages[0]).toEqual(input[0]);
        expect(calls.map((call) => call.method)).toEqual(["transform", "transform.nack"]);
        expect(transform.getState(sessionId).failureCount).toBe(1);
    });

    it("fails a delta pass whose response carries no recipe and sends the next pass in full", async () => {
        const sessionId = `rust-recipe-omission-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient((request, index) => {
            if (index === 0)
                return recipeResponse(request, structuredClone(request.native_messages));
            if (request.tail_delta) return { status: "ok", served_from: "transform" };
            return recipeResponse(request, structuredClone(request.native_messages));
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const initial = makeMessages(sessionId);
        await transform.run(sessionId, { messages: [...initial] });

        const changed = structuredClone(initial);
        changed[0]!.parts = [{ type: "text", text: "changed after warm prime" }];
        const output = { messages: [...changed] as unknown[] };
        await transform.run(sessionId, output);

        // The delta pass is refused as an invalid recipe; no second body is built for it.
        expect(bodies).toHaveLength(2);
        expect(bodies[1]?.tail_delta).toEqual({
            after: bodies[0]?.full_array_fingerprint,
            replace_from: 0,
            native_replace_from: 0,
        });
        expect(output.messages).toEqual(changed);
        expect(transform.getState(sessionId).consecutiveFailures).toBe(1);

        // The failed pass never applied, so the next pass resends the full array and recovers.
        const recovered = { messages: [...changed] as unknown[] };
        await transform.run(sessionId, recovered);
        expect(bodies).toHaveLength(3);
        expect(bodies[2]?.tail_delta).toBeUndefined();
        expect(bodies[2]?.native_messages).toEqual(changed);
        expect(transform.getState(sessionId).consecutiveFailures).toBe(0);
    });

    it("rejects a recipe that keeps from a previous output the client did not apply", () => {
        const admission = new TransformCaptureAdmission().admit("invalid-recipe");
        if (!("lease" in admission)) throw new Error("admission failed");
        const input = {
            revision: "base",
            values: [{ info: { id: "m0" } }],
            lengths: [17],
        };
        expect(() =>
            __rustModeTransformTest.applyTransformRecipe(
                {
                    base_revision: "base",
                    output_revision: "out",
                    previous_output_revision: "stale",
                    operations: [{ op: "keep", source: "previous", start: 0, count: 1 }],
                },
                input,
                undefined,
                (slots) => admission.lease.reserve(slots * 8),
            ),
        ).toThrow("missing_previous_base");
        expect(() =>
            __rustModeTransformTest.applyTransformRecipe(
                { base_revision: "other", output_revision: "out", operations: [] },
                input,
                undefined,
                (slots) => admission.lease.reserve(slots * 8),
            ),
        ).toThrow("wrong_base_revision");
        expect(() =>
            __rustModeTransformTest.applyTransformRecipe(
                { base_revision: "base", output_revision: "out" },
                input,
                undefined,
                (slots) => admission.lease.reserve(slots * 8),
            ),
        ).toThrow("rust transform recipe rejected: malformed");
        admission.lease.release();
    });

    it("evicts the least recently retained applied output once the optional budget is exceeded", () => {
        const evicted: string[] = [];
        const budget = new __rustModeTransformTest.AppliedOutputBudget(100, (sessionId) =>
            evicted.push(sessionId),
        );
        expect(budget.retain("a", 60)).toBe(true);
        expect(budget.retain("b", 30)).toBe(true);
        expect(budget.usedBytes).toBe(90);
        // Re-retaining a session replaces its charge instead of adding to it.
        expect(budget.retain("a", 50)).toBe(true);
        expect(budget.usedBytes).toBe(80);
        expect(evicted).toEqual([]);
        // The oldest retention goes first; `b` is older than the refreshed `a`.
        expect(budget.retain("c", 40)).toBe(true);
        expect(evicted).toEqual(["b"]);
        expect(budget.usedBytes).toBe(90);
        expect(budget.retain("d", 101)).toBe(false);
        expect(budget.usedBytes).toBe(90);
        budget.release("a");
        expect(budget.usedBytes).toBe(40);
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
            return recipeResponse(request, structuredClone(moduleNativeSnapshot));
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const buildMessages = (count: number, mutatePrefix = false): MessageLike[] =>
            rowMessages(sessionId, rows.slice(0, count), (row) =>
                mutatePrefix && row.id === "m-1" ? "MESSAGE m-1" : `message ${row.id}`,
            );

        const first = buildMessages(3);
        await transform.run(sessionId, { messages: [...first] });
        const appended = buildMessages(4);
        await transform.run(sessionId, { messages: [...appended] });
        expect(bodies).toHaveLength(2);
        expect(bodies[1]?.tail_delta).toEqual({
            after: expect.any(String),
            replace_from: expect.any(Number),
            native_replace_from: expect.any(Number),
        });

        const mutated = buildMessages(4, true);
        expect(JSON.stringify(mutated[0])).not.toEqual(JSON.stringify(appended[0]));
        const mutatedOutput = { messages: [...mutated] as unknown[] };
        await transform.run(sessionId, mutatedOutput);
        expect(bodies).toHaveLength(3);
        expect(bodies[2]?.tail_delta).toBeUndefined();
        expect(bodies[2]?.messages).toHaveLength(4);
        expect(bodies[2]?.native_messages).toEqual(mutated);
        expect(JSON.stringify(bodies[2]?.messages)).toContain("MESSAGE m-1");
        expect(mutatedOutput.messages).toEqual(mutated);

        const stableOutput = { messages: [...mutated] as unknown[] };
        await transform.run(sessionId, stableOutput);
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
        const { client, bodies } = recordingClient((request) => recipeResponse(request, []));
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const buildMessages = (count: number, mutateTerminal = false): MessageLike[] =>
            rowMessages(sessionId, rows.slice(0, count), (row) =>
                mutateTerminal && row.id === "m-2" ? "MESSAGE m-2" : `message ${row.id}`,
            );

        const first = buildMessages(2);
        await transform.run(sessionId, { messages: [...first] });
        const unchanged = buildMessages(2);
        await transform.run(sessionId, { messages: [...unchanged] });
        expect(bodies[1]?.tail_delta).toEqual({
            after: bodies[0]?.full_array_fingerprint,
            replace_from: 2,
            native_replace_from: 2,
        });

        rows.push({
            id: "m-3",
            timeCreated: 3,
            contributesOrdinal: true,
            hasValidInfo: true,
        });
        const appended = buildMessages(3, true);
        await transform.run(sessionId, { messages: [...appended] });
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
        const { client, bodies } = recordingClient((request) => recipeResponse(request, []));
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
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
        await transform.run(sessionId, { messages: [...first] });
        expect(bodies[0]?.messages).toHaveLength(1);

        rows.push({
            id: "m-3",
            timeCreated: 3,
            contributesOrdinal: true,
            hasValidInfo: true,
        });
        const appended = buildMessages(3, "summary v2");
        await transform.run(sessionId, { messages: [...appended] });
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
        const { client, bodies } = recordingClient((request, index) => {
            if (index === 1 || index === 2) throw new Error("CK message block identity drift");
            return recipeResponse(request, recovered);
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });

        await transform.run(sessionId, { messages: initial as unknown[] });
        for (let retry = 0; retry < 2; retry += 1) {
            const output = { messages: mutated as unknown[] };
            await transform.run(sessionId, output);
            expect(output.messages).toBe(mutated);
        }
        expect(transform.getState(sessionId).consecutiveFailures).toBe(2);

        const recoveredOutput = { messages: mutated as unknown[] };
        await transform.run(sessionId, recoveredOutput);
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
        writer.prepare("INSERT INTO message VALUES (?, ?, ?, ?, ?)").run(
            "assistant",
            sessionId,
            100,
            100,
            JSON.stringify({
                role: "assistant",
                finish: "stop",
                time: { completed: 100 },
            }),
        );
        writer
            .prepare("INSERT INTO message VALUES (?, ?, ?, ?, ?)")
            .run("new-user", sessionId, 200, 200, '{"role":"user"}');
        writer
            .prepare("INSERT INTO part VALUES (?, ?, ?, ?)")
            .run("part", "new-user", sessionId, '{"type":"text","text":"prompt","ignored":true}');
        installRawRows(sessionId, rawRows(1));
        const { client, bodies } = recordingClient((request) =>
            recipeResponse(request, makeMessages(sessionId)),
        );
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        try {
            for (let pass = 0; pass < 3; pass++) {
                const messages = makeMessages(sessionId);
                await transform.run(sessionId, { messages: [...messages] });
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
        await transform.run(sessionId, { messages: [...messages] });
        expect(bodies).toHaveLength(4);
        expect(bodies[3].mid_turn).toBe(false);
    });
});

describe("bounded transform ownership", () => {
    it.each([
        "initial",
        "full-retry",
    ] as const)("rejects a getter installed after ordinal resolution before %s encoding", async (phase) => {
        const sessionId = `rust-ordinal-microtask-${phase}`;
        installAvailabilityDb(sessionId, {});
        let rows = rawRows(1);
        let messages = makeMessages(sessionId);
        let armed = phase === "initial";
        let installed = false;
        let getterCalls = 0;
        const getter = () => {
            getterCalls += 1;
            return "changed after ordinal validation";
        };
        unregisters.push(
            setRawMessageProvider(sessionId, {
                readMessages: () => rows as unknown as RawMessage[],
                readMessageOrdinalPage: (after, limit) =>
                    rows
                        .filter((row) => !after || row.timeCreated > after.timeCreated)
                        .slice(0, limit),
                getStoredMessageCount: () => {
                    if (armed) {
                        armed = false;
                        // The scan and prime continuations run first; the third microtask follows annotation but precedes the caller's encoding.
                        queueMicrotask(() =>
                            queueMicrotask(() =>
                                queueMicrotask(() => {
                                    installed = true;
                                    Object.defineProperty(messages[0]!.parts[0], "text", {
                                        get: getter,
                                        enumerable: true,
                                        configurable: true,
                                    });
                                }),
                            ),
                        );
                    }
                    return rows.length;
                },
            }),
        );
        const { client, bodies, calls } = recordingClient((request, index) => {
            if (phase === "full-retry" && index === 1) {
                armed = true;
                return {
                    status: "need_full_sync",
                    note_deliveries: [{ transform_pass_id: "discarded" }],
                };
            }
            return recipeResponse(request, request.native_messages);
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        if (phase === "full-retry") {
            await transform.run(sessionId, { messages: [...messages] });
            expect(transform.getState(sessionId).initialized).toBe(true);
            rows = rawRows(2);
            messages = rowMessages(sessionId, rows, () => "hello");
        }
        const memoBefore = transform.getState(sessionId).ordinals;
        const output = { messages: messages as unknown[] };
        await transform.run(sessionId, output);
        expect(installed).toBe(true);
        expect(getterCalls).toBe(0);
        expect(bodies).toHaveLength(phase === "initial" ? 0 : 2);
        expect(output.messages).toBe(messages);
        expect(transform.getState(sessionId).ordinals).toEqual(memoBefore);
        expect(calls.some((call) => call.method === "transform.ack")).toBe(false);
        if (phase === "full-retry") {
            expect(bodies[1].tail_delta).toBeDefined();
            expect(calls.at(-1)?.method).toBe("transform.nack");
        }
        expect(defaultTransformCaptureAdmission.chargedBytes).toBe(0);
    });

    it.each([
        "x",
        "\u001f",
    ])("budgets escaped wire text before preflight for %j", async (character) => {
        const sessionId = `rust-escaped-wire-${character.charCodeAt(0)}`;
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const messages = makeMessages(sessionId);
        messages[0]!.parts = [{ type: "text", text: character.repeat(100_000) }];
        const deps = makeDeps();
        const directoryRead = spyOn(deps.client.session, "get");
        const admission = new TransformCaptureAdmission({
            maxPasses: 1,
            maxBytes: 1_200_000,
        });
        const { client, bodies } = recordingClient((page) =>
            page.transform_page_complete === false ? { staged: true } : recipeResponse(page, []),
        );
        const transform = createRustModeTransform(deps, {
            moduleClient: client,
            captureAdmission: admission,
        });
        const output = { messages: messages as unknown[] };
        try {
            await transform.run(sessionId, output);
            expect(directoryRead.mock.calls.length > 0).toBe(character === "x");
            expect(bodies.length > 0).toBe(character === "x");
            expect(transform.getState(sessionId).initialized).toBe(character === "x");
            if (character !== "x") expect(output.messages).toEqual(messages);
            expect(admission.chargedBytes).toBe(0);
        } finally {
            directoryRead.mockRestore();
        }
    });

    it("admits a full pass over 1,000 realistic messages under the default budget with room for a second session", async () => {
        const sessionId = "rust-admission-thousand";
        const rows = rawRows(1000);
        installRawRows(sessionId, rows);
        // Two parts per message, about 2 KiB of JSON each, approximates a long OpenCode session.
        const messages: MessageLike[] = rows.map((row, index) => ({
            info: {
                id: row.id,
                role: index % 2 === 0 ? "user" : "assistant",
                sessionID: sessionId,
                time: { created: index, completed: index + 1 },
                providerID: "anthropic",
                modelID: "claude-sonnet",
            },
            parts: [
                { id: `${row.id}-text`, type: "text", text: "x".repeat(1_800) },
                {
                    id: `${row.id}-tool`,
                    type: "tool",
                    tool: "bash",
                    callID: `call-${index}`,
                    state: {
                        status: "completed",
                        input: { command: "ls" },
                        output: "ok",
                        title: "ls",
                    },
                },
            ],
        }));
        const jsonBytes = Buffer.byteLength(JSON.stringify(messages));
        expect(jsonBytes).toBeGreaterThan(2 * 1024 * 1024);
        const admission = new TransformCaptureAdmission();
        let peakCharge = 0;
        const { client, bodies } = recordingClient((page) => {
            peakCharge = Math.max(peakCharge, admission.chargedBytes);
            // The request pages; the complete page carries the response for the whole array.
            return page.transform_page_complete === false
                ? { staged: true }
                : recipeResponse(page, messages);
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
            captureAdmission: admission,
        });
        const output = { messages: [...messages] as unknown[] };
        await transform.run(sessionId, output);
        expect(bodies.length).toBeGreaterThan(0);
        expect(output.messages).toHaveLength(messages.length);
        expect(transform.getState(sessionId).failureCount).toBe(0);
        // The held charge is a bounded multiple of the JSON size, and two such sessions fit the 64 MiB owner.
        expect(peakCharge).toBeGreaterThan(jsonBytes);
        expect(peakCharge).toBeLessThan(admission.remainingBytes / 2);
        expect(admission.chargedBytes).toBe(0);
    });

    for (const fault of ["mutation", "accessor", "invalidation"] as const) {
        it.each([
            "reconnect",
            "attempt-mismatch",
        ])(`stops a series restart after %s when ${fault} lands first`, async (restart) => {
            const sessionId = `rust-${restart}-${fault}`;
            installAvailabilityDb(sessionId, {});
            installRawRows(sessionId, rawRows(1));
            const messages = makeMessages(sessionId);
            messages[0]!.parts = [{ type: "text", text: "x".repeat(600_000) }];
            const member = messages[0];
            let getterCalls = 0;
            const getter = () => {
                getterCalls += 1;
                return "changed";
            };
            const reachedPage1 = Promise.withResolvers<void>();
            const reconnect = Promise.withResolvers<unknown>();
            let reconnectReported = false;
            const { client, bodies, calls } = recordingClient((page) => {
                if (page.transform_page_index === 1 && !reconnectReported) {
                    reconnectReported = true;
                    reachedPage1.resolve();
                    return reconnect.promise;
                }
                if (page.transform_page_complete === true) {
                    return {
                        ...recipeResponse(page, [{ role: "assistant", parts: [] }]),
                        note_deliveries: [{ transform_pass_id: "restarted" }],
                    };
                }
                return {
                    staged: true,
                    note_deliveries: [{ transform_pass_id: "page-zero" }],
                };
            });
            const transform = createRustModeTransform(makeDeps(), {
                moduleClient: client,
            });
            const output = { messages: messages as unknown[] };
            const pass = transform.run(sessionId, output);
            await Promise.race([reachedPage1.promise, pass]);
            // Enabling state: page zero was accepted and page one is pending.
            expect(bodies.filter((body) => body.transform_page_index === 0)).toHaveLength(1);
            expect(bodies.at(-1)?.transform_page_index).toBe(1);
            if (fault === "mutation") (member.parts[0] as { text: string }).text = "changed";
            else if (fault === "accessor")
                Object.defineProperty(member.parts[0], "text", {
                    get: getter,
                    enumerable: true,
                    configurable: true,
                });
            else transform.invalidateWireState(sessionId);
            reconnect.resolve(
                restart === "reconnect"
                    ? {
                          transport_status: "connection_generation_changed",
                          previous_generation: 3,
                          current_generation: 4,
                      }
                    : { code: "authority_transform_page_attempt_mismatch" },
            );
            await pass;
            expect(getterCalls).toBe(0);
            expect(bodies.filter((body) => body.transform_page_index === 0)).toHaveLength(1);
            expect(output.messages).toBe(messages);
            expect(output.messages[0]).toBe(member);
            expect(calls.some((call) => call.method === "transform.ack")).toBe(false);
            // Every delivery either series reported is NACKed for this attempt.
            expect(
                calls
                    .filter((call) => call.method === "transform.nack")
                    .map((call) => (call.body as { transform_pass_id: string }).transform_pass_id)
                    .sort(),
            ).toEqual(["page-zero"]);
            expect(transform.getState(sessionId).failureCount).toBe(0);
            expect(defaultTransformCaptureAdmission.chargedBytes).toBe(0);
        });
    }

    for (const offset of [0, 1] as const) {
        it(`${offset === 0 ? "publishes at the exact candidate charge" : "declines one byte short of the candidate charge"} and leaves the host array intact`, async () => {
            const sessionId = `rust-candidate-charge-${offset}`;
            installRawRows(sessionId, rawRows(1));
            const messages = makeMessages(sessionId);
            const member = messages[0];
            const candidateLength = 12;
            const started = Promise.withResolvers<void>();
            const response = Promise.withResolvers<unknown>();
            const { client, bodies, calls } = recordingClient(() => {
                started.resolve();
                return response.promise;
            });
            const admission = new TransformCaptureAdmission({
                maxPasses: 2,
                maxBytes: 4 * 1024 * 1024,
            });
            const transform = createRustModeTransform(makeDeps(), {
                moduleClient: client,
                captureAdmission: admission,
            });
            const output = { messages: [...messages] as unknown[] };
            const array = output.messages;
            const logSpy = spyOn(logger.sessionLog, "warn");
            try {
                const pass = transform.run(sessionId, output);
                await Promise.race([started.promise, pass]);
                expect(calls).toHaveLength(1);
                const blocker = admission.admit(`${sessionId}-blocker`);
                if (!("lease" in blocker)) throw new Error("blocker not admitted");
                expect(
                    blocker.lease.reserve(admission.remainingBytes - candidateLength * 24 + offset),
                ).toBe(true);
                response.resolve(
                    recipeForLast(
                        bodies,
                        Array.from({ length: candidateLength }, (_, index) => ({
                            info: {
                                id: `out-${index}`,
                                role: "assistant",
                                sessionID: sessionId,
                            },
                            parts: [],
                        })),
                        { note_deliveries: [{ transform_pass_id: "candidate" }] },
                    ),
                );
                await pass;
                blocker.lease.release();
                expect(output.messages).toBe(array);
                if (offset === 0) {
                    expect(output.messages).toHaveLength(candidateLength);
                    expect(calls.at(-1)?.method).toBe("transform.ack");
                } else {
                    expect(output.messages).toHaveLength(1);
                    expect(output.messages[0]).toBe(member);
                    expect(calls.at(-1)?.method).toBe("transform.nack");
                    expect(sessionLogs(logSpy, sessionId)).toContain(
                        `rust session ${sessionId} pass declined: capture_bytes (transform capture byte budget exceeded: recipe output array)`,
                    );
                    expect(transform.getState(sessionId).ordinals.entries.size).toBe(0);
                }
                expect(calls.at(-1)?.body).toMatchObject({
                    transform_pass_id: "candidate",
                });
                expect(transform.getState(sessionId).failureCount).toBe(0);
                expect(admission.chargedBytes).toBe(0);
            } finally {
                logSpy.mockRestore();
            }
        });
    }

    it("recovers with a full request when byte pressure rejects a full-sync retry", async () => {
        const sessionId = "rust-full-retry-byte-pressure";
        const rows = rawRows(3);
        installRawRows(sessionId, rows);
        // A compaction summary is wire-invisible, so the append that follows it ships as a tail delta starting after it; the full retry must then project the two leading messages it skipped.
        const input: MessageLike[] = rows.map((row, index) =>
            index === 1
                ? {
                      info: {
                          id: row.id,
                          role: "assistant",
                          sessionID: sessionId,
                          summary: true,
                          finish: "stop",
                      },
                      parts: [{ type: "text", text: "s".repeat(512) }],
                  }
                : {
                      info: { id: row.id, role: "user", sessionID: sessionId },
                      parts: [{ type: "text", text: "x".repeat(512) }],
                  },
        );
        const size = inspectReferenceableMessages(input);
        if (!size.ok) throw new Error("invalid fixture");
        // A full pass fits the budget alone. While another session holds the appended message's projection, the tail-delta pass still fits but its full retry, which adds the two leading messages, does not.
        const wireBytes = size.messageWireBytes.map((bytes) => 4 * bytes);
        const admission = new TransformCaptureAdmission({
            maxPasses: 2,
            maxBytes: size.estimatedBytes + wireBytes[0] + wireBytes[1] + wireBytes[2] + 2000,
        });
        const { client, calls, bodies } = recordingClient((request, index) => {
            if (index === 0) return recipeResponse(request, request.native_messages);
            if (index === 1)
                return {
                    status: "need_full_sync",
                    note_deliveries: [{ transform_pass_id: "discarded" }],
                };
            return {
                ...recipeResponse(request, []),
                note_deliveries: [{ transform_pass_id: "applied" }],
            };
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
            captureAdmission: admission,
        });
        const first = input.slice(0, 2);
        await transform.run(sessionId, { messages: first });
        expect(bodies).toHaveLength(1);
        expect(admission.chargedBytes).toBe(0);
        const blocker = admission.admit("rust-full-retry-byte-pressure-blocker");
        if (!("lease" in blocker)) throw new Error("blocker not admitted");
        expect(blocker.lease.reserve(wireBytes[2])).toBe(true);
        const output = { messages: input as unknown[] };
        const array = output.messages;
        const logSpy = spyOn(logger.sessionLog, "warn");
        try {
            await transform.run(sessionId, output);
            expect(bodies).toHaveLength(2);
            expect(bodies[1].tail_delta).toMatchObject({ native_replace_from: 2 });
            expect(sessionLogs(logSpy, sessionId)).toContain(
                `rust session ${sessionId} pass declined: capture_bytes (transform capture byte budget exceeded: wire projection)`,
            );
        } finally {
            logSpy.mockRestore();
        }
        expect(output.messages).toBe(array);
        expect(output.messages).toHaveLength(3);
        expect(transform.getState(sessionId).failureCount).toBe(0);
        expect(transform.getState(sessionId).forceFullWire).toBe(true);
        expect(admission.chargedBytes).toBe(wireBytes[2]);
        expect(calls.at(-1)?.body).toMatchObject({
            transform_pass_id: "discarded",
        });
        expect(calls.at(-1)?.method).toBe("transform.nack");
        // Once the other session settles, the next call is a full send that fits the budget.
        blocker.lease.release();
        await transform.run(sessionId, output);
        expect(bodies[2].tail_delta).toBeUndefined();
        expect(output.messages).toBe(array);
        expect(output.messages).toHaveLength(0);
        expect(transform.getState(sessionId).forceFullWire).toBe(false);
        expect(calls.at(-1)?.body).toMatchObject({ transform_pass_id: "applied" });
        expect(calls.at(-1)?.method).toBe("transform.ack");
    });

    for (const cancellation of ["supersede", "clear", "invalidate"] as const) {
        it(`propagates ${cancellation} to the transport signal and waits for rejection before release`, async () => {
            const sessionId = `rust-signal-${cancellation}`;
            installRawRows(sessionId, rawRows(1));
            const started = Promise.withResolvers<AbortSignal>();
            const admission = new TransformCaptureAdmission();
            const client: RustModeModuleClient = {
                call: ({ signal }) => {
                    if (!signal) throw new Error("missing capture signal");
                    started.resolve(signal);
                    return new Promise((_resolve, reject) => {
                        signal.addEventListener("abort", () => reject(signal.reason), {
                            once: true,
                        });
                    });
                },
            };
            const transform = createRustModeTransform(makeDeps(), {
                moduleClient: client,
                captureAdmission: admission,
            });
            const messages = makeMessages(sessionId);
            const output = { messages: [...messages] as unknown[] };
            const array = output.messages;
            const pass = transform.run(sessionId, output);
            const signal = await Promise.race([
                started.promise,
                pass.then(() => {
                    throw new Error("request never observed signal");
                }),
            ]);
            expect(signal.aborted).toBe(false);
            expect(admission.chargedBytes).toBeGreaterThan(0);
            let superseding: Promise<void> | undefined;
            if (cancellation === "clear") transform.clearSession(sessionId);
            else if (cancellation === "invalidate") transform.invalidateWireState(sessionId);
            else superseding = transform.run(sessionId, output);
            expect(signal.aborted).toBe(true);
            expect(admission.activePasses).toBe(1);
            expect(admission.chargedBytes).toBeGreaterThan(0);
            await superseding;
            await pass;
            expect(admission.activePasses).toBe(0);
            expect(admission.chargedBytes).toBe(0);
            expect(output.messages).toBe(array);
            expect(output.messages[0]).toBe(messages[0]);
            expect(transform.getState(sessionId).failureCount).toBe(0);
        });
    }

    it("charges every existing ordinal entry and ID before copying a warm memo", async () => {
        const sessionId = "rust-warm-memo-budget";
        const rows = rawRows(2);
        rows[1].id = "persisted-only-".repeat(400);
        installRawRows(sessionId, rows);
        const admission = new TransformCaptureAdmission({
            maxPasses: 64,
            maxBytes: 1024 * 1024,
        });
        const { client, bodies } = recordingClient((request) => recipeResponse(request, []));
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
            captureAdmission: admission,
        });
        const messages = makeMessages(sessionId);
        await transform.run(sessionId, { messages: [...messages] });
        expect(bodies).toHaveLength(1);
        const priorMemo = transform.getState(sessionId).ordinals;
        expect(priorMemo.entries.size).toBe(2);
        const inspection = inspectReferenceableMessages(messages);
        if (!inspection.ok) throw new Error("invalid budget fixture");
        const memoBytes = rows.reduce(
            (sum, row) => sum + ORDINAL_ENTRY_RETAINED_BYTES + row.id.length * 2,
            0,
        );
        const blocker = admission.admit("rust-warm-memo-budget-blocker");
        if (!("lease" in blocker)) throw new Error("blocker admission failed");
        const logSpy = spyOn(logger.sessionLog, "warn");
        try {
            expect(
                blocker.lease.reserve(
                    admission.remainingBytes - inspection.estimatedBytes - memoBytes + 1,
                ),
            ).toBe(true);
            const output = { messages: [...messages] };
            const array = output.messages;
            await transform.run(sessionId, output);
            expect(sessionLogs(logSpy, sessionId)).toContain(
                `rust session ${sessionId} pass declined: capture_bytes (transform capture byte budget exceeded: ordinal memo copy)`,
            );
            expect(bodies).toHaveLength(1);
            // The declined pass republishes the first pass's empty output; nothing was appended since.
            expect(output.messages).toBe(array);
            expect(output.messages).toHaveLength(0);
            expect(transform.getState(sessionId).ordinals).toEqual(priorMemo);
            expect(transform.getState(sessionId).failureCount).toBe(0);
            expect(admission.activePasses).toBe(1);
            expect(admission.chargedBytes).toBe(blocker.lease.chargedBytes);
        } finally {
            blocker.lease.release();
            logSpy.mockRestore();
        }
        expect(admission.chargedBytes).toBe(0);
        await transform.run(sessionId, { messages: [...messages] });
        expect(bodies).toHaveLength(2);
        expect(bodies[1]?.tail_delta).toBeDefined();
        expect(transform.getState(sessionId).ordinals).toEqual(priorMemo);
    });

    it("reuses validated input lengths without repeated measurement on a full-sync retry", async () => {
        const sessionId = "rust-full-retry-input-lengths";
        const rows = rawRows(4);
        installRawRows(sessionId, rows);
        const messages = rowMessages(sessionId, rows);
        const { client, bodies } = recordingClient((request, index) => {
            if (index === 1) return { status: "need_full_sync" };
            return {
                ...recipeResponse(request, []),
                operations: [
                    {
                        op: "keep",
                        source: "input",
                        start: 0,
                        count: (request.native_messages as unknown[]).length,
                    },
                ],
            };
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        await transform.run(sessionId, { messages: messages.slice(0, 3) });
        expect(bodies).toHaveLength(1);
        expect(transform.getState(sessionId).initialized).toBe(true);

        const lengthSpy = spyOn(editRecipe, "canonicalJsonLength");
        try {
            const output = { messages: [...messages] };
            await transform.run(sessionId, output);
            expect(bodies).toHaveLength(3);
            expect(bodies[1].tail_delta).toMatchObject({ native_replace_from: 2 });
            expect(bodies[1].native_messages).toEqual(messages.slice(2));
            expect(bodies[2].tail_delta).toBeUndefined();
            expect(bodies[2].native_messages).toEqual(messages);
            expect(bodies[2].base_revision).not.toBe(bodies[1].base_revision);
            expect(transform.getState(sessionId).failureCount).toBe(0);
            expect(transform.getState(sessionId).forceFullWire).toBe(false);
            expect(output.messages).toHaveLength(messages.length);
            for (const [index, message] of messages.entries()) {
                expect(output.messages[index]).toBe(message);
            }
            const measuredMessages = lengthSpy.mock.calls
                .map(([value]) => value)
                .filter((value) => messages.some((message) => message === value));
            expect(measuredMessages).toHaveLength(2);
            expect(measuredMessages).toEqual(messages.slice(2));
        } finally {
            lengthSpy.mockRestore();
        }
    });

    it("retains full-sync recovery after a failed retry until a full request publishes", async () => {
        const sessionId = `rust-failed-full-retry-${Date.now()}`;
        installRawRows(sessionId, rawRows(3));
        const { client, bodies } = recordingClient((request, index) => {
            if (index === 1) return { status: "need_full_sync" };
            if (index === 2) throw new Error("full request connection reset");
            return recipeResponse(request, request.native_messages);
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const warm = rowMessages(sessionId, rawRows(1));
        await transform.run(sessionId, { messages: [...warm] });
        const changed = rowMessages(sessionId, rawRows(2));
        const output = { messages: [...changed] as unknown[] };
        const array = output.messages;
        await transform.run(sessionId, output);
        // The failed pass serves the warm output followed by the appended message.
        expect(output.messages).toBe(array);
        expect(output.messages).toEqual(changed);
        expect(output.messages[1]).toBe(changed[1]);
        expect(bodies).toHaveLength(3);
        expect(transform.getState(sessionId).forceFullWire).toBe(true);
        const next = rowMessages(sessionId, rawRows(3));
        await transform.run(sessionId, { messages: [...next] });
        expect(bodies[3].tail_delta).toBeUndefined();
        expect(transform.getState(sessionId).forceFullWire).toBe(false);
        expect(transform.getState(sessionId).consecutiveFailures).toBe(0);
    });

    const trap = (): never => {
        throw new Error("user code ran during a transform guard");
    };

    for (const sharedArray of [true, false]) {
        for (const fault of ["mutation", "supersession", "clear", "invalidation"] as const) {
            it(`rejects ${fault} at the persisted ordinal yield with ${sharedArray ? "shared" : "distinct"} arrays`, async () => {
                const sessionId = `rust-scan-${fault}-${sharedArray}`;
                const rows = rawRows(MODULE_ORDINAL_PAGE_SIZE + 1);
                const pageRead = Promise.withResolvers<void>();
                const pageSizes: number[] = [];
                unregisters.push(
                    setRawMessageProvider(sessionId, {
                        readMessages: () => {
                            throw new Error("must use ordinal pages");
                        },
                        readMessageOrdinalPage: (after, limit) => {
                            const start = after?.timeCreated ?? 0;
                            const page = rows.slice(start, start + limit);
                            pageSizes.push(page.length);
                            if (start === 0) pageRead.resolve();
                            return page;
                        },
                        getStoredMessageCount: () => rows.length,
                    }),
                );
                const admission = new TransformCaptureAdmission();
                const { client, calls, bodies } = recordingClient((request) =>
                    recipeResponse(request, request.native_messages as unknown[]),
                );
                const transform = createRustModeTransform(makeDeps(), {
                    moduleClient: client,
                    captureAdmission: admission,
                });
                const messages = rowMessages(sessionId, rows.slice(-1));
                const member = messages[0];
                const output = { messages: sharedArray ? messages : [...messages] };
                const array = output.messages;
                const pass = transform.run(sessionId, output);
                await Promise.race([pageRead.promise, pass]);
                expect(pageSizes).toEqual([MODULE_ORDINAL_PAGE_SIZE]);
                expect(calls).toHaveLength(0);
                expect(transform.getState(sessionId).ordinals.entries.size).toBe(0);
                const heldBytes = admission.chargedBytes;
                expect(heldBytes).toBeGreaterThan(
                    MODULE_ORDINAL_PAGE_SIZE * ORDINAL_ENTRY_RETAINED_BYTES,
                );
                switch (fault) {
                    case "mutation":
                        (member.parts[0] as { text: string }).text = "edited at scan yield";
                        break;
                    case "supersession": {
                        const newerMessages = rowMessages(sessionId, rows.slice(-1));
                        const newerOutput = { messages: [...newerMessages] };
                        const newerArray = newerOutput.messages;
                        await transform.run(sessionId, newerOutput);
                        expect(newerOutput.messages).toBe(newerArray);
                        expect(newerOutput.messages[0]).toBe(newerMessages[0]);
                        break;
                    }
                    case "clear":
                        transform.clearSession(sessionId);
                        break;
                    case "invalidation":
                        transform.invalidateWireState(sessionId);
                        break;
                }
                expect(admission.activePasses).toBe(1);
                expect(admission.chargedBytes).toBe(heldBytes);
                await pass;
                expect(pageSizes).toEqual(
                    fault === "mutation"
                        ? [MODULE_ORDINAL_PAGE_SIZE, 1]
                        : [MODULE_ORDINAL_PAGE_SIZE],
                );
                expect(calls).toHaveLength(0);
                expect(output.messages).toBe(array);
                expect(output.messages).toHaveLength(1);
                expect(output.messages[0]).toBe(member);
                expect(transform.getState(sessionId).ordinals.entries.size).toBe(0);
                expect(transform.getState(sessionId).initialized).toBe(false);
                expect(transform.getState(sessionId).failureCount).toBe(0);
                expect(admission.activePasses).toBe(0);
                expect(admission.chargedBytes).toBe(0);
                const recoveryInput = rowMessages(sessionId, rows.slice(-1));
                await transform.run(sessionId, { messages: [...recoveryInput] });
                expect(calls).toHaveLength(1);
                expect(bodies[0]?.tail_delta).toBeUndefined();
                expect(transform.getState(sessionId).ordinals.entries.size).toBe(rows.length);
            });
        }

        for (const change of ["member", "append", "rebind", "metadata"] as const) {
            it(`preserves a host ${change} during transport with ${sharedArray ? "shared" : "distinct"} source`, async () => {
                const sessionId = `rust-host-${change}-${sharedArray}`;
                installRawRows(sessionId, rawRows(1));
                const started = Promise.withResolvers<void>();
                const response = Promise.withResolvers<unknown>();
                const { client, bodies, calls } = recordingClient(() => {
                    started.resolve();
                    return response.promise;
                });
                const transform = createRustModeTransform(makeDeps(), {
                    moduleClient: client,
                });
                const messages = makeMessages(sessionId);
                const original = messages[0];
                const output = { messages: sharedArray ? messages : [...messages] };
                const originalArray = output.messages;
                const pass = transform.run(sessionId, output);
                await Promise.race([started.promise, pass]);
                expect(calls).toHaveLength(1);
                const current = rowMessages(sessionId, rawRows(1), () => "host edit")[0];
                if (change === "member") output.messages[0] = current;
                else if (change === "append") output.messages.push(current);
                else if (change === "rebind") output.messages = [current];
                else Object.assign(output.messages, { bookkeeping: "host edit" });
                const currentArray = output.messages;
                response.resolve(
                    recipeForLast(bodies, [], {
                        note_deliveries: [{ transform_pass_id: "unapplied" }],
                    }),
                );
                await pass;
                expect(output.messages).toBe(currentArray);
                expect(output.messages).toHaveLength(change === "append" ? 2 : 1);
                expect(output.messages.at(-1)).toBe(change === "metadata" ? original : current);
                if (change === "metadata")
                    expect(output.messages).toHaveProperty("bookkeeping", "host edit");
                if (change === "append") expect(output.messages[0]).toBe(original);
                if (change === "rebind") expect(originalArray[0]).toBe(original);
                if (!sharedArray) expect(messages[0]).toBe(original);
                expect(calls.map((call) => call.method)).toEqual(["transform", "transform.nack"]);
                expect(transform.getState(sessionId).ordinals.entries.size).toBe(0);
                expect(transform.getState(sessionId).failureCount).toBe(0);
            });
        }
    }

    for (const window of ["source-await", "pre-apply"] as const) {
        for (const unsupported of ["accessor", "toJSON", "proxy"] as const) {
            it(`rejects nested ${unsupported} installed at ${window} without invoking it`, async () => {
                const sessionId = `rust-nested-${window}-${unsupported}`;
                installRawRows(sessionId, rawRows(1));
                const deps = makeDeps();
                const started = Promise.withResolvers<void>();
                const directory = Promise.withResolvers<{
                    data: { directory: string };
                }>();
                let directoryReached = false;
                if (window === "source-await") {
                    deps.sessionMetadataReadStateBySession = new Map();
                    deps.client = {
                        session: {
                            get: () => {
                                directoryReached = true;
                                started.resolve();
                                return directory.promise;
                            },
                        },
                    } as never;
                }
                const messages = makeMessages(sessionId);
                const output = { messages };
                const member = messages[0];
                const hook = mock(trap);
                // The hook replaces the existing `text` field, so the recheck walks into it rather than stopping at an added key.
                const install = () => {
                    const part = member.parts[0] as Record<string, unknown>;
                    if (unsupported === "proxy") {
                        part.text = new Proxy(
                            {},
                            {
                                get: hook,
                                ownKeys: hook,
                                getPrototypeOf: hook,
                                getOwnPropertyDescriptor: hook,
                            },
                        );
                    } else if (unsupported === "toJSON") {
                        Object.defineProperty(part, "toJSON", {
                            value: hook,
                            enumerable: true,
                        });
                    } else {
                        Object.defineProperty(part, "text", {
                            get: hook,
                            enumerable: true,
                        });
                    }
                };
                const methods: string[] = [];
                let transportProcessedBeforeInstall = false;
                const logSpy = spyOn(logger.sessionLog, "debug");
                const client: RustModeModuleClient = {
                    call: ({ method, body }) => {
                        methods.push(method);
                        if (method !== "transform") return Promise.resolve({ ok: true });
                        if (window === "pre-apply")
                            queueMicrotask(() =>
                                queueMicrotask(() => {
                                    transportProcessedBeforeInstall = sessionLogs(
                                        logSpy,
                                        sessionId,
                                    ).some((line) => line.includes("stage=rust.transport"));
                                    install();
                                }),
                            );
                        return Promise.resolve(
                            recipeResponse(body as Record<string, unknown>, [], {
                                note_deliveries: [{ transform_pass_id: "nested-unapplied" }],
                            }),
                        );
                    },
                };
                const transform = createRustModeTransform(deps, {
                    moduleClient: client,
                });
                try {
                    const pass = transform.run(sessionId, output);
                    if (window === "source-await") {
                        await Promise.race([started.promise, pass]);
                        expect(directoryReached).toBe(true);
                        expect(methods).toHaveLength(0);
                        install();
                        directory.resolve({ data: { directory: "/tmp/project" } });
                    }
                    await pass;
                    expect(hook).not.toHaveBeenCalled();
                    expect(output.messages).toBe(messages);
                    expect(output.messages).toHaveLength(1);
                    expect(output.messages[0]).toBe(member);
                    expect(methods).toEqual(
                        window === "source-await" ? [] : ["transform", "transform.nack"],
                    );
                    if (window === "pre-apply") expect(transportProcessedBeforeInstall).toBe(true);
                    expect(transform.getState(sessionId).ordinals.entries.size).toBe(0);
                    expect(transform.getState(sessionId).failureCount).toBe(0);
                } finally {
                    directory.resolve({ data: { directory: "/tmp/project" } });
                    logSpy.mockRestore();
                }
            });
        }
    }

    for (const fault of ["mutation", "supersession", "clear", "invalidation"] as const) {
        it(`does not dispatch a need_full_sync retry after ${fault} of the valid first send`, async () => {
            const sessionId = `rust-retry-${fault}`;
            const rows = rawRows(1);
            installRawRows(sessionId, rows);
            const started = Promise.withResolvers<void>();
            const response = Promise.withResolvers<unknown>();
            const { client, bodies, calls } = recordingClient((request, index) => {
                if (index === 0) return recipeResponse(request, request.native_messages);
                started.resolve();
                return response.promise;
            });
            const transform = createRustModeTransform(makeDeps(), {
                moduleClient: client,
            });
            const seed = makeMessages(sessionId);
            await transform.run(sessionId, { messages: [...seed] });
            const priorMemo = transform.getState(sessionId).ordinals;
            expect(priorMemo.entries.size).toBe(1);
            rows.push(rawRows(2)[1]);
            const messages = rowMessages(sessionId, rows);
            const output = { messages: [...messages] };
            const array = output.messages;
            const pass = transform.run(sessionId, output);
            await Promise.race([started.promise, pass]);
            // Enabling state: the tail-delta send is pending its response.
            expect(bodies).toHaveLength(2);
            expect(bodies[1]?.tail_delta).toBeDefined();
            switch (fault) {
                case "mutation":
                    (messages[1].parts[0] as { text: string }).text = "mutated before full retry";
                    break;
                case "supersession":
                    await transform.run(sessionId, { messages: [...messages] });
                    break;
                case "clear":
                    transform.clearSession(sessionId);
                    break;
                case "invalidation":
                    transform.invalidateWireState(sessionId);
                    break;
            }
            response.resolve({
                status: "need_full_sync",
                note_deliveries: [{ transform_pass_id: "retry-discarded" }],
            });
            await pass;
            expect(bodies).toHaveLength(2);
            expect(calls.map((call) => call.method)).toEqual([
                "transform",
                "transform",
                "transform.nack",
            ]);
            expect(calls.at(-1)?.body).toMatchObject({
                transform_pass_id: "retry-discarded",
            });
            expect(output.messages).toBe(array);
            expect(output.messages[0]).toBe(messages[0]);
            expect(output.messages[1]).toBe(messages[1]);
            const state = transform.getState(sessionId);
            if (fault === "clear" || fault === "invalidation") {
                expect(state.ordinals.entries.size).toBe(0);
            } else {
                expect(state.ordinals).toEqual(priorMemo);
            }
            // A dispatched pass marks the next send full before awaiting the daemon; only a cleared session starts from fresh state.
            expect(state.forceFullWire).toBe(fault !== "clear");
            expect(state.failureCount).toBe(0);
        });
    }

    it("declines before publication and NACKs known deliveries when the source changes between pages", async () => {
        const sessionId = "rust-page-mutation";
        installAvailabilityDb(sessionId, {});
        installRawRows(sessionId, rawRows(1));
        const messages = rowMessages(sessionId, rawRows(1), () => "x".repeat(600_000));
        const member = messages[0];
        const hook = mock(trap);
        const delivered: string[] = [];
        const { client, calls, bodies } = recordingClient((page) => {
            // Page bodies are frozen text, so the series completes; the accessor must stay unread until the recheck refuses publication.
            if (page.transform_page_index === 1)
                Object.defineProperty(member, "parts", { get: hook, enumerable: true });
            if (page.transform_page_complete !== true) return { staged: true };
            delivered.push("paged");
            return {
                ...recipeResponse(page, [{ role: "assistant", parts: [] }]),
                note_deliveries: [{ transform_pass_id: "paged" }],
            };
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const output = { messages: messages as unknown[] };
        await transform.run(sessionId, output);
        expect(bodies.length).toBeGreaterThan(1);
        expect(bodies.at(-1)?.transform_page_complete).toBe(true);
        expect(hook).not.toHaveBeenCalled();
        expect(output.messages).toBe(messages);
        expect(output.messages).toHaveLength(1);
        expect(output.messages[0]).toBe(member);
        expect(calls.some((call) => call.method === "transform.ack")).toBe(false);
        // Every delivery the daemon reported is NACKed.
        expect(
            calls
                .filter((call) => call.method === "transform.nack")
                .map((call) => (call.body as { transform_pass_id: string }).transform_pass_id),
        ).toEqual(delivered);
        expect(transform.getState(sessionId).ordinals.entries.size).toBe(0);
        expect(transform.getState(sessionId).failureCount).toBe(0);
    });

    it("declines an unsupported source before any dispatch and leaves the host array intact", async () => {
        const sessionId = `rust-unsupported-source-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const { client, calls } = recordingClient((request) => recipeResponse(request, []));
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const hooked = makeMessages(sessionId);
        const getter = mock(trap);
        Object.defineProperty(hooked[0], "parts", {
            get: getter,
            enumerable: true,
        });
        const output = { messages: [...hooked] as unknown[] };
        const array = output.messages;
        await transform.run(sessionId, output);
        expect(getter).not.toHaveBeenCalled();
        expect(calls).toHaveLength(0);
        expect(output.messages).toBe(array);
        expect(output.messages[0]).toBe(hooked[0]);
        expect(transform.getState(sessionId).failureCount).toBe(0);
        expect(transform.getState(sessionId).consecutiveFailures).toBe(0);
    });

    it("declines a proxied or non-replaceable host container without dispatch", async () => {
        const sessionId = `rust-host-container-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const { client, calls } = recordingClient((request) => recipeResponse(request, []));
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const messages = makeMessages(sessionId);
        const frozen = Object.freeze([...messages]) as unknown[];
        await transform.run(sessionId, { messages: frozen });
        const proxied = new Proxy([...messages] as unknown[], {});
        await transform.run(sessionId, { messages: proxied });
        expect(calls).toHaveLength(0);
        expect(transform.getState(sessionId).failureCount).toBe(0);
    });

    it("permits no further dispatch or publication when an accessor is installed during preflight", async () => {
        const sessionId = `rust-hook-during-await-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const started = Promise.withResolvers<void>();
        const directory = Promise.withResolvers<{ data: { directory: string } }>();
        const deps = makeDeps();
        deps.client = {
            session: {
                get: () => {
                    started.resolve();
                    return directory.promise;
                },
            },
        } as never;
        deps.sessionMetadataReadStateBySession = new Map();
        const { client, calls } = recordingClient((request) => recipeResponse(request, []));
        const transform = createRustModeTransform(deps, { moduleClient: client });
        const messages = makeMessages(sessionId);
        const output = { messages: [...messages] as unknown[] };
        const array = output.messages;
        const getter = mock(trap);
        const pass = transform.run(sessionId, output);
        await Promise.race([
            started.promise,
            pass.then(() => {
                throw new Error("preflight not reached");
            }),
        ]);
        Object.defineProperty(messages[0], "info", {
            get: getter,
            enumerable: true,
        });
        directory.resolve({ data: { directory: "/tmp/project" } });
        await pass;
        expect(getter).not.toHaveBeenCalled();
        expect(calls).toHaveLength(0);
        expect(output.messages).toBe(array);
        expect(output.messages[0]).toBe(messages[0]);
        expect(transform.getState(sessionId).failureCount).toBe(0);
    });

    it("rejects publication when a message is edited in place while the transform response is pending", async () => {
        const sessionId = `rust-mutation-in-transport-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const started = Promise.withResolvers<void>();
        const response = Promise.withResolvers<unknown>();
        const { client, bodies, calls } = recordingClient(() => {
            started.resolve();
            return response.promise;
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const messages = makeMessages(sessionId);
        const output = { messages: [...messages] as unknown[] };
        const array = output.messages;
        const member = messages[0];
        const pass = transform.run(sessionId, output);
        await Promise.race([
            started.promise,
            pass.then(() => {
                throw new Error("transport not reached");
            }),
        ]);
        expect(calls).toHaveLength(1);
        (messages[0].parts[0] as { text: string }).text = "edited during transport";
        response.resolve(
            recipeForLast(bodies, [{ info: { id: "stale" }, parts: [] }], {
                note_deliveries: [{ transform_pass_id: "pass-stale" }],
            }),
        );
        await pass;
        expect(output.messages).toBe(array);
        expect(output.messages).toHaveLength(1);
        expect(output.messages[0]).toBe(member);
        expect(member.parts[0]).toMatchObject({ text: "edited during transport" });
        expect(calls.map((call) => call.method)).toEqual(["transform", "transform.nack"]);
        expect(transform.getState(sessionId).failureCount).toBe(0);
        expect(transform.getState(sessionId).ordinals.entries.size).toBe(0);
    });

    it("rejects publication and promotes no memo when the wire state is invalidated mid-flight", async () => {
        const sessionId = `rust-invalidate-mid-flight-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const started = Promise.withResolvers<void>();
        const response = Promise.withResolvers<unknown>();
        const { client, bodies, calls } = recordingClient(() => {
            started.resolve();
            return response.promise;
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const messages = makeMessages(sessionId);
        const output = { messages: [...messages] as unknown[] };
        const array = output.messages;
        const member = messages[0];
        const pass = transform.run(sessionId, output);
        await Promise.race([
            started.promise,
            pass.then(() => {
                throw new Error("transport not reached");
            }),
        ]);
        expect(calls).toHaveLength(1);
        transform.invalidateWireState(sessionId);
        response.resolve(
            recipeForLast(bodies, [{ info: { id: "stale" }, parts: [] }], {
                note_deliveries: [{ transform_pass_id: "pass-stale" }],
            }),
        );
        await pass;
        expect(output.messages).toBe(array);
        expect(output.messages).toHaveLength(1);
        expect(output.messages[0]).toBe(member);
        expect(calls.map((call) => call.method)).toEqual(["transform", "transform.nack"]);
        expect(transform.getState(sessionId).ordinals.entries.size).toBe(0);
        expect(transform.getState(sessionId).failureCount).toBe(0);
    });

    it("stops an in-flight pass when the session is cleared and keeps the host array intact", async () => {
        const sessionId = `rust-clear-mid-flight-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const started = Promise.withResolvers<void>();
        const response = Promise.withResolvers<unknown>();
        const { client, bodies, calls } = recordingClient(() => {
            started.resolve();
            return response.promise;
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const messages = makeMessages(sessionId);
        const output = { messages: [...messages] as unknown[] };
        const array = output.messages;
        const member = messages[0];
        const pass = transform.run(sessionId, output);
        await Promise.race([
            started.promise,
            pass.then(() => {
                throw new Error("transport not reached");
            }),
        ]);
        expect(calls).toHaveLength(1);
        transform.clearSession(sessionId);
        response.resolve(recipeForLast(bodies, [{ info: { id: "stale" }, parts: [] }]));
        await pass;
        expect(output.messages).toBe(array);
        expect(output.messages).toHaveLength(1);
        expect(output.messages[0]).toBe(member);
        expect(calls.map((call) => call.method)).toEqual(["transform"]);
    });

    it("keeps every pass within the global count limit and declines without queueing", async () => {
        const admission = new TransformCaptureAdmission({
            maxPasses: 2,
            maxBytes: 64 * 1024 * 1024,
        });
        const started = Promise.withResolvers<void>();
        const release = Promise.withResolvers<void>();
        const { client, calls } = recordingClient(async (request, index) => {
            if (index === 1) started.resolve();
            await release.promise;
            return recipeResponse(request, request.native_messages);
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
            captureAdmission: admission,
        });
        const sessions = ["a", "b", "c"].map((name) => `rust-count-pressure-${name}-${Date.now()}`);
        for (const sessionId of sessions) installRawRows(sessionId, rawRows(1));
        const outputs = sessions.map((sessionId) => ({
            messages: [...makeMessages(sessionId)] as unknown[],
        }));
        const declinedArray = outputs[2].messages;
        const declinedMember = declinedArray[0];
        const logSpy = spyOn(logger.sessionLog, "warn");
        const passes = sessions.map((sessionId, index) => transform.run(sessionId, outputs[index]));
        try {
            await Promise.race([started.promise, Promise.all(passes)]);
            await passes[2];
            expect(admission.activePasses).toBe(2);
            expect(calls).toHaveLength(2);
            expect(sessionLogs(logSpy, sessions[2])).toContain(
                "rust transform declined before dispatch: pass_count",
            );
            expect(outputs[2].messages).toBe(declinedArray);
            expect(outputs[2].messages[0]).toBe(declinedMember);
            release.resolve();
            await Promise.all(passes);
            expect(admission.activePasses).toBe(0);
            expect(admission.chargedBytes).toBe(0);
            expect(transform.getState(sessions[2]).failureCount).toBe(0);
            await transform.run(sessions[2], outputs[2]);
            expect(calls).toHaveLength(3);
        } finally {
            release.resolve();
            await Promise.all(passes);
            logSpy.mockRestore();
        }
    });

    it("declines on byte pressure alone while the aggregate charge stays within the budget", async () => {
        const large = `rust-byte-pressure-large-${Date.now()}`;
        const small = `rust-byte-pressure-small-${Date.now()}`;
        const rows = rawRows(1);
        installRawRows(large, rows);
        installRawRows(small, rows);
        const largeMessages = rowMessages(large, rows, () => "x".repeat(4_000));
        const smallMessages = rowMessages(small, rows, () => "y".repeat(2_000));
        const largeInspection = inspectReferenceableMessages(largeMessages);
        const smallInspection = inspectReferenceableMessages(smallMessages);
        if (!largeInspection.ok || !smallInspection.ok) throw new Error("invalid budget fixture");
        // Capture walk, wire projection, the two memo copies, and one retained length slot per message.
        const heldCharge =
            largeInspection.estimatedBytes +
            largeInspection.messageWireBytes.reduce((sum, bytes) => sum + 4 * bytes, 0) +
            largeMessages.length * ORDINAL_ENTRY_RETAINED_BYTES * 2 +
            rows.reduce(
                (sum, row) => sum + 2 * ORDINAL_ENTRY_RETAINED_BYTES + row.id.length * 2,
                0,
            ) +
            largeMessages.length * 8;
        const maxBytes = heldCharge + smallInspection.estimatedBytes - 1;
        const admission = new TransformCaptureAdmission({
            maxPasses: 64,
            maxBytes,
        });
        const started = Promise.withResolvers<void>();
        const release = Promise.withResolvers<void>();
        const { client, calls } = recordingClient(async (request, index) => {
            if (index === 0) {
                started.resolve();
                await release.promise;
            }
            return recipeResponse(request, request.native_messages);
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
            captureAdmission: admission,
        });
        const largeOutput = { messages: [...largeMessages] as unknown[] };
        const logSpy = spyOn(logger.sessionLog, "debug");
        const first = transform.run(large, largeOutput);
        try {
            await Promise.race([started.promise, first]);
            expect(calls).toHaveLength(1);
            expect(admission.activePasses).toBe(1);
            expect(admission.chargedBytes).toBe(heldCharge);
            expect(admission.chargedBytes).toBeLessThanOrEqual(maxBytes);
            expect(smallInspection.estimatedBytes).toBeGreaterThan(admission.remainingBytes);
            const smallOutput = { messages: [...smallMessages] as unknown[] };
            const array = smallOutput.messages;
            await transform.run(small, smallOutput);
            expect(calls).toHaveLength(1);
            expect(
                sessionLogs(logSpy, small).some((line) => line.includes("declined:capture_bytes")),
            ).toBe(true);
            expect(smallOutput.messages).toBe(array);
            expect(smallOutput.messages[0]).toBe(smallMessages[0]);
            expect(admission.activePasses).toBe(1);
            expect(admission.chargedBytes).toBe(heldCharge);
            expect(transform.getState(small).failureCount).toBe(0);
            release.resolve();
            await first;
            expect(admission.chargedBytes).toBe(0);
            await transform.run(small, smallOutput);
            expect(calls).toHaveLength(2);
            expect(transform.getState(small).initialized).toBe(true);
        } finally {
            release.resolve();
            await first;
            logSpy.mockRestore();
        }
    });

    it("holds the charge through a slow cancellation until the cancelled owner settles", async () => {
        const admission = new TransformCaptureAdmission({
            maxPasses: 4,
            maxBytes: 64 * 1024 * 1024,
        });
        const sessionId = `rust-slow-cancel-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const started = Promise.withResolvers<void>();
        const response = Promise.withResolvers<unknown>();
        const { client, bodies, calls } = recordingClient(() => {
            started.resolve();
            return response.promise;
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
            captureAdmission: admission,
        });
        const messages = makeMessages(sessionId);
        const first = transform.run(sessionId, { messages: [...messages] });
        await Promise.race([started.promise, first]);
        expect(calls).toHaveLength(1);
        const chargedWhileHeld = admission.chargedBytes;
        expect(chargedWhileHeld).toBeGreaterThan(0);
        // The newer call declines and requests cancellation; the charge stays until the owner settles.
        await transform.run(sessionId, { messages: [...messages] });
        expect(admission.activePasses).toBe(1);
        expect(admission.chargedBytes).toBe(chargedWhileHeld);
        response.resolve(recipeForLast(bodies, []));
        await first;
        expect(admission.activePasses).toBe(0);
        expect(admission.chargedBytes).toBe(0);
    });

    it("releases capture admission before the ACK so a paused ACK does not block the next pass", async () => {
        const admission = new TransformCaptureAdmission({
            maxPasses: 1,
            maxBytes: 64 * 1024 * 1024,
        });
        const sessionId = `rust-ack-release-${Date.now()}`;
        installRawRows(sessionId, rawRows(2));
        const ackStarted = Promise.withResolvers<void>();
        const releaseAck = Promise.withResolvers<void>();
        const applied = [{ info: { id: "applied" }, parts: [] }];
        const { client, calls } = recordingClient(
            (request, index) =>
                index === 0
                    ? {
                          ...recipeResponse(request, applied),
                          note_deliveries: [{ transform_pass_id: "pass-first" }],
                      }
                    : recipeResponse(request, request.native_messages as unknown[]),
            async (method) => {
                if (method === "transform.ack") {
                    ackStarted.resolve();
                    await releaseAck.promise;
                    throw new Error("ack transport failed");
                }
                return { ok: true };
            },
        );
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
            captureAdmission: admission,
        });
        const firstInput = rowMessages(sessionId, rawRows(1));
        const firstOutput = { messages: [...firstInput] as unknown[] };
        const firstArray = firstOutput.messages;
        const first = transform.run(sessionId, firstOutput);
        await Promise.race([ackStarted.promise, first]);
        // Publication happened and the slot is free while the ACK is still pending.
        expect(firstOutput.messages).toBe(firstArray);
        expect(firstOutput.messages[0]).toBe(applied[0]);
        expect(admission.activePasses).toBe(0);
        expect(admission.chargedBytes).toBe(0);
        expect(admission.remainingBytes).toBe(64 * 1024 * 1024);
        expect(transform.getState(sessionId).initialized).toBe(true);
        expect(transform.getState(sessionId).ordinals.entries.get("m-1")).toBe(1);
        expect(calls.map((call) => call.method)).toEqual(["transform", "transform.ack"]);
        expect(calls[1]?.body).toMatchObject({ transform_pass_id: "pass-first" });
        const secondInput = rowMessages(sessionId, rawRows(2));
        const secondOutput = { messages: [...secondInput] as unknown[] };
        await transform.run(sessionId, secondOutput);
        expect(calls.filter((call) => call.method === "transform")).toHaveLength(2);
        const getter = mock(trap);
        Object.defineProperty(firstInput[0], "parts", {
            get: getter,
            enumerable: true,
        });
        releaseAck.resolve();
        await first;
        // ACK failure never rolls back the published output.
        expect(firstOutput.messages[0]).toBe(applied[0]);
        expect(firstOutput.messages).toBe(firstArray);
        expect(getter).not.toHaveBeenCalled();
        expect(transform.getState(sessionId).failureCount).toBe(0);
    });

    it("releases rejected capture state before a paused NACK and keeps delivery IDs separate", async () => {
        const admission = new TransformCaptureAdmission({
            maxPasses: 1,
            maxBytes: 64 * 1024 * 1024,
        });
        const sessionId = "rust-nack-release";
        installRawRows(sessionId, rawRows(1));
        const started = Promise.withResolvers<void>();
        const response = Promise.withResolvers<unknown>();
        const nackStarted = Promise.withResolvers<void>();
        const releaseNack = Promise.withResolvers<void>();
        const { client, bodies, calls } = recordingClient(
            (request, index) => {
                if (index === 0) {
                    started.resolve();
                    return response.promise;
                }
                return {
                    ...recipeResponse(request, request.native_messages as unknown[]),
                    note_deliveries: [{ transform_pass_id: "accepted" }],
                };
            },
            async (method) => {
                if (method === "transform.nack") {
                    nackStarted.resolve();
                    await releaseNack.promise;
                }
                return { ok: true };
            },
        );
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
            captureAdmission: admission,
        });
        const messages = makeMessages(sessionId);
        const output = { messages: [...messages] };
        const array = output.messages;
        const member = messages[0];
        const first = transform.run(sessionId, output);
        try {
            await Promise.race([started.promise, first]);
            expect(calls).toHaveLength(1);
            (member.parts[0] as { text: string }).text = "host edit";
            response.resolve(
                recipeForLast(bodies, [], {
                    note_deliveries: [{ transform_pass_id: "discarded" }],
                }),
            );
            await Promise.race([nackStarted.promise, first]);
            expect(calls.map((call) => call.method)).toEqual(["transform", "transform.nack"]);
            expect(admission.activePasses).toBe(0);
            expect(admission.chargedBytes).toBe(0);
            expect(admission.remainingBytes).toBe(64 * 1024 * 1024);
            expect(transform.getState(sessionId).ordinals.entries.size).toBe(0);
            expect(transform.getState(sessionId).initialized).toBe(false);
            const next = makeMessages(sessionId);
            await transform.run(sessionId, { messages: [...next] });
            expect(calls.filter((call) => call.method === "transform")).toHaveLength(2);
            expect(
                calls
                    .filter((call) => call.method !== "transform")
                    .map((call) => [
                        call.method,
                        (call.body as { transform_pass_id: string }).transform_pass_id,
                    ]),
            ).toEqual([
                ["transform.nack", "discarded"],
                ["transform.ack", "accepted"],
            ]);
            const getter = mock(trap);
            Object.defineProperty(member, "parts", { get: getter, enumerable: true });
            releaseNack.resolve();
            await first;
            expect(getter).not.toHaveBeenCalled();
            expect(output.messages).toBe(array);
            expect(output.messages).toHaveLength(1);
            expect(output.messages[0]).toBe(member);
            expect(transform.getState(sessionId).failureCount).toBe(0);
        } finally {
            response.resolve(recipeForLast(bodies, []));
            releaseNack.resolve();
            await first;
        }
    });

    it("shares default admission across factories and cancels the earlier session owner", async () => {
        const sessionId = "rust-cross-factory-owner";
        installRawRows(sessionId, rawRows(1));
        const started = Promise.withResolvers<void>();
        const response = Promise.withResolvers<unknown>();
        const { client, bodies, calls } = recordingClient(() => {
            started.resolve();
            return response.promise;
        });
        const other = recordingClient((request) =>
            recipeResponse(request, request.native_messages),
        );
        const firstFactory = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const secondFactory = createRustModeTransform(makeDeps(), {
            moduleClient: other.client,
        });
        const input = makeMessages(sessionId);
        const firstOutput = { messages: [...input] };
        const firstArray = firstOutput.messages;
        const logSpy = spyOn(logger.sessionLog, "debug");
        const first = firstFactory.run(sessionId, firstOutput);
        try {
            await Promise.race([started.promise, first]);
            expect(calls).toHaveLength(1);
            const heldBytes = defaultTransformCaptureAdmission.chargedBytes;
            expect(heldBytes).toBeGreaterThan(0);
            const secondInput = makeMessages(sessionId);
            const secondOutput = { messages: secondInput };
            await secondFactory.run(sessionId, secondOutput);
            expect(sessionLogs(logSpy, sessionId)).toContain(
                "rust transform declined before dispatch: session_busy",
            );
            expect(other.calls).toHaveLength(0);
            expect(secondOutput.messages).toBe(secondInput);
            expect(secondOutput.messages[0]).toBe(secondInput[0]);
            expect(defaultTransformCaptureAdmission.activePasses).toBe(1);
            expect(defaultTransformCaptureAdmission.chargedBytes).toBe(heldBytes);
            response.resolve(
                recipeForLast(bodies, [], {
                    note_deliveries: [{ transform_pass_id: "old-factory" }],
                }),
            );
            await first;
            expect(calls.map((call) => call.method)).toEqual(["transform", "transform.nack"]);
            expect(firstOutput.messages).toBe(firstArray);
            expect(firstOutput.messages[0]).toBe(input[0]);
            expect(firstFactory.getState(sessionId).ordinals.entries.size).toBe(0);
            expect(defaultTransformCaptureAdmission.activePasses).toBe(0);
            expect(defaultTransformCaptureAdmission.chargedBytes).toBe(0);
            await secondFactory.run(sessionId, secondOutput);
            expect(other.calls).toHaveLength(1);
            expect(secondFactory.getState(sessionId).initialized).toBe(true);
        } finally {
            response.resolve(recipeForLast(bodies, []));
            await first;
            logSpy.mockRestore();
        }
    });

    it("preserves the payload identity of kept and returned messages on publication", async () => {
        const sessionId = `rust-payload-identity-${Date.now()}`;
        installRawRows(sessionId, rawRows(2));
        const returned = [
            { info: { id: "kept" }, parts: [] },
            { info: { id: "new" }, parts: [] },
        ];
        const { client } = recordingClient((request, index) =>
            index === 0
                ? recipeResponse(request, [returned[0]])
                : {
                      base_revision: request.base_revision,
                      output_revision: "out-identity",
                      previous_output_revision: request.previous_output_revision,
                      operations: [
                          { op: "keep", source: "previous", start: 0, count: 1 },
                          { op: "insert", values: [returned[1]] },
                      ],
                  },
        );
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const firstInput = rowMessages(sessionId, rawRows(1));
        await transform.run(sessionId, { messages: [...firstInput] });
        const secondInput = rowMessages(sessionId, rawRows(2));
        const secondOutput = { messages: [...secondInput] as unknown[] };
        const array = secondOutput.messages;
        await transform.run(sessionId, secondOutput);
        expect(secondOutput.messages).toBe(array);
        expect(secondOutput.messages[0]).toBe(returned[0]);
        expect(secondOutput.messages[1]).toBe(returned[1]);
        expect(transform.getState(sessionId).ordinals.entries.size).toBe(2);
    });

    it("does not resend after an outcome-unknown transport failure and recovers on the next attempt", async () => {
        const sessionId = `rust-unknown-send-${Date.now()}`;
        installRawRows(sessionId, rawRows(1));
        const { client, calls } = recordingClient((request, index) => {
            if (index === 0) throw new Error("connection reset after write");
            return recipeResponse(request, request.native_messages);
        });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
        });
        const messages = makeMessages(sessionId);
        const output = { messages: [...messages] as unknown[] };
        const array = output.messages;
        await transform.run(sessionId, output);
        expect(calls).toHaveLength(1);
        expect(output.messages).toBe(array);
        expect(output.messages[0]).toBe(messages[0]);
        expect(transform.getState(sessionId).failureCount).toBe(1);
        await transform.run(sessionId, output);
        expect(calls).toHaveLength(2);
        expect(transform.getState(sessionId).consecutiveFailures).toBe(0);
    });
});

describe("fail-open after an applied pass", () => {
    const folded = (sessionId: string): MessageLike => ({
        info: { id: "fold-1", role: "user", sessionID: sessionId },
        parts: [{ type: "text", text: "folded history" }],
    });

    /** The first transform call applies a fold of the whole input; every later call fails. */
    function failAfterFirst(sessionId: string) {
        return recordingClient((request, index) => {
            if (index > 0) throw new Error("request deadline expired after a possible send");
            return recipeResponse(request, [folded(sessionId)]);
        });
    }

    function toolMessage(sessionId: string, id: string, completed: boolean): MessageLike {
        return {
            info: { id, role: "assistant", sessionID: sessionId },
            parts: [
                {
                    type: "tool",
                    callID: `call-${id}`,
                    tool: "read",
                    state: completed
                        ? { status: "completed", input: { path: "a" }, output: "contents" }
                        : { status: "running", input: { path: "a" } },
                },
            ],
        };
    }

    it("serves the last applied output plus the messages appended since", async () => {
        const sessionId = `rust-fail-open-append-${Date.now()}`;
        const rows = rawRows(6);
        installRawRows(sessionId, rows);
        const { client, bodies } = failAfterFirst(sessionId);
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const debugSpy = spyOn(logger.sessionLog, "debug");
        try {
            await transform.run(sessionId, { messages: rowMessages(sessionId, rows.slice(0, 3)) });

            const grown = rowMessages(sessionId, rows.slice(0, 5));
            const output = { messages: [...grown] as unknown[] };
            await transform.run(sessionId, output);
            expect(bodies).toHaveLength(2);
            expect(output.messages).toEqual([folded(sessionId), grown[3], grown[4]]);
            expect(output.messages[1]).toBe(grown[3]);
            expect(transform.getState(sessionId).failureCount).toBe(1);

            // A second consecutive failure still reuses the same applied output.
            const again = rowMessages(sessionId, rows.slice(0, 6));
            const againOutput = { messages: [...again] as unknown[] };
            await transform.run(sessionId, againOutput);
            expect(againOutput.messages).toEqual([folded(sessionId), ...again.slice(3)]);

            const passLines = sessionLogs(debugSpy, sessionId).filter((line) =>
                line.startsWith("rust pass:"),
            );
            expect(passLines[1]).toContain("served_from=last_applied in=5 out=3");
            expect(passLines[2]).toContain("served_from=last_applied in=6 out=4");
        } finally {
            debugSpy.mockRestore();
        }
    });

    it("serves the input unchanged after an in-place edit of an acknowledged message", async () => {
        const sessionId = `rust-fail-open-edit-${Date.now()}`;
        const rows = rawRows(4);
        installRawRows(sessionId, rows);
        const { client } = failAfterFirst(sessionId);
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const debugSpy = spyOn(logger.sessionLog, "debug");
        try {
            await transform.run(sessionId, { messages: rowMessages(sessionId, rows.slice(0, 3)) });

            const edited = rowMessages(sessionId, rows, (row) =>
                row.id === "m-1" ? "EDITED m-1" : `message ${row.id}`,
            );
            const output = { messages: [...edited] as unknown[] };
            await transform.run(sessionId, output);
            expect(output.messages).toEqual(edited);

            const removed = rowMessages(sessionId, [rows[0], rows[2], rows[3]] as RawRow[]);
            const removedOutput = { messages: [...removed] as unknown[] };
            await transform.run(sessionId, removedOutput);
            expect(removedOutput.messages).toEqual(removed);

            const passLines = sessionLogs(debugSpy, sessionId).filter((line) =>
                line.startsWith("rust pass:"),
            );
            expect(passLines[1]).toContain("served_from=raw in=4 out=4");
        } finally {
            debugSpy.mockRestore();
        }
    });

    it("serves the input unchanged when no output was applied", async () => {
        const sessionId = `rust-fail-open-none-${Date.now()}`;
        const rows = rawRows(2);
        installRawRows(sessionId, rows);
        const { client } = recordingClient(() => {
            throw new Error("module transport deadline expired while queued");
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const input = rowMessages(sessionId, rows);
        const output = { messages: [...input] as unknown[] };
        await transform.run(sessionId, output);
        expect(output.messages).toEqual(input);
        expect(transform.getState(sessionId).failureCount).toBe(1);
    });

    it("keeps a tool call and its result together at the boundary", async () => {
        const sessionId = `rust-fail-open-tool-${Date.now()}`;
        const rows = rawRows(4);
        installRawRows(sessionId, rows);
        const { client } = failAfterFirst(sessionId);
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const [user] = rowMessages(sessionId, rows.slice(0, 1));
        await transform.run(sessionId, {
            messages: [user, toolMessage(sessionId, "m-2", true)],
        });

        const appended = [
            toolMessage(sessionId, "m-3", true),
            ...rowMessages(sessionId, [rows[3] as RawRow]),
        ];
        const output = {
            messages: [user, toolMessage(sessionId, "m-2", true), ...appended] as unknown[],
        };
        await transform.run(sessionId, output);
        expect(output.messages).toEqual([folded(sessionId), ...appended]);
        expect((output.messages[1] as MessageLike).parts).toEqual(appended[0]?.parts);

        // A terminal tool call completed in place since the applied pass is not append-only.
        const pendingSession = `${sessionId}-pending`;
        installRawRows(pendingSession, rows);
        const pending = failAfterFirst(pendingSession);
        const pendingTransform = createRustModeTransform(makeDeps(), {
            moduleClient: pending.client,
        });
        const [pendingUser] = rowMessages(pendingSession, rows.slice(0, 1));
        await pendingTransform.run(pendingSession, {
            messages: [pendingUser, toolMessage(pendingSession, "m-2", false)],
        });
        const completed = [
            pendingUser,
            toolMessage(pendingSession, "m-2", true),
            ...rowMessages(pendingSession, [rows[2] as RawRow]),
        ];
        const completedOutput = { messages: [...completed] as unknown[] };
        await pendingTransform.run(pendingSession, completedOutput);
        expect(completedOutput.messages).toEqual(completed);
    });
});

describe("capture scaled to the appended messages", () => {
    const folded = (sessionId: string): MessageLike => ({
        info: { id: "fold-1", role: "user", sessionID: sessionId },
        parts: [{ type: "text", text: "folded history" }],
    });

    /** An admission that records each pass's lease charge at release. */
    function chargeRecordingAdmission(limits?: { maxPasses: number; maxBytes: number }) {
        const admission = new TransformCaptureAdmission(limits);
        const charges: number[] = [];
        const admit = admission.admit.bind(admission);
        admission.admit = (sessionId) => {
            const admitted = admit(sessionId);
            if ("lease" in admitted) {
                const lease = admitted.lease;
                const release = lease.release.bind(lease);
                lease.release = () => {
                    charges.push(lease.chargedBytes);
                    release();
                };
            }
            return admitted;
        };
        return { admission, charges };
    }

    function passLines(spy: Parameters<typeof sessionLogs>[0], sessionId: string): string[] {
        return sessionLogs(spy, sessionId).filter((line) => line.startsWith("rust pass:"));
    }

    it("keeps capturing a history past the 64 MiB budget at a per-pass charge set by the append", async () => {
        const sessionId = `rust-delta-capture-large-${Date.now()}`;
        const rows = rawRows(3_000);
        installRawRows(sessionId, rows);
        const texts = new Map(rows.map((row) => [row.id, `${row.id} ${"x".repeat(24_000)}`]));
        const { client, bodies } = recordingClient((request) =>
            recipeResponse(request, [folded(sessionId)]),
        );
        const { admission, charges } = chargeRecordingAdmission();
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
            captureAdmission: admission,
        });
        const debugSpy = spyOn(logger.sessionLog, "debug");
        let history: MessageLike[] = [];
        try {
            // Each pass gets fresh message objects, as a host that clones its history does.
            for (let count = 200; count <= 3_000; count += 200) {
                history = rowMessages(sessionId, rows.slice(0, count), (row) =>
                    String(texts.get(row.id)),
                );
                const output = { messages: [...history] as unknown[] };
                await transform.run(sessionId, output);
                expect(output.messages).toEqual([folded(sessionId)]);
            }
            const appended = rowMessages(sessionId, rows.slice(0, 3_000), (row) =>
                String(texts.get(row.id)),
            );
            appended.push(...rowMessages(sessionId, [{ ...rows[0], id: "m-3001" } as RawRow]));
            await transform.run(sessionId, { messages: appended });
            const lines = passLines(debugSpy, sessionId);
            expect(lines).toHaveLength(16);
            for (const line of lines) expect(line).toContain("applied=true");
            // After the first pass only the former terminal and the appended messages are sent.
            expect(lines[0]).toContain("wire_messages:200 ");
            for (const line of lines.slice(1, 15)) expect(line).toContain("wire_messages:201 ");
            expect(lines[15]).toContain("in=3001 out=1");
            expect(lines[15]).toContain("wire_messages:2 ");
        } finally {
            debugSpy.mockRestore();
        }
        expect(Buffer.byteLength(JSON.stringify(history))).toBeGreaterThan(64 * 1024 * 1024);
        expect(bodies.at(-1)?.native_messages).toHaveLength(2);
        // The same 200-message append costs the same at 400 and at 3,000 messages.
        expect(charges).toHaveLength(16);
        const [, atFourHundred] = charges as [number, number];
        const atThreeThousand = charges[14] as number;
        expect(atThreeThousand).toBeLessThan(atFourHundred * 1.1);
        expect(charges.at(-1)).toBeLessThan(2 * 1024 * 1024);
        expect(admission.chargedBytes).toBe(0);
    }, 60_000);

    it("detects an in-place edit of an old message and falls back to a full capture", async () => {
        const sessionId = `rust-delta-capture-edit-${Date.now()}`;
        const rows = rawRows(5);
        installRawRows(sessionId, rows);
        const { client, bodies } = recordingClient((request) =>
            recipeResponse(request, [folded(sessionId)]),
        );
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const history = rowMessages(sessionId, rows.slice(0, 4));
        await transform.run(sessionId, { messages: [...history] });
        await transform.run(sessionId, { messages: [...history] });
        expect(bodies[1]?.tail_delta).toBeDefined();

        // The same object, edited in place, must not verify against its retained digest.
        (history[1]?.parts as Array<{ text: string }>)[0].text = "EDITED m-2";
        const grown = [...history, ...rowMessages(sessionId, rows.slice(4))];
        await transform.run(sessionId, { messages: [...grown] });
        expect(bodies).toHaveLength(3);
        expect(bodies[2]?.tail_delta).toBeUndefined();
        expect(bodies[2]?.native_messages).toEqual(grown);
        await transform.run(sessionId, { messages: [...grown] });
        expect(bodies[3]?.tail_delta).toBeDefined();
    });

    it("rejects a verified prefix message edited while the pass awaits the daemon", async () => {
        const sessionId = `rust-delta-capture-mid-pass-${Date.now()}`;
        const rows = rawRows(4);
        installRawRows(sessionId, rows);
        const history = rowMessages(sessionId, rows);
        const { client, bodies } = recordingClient((request, index) => {
            if (index === 1) (history[0]?.parts as Array<{ text: string }>)[0].text = "EDITED";
            return recipeResponse(request, [folded(sessionId)]);
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const debugSpy = spyOn(logger.sessionLog, "debug");
        try {
            await transform.run(sessionId, { messages: [...history] });
            const output = { messages: [...history] as unknown[] };
            await transform.run(sessionId, output);
            expect(bodies[1]?.tail_delta).toBeDefined();
            expect(output.messages).toEqual(history);
            expect(sessionLogs(debugSpy, sessionId)).toContain(
                `rust session ${sessionId} pass declined: source_changed (publish)`,
            );
        } finally {
            debugSpy.mockRestore();
        }
    });

    it("still rejects a hostile message appended after a verified prefix", async () => {
        const sessionId = `rust-delta-capture-hostile-${Date.now()}`;
        const rows = rawRows(4);
        installRawRows(sessionId, rows);
        const { client, bodies } = recordingClient((request) =>
            recipeResponse(request, [folded(sessionId)]),
        );
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const history = rowMessages(sessionId, rows.slice(0, 3));
        await transform.run(sessionId, { messages: [...history] });
        let reads = 0;
        const accessor = rowMessages(sessionId, rows.slice(3))[0] as MessageLike;
        Object.defineProperty(accessor, "parts", {
            enumerable: true,
            get: () => {
                reads += 1;
                return [];
            },
        });
        const debugSpy = spyOn(logger.sessionLog, "debug");
        try {
            const cases: Array<[unknown[], string]> = [
                [
                    [...history, new Proxy(rowMessages(sessionId, rows.slice(3))[0] as object, {})],
                    "proxy at /3",
                ],
                [[...history, accessor], "accessor at /3/parts"],
                [[new Proxy(history[0] as object, {}), ...history.slice(1)], "proxy at /0"],
            ];
            for (const [messages, detail] of cases) {
                const output = { messages: [...messages] };
                await transform.run(sessionId, output);
                expect(output.messages).toEqual(messages);
                expect(sessionLogs(debugSpy, sessionId)).toContain(
                    `rust session ${sessionId} pass declined: unsupported_source (${detail})`,
                );
            }
        } finally {
            debugSpy.mockRestore();
        }
        expect(reads).toBe(0);
        expect(bodies).toHaveLength(1);
    });

    it("serves the last applied output plus the appended messages on a capture_bytes decline", async () => {
        const sessionId = `rust-delta-capture-bytes-${Date.now()}`;
        const rows = rawRows(4);
        installRawRows(sessionId, rows);
        const { client, bodies } = recordingClient((request) =>
            recipeResponse(request, [folded(sessionId)]),
        );
        const { admission } = chargeRecordingAdmission({ maxPasses: 64, maxBytes: 512 * 1024 });
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
            captureAdmission: admission,
        });
        const warnSpy = spyOn(logger.sessionLog, "warn");
        const debugSpy = spyOn(logger.sessionLog, "debug");
        try {
            await transform.run(sessionId, { messages: rowMessages(sessionId, rows.slice(0, 3)) });
            // The appended message fits its capture but not its four-fold wire projection.
            const grown = rowMessages(sessionId, rows, (row) =>
                row.id === "m-4" ? "y".repeat(100_000) : `message ${row.id}`,
            );
            const output = { messages: [...grown] as unknown[] };
            await transform.run(sessionId, output);
            expect(bodies).toHaveLength(1);
            expect(sessionLogs(warnSpy, sessionId)).toContain(
                `rust session ${sessionId} pass declined: capture_bytes (transform capture byte budget exceeded: wire projection)`,
            );
            expect(output.messages).toEqual([folded(sessionId), grown[3]]);
            expect(output.messages[1]).toBe(grown[3]);
            expect(passLines(debugSpy, sessionId)[1]).toContain(
                "decision=declined:capture_bytes reason=none served_from=last_applied in=4 out=2",
            );
            expect(transform.getState(sessionId).failureCount).toBe(0);

            // An edit of an acknowledged message leaves no append-only source; the input is served.
            const edited = rowMessages(sessionId, rows, (row) =>
                row.id === "m-4" ? "y".repeat(100_000) : `EDITED ${row.id}`,
            );
            const editedOutput = { messages: [...edited] as unknown[] };
            await transform.run(sessionId, editedOutput);
            expect(editedOutput.messages).toEqual(edited);
        } finally {
            warnSpy.mockRestore();
            debugSpy.mockRestore();
        }
    });

    it("drops the wire cache when its digests exceed the retained history budget", async () => {
        const sessionId = `rust-delta-capture-retained-${Date.now()}`;
        const rows = rawRows(3);
        installRawRows(sessionId, rows);
        const { client, bodies } = recordingClient((request) =>
            recipeResponse(request, [folded(sessionId)]),
        );
        const transform = createRustModeTransform(makeDeps(), {
            moduleClient: client,
            retainedHistoryBudgetBytes: 2 * 16,
        });
        await transform.run(sessionId, { messages: rowMessages(sessionId, rows.slice(0, 2)) });
        await transform.run(sessionId, { messages: rowMessages(sessionId, rows) });
        expect(bodies[1]?.tail_delta).toBeDefined();
        await transform.run(sessionId, { messages: rowMessages(sessionId, rows) });
        expect(bodies[2]?.tail_delta).toBeUndefined();
        expect(bodies[2]?.native_messages).toHaveLength(3);
    });
});
