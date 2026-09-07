/// <reference types="bun-types" />

import { afterEach, describe, expect, it } from "bun:test";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildHiddenAgentRegistrations } from "../../agents/hidden-agent-registrations";
import { SIDEKICK_SYSTEM_PROMPT } from "../../features/context/sidekick/agent";
import { SMART_NOTE_COMPILER_SYSTEM_PROMPT } from "../../features/context/smart-notes/compiler-prompt";
import { Database } from "../../shared/sqlite";
import { clearCtxReduceAvailability } from "./ctx-reduce-availability";
import { createSystemPromptHashHandler, isEidnaraInternalAgent } from "./system-prompt-hash";

const tempDirs: string[] = [];
const originalXdgDataHome = process.env.XDG_DATA_HOME;

/** An empty data home has no `opencode.db`, so the ctx_reduce verdict is frozen fail-open and the hash persists. */
function useTempDataHome(prefix: string): string {
    const dir = mkdtempSync(join(tmpdir(), prefix));
    tempDirs.push(dir);
    process.env.XDG_DATA_HOME = dir;
    return dir;
}

afterEach(() => {
    process.env.XDG_DATA_HOME = originalXdgDataHome;
    for (const dir of tempDirs) {
        try {
            rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
        } catch {
            /* */
        }
    }
    tempDirs.length = 0;
});

type HandlerDeps = Parameters<typeof createSystemPromptHashHandler>[0];

function buildHandler(
    opts?: Partial<HandlerDeps>,
): ReturnType<typeof createSystemPromptHashHandler> {
    return createSystemPromptHashHandler({
        resolveModel: () => ({ providerID: "provider", modelID: "default-model" }),
        historyRefreshSessions: new Set<string>(),
        systemPromptRefreshSessions: new Set<string>(),
        pendingMaterializationSessions: new Set<string>(),
        lastHeuristicsTurnId: new Map<string, string>(),
        ...opts,
    });
}

/** Runs one pass so the session holds a persisted hash that mismatches later content. */
async function seedHash(
    handler: ReturnType<typeof createSystemPromptHashHandler>["handler"],
    sessionId: string,
): Promise<void> {
    await handler({ sessionID: sessionId }, { system: ["seed prompt"] });
}

describe("system-prompt-hash drain semantics", () => {
    it("drains pre-existing systemPromptRefresh flag set by /ctx-flush", async () => {
        useTempDataHome("sph-drain-existing-");
        const sessionId = "ses-existing-flag";
        const systemPromptRefreshSessions = new Set<string>([sessionId]);
        const { handler } = buildHandler({ systemPromptRefreshSessions });

        await handler({ sessionID: sessionId }, { system: ["You are a helpful agent."] });

        expect(systemPromptRefreshSessions.has(sessionId)).toBe(false);
    });

    it("does NOT drain just-added flag from hash-change detection", async () => {
        useTempDataHome("sph-drain-just-added-");
        const sessionId = "ses-hash-change";
        const systemPromptRefreshSessions = new Set<string>();
        const historyRefreshSessions = new Set<string>();
        const pendingMaterializationSessions = new Set<string>();
        const { handler } = buildHandler({
            historyRefreshSessions,
            systemPromptRefreshSessions,
            pendingMaterializationSessions,
        });
        await seedHash(handler, sessionId);

        await handler(
            { sessionID: sessionId },
            { system: ["You are a helpful agent.", "New system content here"] },
        );

        expect(historyRefreshSessions.has(sessionId)).toBe(true);
        expect(pendingMaterializationSessions.has(sessionId)).toBe(true);
        expect(systemPromptRefreshSessions.has(sessionId)).toBe(true);
    });

    it("does NOT drain if handler short-circuits before the drain (early return)", async () => {
        useTempDataHome("sph-drain-early-return-");
        const sessionId = "ses-empty-prompt";
        const systemPromptRefreshSessions = new Set<string>([sessionId]);
        const { handler } = buildHandler({ systemPromptRefreshSessions });

        await handler({ sessionID: sessionId }, { system: [] });

        expect(systemPromptRefreshSessions.has(sessionId)).toBe(true);
    });

    it("on subsequent pass after hash-change pass, drains the surviving flag", async () => {
        useTempDataHome("sph-drain-followup-");
        const sessionId = "ses-followup";
        const systemPromptRefreshSessions = new Set<string>();
        const { handler } = buildHandler({ systemPromptRefreshSessions });
        await seedHash(handler, sessionId);

        await handler({ sessionID: sessionId }, { system: ["New prompt content"] });
        expect(systemPromptRefreshSessions.has(sessionId)).toBe(true);

        await handler({ sessionID: sessionId }, { system: ["New prompt content"] });
        expect(systemPromptRefreshSessions.has(sessionId)).toBe(false);
    });
});

