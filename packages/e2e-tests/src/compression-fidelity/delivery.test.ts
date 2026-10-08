import { describe, expect, it } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { RetainedCapture, RustPassLine } from "../rust-harness";
import { parseRustPassLine } from "../rust-harness";
import { COMPRESSION_FIDELITY_CORPUS_SHA256 } from "./corpus";
import { emitObservation, judgeDelivery, reviewedTiers, servedTier } from "./delivery";

const TITLE = "Pooling-first rejected";
const BODIES = ["full P1 body", "condensed P2", "short P3"];
const PROBE = "Connection setup shows up in every flame graph";

function capture(messages: string[]): RetainedCapture {
    return {
        sessionId: "ses_a",
        caseId: "C1",
        scenarioId: "C1.S1",
        sequence: 0,
        request: {
            receivedAt: 0,
            method: "POST",
            path: "/v1/messages",
            headers: { "x-opencode-session-id": "ses_a" },
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
            probes: [PROBE],
        });
        expect(verdict).toEqual({ refusals: [], tier: "p1", historyMessage: 0, leaks: [] });
    });

    it("refuses a missing capture, an empty capture, and a missing pass separately", () => {
        const base = { title: TITLE, bodies: BODIES, probes: [PROBE] };
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
            probes: [PROBE],
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
            probes: [PROBE],
        });
        expect(leaked.refusals).toEqual(["raw_tail_leak"]);
        expect(leaked.leaks).toEqual([PROBE]);
        expect(
            judgeDelivery({
                capture: capture([history("a paraphrase nobody reviewed"), "follow-up"]),
                pass: APPLIED,
                title: TITLE,
                bodies: BODIES,
                probes: [PROBE],
            }).refusals,
        ).toEqual(["body_unmatched"]);
    });

    it("reads P4 as the heading alone and P5 as absence", () => {
        expect(
            servedTier([`<session-history>\n## 1-6 · ${TITLE}\n</session-history>`], TITLE, BODIES),
        ).toEqual({
            tier: "p4",
            message: 0,
        });
        expect(servedTier(["<session-history>\n</session-history>"], TITLE, BODIES)).toEqual({
            tier: "p5",
            message: -1,
        });
    });

    it("reads the title and tier bodies of a reviewed output", () => {
        expect(
            reviewedTiers(
                '<history_segment start="1" end="2" title="T"><p1>one</p1><p2>two</p2><p3>three</p3><p4 /></history_segment>',
            ),
        ).toEqual({ title: "T", bodies: ["one", "two", "three"] });
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
