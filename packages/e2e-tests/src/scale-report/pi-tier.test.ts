import { describe, expect, it } from "bun:test";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { PI_PLUGIN_ROOT } from "../pi-runner/spawn";
import { retryPosition, writePiTier } from "./pi-tier";

const piModule = Bun.resolveSync("@earendil-works/pi-coding-agent", PI_PLUGIN_ROOT);

function digest(path: string): string {
    return createHash("sha256").update(readFileSync(path)).digest("hex");
}

describe("Pi session tiers", () => {
    it("writes identical bytes twice for one seed and other bytes for another", () => {
        const dir = mkdtempSync(join(tmpdir(), "eidnara-pi-tier-"));
        try {
            const options = { messages: 10_000, window: 300, seed: 850, cwd: "/project" };
            writePiTier(join(dir, "a.jsonl"), options);
            writePiTier(join(dir, "b.jsonl"), options);
            writePiTier(join(dir, "c.jsonl"), { ...options, seed: 851 });
            expect(digest(join(dir, "a.jsonl"))).toBe(digest(join(dir, "b.jsonl")));
            expect(digest(join(dir, "a.jsonl"))).not.toBe(digest(join(dir, "c.jsonl")));
        } finally {
            rmSync(dir, { recursive: true, force: true });
        }
    });

    it("loads in Pi with every shape, the failed entry, and its retry on one branch", async () => {
        const { SessionManager } = await import(piModule);
        const dir = mkdtempSync(join(tmpdir(), "eidnara-pi-tier-"));
        try {
            const path = join(dir, "tier.jsonl");
            const messages = 10_000;
            const sessionId = writePiTier(path, { messages, window: 300, seed: 850, cwd: dir });
            const manager = SessionManager.open(path, undefined, dir);
            expect(manager.getSessionId()).toBe(sessionId);
            const branch = manager.getBranch() as { id: string; parentId: string | null }[];
            expect(branch).toHaveLength(messages + 1);
            expect(branch.at(-1)?.id).toBe(`m${messages}`);
            const context = manager.buildSessionContext().messages as {
                role: string;
                stopReason?: string;
                content?: { type: string }[];
            }[];
            expect(new Set(context.map((message) => message.role))).toEqual(
                new Set(["user", "assistant", "toolResult", "bashExecution"]),
            );
            const retry = retryPosition(messages, 300);
            const failed = branch.findIndex((entry) => entry.id === "failed-1");
            expect(branch[failed + 1]?.id).toBe(`m${retry}`);
            expect(context[failed]?.stopReason).toBe("error");
            expect(context[failed - 1]?.role).toBe("toolResult");
            expect(context[failed + 1]?.role).toBe("assistant");
            expect(
                context.some((message) =>
                    message.content?.some((part) => part.type === "toolCall"),
                ),
            ).toBe(true);
        } finally {
            rmSync(dir, { recursive: true, force: true });
        }
    });
});