describe("system-prompt-hash token estimation", () => {
    it("does not refresh systemPromptTokens when the system prompt hash is unchanged", async () => {
        useTempDataHome("sph-unchanged-token-skip-");
        const sessionId = "ses-unchanged-token-skip";
        const { handler, promptStateFor } = buildHandler();

        const firstPassSystem = ["You are a helpful coding assistant."];
        await handler({ sessionID: sessionId }, { system: firstPassSystem });

        const initialized = promptStateFor(sessionId);
        expect(initialized?.systemPromptHash).toBe(
            createHash("md5").update(firstPassSystem.join("\n")).digest("hex"),
        );
        expect(initialized?.systemPromptTokens).toBeGreaterThan(5);

        await handler({ sessionID: sessionId }, { system: [...firstPassSystem] });

        const unchanged = promptStateFor(sessionId);
        expect(unchanged).toBe(initialized);
    });
});

describe("system-prompt-hash fail-open (per-turn handler must never throw)", () => {
    it("resolves and still records the hash when isSubagentSession throws", async () => {
        useTempDataHome("sph-fail-open-");
        const sessionId = "ses-fail-open";
        const { handler, promptStateFor } = buildHandler({
            isSubagentSession: () => {
                throw new Error("subagent lookup exploded");
            },
        });

        const system = ["You are a helpful agent."];
        await expect(handler({ sessionID: sessionId }, { system })).resolves.toBeUndefined();

        expect(system).toEqual(["You are a helpful agent."]);
        expect(promptStateFor(sessionId)?.isSubagent).toBe(false);
        expect(promptStateFor(sessionId)?.systemPromptHash).toBeString();
    });

    it("clearSession drops the recorded state", async () => {
        useTempDataHome("sph-clear-session-");
        const sessionId = "ses-clear";
        const { handler, promptStateFor, clearSession } = buildHandler();
        await handler({ sessionID: sessionId }, { system: ["prompt"] });
        expect(promptStateFor(sessionId)).toBeDefined();

        clearSession(sessionId);

        expect(promptStateFor(sessionId)).toBeUndefined();
    });
});

describe("system-prompt-hash skips OpenCode internal hidden agents", () => {
    const TITLE_PROMPT_HEAD =
        "You are a title generator. You output ONLY a thread title. Nothing else.";
    const SUMMARY_PROMPT_HEAD =
        "Summarize what was done in this conversation. Write like a pull request description.";
    const COMPACTION_PROMPT_HEAD =
        "You are an anchored context summarization assistant for coding sessions.";

    for (const [label, head] of [
        ["title", TITLE_PROMPT_HEAD],
        ["summary", SUMMARY_PROMPT_HEAD],
        ["compaction", COMPACTION_PROMPT_HEAD],
    ] as const) {
        it(`skips tracking for the ${label} agent (prompt signature)`, async () => {
            useTempDataHome(`sph-skip-${label}-`);
            const sessionId = `ses-${label}`;
            const { handler, promptStateFor } = buildHandler();

            const system = [head];
            await handler({ sessionID: sessionId }, { system });

            expect(system).toEqual([head]);
            expect(promptStateFor(sessionId)).toBeUndefined();
        });
    }

    it("does NOT update systemPromptHash for internal-agent calls", async () => {
        useTempDataHome("sph-skip-no-hash-update-");
        const sessionId = "ses-no-hash-update";
        const { handler, promptStateFor } = buildHandler();
        await handler({ sessionID: sessionId }, { system: ["main agent prompt"] });
        const mainAgentHash = promptStateFor(sessionId)?.systemPromptHash;

        await handler({ sessionID: sessionId }, { system: [TITLE_PROMPT_HEAD] });

        expect(promptStateFor(sessionId)?.systemPromptHash).toBe(mainAgentHash);
    });

    it("tracks normal agents whose prompts don't match signatures", async () => {
        useTempDataHome("sph-tracks-normal-");
        const sessionId = "ses-normal";
        const { handler, promptStateFor } = buildHandler();

        const system = ["You are a helpful coding assistant."];
        await handler({ sessionID: sessionId }, { system });

        expect(system).toEqual(["You are a helpful coding assistant."]);
        expect(promptStateFor(sessionId)).toBeDefined();
    });
});

