import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { readdirSync } from "node:fs";
import { PiTestHarness } from "../src/pi-harness";
import { detectPiPrereqs } from "../src/pi-runner/spawn";
import { publishedToolName } from "../src/scripted-tool-call";

const piPrereqs = detectPiPrereqs();

/** Under `EIDNARA_E2E_REQUIRE_PI=1`, unmet prerequisites fail the run instead of skipping it. */
const PI_REQUIRED = process.env.EIDNARA_E2E_REQUIRE_PI === "1";

/** SQLite leaves `-wal`, `-shm`, and `-journal` sidecars beside the primary file. */
const STORAGE_FILE = /\.(db|sqlite|sqlite3)(-(wal|shm|journal))?$/;

const REGISTERED_TOOLS = ["ctx_search", "ctx_memory", "ctx_note"] as const;

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
        expect(turn.assistantText).toBe("pi smoke ok");

        const req = h.mock.lastRequest();
        expect(req).not.toBeNull();
        expect(JSON.stringify(req!.body)).toContain("hello from pi smoke");
        for (const tool of REGISTERED_TOOLS) {
            expect(publishedToolName(req!.body, tool), `${tool} in req.body.tools`).toBe(tool);
        }

        const listing = listRecursive(h.env.dataDir);
        const storageFiles = listing.filter((entry) => STORAGE_FILE.test(entry));
        expect(
            storageFiles,
            `storage file under ${h.env.dataDir}; listing:\n${listing.join("\n")}`,
        ).toEqual([]);
    }, 60_000);
});

describe.skipIf(piPrereqs.ok || !PI_REQUIRED)("pi smoke prerequisites", () => {
    it("are present when EIDNARA_E2E_REQUIRE_PI=1", () => {
        expect(
            piPrereqs.ok,
            `EIDNARA_E2E_REQUIRE_PI=1 refuses to skip: ${piPrereqs.skipReason ?? "unknown reason"}`,
        ).toBe(true);
    });
});

describe.skipIf(piPrereqs.ok || PI_REQUIRED)("pi smoke skip visibility", () => {
    it("prints a skip reason when prerequisites are unmet", () => {
        console.log(`[pi-e2e] SKIPPED: ${piPrereqs.skipReason ?? "unknown reason"}`);
        expect(piPrereqs.skipReason && piPrereqs.skipReason.length > 0).toBe(true);
    });
});
