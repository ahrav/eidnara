import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { PiTestHarness } from "../src/pi-harness";
import { detectPiPrereqs } from "../src/pi-runner/spawn";
import { RustTestHarness } from "../src/rust-harness";
import { missingPayloadFiles, PAYLOAD_DIR_ENV, PayloadHost } from "../src/rust-runner/payload-host";
import { rustPrereqs } from "../src/rust-scenario-support";
import {
    DEFAULT_SCRIPTED_TOOL_USAGE,
    findToolResultText,
    publishedToolName,
    runScriptedToolCall,
} from "../src/scripted-tool-call";

const PAYLOAD_DIR = process.env[PAYLOAD_DIR_ENV];
const SUITE_TIMEOUT_MS = 900_000;
const ADMISSION_TIMEOUT_MS = 300_000;
const CATCH_UP_TIMEOUT_MS = 120_000;
const FALLBACK = "results come from the legacy snapshot scan";

/** Each query and its memory use disjoint word sets, so a ranked row comes from the dense lane. Each harness writes both memories into its own project, so a row ranks only against its own harness's memories. */
const BEFORE = {
    content: "Container images ship to production only on weekday mornings.",
    query: "release cadence restrictions for deploying builds",
};
const AFTER = {
    content: "Database migrations require a reviewed rollback script before merge.",
    query: "undo plan when altering schema tables",
};

function words(text: string): Set<string> {
    return new Set(text.toLowerCase().match(/[a-z0-9]+/g) ?? []);
}

function createdId(resultText: string): string {
    const id = /"objectId":"(mem_[0-9a-f]{32})"/.exec(resultText)?.[1];
    if (!id) throw new Error(`memory create returned no object: ${resultText}`);
    return id;
}

function rowFor(result: string, objectId: string): { header: string; body: string } | null {
    const lines = result.split("\n");
    const at = lines.findIndex((line) => line.includes(` id=${objectId} `));
    return at < 0 ? null : { header: lines[at] ?? "", body: lines[at + 1] ?? "" };
}

function expectDenseOnly(result: string, objectId: string, content: string): void {
    expect(result).not.toContain(FALLBACK);
    const row = rowFor(result, objectId);
    expect(row, result).not.toBeNull();
    expect(row?.header).toMatch(/^\[1\] \[memory\] .* match=fused lanes=dense(?:\s|$)/);
    expect(row?.body).toBe(content);
}

let piCallCounter = 0;

async function piToolCall(
    pi: PiTestHarness,
    tool: string,
    input: Record<string, unknown>,
): Promise<string> {
    const callId = `toolu_payload_pi_${++piCallCounter}`;
    let published = false;
    pi.mock.reset();
    pi.mock.addMatcher((body) => {
        if (published) return null;
        const name = publishedToolName(body, tool);
        if (!name) return null;
        published = true;
        return {
            content: [{ type: "tool_use", id: callId, name, input }],
            stop_reason: "tool_use" as const,
            usage: DEFAULT_SCRIPTED_TOOL_USAGE,
        };
    });
    pi.mock.setDefault({ text: "scripted tool follow-up", usage: DEFAULT_SCRIPTED_TOOL_USAGE });
    await pi.sendPrompt(`Run ${tool}.`);
    if (!published) throw new Error(`Pi never published ${tool}`);
    const result = findToolResultText(pi, callId);
    if (result === null) throw new Error(`Pi returned no tool_result for ${tool}`);
    return result;
}

async function searchUntilRanked(
    search: (query: string) => Promise<string>,
    query: string,
    objectId: string,
): Promise<string> {
    const deadline = Date.now() + CATCH_UP_TIMEOUT_MS;
    let result = await search(query);
    while (rowFor(result, objectId) === null && Date.now() < deadline) {
        await Bun.sleep(2_000);
        result = await search(query);
    }
    return result;
}

describe.skipIf(!PAYLOAD_DIR)("fused search through a built payload", () => {
    let oc: RustTestHarness<PayloadHost>;
    let pi: PiTestHarness;

    beforeAll(async () => {
        const payload = PAYLOAD_DIR as string;
        expect(missingPayloadFiles(payload), `${PAYLOAD_DIR_ENV}=${payload}`).toEqual([]);
        expect(rustPrereqs.skipReason ?? "present").toBe("present");
        expect(detectPiPrereqs().skipReason ?? "present").toBe("present");
        oc = await RustTestHarness.createWithHost({}, async (env) =>
            PayloadHost.start(env.dataDir, payload),
        );
        pi = await PiTestHarness.create({
            eidnaraConfig: { host: { connection_file: oc.host.connectionFile } },
        });
    }, SUITE_TIMEOUT_MS);

    afterAll(async () => {
        await pi?.dispose();
        await oc?.dispose();
    }, 120_000);

    it(
        "ranks memories from before and after activation through each plugin's dense lane",
        async () => {
            for (const memory of [BEFORE, AFTER]) {
                const shared = [...words(memory.query)].filter((word) =>
                    words(memory.content).has(word),
                );
                expect(shared).toEqual([]);
            }
            const ocSession = await oc.createSession();
            // The memory subset before admission keeps the fallback check to one source; afterwards each search uses the default sources.
            const ocSearch = async (query: string, sources?: string[]) =>
                (
                    await runScriptedToolCall(oc, ocSession, {
                        tool: "eidnara_search",
                        input: sources ? { query, sources } : { query },
                        prompt: "Search project memory.",
                    })
                ).resultText;
            const piSearch = (query: string, sources?: string[]) =>
                piToolCall(pi, "eidnara_search", sources ? { query, sources } : { query });
            const create = (content: string) => ({
                action: "create",
                content,
                category: "PROJECT_RULES",
            });

            const ocBefore = createdId(
                (
                    await runScriptedToolCall(oc, ocSession, {
                        tool: "eidnara_memory",
                        input: create(BEFORE.content),
                        prompt: "Record this project rule.",
                    })
                ).resultText,
            );
            const piBefore = createdId(
                await piToolCall(pi, "eidnara_memory", create(BEFORE.content)),
            );

            expect(await ocSearch(BEFORE.query, ["memory"])).toContain(FALLBACK);
            expect(await piSearch(BEFORE.query, ["memory"])).toContain(FALLBACK);

            await oc.host.installSearchAdmission();
            await oc.host.waitForAdmitted(oc.env.workdir, ocSession, ADMISSION_TIMEOUT_MS);

            const ocAfter = createdId(
                (
                    await runScriptedToolCall(oc, ocSession, {
                        tool: "eidnara_memory",
                        input: create(AFTER.content),
                        prompt: "Record this project rule.",
                    })
                ).resultText,
            );
            const piAfter = createdId(
                await piToolCall(pi, "eidnara_memory", create(AFTER.content)),
            );

            for (const [search, before, after] of [
                [ocSearch, ocBefore, ocAfter],
                [piSearch, piBefore, piAfter],
            ] as const) {
                expectDenseOnly(
                    await searchUntilRanked(search, BEFORE.query, before),
                    before,
                    BEFORE.content,
                );
                expectDenseOnly(
                    await searchUntilRanked(search, AFTER.query, after),
                    after,
                    AFTER.content,
                );
            }
        },
        SUITE_TIMEOUT_MS,
    );
});

describe.skipIf(Boolean(PAYLOAD_DIR))(
    "fused search through a built payload skip visibility",
    () => {
        it("prints why it skips", () => {
            console.log(`[payload-e2e] SKIPPED: ${PAYLOAD_DIR_ENV} names no built payload package`);
        });
    },
);