describe("system-prompt-hash skips Eidnara internal child agents", () => {
    const HISTORIAN_HEAD =
        "You are Historian — the hippocampus of a long-running coding agent. You and the primary agent are one mind.";

    for (const [label, head] of [
        ["historian", HISTORIAN_HEAD],
        ["sidekick", SIDEKICK_SYSTEM_PROMPT],
        ["smart-note-compiler", SMART_NOTE_COMPILER_SYSTEM_PROMPT],
    ] as const) {
        it(`skips tracking for the ${label} agent (prompt signature)`, async () => {
            useTempDataHome(`sph-skip-eidnara-${label}-`);
            const sessionId = `ses-eidnara-${label}`;
            const { handler, promptStateFor } = buildHandler();
            const system = [head];
            await handler({ sessionID: sessionId }, { system });
            expect(system).toEqual([head]);
            expect(promptStateFor(sessionId)).toBeUndefined();
        });
    }

    it("detects every registered hidden-agent prompt", () => {
        const registrations = buildHiddenAgentRegistrations({
            smartNoteCompilerPrompt: SMART_NOTE_COMPILER_SYSTEM_PROMPT,
            sidekickPrompt: SIDEKICK_SYSTEM_PROMPT,
        });

        for (const registration of registrations) {
            expect(registration.prompt, registration.id).toBeString();
            expect(isEidnaraInternalAgent(registration.prompt as string), registration.id).toBe(
                true,
            );
        }
    });

    it("skips tracking via the internalChildSessions flag even when the prompt has no known signature", async () => {
        useTempDataHome("sph-skip-eidnara-flag-");
        const sessionId = "ses-eidnara-flagged";
        const { handler, promptStateFor } = buildHandler({
            internalChildSessions: new Set<string>([sessionId]),
        });
        const system = ["Some custom internal prompt with no known opener."];
        await handler({ sessionID: sessionId }, { system });
        expect(system).toHaveLength(1);
        expect(promptStateFor(sessionId)).toBeUndefined();
    });

    it("does NOT update systemPromptHash for internal Eidnara child calls", async () => {
        useTempDataHome("sph-skip-eidnara-no-hash-");
        const sessionId = "ses-eidnara-no-hash";
        const { handler, promptStateFor } = buildHandler();
        await handler({ sessionID: sessionId }, { system: ["main agent prompt"] });
        const mainAgentHash = promptStateFor(sessionId)?.systemPromptHash;

        await handler({ sessionID: sessionId }, { system: [HISTORIAN_HEAD] });

        expect(promptStateFor(sessionId)?.systemPromptHash).toBe(mainAgentHash);
    });

    it("ORDER INVARIANT: an internal Eidnara child that is ALSO a subagent still skips entirely", async () => {
        useTempDataHome("sph-subagent-internal-");
        const sessionId = "ses-internal-and-subagent";
        const { handler, promptStateFor } = buildHandler({
            internalChildSessions: new Set<string>([sessionId]),
            isSubagentSession: () => true,
        });
        const system = ["Some internal Eidnara prompt."];
        await handler({ sessionID: sessionId }, { system });

        expect(system).toHaveLength(1);
        expect(promptStateFor(sessionId)).toBeUndefined();
    });

    it("records isSubagent for a tracked subagent session", async () => {
        useTempDataHome("sph-subagent-recorded-");
        const sessionId = "ses-subagent";
        const { handler, promptStateFor } = buildHandler({ isSubagentSession: () => true });
        await handler({ sessionID: sessionId }, { system: ["You are a coding subagent."] });
        expect(promptStateFor(sessionId)?.isSubagent).toBe(true);
    });
});

