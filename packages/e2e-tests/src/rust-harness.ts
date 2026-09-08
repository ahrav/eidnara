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
import { managedSubtreePath } from "@eidnara/opencode/shared/host-lifecycle/paths";
import { ballastProse } from "./ballast";
import {
    DEFAULT_MOCK_RESPONSE,
    type SdkClientCore,
    type SharedHarnessOptions,
} from "./harness-primitives";
import { MockProvider } from "./mock-provider/server";
import {
    createIsolatedEnv,
    type IsolatedEnv,
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
}

export interface SdkClient extends SdkClientCore {
    session: SdkClientCore["session"] & {
        revert: (opts: {
            path: { id: string };
            body: { messageID: string; partID?: string };
        }) => Promise<{ data?: unknown }>;
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
    stateSyncMs: number;
    wireBuildMs: number;
    wireMessages: number;
    transportMs: number;
    transportPages: number;
    transportBytes: number;
    rowVersion: number;
    raw: string;
}

export class RustTestHarness {
    readonly mock: MockProvider;
    readonly env: IsolatedEnv;
    readonly host: HermeticHostStack;
    readonly logPath: string;

    private opencodeInstance: SpawnedOpencode;
    private clientInstance: SdkClient;
    private modelContextLimit: number | undefined;
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
    }) {
        this.mock = args.mock;
        this.mockBaseURL = args.mockBaseURL;
        this.env = args.env;
        this.host = args.host;
        this.opencodeInstance = args.opencode;
        this.clientInstance = args.client;
        this.logPath = args.logPath;
        this.modelContextLimit = args.modelContextLimit;
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

        const mock = new MockProvider();
        const { baseURL } = await mock.start();
        const mockDefault = options.mockDefault ?? DEFAULT_MOCK_RESPONSE;
        mock.setDefault(mockDefault);

        const env = createIsolatedEnv();
        const host = await HermeticHostStack.start({
            dataDir: env.dataDir,
            fixtureBin,
        });

        const logPath = join(managedSubtreePath(env.dataDir), "eidnara-e2e.log");

        let opencode: SpawnedOpencode;
        try {
            opencode = await RustTestHarness.spawnServe({
                env,
                mockURL: baseURL,
                connectionFile: host.connectionFile,
                logPath,
                options,
            });
        } catch (error) {
            // Teardown steps run independently: a failure does not skip later steps.
            // The mock HTTP listener must outlive this scope.
            // A teardown failure does not replace the reported spawn failure.
            try {
                await host.stop();
            } catch {
                // ignore
            }
            try {
                await mock.stop();
            } catch {
                // ignore
            }
            throw error;
        }

        const sdk = await import("@opencode-ai/sdk");
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
            modelContextLimit: options.modelContextLimit,
        });
    }

    /** The connection file makes the runner write user-tier rust consent and the project's `transform_mode`. */
    private static spawnServe(args: {
        env: IsolatedEnv;
        mockURL: string;
        connectionFile: string;
        logPath: string;
        options: RustTestHarnessOptions;
    }): Promise<SpawnedOpencode> {
        return spawnOpencode({
            mockProviderURL: args.mockURL,
            existingEnv: args.env,
            modelContextLimit: args.options.modelContextLimit,
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
        });
        const sdk = await import("@opencode-ai/sdk");
        // SAFETY: SdkClient is bounded subset of createOpencodeClient used by this harness.
        this.clientInstance = sdk.createOpencodeClient({
            baseUrl: this.opencodeInstance.url,
        }) as unknown as SdkClient;
    }

    async createSession(): Promise<string> {
        const maxAttempts = 5;
        for (let i = 1; i <= maxAttempts; i++) {
            const res = await this.clientInstance.session.create({
                query: { directory: this.env.workdir },
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
            // OpenCode orders generated message IDs by descending timestamp.
            // The fixture precedes the live seed messages so a later prompt remains newest.
            // This ordering keeps OpenCode and the raw ordinal reader consistent.
            const firstTimestamp = Math.max(1, row.latest - options.count - 1);
            const descendingId = (
                prefix: "msg" | "prt",
                timestamp: number,
                counter: number,
            ): string => {
                const encoded = ~(BigInt(timestamp) * 0x1000n + BigInt(counter));
                const timeBytes = Buffer.alloc(6);
                for (let byte = 0; byte < timeBytes.length; byte += 1) {
                    timeBytes[byte] = Number((encoded >> BigInt(40 - 8 * byte)) & 0xffn);
                }
                return `${prefix}_${timeBytes.toString("hex")}${counter.toString(36).padStart(14, "0")}`;
            };
            const append = db.transaction(() => {
                for (let index = 0; index < options.count; index += 1) {
                    const suffix = index.toString().padStart(4, "0");
                    const timestamp = firstTimestamp + index;
                    const messageId = descendingId("msg", timestamp, 1);
                    const partId = descendingId("prt", timestamp, 2);
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
        options: { agent?: string; timeoutMs?: number } = {},
    ): Promise<unknown> {
        const timeoutMs = options.timeoutMs ?? 180_000;
        const promptPromise = this.clientInstance.session.prompt({
            path: { id: sessionId },
            body: {
                model: { providerID: "mock-anthropic", modelID: "mock-sonnet" },
                parts: [{ type: "text", text }],
                ...(options.agent ? { agent: options.agent } : {}),
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
        return result;
    }

    /** `session.revert` removes the selected message and every later message. */
    async revertMessage(sessionId: string, messageId: string): Promise<void> {
        await this.clientInstance.session.revert({
            path: { id: sessionId },
            body: { messageID: messageId },
        });
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
            const idx = line.indexOf("rust pass: ");
            if (idx < 0) continue;
            const body = line.slice(idx + "rust pass: ".length);
            const elapsedMs = Number(field(body, "elapsed") || "0");
            const moduleElapsedMs = Number(field(body, "module") || "0");
            parsed.push({
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
                stateSyncMs: Number(stageField(body, "state_sync") || "0"),
                wireBuildMs: Number(stageField(body, "wire_build") || "0"),
                wireMessages: Number(stageField(body, "wire_messages") || "0"),
                transportMs: Number(stageField(body, "transport") || "0"),
                transportPages: Number(stageField(body, "transport_pages") || "0"),
                transportBytes: Number(stageField(body, "transport_bytes") || "0"),
                rowVersion: Number(field(body, "row_version") || "0"),
                raw: line,
            });
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

    async dispose(): Promise<void> {
        try {
            await this.opencodeInstance.kill();
        } catch {
            // ignore
        }
        try {
            await this.host.stop();
        } catch {
            // ignore
        }
        try {
            await this.mock.stop();
        } catch {
            // ignore
        }
        try {
            rmSync(join(this.env.dataDir, ".."), {
                recursive: true,
                force: true,
            });
        } catch {
            // ignore
        }
    }
}

function field(body: string, key: string): string {
    const match = body.match(new RegExp(`(?:^|\\s)${key}=([^\\s]+)`));
    return match ? match[1]! : "";
}

function stageField(body: string, key: string): string {
    const match = body.match(new RegExp(`(?:^|\\s)${key}:([^\\s]+)`));
    return match ? match[1]! : "";
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
