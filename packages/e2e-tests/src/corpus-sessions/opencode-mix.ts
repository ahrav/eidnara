import { join } from "node:path";

import { MOCK_MODEL_ID } from "../opencode-runner/spawn.ts";
import type { HarnessHost, RustTestHarness } from "../rust-harness.ts";
import { runScriptedToolCall } from "../scripted-tool-call.ts";
import { OVERSIZE_BYTES } from "./workspace.ts";

/** The local MCP server the OpenCode mix registers as `corpus`; OpenCode publishes its tool as `corpus_probe`. */
export const MCP_SERVER = join(import.meta.dir, "mcp-probe-server.ts");

export function openCodeMcpConfig(): Record<string, unknown> {
    return { mcp: { corpus: { type: "local", command: ["bun", MCP_SERVER], enabled: true } } };
}

/** The SDK calls the mix makes beyond the harness's typed surface: abort, and a prompt with a file part. */
interface RawSessionClient {
    session: {
        abort(opts: { path: { id: string } }): Promise<unknown>;
        prompt(opts: {
            path: { id: string };
            body: { model: { providerID: string; modelID: string }; parts: unknown[] };
        }): Promise<unknown>;
    };
}

function rawClient(h: RustTestHarness<HarnessHost>): RawSessionClient {
    // SAFETY: `h.client` is the OpenCode SDK client, which serves `session.abort` and accepts file
    // parts in `session.prompt`; the harness type declares only the calls the harness makes.
    return h.client as unknown as RawSessionClient;
}

/** What the mix drove, for reconciliation and the fixture census. */
export interface MixRun {
    /** Tool call ids whose result carries the canary. */
    readonly canaryCalls: readonly string[];
}

/**
 * Drives the R1 tool mix through one OpenCode session in `root`: one tool call per prompt, an
 * image attachment, an aborted call, and a closing turn, so every earlier message reaches a pass.
 */
export async function runOpenCodeMix(
    h: RustTestHarness<HarnessHost>,
    sessionId: string,
    root: string,
): Promise<MixRun> {
    const canaryCalls: string[] = [];
    const call = async (tool: string, input: Record<string, unknown>, canary = false) => {
        await runScriptedToolCall(h, sessionId, { tool, input, prompt: `run ${tool}` });
        const callId = scriptedCallId(h);
        if (canary && callId) canaryCalls.push(callId);
    };
    await call("read", { filePath: join(root, "README.md") });
    await call("bash", { command: "cat canary.txt", description: "Read the canary" }, true);
    await call("bash", { command: "cat data.json", description: "Read JSON" });
    await call("bash", { command: "printf 'a\\377b'", description: "Print invalid UTF-8" });
    await call("edit", {
        filePath: join(root, "notes.txt"),
        oldString: "alpha",
        newString: "beta",
    });
    await call("write", { filePath: join(root, "out.txt"), content: "written by the mix\n" });
    await call("grep", { pattern: "filler", include: "*.md" });
    await call("glob", { pattern: "*.md" });
    await call("todowrite", {
        todos: [{ content: "Check the corpus", status: "pending", priority: "medium", id: "t1" }],
    });
    await call("task", {
        description: "Inspect crates",
        prompt: "List the crates directory.",
        subagent_type: "general",
    });
    await call("corpus_probe", { kind: "text", bytes: OVERSIZE_BYTES });
    await call("corpus_probe", { kind: "json" });
    await abortedCall(h, sessionId);
    await imagePrompt(h, sessionId, root);
    h.resetMock();
    h.mock.setDefault({ text: "closing", usage: { input_tokens: 100, output_tokens: 5 } });
    await h.sendPrompt(sessionId, "close the session");
    return { canaryCalls };
}

/** The OpenCode side of the cross-harness mix: one `read` of `README.md`, then a closing turn. */
export async function runOpenCodeCommonMix(
    h: RustTestHarness<HarnessHost>,
    sessionId: string,
    root: string,
): Promise<void> {
    await runScriptedToolCall(h, sessionId, {
        tool: "read",
        input: { filePath: join(root, "README.md") },
        prompt: "read the readme",
    });
    h.resetMock();
    h.mock.setDefault({ text: "closing", usage: { input_tokens: 100, output_tokens: 5 } });
    await h.sendPrompt(sessionId, "close the session");
}

/** The call id of the newest tool result the mock saw since its last reset. */
function scriptedCallId(h: RustTestHarness<HarnessHost>): string | null {
    let newest: string | null = null;
    for (const request of h.mock.requests()) {
        for (const message of (request.body.messages ?? []) as { content?: unknown }[]) {
            if (!Array.isArray(message.content)) continue;
            for (const block of message.content as { type?: string; tool_use_id?: string }[]) {
                if (block.type === "tool_result" && block.tool_use_id) newest = block.tool_use_id;
            }
        }
    }
    return newest;
}

/** A `bash` call the session aborts while it runs: OpenCode settles it as an interrupted error. */
async function abortedCall(h: RustTestHarness<HarnessHost>, sessionId: string): Promise<void> {
    const callId = "toolu_corpus_aborted";
    h.resetMock();
    let served = false;
    h.mock.addMatcher(() => {
        if (served) return null;
        served = true;
        return {
            content: [
                {
                    type: "tool_use",
                    id: callId,
                    name: "bash",
                    input: { command: "sleep 30", description: "Wait" },
                },
            ],
            stop_reason: "tool_use" as const,
            usage: { input_tokens: 100, output_tokens: 5 },
        };
    });
    h.mock.setDefault({ text: "after abort", usage: { input_tokens: 100, output_tokens: 5 } });
    const prompt = h.sendPrompt(sessionId, "run a long command");
    const part = async () => {
        const messages = (await h.listMessages(sessionId)) as {
            parts?: { callID?: string; state?: { status?: string; output?: string } }[];
        }[];
        return messages.flatMap((message) => message.parts ?? []).find((p) => p.callID === callId);
    };
    const deadline = Date.now() + 30_000;
    while ((await part())?.state?.status !== "running") {
        if (Date.now() > deadline) throw new Error("the long bash call never started running");
        await Bun.sleep(100);
    }
    await rawClient(h).session.abort({ path: { id: sessionId } });
    await prompt.catch(() => null);
    const settled = (await part())?.state;
    if (!settled?.output?.includes("User aborted the command")) {
        throw new Error(`the aborted bash call settled as ${JSON.stringify(settled)}`);
    }
}

/** A user prompt carrying an image file part beside its text. */
async function imagePrompt(
    h: RustTestHarness<HarnessHost>,
    sessionId: string,
    root: string,
): Promise<void> {
    h.resetMock();
    h.mock.setDefault({ text: "an image", usage: { input_tokens: 100, output_tokens: 5 } });
    const bytes = await Bun.file(join(root, "pixel.png")).arrayBuffer();
    const url = `data:image/png;base64,${Buffer.from(bytes).toString("base64")}`;
    await rawClient(h).session.prompt({
        path: { id: sessionId },
        body: {
            model: { providerID: "mock-anthropic", modelID: MOCK_MODEL_ID },
            parts: [
                { type: "text", text: "describe this image" },
                { type: "file", mime: "image/png", filename: "pixel.png", url },
            ],
        },
    });
}
