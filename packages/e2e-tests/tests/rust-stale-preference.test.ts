/**
 * Stale preference through the real harness: the evaluator's fact world lived through
 * `opencode serve`, the Eidnara plugin, and the daemon; every question turn's provider request
 * captured; and `eval_runner stale-arms` turning the capture into the M0 export. Gated like the
 * S0 campaign: it runs only where `EIDNARA_EVAL_S0_BUDGET_MS` grants it a budget.
 */

import { describe, expect, it } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EVAL_RUNNER } from "../src/rust-runner/daemon-examples";
import { buildDaemonExample } from "../src/rust-runner/hermetic-host";
import { printSkip, rustPrereqs } from "../src/rust-scenario-support";
import { captureStaleWorld, DEFAULT_STALE_DRIVER, type FactWorld } from "../src/stale-preference";

const budget = process.env.EIDNARA_EVAL_S0_BUDGET_MS;
const SUBJECTS = 12;

interface PartText {
    at: { message: number; part: number };
    text: string;
}

interface ExportPair {
    task: string;
    key: string;
    live_value: string;
    restating_ordinal: number;
    delivery: string;
    history: { message: number; part: number };
    stale_spans: Array<{ start: number; end: number }>;
    stale_elsewhere: boolean;
    request: { messages: Array<{ content: Array<{ text?: string }> }> };
    arms: Record<
        "precedence_line" | "footer" | "anchored_replacement" | "omission_oracle",
        PartText
    > & { positive_control: PartText[] };
}

function evalRunner(binary: string, args: string[]): Record<string, unknown> {
    const run = spawnSync(binary, args, { encoding: "utf8" });
    if (run.status !== 0) throw new Error(`eval_runner ${args[0]} failed: ${run.stderr}`);
    return JSON.parse(run.stdout.trim()) as Record<string, unknown>;
}

if (!budget) printSkip("rust-stale-preference", "set EIDNARA_EVAL_S0_BUDGET_MS to run it");

describe.skipIf(!rustPrereqs.ok || !budget)("stale preference through OpenCode and Eidnara", () => {
    it("captures every question turn and exports the five arms over the served history", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-stale-"));
        try {
            const binary = await buildDaemonExample(EVAL_RUNNER);
            evalRunner(binary, [
                "stale-world",
                "--subjects",
                `${SUBJECTS}`,
                "--seed",
                "6840316923092140034",
                "--publish",
                root,
            ]);
            const world = JSON.parse(
                readFileSync(join(root, "stale-world.json"), "utf8"),
            ) as FactWorld;
            const capture = await captureStaleWorld(world, {
                ...DEFAULT_STALE_DRIVER,
                tokensPerTurn: 16_000,
            });
            expect(Object.keys(capture.requests).sort()).toEqual(
                world.pairs.map((pair) => pair.task).sort(),
            );
            expect(capture.segments.length).toBeGreaterThan(0);
            const capturePath = join(root, "capture.json");
            writeFileSync(capturePath, JSON.stringify(capture));
            const summary = evalRunner(binary, [
                "stale-arms",
                "--capture",
                capturePath,
                "--publish",
                join(root, "out"),
            ]);
            const exported = JSON.parse(
                readFileSync(join(root, "out", "stale-preference-export.json"), "utf8"),
            ) as { pairs: ExportPair[]; unlocatable: Record<string, unknown> };
            expect(exported.pairs.length + Object.keys(exported.unlocatable).length).toBe(SUBJECTS);
            expect(summary.pairs).toBe(exported.pairs.length);
            // The daemon's summarizer folded the stale statements into served history.
            expect(exported.pairs.length).toBeGreaterThan(0);
            for (const pair of exported.pairs) {
                const world_pair = world.pairs.find((p) => p.task === pair.task);
                const served =
                    pair.request.messages[pair.history.message]?.content[pair.history.part]?.text ??
                    "";
                // Spans are UTF-8 byte ranges, as eval-core computes them; every one is the value.
                const bytes = Buffer.from(served, "utf8");
                for (const span of pair.stale_spans) {
                    expect(bytes.subarray(span.start, span.end).toString("utf8")).toBe(
                        world_pair?.stale_value ?? "",
                    );
                }
                // The restating message is the ordinal every marker names: in the raw
                // tail under its ordinal (a hint the host appended may follow), or inside a served segment whose range holds it
                // (a segment demoted to its title serves no prose).
                const texts = pair.request.messages.flatMap((m) =>
                    m.content.map((part) => part.text ?? ""),
                );
                const raw = texts.some((text) =>
                    text.startsWith(`§${pair.restating_ordinal}§ ${world_pair?.restatement}`),
                );
                const folded = [...texts.join("\n").matchAll(/## (\d+)-(\d+) · /g)].some(
                    ([, start, end]) =>
                        Number(start) <= pair.restating_ordinal &&
                        pair.restating_ordinal <= Number(end),
                );
                expect(raw || folded).toBe(true);
                const replaced = pair.arms.anchored_replacement;
                expect(replaced.text).toContain(
                    `[corrected @${pair.restating_ordinal}: ${pair.key} = ${pair.live_value}]`,
                );
                // (d) rewrites the stale segment's body only; the value may stay in a heading
                // or a later segment, which `stale_elsewhere` reports.
                if (!pair.stale_elsewhere) {
                    expect(replaced.text.includes(world_pair?.stale_value ?? "")).toBe(false);
                }
                expect(pair.arms.footer.text).toContain(
                    `[corrections: ${pair.key} = ${pair.live_value} @${pair.restating_ordinal}]`,
                );
                expect(pair.arms.precedence_line.text).toContain("<memory-updates>");
                expect(pair.arms.positive_control.length).toBeGreaterThan(0);
            }
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 1_800_000);
});
