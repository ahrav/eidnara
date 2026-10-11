import { describe, expect, it } from "bun:test";
import { spawn } from "node:child_process";
import { PassThrough } from "node:stream";
import {
    attachStrictJsonlReader,
    type PiRpcEvent,
    PiRpcProcess,
    PiRpcProtocol,
    serializeRpcMessage,
} from "./rpc-client";

describe("Pi RPC protocol", () => {
    it("frames strict JSONL records on LF only", async () => {
        const stream = new PassThrough();
        const lines: string[] = [];
        attachStrictJsonlReader(stream, (line) => lines.push(line));

        stream.write('{"text":"has \\u2028 separator"}\n{"ok":true}\r\npartial');
        stream.end(" tail");

        await new Promise((resolve) => stream.once("close", resolve));
        expect(lines).toEqual(['{"text":"has \\u2028 separator"}', '{"ok":true}', "partial tail"]);
    });

    it("serializes commands as one JSONL record", () => {
        expect(serializeRpcMessage({ type: "get_state", id: "req-1" })).toBe(
            '{"type":"get_state","id":"req-1"}\n',
        );
    });

    it("correlates responses by id without dispatching them as events", async () => {
        const protocol = new PiRpcProtocol();
        const events: PiRpcEvent[] = [];
        const writes: string[] = [];
        protocol.onEvent((event) => events.push(event));

        const pending = protocol.sendCommand(
            (line) => writes.push(line),
            "get_state",
            {},
            { timeoutMs: 1_000 },
        );
        const sent = JSON.parse(writes[0]!) as { id: string; type: string };
        expect(sent.type).toBe("get_state");

        protocol.dispatchLine(JSON.stringify({ type: "agent_start" }));
        protocol.dispatchLine(
            JSON.stringify({
                id: sent.id,
                type: "response",
                command: "get_state",
                success: true,
                data: { sessionId: "s1" },
            }),
        );

        await expect(pending).resolves.toMatchObject({ data: { sessionId: "s1" } });
        expect(events).toEqual([{ type: "agent_start" }]);
    });

    it("waits for matching async events", async () => {
        const protocol = new PiRpcProtocol();
        const wait = protocol.waitForEvent((event) => event.type === "agent_end", {
            timeoutMs: 1_000,
        });

        protocol.dispatchLine(JSON.stringify({ type: "message_end" }));
        protocol.dispatchLine(JSON.stringify({ type: "agent_end", messages: [] }));

        await expect(wait).resolves.toMatchObject({ type: "agent_end" });
    });

    it("reports non-object JSON records as parse errors", () => {
        const protocol = new PiRpcProtocol();
        const events: PiRpcEvent[] = [];
        protocol.onEvent((event) => events.push(event));

        for (const line of ['"loading"', "123", "null", "[1,2]", "not json"]) {
            protocol.dispatchLine(line);
        }
        protocol.dispatchLine(JSON.stringify({ type: "agent_start" }));

        expect(events.map((event) => event.type)).toEqual([
            "rpc_parse_error",
            "rpc_parse_error",
            "rpc_parse_error",
            "rpc_parse_error",
            "rpc_parse_error",
            "agent_start",
        ]);
        expect(events.slice(0, 5).map((event) => event.line)).toEqual([
            '"loading"',
            "123",
            "null",
            "[1,2]",
            "not json",
        ]);
    });

    it("fails outstanding event waits and commands together when the process is gone", async () => {
        const protocol = new PiRpcProtocol();
        const exit = new Error("Pi RPC process exited with code 1 signal null\nboom");

        const wait = protocol.waitForEvent((event) => event.type === "agent_end", {
            timeoutMs: 60_000,
        });
        const command = protocol.sendCommand(
            () => undefined,
            "get_state",
            {},
            { timeoutMs: 60_000 },
        );
        protocol.rejectPending(exit);

        await expect(wait).rejects.toBe(exit);
        await expect(command).rejects.toBe(exit);

        // A settled wait no longer observes events: dispatch must not throw or resurrect it.
        protocol.dispatchLine(JSON.stringify({ type: "agent_end" }));
        protocol.rejectPending(new Error("second"));
    });

    it("fails a cancelled event wait", async () => {
        const protocol = new PiRpcProtocol();
        const cancel = new AbortController();
        const wait = protocol.waitForEvent((event) => event.type === "agent_end", {
            timeoutMs: 60_000,
            label: "agent_end",
            signal: cancel.signal,
        });
        cancel.abort();
        await expect(wait).rejects.toThrow(/cancelled/);
    });
});

describe("Pi RPC process", () => {
    it("fails a command promptly once Pi has exited", async () => {
        const child = spawn("/bin/sh", ["-c", "exit 3"], { stdio: ["pipe", "pipe", "pipe"] });
        const rpc = new PiRpcProcess(child);
        await new Promise((resolve) => child.once("close", resolve));
        const started = Date.now();
        await expect(rpc.sendCommand("get_state", {}, { timeoutMs: 5_000 })).rejects.toThrow(
            /exited with code 3/,
        );
        expect(Date.now() - started).toBeLessThan(1_000);
    });

    it("fails a pending command when Pi exits", async () => {
        const child = spawn("/bin/sh", ["-c", "read line; exit 4"], {
            stdio: ["pipe", "pipe", "pipe"],
        });
        const rpc = new PiRpcProcess(child);
        const started = Date.now();
        await expect(rpc.sendCommand("get_state", {}, { timeoutMs: 5_000 })).rejects.toThrow(
            /exited with code 4/,
        );
        expect(Date.now() - started).toBeLessThan(1_000);
    });
});
