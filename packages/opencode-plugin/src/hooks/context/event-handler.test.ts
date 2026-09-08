/// <reference types="bun-types" />

import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
    generateMessageId,
    injectCompactionMarker,
} from "../../features/context/compaction-marker";
import {
    applyStickySnapshotCache,
    resetSidebarSnapshotCache,
} from "../../plugin/sidebar-snapshot-cache";
import { _resetHarnessForTesting, setHarness } from "../../shared/harness";
import type { SidebarSnapshot } from "../../shared/rpc-types";
import { Database } from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";
import { closeCompactionMarkerConnection, MARKER_SUMMARY_TEXT } from "./compaction-marker-manager";
import { type ContextUsageEntry, createEventHandler, type EventHandlerDeps } from "./event-handler";
import { DEFAULT_CONTEXT_LIMIT, resolveContextLimit } from "./event-resolvers";
import type { RawMessage } from "./read-session-raw";
import { buildTrueRawTokenIndex } from "./read-session-true-raw-tokens";

const SESSION = "ses-1";
const RETAINED_ID = generateMessageId(1_002, 0n, "retained");

const ZERO_SNAPSHOT: SidebarSnapshot = {
    sessionId: SESSION,
    usagePercentage: 0,
    inputTokens: 0,
    contextLimit: 0,
    systemPromptTokens: 0,
    compartmentCount: 0,
    memoryCount: 0,
    memoryBlockCount: 0,
    pendingOpsCount: 0,
    historianRunning: false,
    compartmentInProgress: false,
    sessionNoteCount: 0,
    readySmartNoteCount: 0,
    cacheTtl: "5m",
    lastTransformError: null,
    lastDreamerRunAt: null,
    projectIdentity: null,
    compartmentTokens: 0,
    factTokens: 0,
    memoryTokens: 0,
    docsTokens: 0,
    profileTokens: 0,
    conversationTokens: 0,
    toolCallTokens: 0,
    toolDefinitionTokens: 0,
    executeThreshold: 65,
    executeThresholdClamped: false,
    newWorkTokens: 0,
    totalInputTokens: 0,
    recompProgress: null,
    memoryState: null,
    compaction_enabled: true,
};

const originalXdgDataHome = process.env.XDG_DATA_HOME;
let dataHome: string;

function openCodeDb(): Database {
    return new Database(join(dataHome, "opencode", "opencode.db"));
}

function insertMessage(db: Database, id: string, role: string, timeCreated: number): void {
    db.prepare(
        "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
    ).run(id, SESSION, timeCreated, timeCreated, JSON.stringify({ role }));
}

function rowCounts(): { messages: number; parts: number } {
    const db = openCodeDb();
    try {
        const messages = db.prepare("SELECT COUNT(*) AS n FROM message").get() as { n: number };
        const parts = db.prepare("SELECT COUNT(*) AS n FROM part").get() as { n: number };
        return { messages: messages.n, parts: parts.n };
    } finally {
        closeQuietly(db);
    }
}

function injectPluginMarker(): void {
    const injected = injectCompactionMarker({
        sessionId: SESSION,
        endOrdinal: 2,
        endMessageId: RETAINED_ID,
        summaryText: MARKER_SUMMARY_TEXT,
        directory: dataHome,
    });
    expect(injected).not.toBeNull();
    expect(rowCounts()).toEqual({ messages: 3, parts: 2 });
}

interface Harness {
    deps: EventHandlerDeps;
    calls: Record<"cache" | "wire" | "deleted", string[]>;
    handle: (type: string, properties?: unknown) => Promise<void>;
}

function buildHarness(): Harness {
    const calls = { cache: [] as string[], wire: [] as string[], deleted: [] as string[] };
    const deps: EventHandlerDeps = {
        contextUsageMap: new Map<string, ContextUsageEntry>(),
        internalChildSessions: new Set<string>(),
        subagentSessions: new Set<string>(),
        onSessionCacheInvalidated: (id) => calls.cache.push(id),
        onRustWireInvalidated: (id) => calls.wire.push(id),
        onSessionDeleted: (id) => calls.deleted.push(id),
    };
    const handler = createEventHandler(deps);
    return {
        deps,
        calls,
        handle: (type, properties) => handler({ event: { type, properties } }),
    };
}

