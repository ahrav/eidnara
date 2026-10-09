import { describe, expect, it } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { RetainedCapture, RustPassLine } from "../rust-harness";
import { parseRustPassLine } from "../rust-harness";
import type { ScriptSource } from "../rust-runner/hermetic-host";
import { COMPRESSION_FIDELITY_CORPUS_SHA256 } from "./corpus";
import {
    emitObservation,
    historyHeadings,
    judgeDelivery,
    planSeed,
    publishedOf,
    reviewedTiers,
    servedTier,
    stageOf,
} from "./delivery";

const TITLE = "Pooling-first rejected";
const BODIES = ["full P1 body", "condensed P2", "short P3"];
const PROBE = "Connection setup shows up in every flame graph";

function capture(messages: string[]): RetainedCapture {
    return {
        sessionId: "ses_a",
        caseId: "C1",
        scenarioId: "C1.S1",
        request: {
            receivedAt: 0,
            method: "POST",
            path: "/v1/messages",
            headers: { "x-session-id": "ses_a" },
            body: { messages: messages.map((text) => ({ role: "user", content: text })) },
        },
    };
}

function pass(fields: string): RustPassLine {
    const parsed = parseRustPassLine(`[ses_a] rust pass: ${fields}`);
    if (!parsed) throw new Error("unparsed pass line");
    return parsed;
}

const APPLIED = pass("decision=SOFT+ reason=none served_from=transform applied=true");
const history = (body: string) =>
    `<session-history>\n## 1-6 · ${TITLE}\n${body}\n</session-history>`;

