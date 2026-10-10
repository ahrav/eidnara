import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

interface TurnRow {
    arm: string;
    session: number;
    turn: number;
    kind: string;
    ms: number;
    error: string | null;
    scriptMismatch: number;
    forwardedMainCalls: number;
    harnessRss: number | null;
    harnessCpuMs: number | null;
    hostRss: number | null;
    hostCpuMs: number | null;
    factId?: string;
    factKind?: string;
    scope?: string;
    expected?: string;
    answer?: string;
    grade?: string;
}

interface Call {
    arm: string;
    role: string;
    caller: string;
    turn: string | null;
    status: number;
    ms: number;
    firstByteMs: number | null;
    inputTokens: number;
    outputTokens: number;
    cacheReadTokens: number;
    cacheWriteTokens: number;
    estimatedInputTokens: number;
    retries?: number;
    delivered?: { answer: boolean; stale: boolean; inLastUser: boolean };
    toolsUsed?: string[];
}

const PRICE = { input: 5, output: 25, cacheRead: 0.5, cacheWrite: 6.25 };

const jsonl = <T>(path: string): T[] =>
    existsSync(path)
        ? readFileSync(path, "utf8")
              .split("\n")
              .filter((l) => l.trim().length > 0)
              .flatMap((l) => {
                  try {
                      return [JSON.parse(l) as T];
                  } catch {
                      return [];
                  }
              })
        : [];

function pct(values: number[], p: number): number {
    if (values.length === 0) return 0;
    const sorted = [...values].sort((a, b) => a - b);
    return sorted[Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1)] as number;
}

function binomTwoSided(k: number, n: number): number {
    if (n === 0) return 1;
    const lo = Math.min(k, n - k);
    let tail = 0;
    let c = 1;
    for (let i = 0; i <= n; i++) {
        if (i > 0) c = (c * (n - i + 1)) / i;
        if (i <= lo) tail += c;
    }
    return Math.min(1, (2 * tail) / 2 ** n);
}

