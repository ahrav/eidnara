import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { readdirSync } from "node:fs";
import { PiTestHarness } from "../src/pi-harness";
import { detectPiPrereqs } from "../src/pi-runner/spawn";

const piPrereqs = detectPiPrereqs();

const STORAGE_FILE = /\.(db|sqlite|sqlite3)$/;

function listRecursive(dir: string): string[] {
    return readdirSync(dir, { recursive: true, encoding: "utf8" }).sort();
}

describe.skipIf(!piPrereqs.ok)("pi smoke", () => {
    let h: PiTestHarness;

    beforeAll(async () => {
        h = await PiTestHarness.create();
    });

    afterAll(async () => {
        await h?.dispose();
    });

    it("loads the built extension, completes one mock turn, and leaves no storage file", async () => {
        h.mock.reset();
        h.mock.setDefault({
            text: "pi smoke ok",
            usage: { input_tokens: 100, output_tokens: 10, cache_creation_input_tokens: 100 },
        });

        const turn = await h.sendPrompt("hello from pi smoke", { timeoutMs: 60_000 });
        expect(turn.exitCode).toBeNull();
        expect(turn.stderr).not.toContain("Failed to load extension");
        expect(turn.sessionId).toBeTruthy();

        const req = h.mock.lastRequest();
        expect(req).not.toBeNull();
        const body = JSON.stringify(req!.body);
        expect(body).toContain("hello from pi smoke");
        expect(body).toContain("ctx_search");
        expect(body).toContain("ctx_memory");
        expect(body).toContain("ctx_note");

        const listing = listRecursive(h.env.dataDir);
        const storageFiles = listing.filter((entry) => STORAGE_FILE.test(entry));
        expect(
            storageFiles,
            `storage file under ${h.env.dataDir}; listing:\n${listing.join("\n")}`,
        ).toEqual([]);
    }, 60_000);
});

describe.skipIf(piPrereqs.ok)("pi smoke skip visibility", () => {
    it("prints a skip reason when prerequisites are unmet", () => {
        console.log(`[pi-e2e] SKIPPED: ${piPrereqs.skipReason ?? "unknown reason"}`);
        expect(piPrereqs.skipReason && piPrereqs.skipReason.length > 0).toBe(true);
    });
});
