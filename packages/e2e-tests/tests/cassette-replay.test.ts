/**
 * Agent-loop cassette faithfulness through a real OpenCode process: a recorded two-request tool
 * loop replays with the same tool-call sequence, arguments, request count, and outcome; the
 * scripted block is never entered; a request past the recording is one typed miss that stops
 * the run; a changed tool result is a `ToolResultDrift` miss; and a planted credential leaves
 * no cassette at all.
 *
 * Record and replay share one OpenCode instance: the system prompt carries the working directory
 * and today's date, so a cassette is bound to the environment that recorded it.
 */

import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CassetteOracle, CassetteRefused } from "../src/mock-provider/cassette-oracle";
import { RustTestHarness } from "../src/rust-harness";
import { rustPrereqs } from "../src/rust-scenario-support";
import { findToolResultText } from "../src/scripted-tool-call";

const NAMESPACE = "e2e:cassette:fresh";
const PROMPT = "Use the glob tool once for *.md and summarize in one word.";
const USAGE = { input_tokens: 100, output_tokens: 20 };
const CANARY = "sk-ant-api03-Zk9Qw2Lm7Pv4Rt8Zw1Yc6Nb3Hd5Kf0JgAbCdEfGh";

interface Part {
    type?: string;
    tool?: string;
    text?: string;
    state?: { input?: unknown; status?: string };
}

/** The tool calls and final text of one session, in message order. */
async function transcript(h: RustTestHarness, sessionId: string) {
    const messages = (await h.listMessages(sessionId)) as Array<{
        info?: { role?: string };
        parts?: Part[];
    }>;
    const calls: Array<{ tool: string; input: unknown; status: string | undefined }> = [];
    const texts: string[] = [];
    for (const message of messages) {
        if (message.info?.role !== "assistant") continue;
        for (const part of message.parts ?? []) {
            if (part.type === "tool" && part.tool) {
                calls.push({
                    tool: part.tool,
                    input: part.state?.input,
                    status: part.state?.status,
                });
            } else if (part.type === "text" && part.text) {
                texts.push(part.text);
            }
        }
    }
    return { calls, texts };
}

async function replayer(path: string): Promise<CassetteOracle> {
    const oracle = await CassetteOracle.start();
    await oracle.open("replay", NAMESPACE, path);
    return oracle;
}

let scriptedCalls = 0;

/**
 * Scripts one tool call and a text follow-up on the bound cassette, then drives the prompt.
 * `runScriptedToolCall` is not used because its `reset()` would unbind the cassette.
 */
async function recordToolLoop(
    h: RustTestHarness,
    recorder: CassetteOracle,
    tool: string,
    input: Record<string, unknown>,
    prompt: string,
): Promise<{ sessionId: string; resultText: string }> {
    h.mock.reset();
    h.mock.useCassette({ oracle: recorder, mode: "record", namespace: NAMESPACE });
    const callId = `toolu_cassette_${++scriptedCalls}`;
    let published = false;
    h.mock.addMatcher((body) => {
        const tools = Array.isArray(body.tools) ? (body.tools as Array<{ name?: string }>) : [];
        if (published || !tools.some((candidate) => candidate.name === tool)) return null;
        published = true;
        return {
            content: [{ type: "tool_use", id: callId, name: tool, input }],
            stop_reason: "tool_use" as const,
            usage: USAGE,
        };
    });
    h.mock.setDefault({ text: "recorded-follow-up", usage: USAGE });
    const sessionId = await h.createSession();
    await h.sendPrompt(sessionId, prompt);
    const resultText = findToolResultText(h, callId);
    if (resultText === null) throw new Error(`no tool_result for ${tool}`);
    return { sessionId, resultText };
}

