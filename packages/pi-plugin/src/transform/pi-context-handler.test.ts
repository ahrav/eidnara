import { afterEach, beforeEach, describe, expect, it, spyOn } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HostModuleTransport } from "@eidnara/opencode/hooks/context/module-transport";

import { createTestAgentSession, type TestAgentSession } from "../__tests__/agent-session";
import eidnaraPiExtension, { __test } from "../index";
import { EIDNARA_PI_SUBAGENT_ENV } from "../subagent-runner";

type Json = Record<string, unknown>;

const saved = {
    XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
    XDG_DATA_HOME: process.env.XDG_DATA_HOME,
};
let root: string;
let harness: TestAgentSession | undefined;

beforeEach(() => {
    root = mkdtempSync(join(tmpdir(), "eidnara-pi-context-"));
    process.env.XDG_CONFIG_HOME = join(root, "config");
    process.env.XDG_DATA_HOME = join(root, "data");
    // A summarizer chain in user configuration makes Eidnara own compaction.
    mkdirSync(join(root, "config", "eidnara"), { recursive: true });
    writeFileSync(
        join(root, "config", "eidnara", "eidnara.jsonc"),
        JSON.stringify({ history_summarizer: { model: "fixture/deterministic" } }),
    );
    delete process.env[EIDNARA_PI_SUBAGENT_ENV];
    __test.clearPiEidnaraActive();
});

afterEach(async () => {
    await harness?.dispose();
    harness = undefined;
    __test.clearPiEidnaraActive();
    for (const [key, value] of Object.entries(saved)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
    rmSync(root, { recursive: true, force: true });
});

function textOf(message: unknown): string {
    const content = (message as { content?: unknown }).content;
    if (typeof content === "string") return content;
    return (content as { text?: string }[]).map((part) => part.text ?? "").join("");
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

describe("Pi context handler under createAgentSession", () => {
    it("serves the applied values to the provider and names rows by entry ids", async () => {
        const { call, transforms } = daemon((body) => ({
            status: "ok",
            action: "HARD",
            boundary: null,
            base_revision: body.base_revision,
            output_revision: "out-1",
            operations: [
                {
                    op: "insert",
                    values: [
                        {
                            id: "eidnara:synthetic:m0",
                            message: { role: "user", content: "folded history", timestamp: 0 },
                        },
                    ],
                },
                {
                    op: "keep",
                    source: "input",
                    start: 0,
                    count: (body.native_messages as unknown[]).length,
                },
            ],
        }));
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
            });
            await harness.session.prompt("first question");
            const served = harness.requests[0]?.messages ?? [];
            expect(served.map(textOf)).toEqual(["folded history", "first question"]);
            const sent = transforms[0] ?? {};
            expect(sent.serializer_profile).toBe("pi");
            const userEntry = harness.sessionManager
                .getEntries()
                .find((entry) => entry.type === "message");
            expect((sent.native_messages as { id: string }[]).map((row) => row.id)).toEqual([
                userEntry?.id as string,
            ]);
        } finally {
            call.mockRestore();
        }
    });

    it("leaves Pi's array unchanged when the daemon declines", async () => {
        const { call } = daemon(() => {
            throw new Error("daemon unavailable");
        });
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
            });
            await harness.session.prompt("plain question");
            expect((harness.requests[0]?.messages ?? []).map(textOf)).toEqual(["plain question"]);
        } finally {
            call.mockRestore();
        }
    });
});