describe("system-prompt-hash honors per-agent opt-out", () => {
    it("skips tracking when injectionEnabled=false (global escape hatch)", async () => {
        useTempDataHome("sph-optout-disabled-");
        const sessionId = "ses-disabled";
        const { handler, promptStateFor } = buildHandler({ injectionEnabled: false });

        const system = ["You are a helpful coding assistant."];
        await handler({ sessionID: sessionId }, { system });

        expect(system).toEqual(["You are a helpful coding assistant."]);
        expect(promptStateFor(sessionId)).toBeUndefined();
    });

    it("skips tracking when an agent prompt contains a custom skip signature", async () => {
        useTempDataHome("sph-optout-skip-sig-");
        const sessionId = "ses-skipsig";
        const { handler, promptStateFor } = buildHandler({
            injectionSkipSignatures: ["<!-- eidnara: skip -->"],
        });

        const system = ["You are a read-only QA agent.\n<!-- eidnara: skip -->\nDeny all writes."];
        await handler({ sessionID: sessionId }, { system });

        expect(system).toHaveLength(1);
        expect(promptStateFor(sessionId)).toBeUndefined();
    });

    it("matches multiple skip signatures (any one match opts the agent out)", async () => {
        useTempDataHome("sph-optout-multi-sig-");
        const sessionId = "ses-multisig";
        const { handler, promptStateFor } = buildHandler({
            injectionSkipSignatures: ["<!-- eidnara: skip -->", "I AM A TINY SPECIALIZED AGENT"],
        });

        await handler(
            { sessionID: sessionId },
            { system: ["I AM A TINY SPECIALIZED AGENT — do nothing else."] },
        );

        expect(promptStateFor(sessionId)).toBeUndefined();
    });

    it("does NOT skip when skip signatures don't match the prompt", async () => {
        useTempDataHome("sph-optout-no-match-");
        const sessionId = "ses-nomatch";
        const { handler, promptStateFor } = buildHandler({
            injectionSkipSignatures: ["<!-- eidnara: skip -->"],
        });

        await handler(
            { sessionID: sessionId },
            { system: ["You are a normal agent without any skip marker."] },
        );

        expect(promptStateFor(sessionId)).toBeDefined();
    });

    it("ignores empty skip-signature strings (would otherwise match everything)", async () => {
        useTempDataHome("sph-optout-empty-sig-");
        const sessionId = "ses-emptysig";
        const { handler, promptStateFor } = buildHandler({
            injectionSkipSignatures: ["", "<!-- eidnara: skip -->"],
        });

        await handler(
            { sessionID: sessionId },
            { system: ["You are a normal agent — no skip marker here."] },
        );

        expect(promptStateFor(sessionId)).toBeDefined();
    });

    it("does NOT update systemPromptHash for opted-out calls", async () => {
        useTempDataHome("sph-optout-no-hash-update-");
        const sessionId = "ses-optout-no-hash";
        const tracking = buildHandler();
        await tracking.handler({ sessionID: sessionId }, { system: ["main agent prompt"] });
        expect(tracking.promptStateFor(sessionId)).toBeDefined();

        const optedOut = buildHandler({ injectionEnabled: false });
        await optedOut.handler({ sessionID: sessionId }, { system: ["Custom agent prompt"] });

        expect(optedOut.promptStateFor(sessionId)).toBeUndefined();
    });
});

describe("provisional ctx_reduce availability (pre-first-user race)", () => {
    function createOpenCodeDb(dataHome: string, firstUser?: { sessionId: string; tools: unknown }) {
        mkdirSync(join(dataHome, "opencode"), { recursive: true });
        const oc = new Database(join(dataHome, "opencode", "opencode.db"));
        oc.exec(
            "CREATE TABLE IF NOT EXISTS message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT)",
        );
        if (firstUser) {
            oc.prepare(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, 1, 1, ?)",
            ).run(
                "msg-first-user",
                firstUser.sessionId,
                JSON.stringify({ role: "user", tools: firstUser.tools }),
            );
        }
        oc.close();
    }

    it("does not persist a hash while the availability verdict is provisional", async () => {
        const dir = useTempDataHome("sph-provisional-");
        createOpenCodeDb(dir);
        const sessionId = "ses-provisional";
        clearCtxReduceAvailability(sessionId);
        const { handler, promptStateFor } = buildHandler();

        await handler({ sessionID: sessionId }, { system: ["Base agent prompt"] });

        expect(promptStateFor(sessionId)).toBeUndefined();
    });

    it("persists the hash from the frozen deny-verdict variant once the first user row exists", async () => {
        const dir = useTempDataHome("sph-frozen-deny-");
        const sessionId = "ses-frozen-deny";
        clearCtxReduceAvailability(sessionId);
        createOpenCodeDb(dir, { sessionId, tools: { "*": false, read: true } });
        const { handler, promptStateFor } = buildHandler();

        await handler({ sessionID: sessionId }, { system: ["Base agent prompt"] });

        expect(promptStateFor(sessionId)?.systemPromptHash).toMatch(/^[0-9a-f]{32}$/);
    });
});