describe.skipIf(!rustPrereqs.ok)("agent-loop cassette", () => {
    let h: RustTestHarness;
    let dir: string;
    let cassettePath: string;

    beforeAll(async () => {
        h = await RustTestHarness.create();
        dir = mkdtempSync(join(tmpdir(), "eidnara-cassette-"));
        cassettePath = join(dir, "fresh.json");
        writeFileSync(driftFile(), "one\n");
    });

    /** A workspace file whose content, not name, changes between record and replay. */
    const driftFile = () => join(h.env.workdir, "drift.md");

    afterAll(async () => {
        await h?.dispose();
        if (dir) rmSync(dir, { recursive: true, force: true });
    });

    it("records a tool loop, replays it faithfully, and stops at the first miss", async () => {
        // Record.
        const recorder = await CassetteOracle.start();
        expect(await recorder.open("record", NAMESPACE, cassettePath)).toEqual({ cases: 0 });
        const { sessionId: recordingSession, resultText } = await recordToolLoop(
            h,
            recorder,
            "glob",
            { pattern: "*.md" },
            PROMPT,
        );
        expect(resultText).toContain("drift.md");
        // The tool turn, its follow-up, and OpenCode's concurrent title request.
        const recordedRequests = h.mock.requests();
        expect(recordedRequests.length).toBeGreaterThanOrEqual(2);
        const recordedTranscript = await transcript(h, recordingSession);
        expect(recordedTranscript.calls.map((call) => call.tool)).toEqual(["glob"]);
        expect(recordedTranscript.texts).toEqual(["recorded-follow-up"]);
        const closed = await recorder.close();
        await recorder.stop();
        expect(closed).toMatchObject({ cases: recordedRequests.length, misses: 0 });
        expect(closed.input_sha256).toMatch(/^[0-9a-f]{64}$/);
        const bytes = readFileSync(cassettePath, "utf8");
        expect(JSON.parse(bytes).provenance.input_sha256).toBe(closed.input_sha256);
        for (const secret of ["test-key-not-real", "x-api-key", "x-session-id"]) {
            expect(bytes).not.toContain(secret);
        }
        expect(readdirSync(dir)).toEqual(["fresh.json"]);

        // Replay in a fresh session with the scripted block loaded with sentinels.
        const oracle = await replayer(cassettePath);
        h.mock.reset();
        h.mock.useCassette({ oracle, mode: "replay", namespace: NAMESPACE });
        h.mock.script([{ text: "SENTINEL", usage: USAGE }]);
        h.mock.setDefault({ text: "DEFAULT", usage: USAGE });
        const replaySession = await h.createSession();
        await h.sendPrompt(replaySession, PROMPT);
        const replayedRequests = h.mock.requests();
        expect(replayedRequests).toHaveLength(recordedRequests.length);
        const shape = (requests: typeof recordedRequests) =>
            requests.map((request) => request.body.messages?.length).sort();
        expect(shape(replayedRequests)).toEqual(shape(recordedRequests));
        const replayedTranscript = await transcript(h, replaySession);
        expect(replayedTranscript).toEqual(recordedTranscript);
        expect(replayedTranscript.calls[0]?.input).toEqual({ pattern: "*.md" });
        expect(h.mock.scriptedSelectionCount()).toBe(0);
        expect(h.mock.defaultHits()).toBe(0);
        expect(h.mock.cassetteMissLog()).toEqual([]);
        const replayedText = JSON.stringify(await h.listMessages(replaySession));
        expect(replayedText).not.toContain("SENTINEL");
        expect(replayedText).not.toContain("DEFAULT");

        // A further request has nothing recorded: one typed miss, and the run stops there.
        const missSession = await h.createSession();
        await h.sendPrompt(missSession, "Anything else.");
        const misses = h.mock.cassetteMissLog();
        expect(misses.length).toBeGreaterThanOrEqual(1);
        expect(new Set(misses.map((miss) => JSON.stringify(miss))).size).toBe(1);
        expect(misses[0]).toMatchObject({
            turn: recordedRequests.length,
            class: "ModelRequestChanged",
        });
        expect(misses[0]?.nearest_recorded).toBe(JSON.parse(bytes).cases.at(-1).request_digest);
        expect((await transcript(h, missSession)).texts).toEqual([]);
        expect(h.mock.scriptedSelectionCount()).toBe(0);
        expect(await oracle.close()).toMatchObject({
            cases: recordedRequests.length,
            misses: 1,
            unconsumed: 0,
        });
        await oracle.stop();
        // A replay never rewrites the recording.
        expect(readFileSync(cassettePath, "utf8")).toBe(bytes);

        // A replay that makes fewer requests than the recording reports them unconsumed.
        const idle = await replayer(cassettePath);
        expect(await idle.close()).toMatchObject({
            cases: recordedRequests.length,
            misses: 0,
            unconsumed: recordedRequests.length,
        });
        await idle.stop();
    });

    it("a changed tool result is a ToolResultDrift miss", async () => {
        const path = join(dir, "drift.json");
        const recorder = await CassetteOracle.start();
        await recorder.open("record", NAMESPACE, path);
        const readPrompt = "Read drift.md and summarize in one word.";
        const { resultText } = await recordToolLoop(
            h,
            recorder,
            "read",
            { filePath: driftFile() },
            readPrompt,
        );
        expect(resultText).toContain("one");
        const recordedRequests = h.mock.requests().length;
        expect(await recorder.close()).toMatchObject({ cases: recordedRequests, misses: 0 });
        await recorder.stop();

        // The same prompt with a changed file: the tool_use replays, its result differs.
        writeFileSync(driftFile(), "two\n");
        const oracle = await replayer(path);
        h.mock.reset();
        h.mock.useCassette({ oracle, mode: "replay", namespace: NAMESPACE });
        const session = await h.createSession();
        await h.sendPrompt(session, readPrompt);
        const misses = h.mock.cassetteMissLog();
        expect(misses.length).toBeGreaterThanOrEqual(1);
        expect(misses[0]?.class).toBe("ToolResultDrift");
        expect(h.mock.scriptedSelectionCount()).toBe(0);
        expect((await transcript(h, session)).calls.map((call) => call.tool)).toEqual(["read"]);
        expect((await transcript(h, session)).texts).toEqual([]);
        expect(await oracle.close()).toMatchObject({ misses: 1 });
        await oracle.stop();
        rmSync(path);
    });

    it("refuses to persist a planted credential and writes no cassette", async () => {
        const path = join(dir, "canary.json");
        const recorder = await CassetteOracle.start();
        await recorder.open("record", NAMESPACE, path);
        h.mock.reset();
        h.mock.useCassette({ oracle: recorder, mode: "record", namespace: NAMESPACE });
        h.mock.setDefault({ text: "ok", usage: USAGE });
        const session = await h.createSession();
        await h.sendPrompt(session, `My key is ${CANARY}; say ok.`);
        expect((await transcript(h, session)).texts).toEqual([]);
        const refusals = h.mock.cassetteRefusalLog();
        expect(refusals.length).toBeGreaterThanOrEqual(1);
        expect(refusals[0]).toBeInstanceOf(CassetteRefused);
        expect(refusals[0]?.kind).toBe("RedactionRefused");
        // The refusal latches: the cassette has no file form, so close refuses and nothing is written.
        const closed = await recorder.close().catch((error: unknown) => error);
        expect(closed).toBeInstanceOf(CassetteRefused);
        expect((closed as CassetteRefused).kind).toBe("RedactionRefused");
        await recorder.stop();
        expect(readdirSync(dir)).toEqual(["fresh.json"]);
        expect(readFileSync(cassettePath, "utf8")).not.toContain(CANARY);
    });
});