function main(): void {
    const out = resolve(process.argv[2] ?? join(tmpdir(), "ab-eval/runs/latest"));
    const armsDir = existsSync(join(out, "results")) ? join(out, "results") : join(out, "arms");
    const arms = readdirSync(armsDir).sort();
    const lines: string[] = [`# A/B report: ${out}`, ""];
    const turnsByArm = new Map<string, TurnRow[]>();
    const callsByArm = new Map<string, Call[]>();
    for (const arm of arms) {
        turnsByArm.set(arm, jsonl<TurnRow>(join(armsDir, arm, "turns.jsonl")));
        callsByArm.set(arm, jsonl<Call>(join(armsDir, arm, "calls.jsonl")));
    }

    lines.push("## Accuracy by fact kind (correct/total; s=stale a=abstained w=wrong)", "");
    const kinds = [
        ...new Set(
            [...turnsByArm.values()]
                .flat()
                .filter((r) => r.kind === "probe")
                .map((r) => `${r.factKind}/${r.scope}`),
        ),
    ].sort();
    lines.push(
        `| kind/scope | ${arms.join(" | ")} |`,
        `| --- | ${arms.map(() => "---").join(" | ")} |`,
    );
    for (const k of [...kinds, "ALL"]) {
        const cells = arms.map((arm) => {
            const rows = (turnsByArm.get(arm) ?? []).filter(
                (r) => r.kind === "probe" && (k === "ALL" || `${r.factKind}/${r.scope}` === k),
            );
            const c = (g: string) => rows.filter((r) => r.grade === g).length;
            return `${c("correct")}/${rows.length} (s${c("stale")} a${c("abstained")} w${c("wrong")})`;
        });
        lines.push(`| ${k} | ${cells.join(" | ")} |`);
    }
    lines.push("");

    lines.push("## Paired on/off (same probe, same world)", "");
    for (const onArm of arms.filter((arm) => /-on(raw)?$/.test(arm))) {
        const harness = onArm.split("-")[0] as string;
        const on = turnsByArm.get(onArm);
        const off = turnsByArm.get(`${harness}-off`);
        if (!on || !off) continue;
        const offBy = new Map(off.filter((r) => r.kind === "probe").map((r) => [r.factId, r]));
        let both = 0;
        let onlyOn = 0;
        let onlyOff = 0;
        let neither = 0;
        const regressions: string[] = [];
        for (const r of on.filter((x) => x.kind === "probe")) {
            const o = offBy.get(r.factId);
            if (!o) continue;
            const a = r.grade === "correct";
            const b = o.grade === "correct";
            if (a && b) both++;
            else if (a) onlyOn++;
            else if (b) {
                onlyOff++;
                regressions.push(
                    `${r.factId} (${r.factKind}/${r.scope}) on=${r.grade} "${(r.answer ?? "").slice(0, 80).replace(/\n/g, " ")}"`,
                );
            } else neither++;
        }
        lines.push(
            `- ${onArm} vs ${harness}-off: both=${both} on-only=${onlyOn} off-only=${onlyOff} neither=${neither}; exact McNemar p=${binomTwoSided(onlyOn, onlyOn + onlyOff).toFixed(4)}`,
        );
        for (const reg of regressions) lines.push(`  - regression: ${reg}`);
    }
    lines.push("");

    lines.push("## Delivery diagnosis for eidnara-on misses", "");
    for (const arm of arms.filter((a) => a.endsWith("-on"))) {
        const calls = callsByArm.get(arm) ?? [];
        for (const r of (turnsByArm.get(arm) ?? []).filter(
            (x) => x.kind === "probe" && x.grade !== "correct" && x.factKind !== "abstain",
        )) {
            const key = `${r.session}:${r.turn}`;
            const probeCalls = calls.filter((c) => c.turn === key && c.caller === "main");
            const first = probeCalls[0]?.delivered;
            const any = probeCalls.some((c) => c.delivered?.answer);
            const tools = [...new Set(probeCalls.flatMap((c) => c.toolsUsed ?? []))];
            lines.push(
                `- ${arm} ${r.factId} ${r.factKind}/${r.scope} grade=${r.grade} initialContext=${first?.answer ?? "?"} afterTools=${any} stale=${probeCalls.some((c) => c.delivered?.stale)} tools=[${tools.join(",")}]`,
            );
        }
    }
    lines.push("");

    lines.push("## Latency (ms)", "");
    lines.push(
        "| arm | work p50 | p95 | p99 | max | >1s | >5s | probe p50 | probe p95 | errors | mismatches |",
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    );
    for (const arm of arms) {
        const rows = turnsByArm.get(arm) ?? [];
        const work = rows.filter((r) => r.kind !== "probe").map((r) => r.ms);
        const probes = rows.filter((r) => r.kind === "probe").map((r) => r.ms);
        lines.push(
            `| ${arm} | ${pct(work, 50)} | ${pct(work, 95)} | ${pct(work, 99)} | ${Math.max(0, ...work)} | ${work.filter((m) => m > 1000).length} | ${work.filter((m) => m > 5000).length} | ${pct(probes, 50)} | ${pct(probes, 95)} | ${rows.filter((r) => r.error).length} | ${rows.reduce((s, r) => s + r.scriptMismatch, 0)} |`,
        );
    }
    lines.push("");

    lines.push("## Work-turn stalls (turns that waited on a model call)", "");
    for (const arm of arms) {
        const calls = callsByArm.get(arm) ?? [];
        const rows = (turnsByArm.get(arm) ?? []).filter(
            (r) => r.kind !== "probe" && r.forwardedMainCalls > 0,
        );
        const byCaller = new Map<string, number>();
        for (const r of rows) {
            const key = `${r.session}:${r.turn}`;
            for (const c of calls.filter(
                (x) => x.turn === key && x.role === "main" && x.caller !== "main_scripted",
            )) {
                byCaller.set(c.caller, (byCaller.get(c.caller) ?? 0) + 1);
            }
        }
        lines.push(
            `- ${arm}: ${rows.length} work turns waited; total wait ${Math.round(rows.reduce((s, r) => s + r.ms, 0) / 1000)}s; max ${Math.max(0, ...rows.map((r) => r.ms))}ms; callers ${JSON.stringify(Object.fromEntries(byCaller))}`,
        );
    }
    lines.push("");

    lines.push("## Model calls and tokens", "");
    lines.push(
        "| arm | caller | calls | input | output | cacheRead | cacheWrite | est $ | p50 ms | retries | errors |",
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    );
    for (const arm of arms) {
        const calls = (callsByArm.get(arm) ?? []).filter((c) => c.caller !== "main_scripted");
        const callers = [...new Set(calls.map((c) => `${c.role}/${c.caller}`))].sort();
        let armCost = 0;
        for (const caller of callers) {
            const cs = calls.filter((c) => `${c.role}/${c.caller}` === caller);
            const sum = (f: (c: Call) => number) => cs.reduce((s, c) => s + f(c), 0);
            const cost =
                (sum((c) => c.inputTokens) * PRICE.input +
                    sum((c) => c.outputTokens) * PRICE.output +
                    sum((c) => c.cacheReadTokens) * PRICE.cacheRead +
                    sum((c) => c.cacheWriteTokens) * PRICE.cacheWrite) /
                1e6;
            armCost += cost;
            lines.push(
                `| ${arm} | ${caller} | ${cs.length} | ${sum((c) => c.inputTokens)} | ${sum((c) => c.outputTokens)} | ${sum((c) => c.cacheReadTokens)} | ${sum((c) => c.cacheWriteTokens)} | ${cost.toFixed(2)} | ${Math.round(
                    pct(
                        cs.map((c) => c.ms),
                        50,
                    ),
                )} | ${sum((c) => c.retries ?? 0)} | ${cs.filter((c) => c.status !== 200).length} |`,
            );
        }
        lines.push(`| ${arm} | TOTAL | | | | | | ${armCost.toFixed(2)} | | | |`);
    }
    lines.push("");

    lines.push("## Context the model saw at probe time (tokens on the first probe request)", "");
    for (const arm of arms) {
        const calls = callsByArm.get(arm) ?? [];
        const firsts = (turnsByArm.get(arm) ?? [])
            .filter((r) => r.kind === "probe")
            .map((r) =>
                calls.find((c) => c.turn === `${r.session}:${r.turn}` && c.caller === "main"),
            )
            .filter((c): c is Call => c !== undefined)
            .map((c) => c.inputTokens + c.cacheReadTokens + c.cacheWriteTokens);
        lines.push(`- ${arm}: p50=${pct(firsts, 50)} max=${Math.max(0, ...firsts)}`);
    }
    lines.push("");

    lines.push("## Resources", "");
    lines.push(
        "| arm | harness RSS max MB | harness RSS last MB | host RSS max MB | host CPU s | harness disk MB | eidnara disk MB |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    );
    for (const arm of arms) {
        const rows = turnsByArm.get(arm) ?? [];
        const sessions = jsonl<{ disk: { harness: number; eidnara: number } }>(
            join(armsDir, arm, "sessions.jsonl"),
        );
        const last = sessions[sessions.length - 1]?.disk;
        const mb = (b: number) => (b / 1e6).toFixed(0);
        lines.push(
            `| ${arm} | ${mb(Math.max(0, ...rows.map((r) => r.harnessRss ?? 0)))} | ${mb(rows[rows.length - 1]?.harnessRss ?? 0)} | ${mb(Math.max(0, ...rows.map((r) => r.hostRss ?? 0)))} | ${(Math.max(0, ...rows.map((r) => r.hostCpuMs ?? 0)) / 1000).toFixed(0)} | ${mb(last?.harness ?? 0)} | ${mb(last?.eidnara ?? 0)} |`,
        );
    }
    lines.push("");
    const report = lines.join("\n");
    writeFileSync(join(out, "report.md"), report);
    console.log(report);
}

main();
