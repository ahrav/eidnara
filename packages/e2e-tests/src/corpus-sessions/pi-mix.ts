import { OVERSIZE_BYTES } from "./workspace.ts";

/** The Pi built-in tools the mix calls, plus the extension tool `pi-mix` registers. */
export const PI_MIX_TOOLS = ["read", "bash", "edit", "write", "grep", "find", "ls", "corpus_probe"];

/** The Pi harness surface the mix drives: the faux model's script and the session. */
export interface PiMixHarness {
    session: {
        prompt(text: string): Promise<void>;
        abort(): Promise<void>;
        subscribe(listener: (event: { type: string; toolCallId?: string }) => void): () => void;
        messages: unknown[];
    };
    respond(steps: unknown[]): void;
}

function turn(content: unknown[], stopReason: "stop" | "toolUse") {
    return {
        role: "assistant",
        content,
        api: "faux",
        provider: "faux",
        model: "faux-model",
        usage: {
            input: 0,
            output: 0,
            cacheRead: 0,
            cacheWrite: 0,
            totalTokens: 0,
            cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        },
        stopReason,
        timestamp: Date.now(),
    };
}

/**
 * A Pi extension registering `corpus_probe`, the custom tool standing in for an MCP tool (Pi has no
 * MCP client): `kind: "text"` returns `bytes` ASCII bytes and `kind: "json"` a JSON document.
 */
export function corpusProbeExtension(pi: {
    registerTool(tool: Record<string, unknown>): void;
}): void {
    pi.registerTool({
        name: "corpus_probe",
        label: "Corpus probe",
        description: "Return probe output of the requested kind",
        parameters: {
            type: "object",
            properties: { kind: { type: "string" }, bytes: { type: "number" } },
            required: ["kind"],
        },
        async execute(_id: string, params: { kind?: string; bytes?: number }) {
            const text =
                params.kind === "json"
                    ? JSON.stringify({ status: "ok", items: [1, 2, 3] })
                    : "x".repeat(Number(params.bytes ?? 10));
            return { content: [{ type: "text", text }], details: {} };
        },
    });
}

/**
 * One user turn, one `read` of `README.md`, and a closing turn: the mix both harnesses run for the
 * cross-harness check.
 */
export async function runPiCommonMix(h: PiMixHarness): Promise<void> {
    h.respond([
        turn(
            [
                {
                    type: "toolCall",
                    id: "call_common",
                    name: "read",
                    arguments: { path: "README.md" },
                },
            ],
            "toolUse",
        ),
        turn([{ type: "text", text: "read done" }], "stop"),
    ]);
    await h.session.prompt("read the readme");
    h.respond([turn([{ type: "text", text: "closing" }], "stop")]);
    await h.session.prompt("close the session");
}

/** Drives the R1 tool mix through one Pi session: one tool call per prompt, then a closing turn. */
export async function runPiMix(h: PiMixHarness): Promise<{ canaryCalls: string[] }> {
    let counter = 0;
    const call = async (name: string, args: Record<string, unknown>) => {
        const id = `call_corpus_${++counter}`;
        h.respond([
            turn([{ type: "toolCall", id, name, arguments: args }], "toolUse"),
            turn([{ type: "text", text: `${name} done` }], "stop"),
        ]);
        await h.session.prompt(`run ${name}`);
        return id;
    };
    await call("read", { path: "README.md" });
    const canary = await call("bash", { command: "cat canary.txt" });
    await call("bash", { command: "cat data.json" });
    await call("bash", { command: "printf 'a\\377b'" });
    await call("edit", { path: "notes.txt", edits: [{ oldText: "alpha", newText: "beta" }] });
    await call("write", { path: "out.txt", content: "written by the mix\n" });
    await call("grep", { pattern: "filler", glob: "*.md" });
    await call("find", { pattern: "*.md" });
    await call("ls", { path: "." });
    await call("read", { path: "pixel.png" });
    await call("corpus_probe", { kind: "text", bytes: OVERSIZE_BYTES });
    await call("corpus_probe", { kind: "json" });
    h.respond([
        turn(
            [
                {
                    type: "toolCall",
                    id: "call_corpus_aborted",
                    name: "bash",
                    arguments: { command: "sleep 30" },
                },
            ],
            "toolUse",
        ),
        turn([{ type: "text", text: "after abort" }], "stop"),
    ]);
    const started = new Promise<void>((resolve, reject) => {
        const timer = setTimeout(() => {
            stop();
            reject(new Error("the long bash call never started running"));
        }, 30_000);
        const stop = h.session.subscribe((event) => {
            if (
                event.type === "tool_execution_start" &&
                event.toolCallId === "call_corpus_aborted"
            ) {
                clearTimeout(timer);
                stop();
                resolve();
            }
        });
    });
    const running = h.session.prompt("run a long command").catch(() => undefined);
    await started;
    await h.session.abort();
    await running;
    const settled = (
        h.session.messages as {
            role?: string;
            toolCallId?: string;
            isError?: boolean;
            content?: { text?: string }[];
        }[]
    ).find(
        (message) => message.role === "toolResult" && message.toolCallId === "call_corpus_aborted",
    );
    if (!settled?.isError || !/ aborted$/.test(settled.content?.[0]?.text ?? "")) {
        throw new Error(`the aborted bash call settled as ${JSON.stringify(settled)}`);
    }
    h.respond([turn([{ type: "text", text: "closing" }], "stop")]);
    await h.session.prompt("close the session");
    return { canaryCalls: [canary] };
}
