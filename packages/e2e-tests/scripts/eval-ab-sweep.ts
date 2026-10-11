import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { loadRun } from "../src/ab-eval/results";

interface TurnRow {
    session: number;
    turn: number;
    kind: string;
    ms: number;
    error: string | null;
    scope?: string;
    factKind?: string;
    factId?: string;
    grade?: string;
    harnessRss: number | null;
    hostRss: number | null;
}

interface Call {
    caller: string;
    status: number;
    inputTokens: number;
    outputTokens: number;
    cacheReadTokens: number;
    cacheWriteTokens: number;
    estimatedInputTokens: number;
}

const PRICE = { input: 5, output: 25, cacheRead: 0.5, cacheWrite: 6.25 };
const WINDOW = 200_000;

function pct(values: number[], p: number): number {
    if (values.length === 0) return 0;
    const sorted = [...values].sort((a, b) => a - b);
    return sorted[Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1)] as number;
}

const CROSS = new Set(["decision", "update", "rationale", "constraint", "multi_hop"]);

function bucket(row: TurnRow): string {
    if (row.factKind === "abstain") return "abstain";
    if (row.factKind === "repo_fact") return "repo";
    if (row.factKind === "tool_detail") return "tool";
    if (row.factKind === "recent_control") return "control";
    if (CROSS.has(row.factKind ?? "")) return row.scope === "cross_session" ? "cross" : "in";
    return "other";
}

function score(rows: TurnRow[], name: string): string {
    const picked = rows.filter((row) => row.kind === "probe" && bucket(row) === name);
    if (picked.length === 0) return "-";
    return `${picked.filter((row) => row.grade === "correct").length}/${picked.length}`;
}

function main(): void {
    const roots = process.argv.slice(2).map((path) => resolve(path));
    if (roots.length === 0) throw new Error("usage: eval-ab-sweep <run dir>...");
    const lines = [
        "| run | arm | cross | in | tool | repo | control | abstain | all | failed turns | over-window | work p50/p99 ms | >5 s | probe p50 ms | est $ | harness/daemon RSS MB |",
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    ];
    for (const root of roots) {
        const run = root.split("/").at(-1) ?? root;
        const { arms, turnsByArm, callsByArm } = loadRun<TurnRow, Call>(root);
        for (const arm of arms) {
            const rows = turnsByArm.get(arm) ?? [];
            const calls = callsByArm.get(arm) ?? [];
            const probes = rows.filter((row) => row.kind === "probe");
            const work = rows.filter((row) => row.kind !== "probe").map((row) => row.ms);
            const cost =
                calls.reduce(
                    (sum, call) =>
                        sum +
                        call.inputTokens * PRICE.input +
                        call.outputTokens * PRICE.output +
                        call.cacheReadTokens * PRICE.cacheRead +
                        call.cacheWriteTokens * PRICE.cacheWrite,
                    0,
                ) / 1e6;
            const overWindow = calls.filter(
                (call) =>
                    (call.caller === "main" || call.caller === "main_scripted") &&
                    (call.estimatedInputTokens > WINDOW ||
                        call.inputTokens + call.cacheReadTokens + call.cacheWriteTokens > WINDOW),
            ).length;
            const mb = (values: (number | null)[]) =>
                Math.round(Math.max(0, ...values.map((value) => value ?? 0)) / 1e6);
            lines.push(
                `| ${run} | ${arm} | ${score(rows, "cross")} | ${score(rows, "in")} | ${score(rows, "tool")} | ${score(rows, "repo")} | ${score(rows, "control")} | ${score(rows, "abstain")} | ${probes.filter((row) => row.grade === "correct").length}/${probes.length} | ${rows.filter((row) => row.error).length} | ${overWindow} | ${pct(work, 50)}/${pct(work, 99)} | ${work.filter((ms) => ms > 5_000).length} | ${pct(
                    probes.map((row) => row.ms),
                    50,
                )} | ${cost.toFixed(0)} | ${mb(rows.map((row) => row.harnessRss))}/${mb(rows.map((row) => row.hostRss))} |`,
            );
        }
    }
    const table = lines.join("\n");
    console.log(table);
    const out = process.env.EVAL_AB_SWEEP_OUT;
    if (out) writeFileSync(out, `${table}\n`);
}

main();