function sessionCreated(id: string, parentID: string, title?: string): unknown {
    return { info: { id, parentID, title } };
}

function assistantUpdated(tokens: { input?: number; cache?: { read?: number; write?: number } }) {
    return {
        info: {
            role: "assistant",
            id: "msg-1",
            sessionID: SESSION,
            providerID: "no-such-provider",
            modelID: "no-such-model",
            finish: "stop",
            tokens,
        },
    };
}

beforeEach(() => {
    dataHome = mkdtempSync(join(tmpdir(), "event-handler-"));
    process.env.XDG_DATA_HOME = dataHome;
    mkdirSync(join(dataHome, "opencode"), { recursive: true });
    const db = openCodeDb();
    db.exec("PRAGMA journal_mode=WAL");
    db.exec(
        "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT)",
    );
    db.exec(
        "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT)",
    );
    insertMessage(db, generateMessageId(1_000, 0n, "boundary"), "user", 1_000);
    insertMessage(db, RETAINED_ID, "assistant", 1_002);
    closeQuietly(db);
    setHarness("opencode");
});

afterEach(() => {
    closeCompactionMarkerConnection();
    _resetHarnessForTesting();
    resetSidebarSnapshotCache();
    process.env.XDG_DATA_HOME = originalXdgDataHome;
    rmSync(dataHome, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
});

describe("createEventHandler — session.created", () => {
    it("adds an eidnara- titled child to internalChildSessions and subagentSessions", async () => {
        const { deps, handle } = buildHarness();
        await handle("session.created", sessionCreated("child-1", "parent-1", "eidnara-sidekick"));

        expect(deps.internalChildSessions?.has("child-1")).toBe(true);
        expect(deps.subagentSessions?.has("child-1")).toBe(true);
    });

    it("adds a plainly titled child to subagentSessions only", async () => {
        const { deps, handle } = buildHarness();
        await handle("session.created", sessionCreated("child-2", "parent-1", "Research task"));

        expect(deps.subagentSessions?.has("child-2")).toBe(true);
        expect(deps.internalChildSessions?.has("child-2")).toBe(false);
    });

    it("tracks nothing for a root session, even with an eidnara- title", async () => {
        const { deps, handle } = buildHarness();
        await handle("session.created", sessionCreated("root-1", "", "eidnara-root"));

        expect(deps.subagentSessions?.size).toBe(0);
        expect(deps.internalChildSessions?.size).toBe(0);
    });

    it("keeps only the newest 1000 children in each set", async () => {
        const { deps, handle } = buildHarness();
        await handle("session.created", sessionCreated("child-0", "parent-1", "eidnara-0"));
        for (let i = 1; i <= 1000; i++) {
            await handle(
                "session.created",
                sessionCreated(`child-${i}`, "parent-1", `eidnara-${i}`),
            );
        }

        expect(deps.subagentSessions?.size).toBe(1000);
        expect(deps.internalChildSessions?.size).toBe(1000);
        expect(deps.subagentSessions?.has("child-0")).toBe(false);
        expect(deps.internalChildSessions?.has("child-0")).toBe(false);
        expect(deps.subagentSessions?.has("child-1")).toBe(true);
        expect(deps.subagentSessions?.has("child-1000")).toBe(true);
    });
});

describe("createEventHandler — message.updated", () => {
    it("drops a user message's cached token estimate so a same-length edit is re-counted", async () => {
        const { handle } = buildHarness();
        const before = "hello hello hello hello hello hello hello hello";
        const after = "h3ll0 w0rld xyzq !!@@ ##$$ %%^^ &&** (())[]{}<>";
        expect(after.length).toBe(before.length);
        const options = {
            providerShapeVersion: "opencode-v1" as const,
            cacheNamespace: `${SESSION}:event-handler-test`,
        };
        const message = (text: string): RawMessage => ({
            id: "msg-user-1",
            role: "user",
            parts: [{ type: "text", text }],
            ordinal: 1,
        });

        const first = buildTrueRawTokenIndex(SESSION, [message(before)], options).tokenForOrdinal(
            1,
        );
        const recounted = buildTrueRawTokenIndex(SESSION, [message(after)], options);
        // The cache key fingerprints part type and byte length only, so the edit is invisible to it.
        expect(recounted.tokenForOrdinal(1)).toBe(first);

        await handle("message.updated", {
            info: { role: "user", id: "msg-user-1", sessionID: SESSION },
        });

        const fresh = buildTrueRawTokenIndex(SESSION, [message(after)], options).tokenForOrdinal(1);
        expect(fresh).not.toBe(first);
    });

    it("records inputTokens and a percentage against the resolved context limit", async () => {
        const { deps, handle } = buildHarness();
        await handle(
            "message.updated",
            assistantUpdated({ input: 1_000, cache: { read: 30_000, write: 1_000 } }),
        );

        const entry = deps.contextUsageMap.get(SESSION);
        expect(entry).toBeDefined();
        expect(entry?.usage.inputTokens).toBe(32_000);
        expect(entry?.hasUsageTokens).toBe(true);
        const limit = resolveContextLimit("no-such-provider", "no-such-model");
        expect(limit).toBe(DEFAULT_CONTEXT_LIMIT);
        expect(entry?.usage.percentage).toBeCloseTo((32_000 / limit) * 100, 6);
    });

    it("records nothing when the update carries no usage tokens", async () => {
        const { deps, handle } = buildHarness();
        await handle("message.updated", assistantUpdated({ input: 0 }));

        expect(deps.contextUsageMap.has(SESSION)).toBe(false);
    });

    it("keeps the newest response's usage when an older response is updated", async () => {
        const { deps, handle } = buildHarness();
        const updated = assistantUpdated({ input: 40_000 });
        updated.info.id = "msg-9";
        await handle("message.updated", updated);

        const older = assistantUpdated({ input: 5_000 });
        older.info.id = "msg-3";
        await handle("message.updated", older);
        expect(deps.contextUsageMap.get(SESSION)?.usage.inputTokens).toBe(40_000);
        expect(deps.contextUsageMap.get(SESSION)?.messageID).toBe("msg-9");

        const newer = assistantUpdated({ input: 41_000 });
        newer.info.id = "msg-9";
        await handle("message.updated", newer);
        expect(deps.contextUsageMap.get(SESSION)?.usage.inputTokens).toBe(41_000);
    });

    it("evicts usage entries older than the TTL on the next event", async () => {
        const { deps, handle } = buildHarness();
        const twoHoursAgo = Date.now() - 2 * 60 * 60 * 1000;
        deps.contextUsageMap.set("stale", {
            usage: { percentage: 1, inputTokens: 1 },
            updatedAt: twoHoursAgo,
        });
        deps.contextUsageMap.set("fresh", {
            usage: { percentage: 1, inputTokens: 1 },
            updatedAt: Date.now(),
        });

        await handle("session.error", { sessionID: "other", error: { message: "nope" } });

        expect(deps.contextUsageMap.has("stale")).toBe(false);
        expect(deps.contextUsageMap.has("fresh")).toBe(true);
    });
});

describe("createEventHandler — message.removed", () => {
    it("invalidates the wire and session caches and removes the plugin marker", async () => {
        injectPluginMarker();
        const { calls, handle } = buildHarness();

        await handle("message.removed", { sessionID: SESSION, messageID: RETAINED_ID });

        expect(calls.wire).toEqual([SESSION]);
        expect(calls.cache).toEqual([SESSION]);
        expect(rowCounts()).toEqual({ messages: 2, parts: 0 });
    });

    it("drops the live usage and sticky snapshot only when the response they came from is removed", async () => {
        const { deps, handle } = buildHarness();
        await handle("message.updated", assistantUpdated({ input: 1_000 }));
        expect(deps.contextUsageMap.get(SESSION)?.messageID).toBe("msg-1");
        const scope = { sessionId: SESSION, directory: "/repo", modelKey: "p/m" };
        applyStickySnapshotCache(scope, { ...ZERO_SNAPSHOT, inputTokens: 1_000 });

        // Removing some other message, such as a plugin notification row, leaves both intact.
        await handle("message.removed", { sessionID: SESSION, messageID: "msg-other" });
        expect(deps.contextUsageMap.get(SESSION)?.usage.inputTokens).toBe(1_000);
        expect(
            applyStickySnapshotCache(scope, { ...ZERO_SNAPSHOT, compartmentInProgress: true })
                .inputTokens,
        ).toBe(1_000);

        await handle("message.removed", { sessionID: SESSION, messageID: "msg-1" });
        expect(deps.contextUsageMap.has(SESSION)).toBe(false);
        expect(
            applyStickySnapshotCache(scope, { ...ZERO_SNAPSHOT, compartmentInProgress: true })
                .inputTokens,
        ).toBe(0);
    });
});

describe("createEventHandler — session.compacted", () => {
    it("removes the plugin marker and invalidates the session cache", async () => {
        injectPluginMarker();
        const { calls, handle } = buildHarness();

        await handle("session.compacted", { sessionID: SESSION });

        expect(calls.cache).toEqual([SESSION]);
        expect(rowCounts()).toEqual({ messages: 2, parts: 0 });
    });

    it("drops the pre-compaction live usage and sticky snapshot", async () => {
        const { deps, handle } = buildHarness();
        await handle("message.updated", assistantUpdated({ input: 90_000 }));
        const scope = { sessionId: SESSION, directory: "/repo", modelKey: "p/m" };
        applyStickySnapshotCache(scope, { ...ZERO_SNAPSHOT, inputTokens: 90_000 });

        await handle("session.compacted", { sessionID: SESSION });

        expect(deps.contextUsageMap.has(SESSION)).toBe(false);
        expect(
            applyStickySnapshotCache(scope, { ...ZERO_SNAPSHOT, compartmentInProgress: true })
                .inputTokens,
        ).toBe(0);
    });
});

describe("createEventHandler — session.deleted", () => {
    it("removes the marker, fires the callbacks, and drops in-memory session state", async () => {
        injectPluginMarker();
        const { deps, calls, handle } = buildHarness();
        deps.contextUsageMap.set(SESSION, {
            usage: { percentage: 10, inputTokens: 100 },
            updatedAt: Date.now(),
        });
        deps.subagentSessions?.add(SESSION);
        deps.internalChildSessions?.add(SESSION);

        await handle("session.deleted", { info: { id: SESSION } });

        expect(calls.deleted).toEqual([SESSION]);
        expect(calls.cache).toEqual([SESSION]);
        expect(deps.contextUsageMap.has(SESSION)).toBe(false);
        expect(deps.subagentSessions?.has(SESSION)).toBe(false);
        expect(deps.internalChildSessions?.has(SESSION)).toBe(false);
        expect(rowCounts()).toEqual({ messages: 2, parts: 0 });
    });
});

describe("createEventHandler — persistence footprint", () => {
    it("writes no context.db under the data home across every event", async () => {
        const { handle } = buildHarness();
        await handle("session.created", sessionCreated("child-1", "parent-1", "eidnara-x"));
        await handle("session.error", { sessionID: SESSION, error: { message: "boom" } });
        await handle("message.updated", assistantUpdated({ input: 10 }));
        await handle("message.removed", { sessionID: SESSION, messageID: RETAINED_ID });
        await handle("session.compacted", { sessionID: SESSION });
        await handle("session.deleted", { info: { id: SESSION } });

        expect(existsSync(join(dataHome, "eidnara", "context.db"))).toBe(false);
        expect(existsSync(join(dataHome, "opencode", "context.db"))).toBe(false);
    });
});