describe("compression fidelity delivery judgment", () => {
    it("credits an applied transform capture serving a reviewed body with a clean tail", () => {
        const verdict = judgeDelivery({
            capture: capture([history(BODIES[0] ?? ""), "follow-up"]),
            pass: APPLIED,
            title: TITLE,
            bodies: BODIES,
            leakProbes: [PROBE],
        });
        expect(verdict).toEqual({ refusals: [], tier: "p1", leaks: [] });
    });

    it("refuses a missing capture, an empty capture, and a missing pass separately", () => {
        const base = { title: TITLE, bodies: BODIES, leakProbes: [PROBE] };
        expect(judgeDelivery({ ...base, capture: undefined, pass: APPLIED }).refusals).toEqual([
            "missing_capture",
        ]);
        expect(judgeDelivery({ ...base, capture: capture([]), pass: APPLIED }).refusals).toEqual([
            "empty_capture",
        ]);
        expect(
            judgeDelivery({
                ...base,
                capture: capture([history(BODIES[0] ?? "")]),
                pass: undefined,
            }).refusals,
        ).toEqual(["missing_pass"]);
    });

    it("refuses raw pass-through and an unapplied recipe even when the request looks folded", () => {
        const base = {
            capture: capture([history(BODIES[0] ?? ""), "follow-up"]),
            title: TITLE,
            bodies: BODIES,
            leakProbes: [PROBE],
        };
        expect(
            judgeDelivery({
                ...base,
                pass: pass("decision=HARD reason=x served_from=raw applied=true"),
            }).refusals,
        ).toEqual(["raw_pass_through"]);
        expect(
            judgeDelivery({
                ...base,
                pass: pass("decision=HARD reason=x served_from=transform applied=false"),
            }).refusals,
        ).toEqual(["not_applied"]);
    });

    it("refuses covered native text in the live tail and an unmatched body", () => {
        const leaked = judgeDelivery({
            capture: capture([history(BODIES[0] ?? ""), `raw: ${PROBE} again`]),
            pass: APPLIED,
            title: TITLE,
            bodies: BODIES,
            leakProbes: [PROBE],
        });
        expect(leaked.refusals).toEqual(["raw_tail_leak"]);
        expect(leaked.leaks).toEqual([PROBE]);
        expect(
            judgeDelivery({
                capture: capture([history("a paraphrase nobody reviewed"), "follow-up"]),
                pass: APPLIED,
                title: TITLE,
                bodies: BODIES,
                leakProbes: [PROBE],
            }).refusals,
        ).toEqual(["body_unmatched"]);
    });

    it("reads P4 as the heading alone and P5 as absence", () => {
        expect(
            servedTier([`<session-history>\n## 1-6 · ${TITLE}\n</session-history>`], TITLE, BODIES),
        ).toBe("p4");
        expect(servedTier(["<session-history>\n</session-history>"], TITLE, BODIES)).toBe("p5");
    });

    it("credits a body only under the case heading inside the history wrapper", () => {
        const unwrapped = `## 1-6 · ${TITLE}\n${BODIES[0]}`;
        expect(servedTier([unwrapped, "follow-up"], TITLE, BODIES)).toBe("p5");
        expect(
            judgeDelivery({
                capture: capture([unwrapped, "follow-up"]),
                pass: APPLIED,
                title: TITLE,
                bodies: BODIES,
                leakProbes: [PROBE],
            }).refusals,
        ).toEqual(["history_absent"]);
        expect(
            servedTier(
                [`I recall ${TITLE} from before.`, history(BODIES[0] ?? ""), "follow-up"],
                TITLE,
                BODIES,
            ),
        ).toBe("p1");
        const mentioned = `<session-history>\n## 1-2 · Other\n${TITLE} came up here\n</session-history>`;
        expect(servedTier([mentioned], TITLE, BODIES)).toBe("p5");
        const since = `<session-history-since>\n## 1-6 · ${TITLE}\n${BODIES[0]}\n</session-history-since>`;
        expect(servedTier([since, "follow-up"], TITLE, BODIES)).toBe("p1");
    });

    it("reads the stage from the case heading, not from a mention of the title", () => {
        const m1Mention = `<session-history-since>\n## 7-8 · Newer\n${TITLE} came up again\n</session-history-since>`;
        const m0Case = history(BODIES[0] ?? "");
        expect(stageOf([`${m1Mention}\n${m0Case}`, "follow-up"], TITLE)).toBe("m0");
        expect(stageOf([m1Mention, "follow-up"], TITLE)).toBe("absent");
        const mentioned = `<session-history>\n## 1-2 · Other\n${TITLE} came up here\n</session-history>`;
        expect(stageOf([mentioned], TITLE)).toBe("absent");
        const m1Case = `<session-history-since>\n## 1-6 · ${TITLE}\n${BODIES[0]}\n</session-history-since>`;
        expect(stageOf([m1Case], TITLE)).toBe("m1");
        expect(stageOf([`## 1-6 · ${TITLE}\n${BODIES[0]}`], TITLE)).toBe("absent");
    });

    it("lists the headings inside the history wrappers only", () => {
        const m1 = `<session-history-since>\n## 7-8 · Newer\nbody\n</session-history-since>`;
        const m0 = `<session-history>\n## 1-6 · ${TITLE}\n## 9-9 · Other\n</session-history>`;
        expect(historyHeadings([`${m1}\n${m0}\n## 10-10 · Unwrapped`, "follow-up"])).toEqual([
            "## 7-8 · Newer",
            `## 1-6 · ${TITLE}`,
            "## 9-9 · Other",
        ]);
        expect(
            historyHeadings(["## 1-1 · Unwrapped", "<session-history>\n</session-history>"]),
        ).toEqual([]);
    });

    it("scans outside both history wrappers, the system text included", () => {
        const m1 = `<session-history-since>\n## 1-6 · ${TITLE}\n${PROBE} quoted in m1\n</session-history-since>`;
        expect(
            judgeDelivery({
                capture: capture([`${m1}\nfollow-up`]),
                pass: APPLIED,
                title: TITLE,
                bodies: BODIES,
                leakProbes: [PROBE],
            }).leaks,
        ).toEqual([]);
        expect(
            judgeDelivery({
                capture: capture([`${history(BODIES[0] ?? "")}\n${PROBE} after the wrapper`]),
                pass: APPLIED,
                title: TITLE,
                bodies: BODIES,
                leakProbes: [PROBE],
            }).refusals,
        ).toEqual(["raw_tail_leak"]);
        const withSystem = capture([history(BODIES[0] ?? ""), "follow-up"]);
        withSystem.request.body.system = [{ type: "text", text: PROBE }];
        expect(
            judgeDelivery({
                capture: withSystem,
                pass: APPLIED,
                title: TITLE,
                bodies: BODIES,
                leakProbes: [PROBE],
            }).leaks,
        ).toEqual([PROBE]);
    });

    it("plans seeded rows in order after the newest message and closes a user-final source", () => {
        const source: ScriptSource = {
            corpusSha256: COMPRESSION_FIDELITY_CORPUS_SHA256,
            case: "C1",
            scenario: "C1.S1",
            source: "C1.V1",
            leakProbes: [],
            memoryExamples: [],
            messages: [
                { info: { role: "user" }, parts: [{ id: "a", type: "text", text: "question" }] },
                {
                    info: { role: "assistant" },
                    parts: [
                        {
                            id: "b",
                            type: "tool",
                            callID: "c",
                            tool: "read",
                            state: { status: "completed", input: {}, output: "o" },
                        },
                    ],
                },
                { info: { role: "user" }, parts: [{ id: "d", type: "text", text: "thanks" }] },
            ],
        };
        const rows = planSeed(
            source,
            { user: { role: "user" }, assistant: { role: "assistant" } },
            100,
        );
        expect(rows.map((row) => row.data.role)).toEqual([
            "user",
            "assistant",
            "user",
            "assistant",
        ]);
        expect(rows.every((row, i) => i === 0 || row.created > (rows[i - 1]?.completed ?? 0))).toBe(
            true,
        );
        expect(rows[0]?.created).toBe(101);
        expect(rows[1]?.data.parentID).toBe(rows[0]?.id);
        expect(rows[3]?.data.parentID).toBe(rows[2]?.id);
        expect(rows[1]?.parts[0]?.data).toMatchObject({
            type: "tool",
            state: { status: "completed", output: "o", title: "", metadata: {} },
        });
        expect(rows[1]?.parts[0]?.data).not.toHaveProperty("id");
        expect(rows[3]?.parts[0]?.data).toMatchObject({ type: "text" });
    });

    it("reads the published counter and rejects a status without one", () => {
        expect(publishedOf({ history_summarizer: { counters: { published: 0 } } })).toBe(0);
        expect(publishedOf({ history_summarizer: { counters: { published: 3 } } })).toBe(3);
        for (const status of [
            {},
            { history_summarizer: {} },
            { history_summarizer: { counters: {} } },
        ]) {
            expect(() => publishedOf(status)).toThrow("history_summarizer.counters.published");
        }
    });

    it("reads the title and tier bodies of a reviewed output", () => {
        expect(
            reviewedTiers(
                '<history_segment start="1" end="2" title="T" importance="70"><p1>one</p1><p2>two</p2><p3>three</p3><p4 /></history_segment>',
            ),
        ).toEqual({ title: "T", importance: 70, bodies: ["one", "two", "three"] });
        expect(() =>
            reviewedTiers('<history_segment title="T"><p1>a</p1><p2>b</p2><p3>c</p3>'),
        ).toThrow("importance");
    });

    it("writes an owner-only observation only when a directory is named", () => {
        const observation = {
            case: "C1",
            source: "C1.V1",
            scenario: "C1.S1",
            stage: "qualification",
            terminal: "served",
            markers: [],
            detail: {},
        };
        expect(emitObservation(observation, undefined)).toBeUndefined();
        const root = mkdtempSync(join(tmpdir(), "cf-observations-"));
        try {
            const dir = join(root, "private");
            const path = emitObservation(observation, dir);
            expect(path).toBe(join(dir, "opencode-delivery.C1.C1.S1.qualification.json"));
            expect(statSync(dir).mode & 0o777).toBe(0o700);
            expect(statSync(path ?? "").mode & 0o777).toBe(0o600);
            expect(JSON.parse(readFileSync(path ?? "", "utf8"))).toMatchObject({
                schema_version: 1,
                owner: "opencode-delivery",
                corpus_sha256: COMPRESSION_FIDELITY_CORPUS_SHA256,
                scenario: "C1.S1",
            });
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});
