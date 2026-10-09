/**
 * RustTestHarness drives OpenCode through the directly composed host fixture.
 *
 * Fixture and OpenCode share an isolated data root; the fixture starts before OpenCode
 * so plugin discovery reaches a published, authenticated host.
 * OpenCode restarts preserve OpenCode's own database and the module store.
 *
 * Wire assertions use the model mock's full request bodies.
 * Rust transform decisions are also surfaced through the per-suite diagnostic log at `EIDNARA_LOG_PATH`:
 * the transform emits `rust pass: decision=… served_from=… applied=…` lines.
 *
 * `bun:sqlite` touches only OpenCode's own `opencode.db`, to seed message history the
 * session API has no bulk path for.
 */

import { Database } from "bun:sqlite";
import { existsSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import {
    generateMessageId,
    generatePartId,
} from "@eidnara/opencode/features/context/compaction-marker";
import { managedSubtreePath } from "@eidnara/opencode/shared/host-lifecycle/paths";
import { ballastProse } from "./ballast";
import {
    DEFAULT_MOCK_RESPONSE,
    type SdkClientCore,
    type SharedHarnessOptions,
} from "./harness-primitives";
import type { ForwardConfig } from "./mock-provider/forward";
import { type CapturedRequest, MockProvider } from "./mock-provider/server";
import {
    createIsolatedEnv,
    type IsolatedEnv,
    MOCK_MODEL_ID,
    type SpawnedOpencode,
    spawnOpencode,
} from "./opencode-runner/spawn";
import {
    buildDirectHostFixture,
    detectRustModePrereqs,
    HermeticHostStack,
    type RustModePrereqs,
} from "./rust-runner/hermetic-host";

export interface RustTestHarnessOptions extends SharedHarnessOptions {
    /** Eidnara USER-tier config overrides (thresholds, memory, etc.). */
    eidnaraConfig?: Record<string, unknown>;
    /** Extra environment for the fixture daemon only. */
    daemonEnv?: Record<string, string>;
    /**
     * Record-and-forward mode: the mock forwards every request to this provider, OpenCode runs
     * `forward.model` at `forward.contextLimit`, and the mock serves no script or default.
     */
    forward?: ForwardConfig;
}

export interface SdkClient extends SdkClientCore {
    session: SdkClientCore["session"] & {
        revert: (opts: {
            path: { id: string };
            body: { messageID: string; partID?: string };
        }) => Promise<{ data?: unknown; error?: unknown; response?: { status?: number } }>;
    };
}

/** One parsed `rust pass:` diagnostic line. */
export interface RustPassLine {
    decision: string;
    reason: string;
    servedFrom: string;
    inputCount: number;
    outputCount: number;
    applied: boolean;
    elapsedMs: number;
    moduleElapsedMs: number;
    adapterElapsedMs: number;
    prefixGuardMs: number;
    wireBuildMs: number;
    wireMessages: number;
    transportMs: number;
    transportPages: number;
    transportBytes: number;
    rowVersion: number;
    /** Time the daemon spent awaiting or running a summarizer at the emergency wall, across reruns. */
    emergencyWaitMs: number;
    /** The final-array admission branch: `fits`, `shrinks`, `limit_unknown`, `declined`, or `none`. */
    admission: string;
    /** The admitted candidate's canonical bytes and heuristic charged tokens. */
    invocationBytes: number;
    invocationCharged: number;
    /** The history body budget the request carried, or `null` when it carried none. */
    historyBudget: number | null;
    raw: string;
}

/** The case and session identity a retained capture is filed under. */
export interface CaptureIdentity {
    sessionId: string;
    caseId: string;
    scenarioId: string;
}

/** One main provider request snapshotted under its session and case identity. */
export interface RetainedCapture extends CaptureIdentity {
    request: CapturedRequest;
}

const SESSION_HEADERS = ["x-opencode-session-id", "x-session-affinity", "x-session-id"] as const;

export function requestSessionId(request: CapturedRequest): string | undefined {
    for (const header of SESSION_HEADERS) {
        const value = request.headers[header];
        if (value !== undefined) return value;
    }
    return undefined;
}

/**
 * Retains provider captures under the session and case identity they were driven under. A
 * capture retained before a `MockProvider.reset()` survives it, and a request that arrived
 * before a `tag()` or for another session is never filed under the new identity.
 */
export class CaptureLedger {
    private readonly retained: RetainedCapture[] = [];
    private readonly seen = new WeakSet<CapturedRequest>();
    private identity: CaptureIdentity | null = null;

    /** `source` returns the captures eligible for retention, such as the main requests. */
    constructor(private readonly source: () => CapturedRequest[]) {}

    /** Retains pending captures under the current identity, then files later ones under `identity`. */
    tag(identity: CaptureIdentity): void {
        this.retain();
        for (const request of this.source()) this.seen.add(request);
        this.identity = identity;
    }

    /** Snapshots every not-yet-seen capture; the tagged session's are retained. */
    retain(): void {
        const identity = this.identity;
        if (!identity) return;
        for (const request of this.source()) {
            if (this.seen.has(request)) continue;
            this.seen.add(request);
            if (requestSessionId(request) !== identity.sessionId) continue;
            this.retained.push({ ...identity, request });
        }
    }

    /** Every retained capture matching `identity`, oldest first, after retaining pending ones. */
    captures(identity: Partial<CaptureIdentity> = {}): RetainedCapture[] {
        this.retain();
        return this.retained.filter(
            (capture) =>
                (identity.sessionId === undefined || capture.sessionId === identity.sessionId) &&
                (identity.caseId === undefined || capture.caseId === identity.caseId) &&
                (identity.scenarioId === undefined || capture.scenarioId === identity.scenarioId),
        );
    }
}

const RUST_PASS_MARKER = "rust pass: ";

/** Top-level fields use `key=value`; stage timings follow `stages=` as `key:value` pairs. */
export function parseRustPassLine(line: string): RustPassLine | null {
    const idx = line.indexOf(RUST_PASS_MARKER);
    if (idx < 0) return null;
    const body = line.slice(idx + RUST_PASS_MARKER.length);
    const elapsedMs = Number(field(body, "elapsed") || "0");
    const moduleElapsedMs = Number(field(body, "module") || "0");
    return {
        decision: field(body, "decision"),
        reason: field(body, "reason"),
        servedFrom: field(body, "served_from"),
        inputCount: Number(field(body, "in") || "0"),
        outputCount: Number(field(body, "out") || "0"),
        applied: field(body, "applied") === "true",
        elapsedMs,
        moduleElapsedMs,
        adapterElapsedMs: Math.max(0, elapsedMs - moduleElapsedMs),
        prefixGuardMs: Number(stageField(body, "prefix_guard") || "0"),
        wireBuildMs: Number(stageField(body, "wire_build") || "0"),
        wireMessages: Number(stageField(body, "wire_messages") || "0"),
        transportMs: Number(stageField(body, "transport") || "0"),
        transportPages: Number(stageField(body, "transport_pages") || "0"),
        transportBytes: Number(stageField(body, "transport_bytes") || "0"),
        rowVersion: Number(field(body, "row_version") || "0"),
        emergencyWaitMs: Number(field(body, "emergency_wait") || "0"),
        admission: field(body, "admission") || "none",
        invocationBytes: Number(field(body, "invocation_bytes") || "0"),
        invocationCharged: Number(field(body, "invocation_charged") || "0"),
        historyBudget: /^\d+$/.test(field(body, "history_budget"))
            ? Number(field(body, "history_budget"))
            : null,
        raw: line,
    };
}

export class RustTestHarness {
    readonly mock: MockProvider;
    readonly env: IsolatedEnv;
    readonly host: HermeticHostStack;
    readonly logPath: string;

    private opencodeInstance: SpawnedOpencode;
    private clientInstance: SdkClient;
    private readonly ledger = new CaptureLedger(() => this.mainRequests());
    private modelContextLimit: number | undefined;
    private readonly modelId: string;
    private readonly mockBaseURL: string;

    private constructor(args: {
        mock: MockProvider;
        mockBaseURL: string;
        env: IsolatedEnv;
        host: HermeticHostStack;
        opencode: SpawnedOpencode;
        client: SdkClient;
        logPath: string;
        modelContextLimit: number | undefined;
        modelId: string;
    }) {
        this.mock = args.mock;
        this.mockBaseURL = args.mockBaseURL;
        this.env = args.env;
        this.host = args.host;
        this.opencodeInstance = args.opencode;
        this.clientInstance = args.client;
        this.logPath = args.logPath;
        this.modelContextLimit = args.modelContextLimit;
        this.modelId = args.modelId;
    }

    static detectPrereqs(): RustModePrereqs {
        return detectRustModePrereqs();
    }

    static async create(options: RustTestHarnessOptions = {}): Promise<RustTestHarness> {
        const prereqs = detectRustModePrereqs();
        if (!prereqs.ok) {
            throw new Error(
                `RustTestHarness prerequisites unmet: ${prereqs.skipReason ?? "unknown"}. ` +
                    "Guard the suite with RustTestHarness.detectPrereqs() and skip instead of creating.",
            );
        }

        const fixtureBin = await buildDirectHostFixture();

        const mock = new MockProvider(options.forward ? { forward: options.forward } : {});
        const { baseURL } = await mock.start();
        const modelId = options.forward?.model ?? MOCK_MODEL_ID;
        const modelContextLimit = options.forward?.contextLimit ?? options.modelContextLimit;
        if (!options.forward) mock.setDefault(options.mockDefault ?? DEFAULT_MOCK_RESPONSE);

        const env = createIsolatedEnv();
        const logPath = join(managedSubtreePath(env.dataDir), "eidnara-e2e.log");

        let host: HermeticHostStack | undefined;
        let opencode: SpawnedOpencode;
        try {
            host = await HermeticHostStack.start({
                dataDir: env.dataDir,
                fixtureBin,
                daemonEnv: options.daemonEnv,
            });
            opencode = await RustTestHarness.spawnServe({
                env,
                mockURL: baseURL,
                connectionFile: host.connectionFile,
                logPath,
                options: { ...options, modelContextLimit },
                modelId,
                mockApiKey: mock.inboundKey,
            });
        } catch (error) {
            await RustTestHarness.teardownStack(mock, host, env);
            throw error;
        }

        const sdk = await import("@opencode-ai/sdk");
        // SAFETY: `SdkClient` names only the `session.*` methods the harness calls; the real client provides them.
        const client = sdk.createOpencodeClient({
            baseUrl: opencode.url,
        }) as unknown as SdkClient;

        return new RustTestHarness({
            mock,
            mockBaseURL: baseURL,
            env,
            host,
            opencode,
            client,
            logPath,
            modelContextLimit,
            modelId,
        });
    }

    /** The connection file the runner writes into the user-tier `host` block. */
    private static spawnServe(args: {
        env: IsolatedEnv;
        mockURL: string;
        connectionFile: string;
        logPath: string;
        options: RustTestHarnessOptions;
        modelId: string;
        mockApiKey: string;
    }): Promise<SpawnedOpencode> {
        return spawnOpencode({
            mockProviderURL: args.mockURL,
            existingEnv: args.env,
            modelContextLimit: args.options.modelContextLimit,
            modelId: args.modelId,
            mockApiKey: args.mockApiKey,
            openCodeConfigExtra: args.options.openCodeConfigExtra,
            eidnaraConfig: args.options.eidnaraConfig,
            userHostConnectionFile: args.connectionFile,
            extraEnv: { EIDNARA_LOG_PATH: args.logPath },
        });
    }

    get opencode(): SpawnedOpencode {
        return this.opencodeInstance;
    }

    get client(): SdkClient {
        return this.clientInstance;
    }

    /**
     * Restarts `opencode serve` against the same data directory.
     * OpenCode's database, the module store, and the direct host persist across restarts.
     */
    async restart(opts: { eidnaraConfig?: Record<string, unknown> } = {}): Promise<void> {
        await this.opencodeInstance.kill();
        this.opencodeInstance = await RustTestHarness.spawnServe({
            env: this.env,
            mockURL: this.mockBaseURL,
            connectionFile: this.host.connectionFile,
            logPath: this.logPath,
            options: {
                modelContextLimit: this.modelContextLimit,
                eidnaraConfig: opts.eidnaraConfig,
            },
            modelId: this.modelId,
            mockApiKey: this.mock.inboundKey,
        });
        const sdk = await import("@opencode-ai/sdk");
        // SAFETY: SdkClient is bounded subset of createOpencodeClient used by this harness.
        this.clientInstance = sdk.createOpencodeClient({
            baseUrl: this.opencodeInstance.url,
        }) as unknown as SdkClient;
    }

    /** Creates a session in `directory`, the isolated workdir by default. */
    async createSession(directory = this.env.workdir): Promise<string> {
        const maxAttempts = 5;
        for (let i = 1; i <= maxAttempts; i++) {
            const res = await this.clientInstance.session.create({
                query: { directory },
            });
            if (res.data) return res.data.id;
            if (i < maxAttempts) {
                await Bun.sleep(200 * i);
                continue;
            }
            throw new Error(
                `session.create failed after ${maxAttempts} attempts. stderr:\n${this.opencodeInstance.stderr()}`,
            );
        }
        throw new Error("session.create failed");
    }

    /**
     * Deletes every message of the session later than `messageId` from OpenCode's own
     * `opencode.db`, the removal a revert performs; `session.revert` marks the session without
     * deleting its rows.
     */
    removeMessagesAfter(sessionId: string, messageId: string): number {
        const db = new Database(join(this.env.dataDir, "opencode", "opencode.db"));
        try {
            db.exec("PRAGMA busy_timeout = 30000");
            return deleteMessagesAfter(db, sessionId, messageId);
        } finally {
            db.close();
        }
    }

    /**
     * `ballastProse()` generates approximately `tokens` tokens of varied prose.
     */
    ballast(tokens: number): string {
        return ballastProse(tokens);
    }

    /**
     * Inserts `count` user text messages of `textBytes` each into OpenCode's own `opencode.db`,
     * cloned from the session's newest user message, so a later prompt observes a large history
     * without driving thousands of prompts. Call before `restart()` so OpenCode reloads the session.
     */
    appendSyntheticHistory(sessionId: string, options: { count: number; textBytes: number }): void {
        const dbPath = join(this.env.dataDir, "opencode", "opencode.db");
        const db = new Database(dbPath);
        try {
            db.exec("PRAGMA busy_timeout = 30000");
            const row = db
                .prepare(
                    "SELECT COALESCE(MAX(time_created), 0) AS latest FROM message WHERE session_id = ?",
                )
                .get(sessionId) as { latest: number };
            const templateRow = db
                .prepare(
                    "SELECT m.data AS message_data, p.data AS part_data FROM message m JOIN part p ON p.message_id = m.id WHERE m.session_id = ? AND json_extract(m.data, '$.role') = 'user' AND json_extract(p.data, '$.type') = 'text' ORDER BY m.time_created DESC LIMIT 1",
                )
                .get(sessionId) as { message_data: string; part_data: string } | undefined;
            if (!templateRow) {
                throw new Error("synthetic history requires an existing user text message");
            }
            const messageTemplate = JSON.parse(templateRow.message_data) as Record<string, unknown>;
            const partTemplate = JSON.parse(templateRow.part_data) as Record<string, unknown>;
            const insertMessage = db.prepare(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
            );
            const insertPart = db.prepare(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?, ?)",
            );
            // Synthetic timestamps end before `row.latest` so a later prompt remains newest.
            const firstTimestamp = Math.max(1, row.latest - options.count - 1);
            const append = db.transaction(() => {
                for (let index = 0; index < options.count; index += 1) {
                    const suffix = index.toString().padStart(4, "0");
                    const timestamp = firstTimestamp + index;
                    const messageId = generateMessageId(timestamp, 1n, `synthetic-${suffix}`);
                    const partId = generatePartId(timestamp, 2n, `synthetic-${suffix}`);
                    const prefix = `synthetic history message ${suffix}: `;
                    const text = `${prefix}${"x".repeat(Math.max(0, options.textBytes - prefix.length))}`;
                    insertMessage.run(
                        messageId,
                        sessionId,
                        timestamp,
                        timestamp,
                        JSON.stringify({
                            ...messageTemplate,
                            id: messageId,
                            sessionID: sessionId,
                            time: {
                                ...((messageTemplate.time as Record<string, unknown> | undefined) ??
                                    {}),
                                created: timestamp,
                            },
                        }),
                    );
                    insertPart.run(
                        partId,
                        messageId,
                        sessionId,
                        timestamp,
                        timestamp,
                        JSON.stringify({
                            ...partTemplate,
                            id: partId,
                            messageID: messageId,
                            sessionID: sessionId,
                            text,
                        }),
                    );
                }
            });
            append();
        } finally {
            db.close();
        }
    }

    async sendPrompt(
        sessionId: string,
        text: string,
        options: { agent?: string; system?: string; timeoutMs?: number } = {},
    ): Promise<unknown> {
        const timeoutMs = options.timeoutMs ?? 180_000;
        const promptPromise = this.clientInstance.session.prompt({
            path: { id: sessionId },
            body: {
                model: { providerID: "mock-anthropic", modelID: this.modelId },
                parts: [{ type: "text", text }],
                ...(options.agent ? { agent: options.agent } : {}),
                ...(options.system ? { system: options.system } : {}),
            },
        });
        const timeout = new Promise<null>((r) => setTimeout(() => r(null), timeoutMs));
        const result = await Promise.race([promptPromise, timeout]);
        if (result === null) {
            throw new Error(
                `sendPrompt did not complete within ${timeoutMs}ms. stderr:\n${this.opencodeInstance
                    .stderr()
                    .slice(-2000)}\nhost log:\n${this.host.hostLog().slice(-2000)}`,
            );
        }
        // The SDK reports a rejected request through `error` instead of throwing.
        if (sdkRejected(result)) {
            throw sdkFailure(
                "session.prompt",
                result,
                `. stderr:\n${this.opencodeInstance.stderr().slice(-2000)}`,
            );
        }
        return result;
    }

    async revertMessage(sessionId: string, messageId: string): Promise<void> {
        const res = await this.clientInstance.session.revert({
            path: { id: sessionId },
            body: { messageID: messageId },
        });
        if (sdkRejected(res)) throw sdkFailure(`session.revert(${messageId})`, res);
    }

    async listMessages(
        sessionId: string,
    ): Promise<Array<{ info?: { id?: string; role?: string } }>> {
        const res = await this.clientInstance.session.messages({
            path: { id: sessionId },
        });
        const data = (res as { data?: unknown }).data;
        return Array.isArray(data)
            ? (data as Array<{ info?: { id?: string; role?: string } }>)
            : [];
    }

    /** Requests carrying the Eidnara system block; internal agents (title, summary, …) lack it. */
    mainRequests() {
        return this.mock
            .requests()
            .filter((r) => JSON.stringify(r.body.system ?? "").includes("## Eidnara"));
    }

    lastMainMessages(): Array<{ role?: string; content?: unknown }> {
        const req = this.mainRequests().at(-1);
        const messages = req?.body.messages;
        return Array.isArray(messages)
            ? (messages as Array<{ role?: string; content?: unknown }>)
            : [];
    }

    /** Byte length of the last main request's messages with `cache_control` markers stripped. */
    lastMainWireBytes(): number {
        const req = this.mainRequests().at(-1);
        if (!req) return 0;
        return Buffer.byteLength(stableSerialize(req.body.messages ?? []));
    }

    lastMainWireSerialized(): string {
        const req = this.mainRequests().at(-1);
        return stableSerialize(req?.body.messages ?? []);
    }

    /** The transform logs its pass line after the provider response is captured; polling avoids that race without a fixed sleep. */
    async waitForRustPasses(minCount: number, timeoutMs = 15_000): Promise<RustPassLine[]> {
        return this.waitFor(
            () => {
                const passes = this.readRustPasses();
                return passes.length >= minCount ? passes : null;
            },
            { timeoutMs, label: `>= ${minCount} rust passes` },
        );
    }

    diagnosticLog(): string {
        if (!existsSync(this.logPath)) return "";
        return readFileSync(this.logPath, "utf8");
    }

    readRustPasses(): RustPassLine[] {
        if (!existsSync(this.logPath)) return [];
        const lines = readFileSync(this.logPath, "utf8").split("\n");
        const parsed: RustPassLine[] = [];
        for (const line of lines) {
            const pass = parseRustPassLine(line);
            if (pass) parsed.push(pass);
        }
        return parsed;
    }

    async waitFor<T>(
        predicate: () => T | null | undefined | false,
        opts: { timeoutMs?: number; intervalMs?: number; label?: string } = {},
    ): Promise<T> {
        const timeoutMs = opts.timeoutMs ?? 60_000;
        const intervalMs = opts.intervalMs ?? 100;
        const deadline = Date.now() + timeoutMs;
        while (Date.now() < deadline) {
            const value = predicate();
            if (value) return value as T;
            await Bun.sleep(intervalMs);
        }
        throw new Error(
            `waitFor timed out after ${timeoutMs}ms${opts.label ? ` (${opts.label})` : ""}`,
        );
    }

    requests() {
        return this.mock.requests();
    }

    /** Retains pending main-request captures, then files later ones under `identity`. */
    tagCaptures(identity: CaptureIdentity): void {
        this.ledger.tag(identity);
    }

    /** Retains the tagged session's pending main requests, then starts a fresh mock run. */
    resetMock(): void {
        this.ledger.retain();
        this.mock.reset();
    }

    /** Every retained main-request capture matching `identity`, oldest first. */
    retainedCaptures(identity: Partial<CaptureIdentity> = {}): RetainedCapture[] {
        return this.ledger.captures(identity);
    }

    async dispose(): Promise<void> {
        let survivingOpencode: unknown;
        try {
            await this.opencodeInstance.kill();
        } catch (error) {
            survivingOpencode = error;
        }
        await RustTestHarness.teardownStack(this.mock, this.host, this.env, {
            preserveState: survivingOpencode !== undefined,
        });
        // No later run can reclaim a surviving OpenCode process, so its state stays on disk and the test fails.
        if (survivingOpencode !== undefined) throw survivingOpencode;
    }

    /**
     * Each step runs even when an earlier one throws, and no step's error escapes, so a caller
     * that is already reporting a failure keeps that failure as the reported one.
     */
    private static async teardownStack(
        mock: MockProvider,
        host: HermeticHostStack | undefined,
        env: IsolatedEnv,
        options: { preserveState?: boolean } = {},
    ): Promise<void> {
        try {
            await host?.stop();
        } catch {
            // A failed teardown keeps `dataDir` and its PID record for the next run's reaper.
        }
        try {
            await mock.stop();
        } catch {
            // ignore
        }
        if (options.preserveState) return;
        // A successful host teardown removes `dataDir` itself, so its presence marks a leaked fixture whose PID record must survive.
        if (existsSync(env.dataDir)) return;
        try {
            rmSync(join(env.dataDir, ".."), {
                recursive: true,
                force: true,
            });
        } catch {
            // ignore
        }
    }
}

