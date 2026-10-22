import { afterEach, beforeEach, describe, expect, it, spyOn } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fauxAssistantMessage } from "@earendil-works/pi-ai/compat";
import { SessionManager } from "@earendil-works/pi-coding-agent";
import { __setProjectIdentityTestHooks } from "@eidnara/opencode/features/context/project-identity";
import { HostModuleTransport } from "@eidnara/opencode/hooks/context/module-transport";
import { defaultTransformCaptureAdmission } from "@eidnara/opencode/hooks/context/transform-capture";
import { HALF_CAP_BLOCKS } from "@eidnara/opencode/hooks/context/window-cap";

import { createTestAgentSession, type TestAgentSession } from "../__tests__/agent-session";
import { textOf, userMessage } from "../__tests__/test-utils";
import eidnaraPiExtension, { __test } from "../index";
import { EIDNARA_PI_SUBAGENT_ENV } from "../subagent-runner";

type Json = Record<string, unknown>;
type Row = { id: string; message: Json };

const saved = {
    XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
    XDG_DATA_HOME: process.env.XDG_DATA_HOME,
};
let root: string;
let harness: TestAgentSession | undefined;

const CHAIN = { history_summarizer: { model: "fixture/deterministic" } };
const COMPACTION_OFF_MODES: [string, Json][] = [
    ["an empty summarizer chain", {}],
    ["compaction.enabled false", { ...CHAIN, compaction: { enabled: false } }],
];
const M0 =
    "<session-history></session-history>\n\n<project-memory>\nmem-1: Deliver it.\n</project-memory>";
const M1 = "(no new content since last materialization)";
const PI_SETTINGS = { compaction: { enabled: false, keepRecentTokens: 1 } };

function writeUserConfig(config: Json): void {
    mkdirSync(join(root, "config", "eidnara"), { recursive: true });
    writeFileSync(join(root, "config", "eidnara", "eidnara.jsonc"), JSON.stringify(config));
}

function unresolvedProject(): string {
    const project = join(root, "unresolved");
    mkdirSync(join(project, ".eidnara"), { recursive: true });
    writeFileSync(join(project, ".eidnara", "eidnara.jsonc"), "{ not json");
    return project;
}

beforeEach(() => {
    root = mkdtempSync(join(tmpdir(), "eidnara-pi-delivery-"));
    process.env.XDG_CONFIG_HOME = join(root, "config");
    process.env.XDG_DATA_HOME = join(root, "data");
    delete process.env[EIDNARA_PI_SUBAGENT_ENV];
    __test.clearPiEidnaraActive();
});

