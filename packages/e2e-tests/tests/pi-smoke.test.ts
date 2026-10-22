import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, readdirSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PiTestHarness } from "../src/pi-harness";
import { awaitStore, textOf } from "../src/pi-runner/rust-fixture";
import { detectPiPrereqs } from "../src/pi-runner/spawn";
import {
    buildDirectHostFixture,
    detectRustModePrereqs,
    HermeticHostStack,
} from "../src/rust-runner/hermetic-host";
import { publishedToolName } from "../src/scripted-tool-call";

const piPrereqs = detectPiPrereqs();
const rustPrereqs = detectRustModePrereqs();
const prereqs = piPrereqs.ok ? rustPrereqs : piPrereqs;

/** Under `EIDNARA_E2E_REQUIRE_PI=1`, unmet prerequisites fail the run instead of skipping it. */
const PI_REQUIRED = process.env.EIDNARA_E2E_REQUIRE_PI === "1";

/** SQLite leaves `-wal`, `-shm`, and `-journal` sidecars beside the primary file. */
const STORAGE_FILE = /\.(db|sqlite|sqlite3)(-(wal|shm|journal))?$/;

const REGISTERED_TOOLS = ["eidnara_search", "eidnara_memory", "eidnara_note"] as const;

function listRecursive(dir: string): string[] {
    return readdirSync(dir, { recursive: true, encoding: "utf8" }).sort();
}

describe.skipIf(!prereqs.ok)("pi smoke", () => {
    let h: PiTestHarness;
    let stack: HermeticHostStack | undefined;
    let hostRoot: string | undefined;

    beforeAll(async () => {
        hostRoot = realpathSync(mkdtempSync(join(tmpdir(), "eidnara-pi-smoke-host-")));
        const dataDir = join(hostRoot, "data");
        mkdirSync(dataDir, { recursive: true });
        stack = await HermeticHostStack.start({
            dataDir,
            fixtureBin: await buildDirectHostFixture(),
            startTimeoutMs: 120_000,
            daemonConfig: { memory: { auto_capture: false } },
        });
        await awaitStore(stack, hostRoot);
        h = await PiTestHarness.create({
            eidnaraConfig: { host: { connection_file: stack.connectionFile } },
        });
    }, 600_000);

    afterAll(async () => {
        await h?.dispose();
        await stack?.stop();
        if (hostRoot) rmSync(hostRoot, { recursive: true, force: true });
    });

    it("loads the built extension, completes one mock turn, and leaves no storage file", async () => {
        h.mock.reset();
        h.mock.setDefault({
            text: "pi smoke ok",
            usage: { input_tokens: 100, output_tokens: 10, cache_creation_input_tokens: 100 },
        });

        const turn = await h.sendPrompt("hello from pi smoke", { timeoutMs: 60_000 });
        expect(turn.exitCode).toBeNull();
        expect(turn.signalCode).toBeNull();
        expect(turn.stderr).not.toContain("Failed to load extension");
        expect(turn.sessionId).toBeTruthy();
        expect(turn.assistantText).toBe("pi smoke ok");

        const req = h.mock.lastRequest();
        expect(req).not.toBeNull();
        expect(JSON.stringify(req!.body)).toContain("hello from pi smoke");
        const messages = (req!.body.messages ?? []) as unknown[];
        expect(textOf(messages[0])).toContain("<session-history>");
        expect(textOf(messages.at(-1))).toContain("hello from pi smoke");
        for (const tool of REGISTERED_TOOLS) {
            expect(publishedToolName(req!.body, tool), `${tool} in req.body.tools`).toBe(tool);
        }

        expect(await stack?.historySummarizerLive()).toBe(false);
        expect((await stack?.backendCounters())?.started).toBe(0);

        const listing = listRecursive(h.env.dataDir);
        const storageFiles = listing.filter((entry) => STORAGE_FILE.test(entry));
        expect(
            storageFiles,
            `storage file under ${h.env.dataDir}; listing:\n${listing.join("\n")}`,
        ).toEqual([]);
    }, 60_000);
});

describe.skipIf(prereqs.ok || !PI_REQUIRED)("pi smoke prerequisites", () => {
    it("are present when EIDNARA_E2E_REQUIRE_PI=1", () => {
        expect(
            prereqs.ok,
            `EIDNARA_E2E_REQUIRE_PI=1 refuses to skip: ${prereqs.skipReason ?? "unknown reason"}`,
        ).toBe(true);
    });
});

describe.skipIf(prereqs.ok || PI_REQUIRED)("pi smoke skip visibility", () => {
    it("prints a skip reason when prerequisites are unmet", () => {
        console.log(`[pi-e2e] SKIPPED: ${prereqs.skipReason ?? "unknown reason"}`);
        expect(prereqs.skipReason && prereqs.skipReason.length > 0).toBe(true);
    });
});