type SdkResult = { data?: unknown; error?: unknown; response?: { status?: number } };

/** The SDK reports a rejected request through `error` instead of throwing. */
function sdkFailure(label: string, res: SdkResult, detail = ""): Error {
    return new Error(
        `${label} failed with status ${res.response?.status ?? "unknown"}: ${JSON.stringify(res.error ?? null)}${detail}`,
    );
}

function sdkRejected(res: SdkResult): boolean {
    return res.error !== undefined || res.data === undefined;
}

function field(body: string, key: string): string {
    const match = body.match(new RegExp(`(?:^|\\s)${key}=([^\\s]+)`));
    return match ? match[1]! : "";
}

function stageField(body: string, key: string): string {
    const match = body.match(new RegExp(`(?:^|\\s|=)${key}:([^\\s]+)`));
    return match ? match[1]! : "";
}

/**
 * Deletes the session's messages, and their parts, that sort after `messageId` in the plugin's
 * `(time_created, id)` session order, so a later message sharing the anchor's millisecond goes too.
 * Returns the number of messages removed.
 */
export function deleteMessagesAfter(db: Database, sessionId: string, messageId: string): number {
    const remove = db.transaction(() => {
        db.prepare(
            "DELETE FROM part WHERE message_id IN (SELECT m.id FROM message m, message anchor WHERE anchor.session_id = ?1 AND anchor.id = ?2 AND m.session_id = ?1 AND (m.time_created, m.id) > (anchor.time_created, anchor.id))",
        ).run(sessionId, messageId);
        return db
            .prepare(
                "DELETE FROM message WHERE id IN (SELECT m.id FROM message m, message anchor WHERE anchor.session_id = ?1 AND anchor.id = ?2 AND m.session_id = ?1 AND (m.time_created, m.id) > (anchor.time_created, anchor.id))",
            )
            .run(sessionId, messageId).changes;
    });
    return remove();
}

/** JSON without `cache_control` markers, because OpenCode moves the marker to the newest message each turn. */
export function stableSerialize(value: unknown): string {
    return JSON.stringify(stripCacheControl(value)) ?? "";
}

type CacheStrippedValue =
    | null
    | boolean
    | number
    | string
    | CacheStrippedValue[]
    | { [key: string]: CacheStrippedValue | undefined };

function stripCacheControl(value: unknown): CacheStrippedValue | undefined {
    if (Array.isArray(value))
        return value.map(stripCacheControl).filter((item) => item !== undefined);
    if (value && typeof value === "object") {
        const out: { [key: string]: CacheStrippedValue | undefined } = {};
        for (const [key, child] of Object.entries(value)) {
            if (key === "cache_control") continue;
            out[key] = stripCacheControl(child);
        }
        return out;
    }
    if (value === null || ["boolean", "number", "string"].includes(typeof value)) {
        return value as null | boolean | number | string;
    }
    return undefined;
}
