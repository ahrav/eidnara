/** PiTestHarness provides an e2e-test facade for Pi Eidnara. */

import { rmSync } from "node:fs";
import { DEFAULT_MOCK_RESPONSE } from "./harness-primitives";
import { MockProvider, type MockResponse } from "./mock-provider/server";
import {
    PiRpcClient,
    type PiRpcEvent,
    type PiState,
    requireSuccessfulResponse,
} from "./pi-runner/rpc-client";
import { createPiIsolatedEnv, type PiIsolatedEnv, type PiRunResult } from "./pi-runner/spawn";

export interface PiTestHarnessOptions {
    eidnaraConfig?: Record<string, unknown>;
    piSettingsExtra?: Record<string, unknown>;
    modelContextLimit?: number;
    mockDefault?: MockResponse;
}

export function finalAssistantText(agentEnd: PiRpcEvent): string | null {
    const messages = agentEnd.messages;
    if (!Array.isArray(messages)) return null;
    for (let i = messages.length - 1; i >= 0; i--) {
        const message = messages[i] as { role?: unknown; content?: unknown } | null;
        if (!message || message.role !== "assistant") continue;
        if (!Array.isArray(message.content)) return "";
        return message.content
            .map((part) => {
                const p = part as { type?: unknown; text?: unknown } | null;
                return p && p.type === "text" && typeof p.text === "string" ? p.text : "";
            })
            .join("");
    }
    return null;
}

export class PiTestHarness {
    readonly mock: MockProvider;
    readonly env: PiIsolatedEnv;

    private readonly rpc: PiRpcClient;

    private constructor(mock: MockProvider, rpc: PiRpcClient) {
        this.mock = mock;
        this.rpc = rpc;
        this.env = rpc.env;
    }

    static async create(options: PiTestHarnessOptions = {}): Promise<PiTestHarness> {
        const mock = new MockProvider();
        const { baseURL } = await mock.start();
        mock.setDefault(options.mockDefault ?? DEFAULT_MOCK_RESPONSE);
        const rpc = new PiRpcClient({
            env: createPiIsolatedEnv(),
            mockProviderURL: baseURL,
            eidnaraConfig: options.eidnaraConfig,
            piSettingsExtra: options.piSettingsExtra,
            modelContextLimit: options.modelContextLimit,
        });

        try {
            await rpc.start();
        } catch (error) {
            await mock.stop();
            throw error;
        }

        return new PiTestHarness(mock, rpc);
    }

    async sendPrompt(
        text: string,
        options: { timeoutMs?: number; images?: unknown[] } = {},
    ): Promise<PiRunResult> {
        // The 180s default accommodates ctx_search subprocesses on GitHub-hosted Ubuntu runners.
        const timeoutMs = options.timeoutMs ?? 180_000;
        const events: PiRpcEvent[] = [];
        const malformedLines: string[] = [];
        let capturing = false;
        const unsubscribe = this.rpc.onEvent((event) => {
            // A non-JSON stdout line corrupts the RPC stream even when a valid `agent_end` follows.
            if (event.type === "rpc_parse_error") malformedLines.push(String(event.line));
            if (event.type === "agent_start") capturing = true;
            if (capturing) events.push(event);
        });
        const agentEnd = this.rpc.waitForEvent((event) => capturing && event.type === "agent_end", {
            timeoutMs,
            label: "agent_end",
        });

        try {
            const promptResponse = await this.rpc.sendCommand(
                "prompt",
                { message: text, ...(options.images ? { images: options.images } : {}) },
                { timeoutMs, label: "prompt response" },
            );
            requireSuccessfulResponse(promptResponse);
            const agentEndEvent = await agentEnd;
            const state = await this.getState();
            const status = await this.rpc.settledProcessStatus();
            if (malformedLines.length > 0) {
                throw new Error(
                    `Pi wrote ${malformedLines.length} non-JSON line(s) to its RPC stdout during the turn:\n${malformedLines.join("\n")}`,
                );
            }
            return {
                sessionId: typeof state.sessionId === "string" ? state.sessionId : null,
                assistantText: finalAssistantText(agentEndEvent),
                events: events as Array<Record<string, unknown>>,
                stdout: events.map((event) => JSON.stringify(event)).join("\n"),
                stderr: this.rpc.getStderr(),
                exitCode: status.exitCode,
                signalCode: status.signalCode,
            };
        } catch (error) {
            void agentEnd.catch(() => undefined);
            const message = error instanceof Error ? error.message : String(error);
            throw new Error(`${message}\n--- pi rpc stderr ---\n${this.rpc.getStderr()}`);
        } finally {
            unsubscribe();
        }
    }

    async getState(): Promise<PiState> {
        const response = await this.rpc.sendCommand<PiState>("get_state");
        return requireSuccessfulResponse(response);
    }

    async dispose(): Promise<void> {
        try {
            await this.rpc.shutdown();
        } finally {
            await this.mock.stop();
            rmSync(this.env.baseDir, { recursive: true, force: true });
        }
    }
}