afterEach(async () => {
    await harness?.dispose();
    harness = undefined;
    __test.clearPiEidnaraActive();
    __setProjectIdentityTestHooks({});
    for (const [key, value] of Object.entries(saved)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
    rmSync(root, { recursive: true, force: true });
});

function rowsOf(body: Json): Row[] {
    return body.native_messages as Row[];
}

function additive(body: Json, boundary: Json | null = null, m0 = M0): Json {
    const synthetic = (id: string, text: string) => ({
        id: `eidnara:synthetic:${id}`,
        message: { role: "user", content: [{ type: "text", text }], timestamp: 0 },
    });
    return {
        status: "ok",
        action: "HARD",
        boundary,
        base_revision: body.base_revision,
        output_revision: `out-${String(body.base_revision)}`,
        operations: [
            { op: "insert", values: [synthetic("m0", m0), synthetic("m1", M1)] },
            { op: "keep", source: "input", start: 0, count: rowsOf(body).length },
        ],
    };
}

function keepWindow(body: Json): Json {
    return {
        status: "ok",
        action: "SOFT",
        boundary: null,
        base_revision: body.base_revision,
        output_revision: `out-${String(body.base_revision)}`,
        operations: [{ op: "keep", source: "input", start: 0, count: rowsOf(body).length }],
    };
}

function daemon(transform: (body: Json) => unknown) {
    const transforms: Json[] = [];
    const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(async (input) => {
        const body = input.body as Json;
        if (input.method === "transform.boundary") return { anchors: [] };
        if (input.method === "transform") {
            transforms.push(body);
            return transform(body);
        }
        return { state: "accepted" };
    });
    return { call, transforms };
}

async function disposeHarness(): Promise<void> {
    await harness?.dispose();
    harness = undefined;
    __test.clearPiEidnaraActive();
}

function served(request: { messages: unknown[] } | undefined): string[] {
    return (request?.messages ?? []).map((message) => textOf(message as never));
}

function messageEntryIds(h: TestAgentSession): string[] {
    return h.sessionManager
        .getEntries()
        .filter((entry) => entry.type === "message")
        .map((entry) => entry.id);
}

function compactions(h: TestAgentSession) {
    return h.sessionManager.getEntries().filter((entry) => entry.type === "compaction");
}

function noSyntheticRow(body: Json): boolean {
    return rowsOf(body).every((row) => !row.id.startsWith("eidnara:synthetic:"));
}

function daemonError(code: string): () => never {
    return () => {
        throw Object.assign(new Error(`transform refused: ${code}`), { code });
    };
}

function preloaded(cwd: string, rows: number, compacted = false): SessionManager {
    const manager = SessionManager.inMemory(cwd);
    const ids: string[] = [];
    for (let n = 0; n < rows; n += 1)
        ids.push(manager.appendMessage(userMessage(`row ${n}`, 1_000 + n)));
    if (compacted && ids[0]) manager.appendCompaction("pi summary", ids[0], 100);
    return manager;
}

const TAGGED = "\u00a73\u00a7 tagged answer";

async function taggedTurns(config: Json) {
    writeUserConfig(config);
    const { call, transforms } = daemon((body) =>
        additive(body, { mid: rowsOf(body).at(-1)?.id, sequence: 1 }),
    );
    harness = await createTestAgentSession({
        cwd: root,
        extensionFactories: [eidnaraPiExtension],
        settings: PI_SETTINGS,
    });
    for (const turn of [1, 2, 3]) {
        harness.respond([fauxAssistantMessage(TAGGED)]);
        await harness.session.prompt(`turn ${turn} ${"x".repeat(400)}`);
        await Bun.sleep(20);
    }
    return { h: harness, call, transforms };
}

function assistantTexts(h: TestAgentSession): string[] {
    return h.sessionManager
        .getEntries()
        .filter((entry) => entry.type === "message" && entry.message.role === "assistant")
        .map((entry) => textOf((entry as { message: never }).message));
}

describe("Pi context handler in compaction-off mode", () => {
    it.each(
        COMPACTION_OFF_MODES,
    )("serves m0, m1, then every Pi message on each context event under %s", async (_, config) => {
        writeUserConfig(config);
        const { call, transforms } = daemon((body) => additive(body));
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
            });
            for (const turn of ["first", "second", "third"]) await harness.session.prompt(turn);

            expect(transforms).toHaveLength(3);
            expect(harness.requests).toHaveLength(3);
            transforms.forEach((sent, turn) => {
                expect(sent.serializer_profile).toBe("pi");
                expect(sent.tool_present).toBe(false);
                expect(sent.todo_tool_present).toBe(true);
                expect(noSyntheticRow(sent)).toBe(true);
                const request = harness?.requests[turn];
                expect(served(request).slice(0, 2)).toEqual([M0, M1]);
                expect(request?.messages.slice(2)).toEqual(rowsOf(sent).map((row) => row.message));
            });
            expect(rowsOf(transforms[2] ?? {}).map((row) => row.id)).toEqual(
                messageEntryIds(harness).slice(0, 5),
            );
        } finally {
            call.mockRestore();
        }
    });

    it.each(
        COMPACTION_OFF_MODES,
    )("submits no pass for a home-directory session without allow_home_project under %s", async (_, config) => {
        writeUserConfig(config);
        __setProjectIdentityTestHooks({ homeDirectory: () => root });
        const { call, transforms } = daemon((body) => additive(body));
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
            });
            for (const turn of ["first", "second"]) await harness.session.prompt(turn);

            expect(transforms).toHaveLength(0);
            expect(served(harness.requests[0])).toEqual(["first"]);
            expect(served(harness.requests[1])).toEqual(["first", "ok", "second"]);
        } finally {
            call.mockRestore();
        }
    });

    it("carries the same pass fields for sessions the daemon binds differently", async () => {
        const firstPass = async (cwd: string): Promise<Json> => {
            const { call, transforms } = daemon(keepWindow);
            try {
                harness = await createTestAgentSession({
                    cwd,
                    extensionFactories: [eidnaraPiExtension],
                });
                await harness.session.prompt("question");
                return transforms[0] ?? {};
            } finally {
                call.mockRestore();
                await disposeHarness();
            }
        };
        const unresolved = unresolvedProject();

        writeUserConfig(CHAIN);
        const foldingProject = await firstPass(root);
        const withdrawnProject = await firstPass(unresolved);
        writeUserConfig({});
        const compactionOff = await firstPass(root);

        const perSession = new Set([
            "session_id",
            "base_revision",
            "native_messages",
            "request_observed_at_ms",
            "system_prompt_hash",
            "render_config",
            "tool_present",
        ]);
        const shared = (sent: Json) =>
            Object.fromEntries(Object.entries(sent).filter(([key]) => !perSession.has(key)));
        expect(Object.keys(withdrawnProject).sort()).toEqual(Object.keys(foldingProject).sort());
        expect(Object.keys(compactionOff).sort()).toEqual(Object.keys(foldingProject).sort());
        expect(shared(withdrawnProject)).toEqual(shared(foldingProject));
        expect(shared(compactionOff)).toEqual(shared(foldingProject));
        const capabilities = (sent: Json) => [sent.tool_present, sent.todo_tool_present];
        expect(capabilities(foldingProject)).toEqual([true, true]);
        expect(capabilities(withdrawnProject)).toEqual([true, true]);
        expect(capabilities(compactionOff)).toEqual([false, true]);
    });

    it("evicts and strips assistant tags in a compaction-on process", async () => {
        const { h, call } = await taggedTurns(CHAIN);
        try {
            expect(compactions(h).length).toBeGreaterThan(0);
            expect(assistantTexts(h)).toEqual(["tagged answer", "tagged answer", "tagged answer"]);
        } finally {
            call.mockRestore();
        }
    });

    it("leaves compaction, eviction, and assistant tags to Pi after additive passes", async () => {
        const { h, call, transforms } = await taggedTurns({});
        try {
            expect(compactions(h)).toHaveLength(0);
            expect(assistantTexts(h)).toEqual([TAGGED, TAGGED, TAGGED]);
            expect(h.session.getAllTools().map((tool) => tool.name)).not.toContain(
                "eidnara_reduce",
            );

            const outcome = await h.session.compact().then(
                () => "compacted",
                (error: Error) => error.message,
            );
            expect(outcome).toBe("compacted");
            const [native] = compactions(h);
            expect(native).toMatchObject({ fromHook: false });
            await h.session.prompt("after compaction");

            const rows = rowsOf(transforms.at(-1) ?? {});
            expect(rows[0]?.id).toBe(`eidnara:compactionSummary:${native?.id}`);
            expect(rows.slice(1).map((row) => row.message)).toEqual(
                h.session.messages.slice(1, -1),
            );
            const request = h.requests.at(-1);
            expect(served(request).slice(0, 2)).toEqual([M0, M1]);
            expect(served(request)[2]).toContain(
                (native as { summary: string } | undefined)?.summary ?? "",
            );
            expect(transforms.every(noSyntheticRow)).toBe(true);
        } finally {
            call.mockRestore();
        }
    });

    it("schedules no eviction in a compaction-off process whose session runs in a folding project", async () => {
        const bootDir = process.cwd();
        process.chdir(unresolvedProject());
        try {
            const { h, call, transforms } = await taggedTurns(CHAIN);
            try {
                expect(transforms.every((sent) => sent.tool_present === false)).toBe(true);
                expect(compactions(h)).toHaveLength(0);
                expect(assistantTexts(h)).toEqual([TAGGED, TAGGED, TAGGED]);
            } finally {
                call.mockRestore();
            }
        } finally {
            process.chdir(bootDir);
        }
    });

    it.each([
        ["", false],
        [" behind Pi's compaction summary", true],
    ])("submits the whole array past half the cap%s while the project withdraws fold authority", async (_, compacted) => {
        const unresolved = unresolvedProject();
        const preloadedRows = HALF_CAP_BLOCKS;
        const lead = compacted ? 1 : 0;
        const passes = async (config: Json, cwd: string): Promise<Row[][]> => {
            writeUserConfig(config);
            const { call, transforms } = daemon(keepWindow);
            try {
                harness = await createTestAgentSession({
                    cwd,
                    extensionFactories: [eidnaraPiExtension],
                    sessionManager: preloaded(cwd, preloadedRows, compacted),
                });
                await harness.session.prompt("now");
                await harness.session.prompt("again");
                return transforms.map(rowsOf);
            } finally {
                call.mockRestore();
                await disposeHarness();
            }
        };
        const leadsWithSummary = (rows: Row[] | undefined) =>
            rows?.[0]?.id.startsWith("eidnara:compactionSummary:") === true;

        const wholeArrays = [lead + preloadedRows + 1, lead + preloadedRows + 3];
        for (const [arm, config, cwd] of [
            ["compaction-off", {}, root],
            ["an unresolved project in a compaction-on process", CHAIN, unresolved],
        ] as const) {
            const sent = await passes(config, cwd);
            expect(
                sent.map((rows) => rows.length),
                arm,
            ).toEqual(wholeArrays);
            expect(sent.every(leadsWithSummary), arm).toBe(compacted);
            expect(sent[0]?.[lead]?.message.content, arm).toBe("row 0");
        }

        const folding = await passes(CHAIN, root);
        expect(folding[0]).toHaveLength(HALF_CAP_BLOCKS);
        expect(folding.every(leadsWithSummary)).toBe(compacted);
        expect(folding[0]?.[lead]?.message.content).toBe(`row ${1 + lead}`);
        expect(folding[1]?.[lead]?.id).toBe(folding[0]?.[lead]?.id ?? "");
        expect(folding[1]).toHaveLength(HALF_CAP_BLOCKS + 2);
    });

    it("submits passes when the boot project's configuration is unresolved", async () => {
        writeUserConfig(CHAIN);
        const unresolved = unresolvedProject();
        const bootDir = process.cwd();
        const { call, transforms } = daemon(keepWindow);
        try {
            process.chdir(unresolved);
            harness = await createTestAgentSession({
                cwd: unresolved,
                extensionFactories: [eidnaraPiExtension],
                settings: PI_SETTINGS,
            });
            for (const turn of [1, 2, 3])
                await harness.session.prompt(`turn ${turn} ${"x".repeat(400)}`);
            expect(transforms).toHaveLength(3);
            expect(transforms.every((sent) => sent.tool_present === false)).toBe(true);
            const outcome = await harness.session.compact().then(
                () => "compacted",
                (error: Error) => error.message,
            );
            expect(outcome).toBe("compacted");
        } finally {
            process.chdir(bootDir);
            call.mockRestore();
        }
    });

    it.each([
        ["invalid_params", daemonError("invalid_params"), 200_000],
        ["queue_full", daemonError("queue_full"), 200_000],
        [
            "an invocation budget refusal",
            (body: Json) => additive(body, null, "m".repeat(400_000)),
            16_000,
        ],
    ] as const)("leaves Pi's array unchanged when %s declines the pass", async (_, decline, contextWindow) => {
        writeUserConfig({});
        let failing = true;
        const { call, transforms } = daemon((body) => (failing ? decline(body) : additive(body)));
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                contextWindow,
            });
            await harness.session.prompt("first");
            failing = false;
            await harness.session.prompt("second");
            failing = true;
            await harness.session.prompt("third");

            expect(transforms).toHaveLength(3);
            expect(served(harness.requests[0])).toEqual(["first"]);
            expect(served(harness.requests[1])).toEqual([M0, M1, "first", "ok", "second"]);
            expect(served(harness.requests[2])).toEqual(["first", "ok", "second", "ok", "third"]);
        } finally {
            call.mockRestore();
        }
    });

    it("leaves Pi's array unchanged when the capture admission refuses a pass", async () => {
        writeUserConfig({});
        const { call, transforms } = daemon((body) => additive(body));
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
            });
            const h = harness;
            const refused = async (prompt: string) => {
                const held = defaultTransformCaptureAdmission.admit(
                    h.sessionManager.getSessionId(),
                );
                if (!("lease" in held)) throw new Error("the session's pass lease is free");
                try {
                    await h.session.prompt(prompt);
                } finally {
                    held.lease.release();
                }
            };
            await refused("first");
            await h.session.prompt("second");
            await refused("third");

            expect(transforms).toHaveLength(1);
            expect(served(h.requests[0])).toEqual(["first"]);
            expect(served(h.requests[1])).toEqual([M0, M1, "first", "ok", "second"]);
            expect(served(h.requests[2])).toEqual(["first", "ok", "second", "ok", "third"]);
        } finally {
            call.mockRestore();
        }
    });
});
