/**
 * A stdio MCP server with one tool, `probe`, for the OpenCode corpus mix: `kind: "text"` returns
 * `bytes` ASCII bytes and `kind: "json"` returns a JSON document.
 */

export {};

/** One JSON-RPC result body the server answers with. */
type McpResult = Record<string, unknown>;

interface Request {
    id?: number;
    method?: string;
    params?: { protocolVersion?: string; arguments?: { kind?: string; bytes?: number } };
}

function send(message: unknown): void {
    process.stdout.write(`${JSON.stringify(message)}\n`);
}

function answer(request: Request): McpResult | undefined {
    switch (request.method) {
        case "initialize":
            return {
                protocolVersion: request.params?.protocolVersion ?? "2024-11-05",
                capabilities: { tools: {} },
                serverInfo: { name: "corpus", version: "1.0.0" },
            };
        case "tools/list":
            return {
                tools: [
                    {
                        name: "probe",
                        description: "Return probe output of the requested kind",
                        inputSchema: {
                            type: "object",
                            properties: { kind: { type: "string" }, bytes: { type: "number" } },
                            required: ["kind"],
                        },
                    },
                ],
            };
        case "tools/call": {
            const { kind, bytes } = request.params?.arguments ?? {};
            const text =
                kind === "json"
                    ? JSON.stringify({ status: "ok", items: [1, 2, 3] })
                    : "x".repeat(Number(bytes ?? 10));
            return { content: [{ type: "text", text }] };
        }
        default:
            return undefined;
    }
}

const decoder = new TextDecoder();
let buffer = "";
for await (const chunk of Bun.stdin.stream()) {
    buffer += decoder.decode(chunk, { stream: true });
    for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
        const line = buffer.slice(0, at).trim();
        buffer = buffer.slice(at + 1);
        if (!line) continue;
        let request: Request;
        try {
            request = JSON.parse(line) as Request;
        } catch {
            continue;
        }
        if (request.id === undefined) continue;
        const result = answer(request);
        send(
            result === undefined
                ? { jsonrpc: "2.0", id: request.id, error: { code: -32601, message: "unknown" } }
                : { jsonrpc: "2.0", id: request.id, result },
        );
    }
}
