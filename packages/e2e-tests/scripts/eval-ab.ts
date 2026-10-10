import { spawnSync } from "node:child_process";
import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
    type Arm,
    type ArmSpec,
    armRoot,
    makeArm,
    procStats,
    sandboxAvailable,
    treeStats,
} from "../src/ab-eval/arms";
import type { CallRecord } from "../src/ab-eval/gateway";
import { buildWorld, grade, type World, writeRepo } from "../src/ab-eval/world";

const REPO_ROOT = resolve(import.meta.dir, "../../..");

interface Options {
    tier: string;
    seed: number;
    arms: string[];
    out: string;
    paceMs: number;
    sessionGapMs: number;
    fixtureBin: string;
    sandbox: boolean;
    enforceWindow: boolean;
    /** `session:turn` keys before whose prompt the arm's daemon stops for `stallMs`. */
    stallAt: Set<string>;
    stallMs: number;
}

function parseArgs(argv: string[]): Options {
    const get = (name: string, fallback: string): string => {
        const i = argv.indexOf(`--${name}`);
        return i >= 0 && argv[i + 1] !== undefined ? (argv[i + 1] as string) : fallback;
    };
    return {
        tier: get("tier", "xs"),
        seed: Number(get("seed", "7")),
        arms: get("arms", "pi-on,pi-off,oc-on,oc-off").split(","),
        out: resolve(get("out", join(tmpdir(), "ab-eval/runs/latest"))),
        paceMs: Number(get("pace-ms", "1500")),
        sessionGapMs: Number(get("session-gap-ms", "30000")),
        fixtureBin: get(
            "fixture-bin",
            join(REPO_ROOT, "target/release/examples/direct_host_fixture"),
        ),
        sandbox:
            get("sandbox", "auto") === "auto"
                ? sandboxAvailable()
                : get("sandbox", "auto") === "on",
        stallAt: new Set(
            get("stall-at", "")
                .split(",")
                .filter((key) => key.length > 0),
        ),
        stallMs: Number(get("stall-ms", "6000")),
        enforceWindow: get("enforce-window", "on") === "on",
    };
}

function specOf(name: string): ArmSpec {
    const [harness, mode] = name.split("-") as [string, string];
    return {
        name,
        harness: harness === "pi" ? "pi" : "opencode",
        eidnara: mode === "on" || mode === "onraw",
        stripClosureTemperature: mode === "on",
    };
}

function prepareWorkdir(world: World, dir: string): void {
    mkdirSync(dir, { recursive: true });
    writeRepo(world, dir);
    writeFileSync(join(dir, ".gitignore"), ".runs/\n");
    spawnSync(
        "sh",
        [
            "-c",
            "git init -q . && git add -A && git -c user.email=ab@eval -c user.name=ab commit -qm init",
        ],
        {
            cwd: dir,
        },
    );
}

async function runArm(arm: Arm, world: World, opts: Options, outDir: string): Promise<void> {
    const turnsFile = join(outDir, "turns.jsonl");
    const log = (msg: string) => console.log(`[${arm.spec.name}] ${msg}`);
    await arm.start();
    log("started");
    for (const session of world.sessions) {
        await arm.openSession(session.index);
        log(`session ${session.index} open (${arm.sessionId()})`);
        for (const turn of session.turns) {
            const key = `${session.index}:${turn.index}`;
            const probed = turn.probe
                ? world.facts.find((f) => f.id === turn.probe?.factId)
                : undefined;
            arm.setTurn(
                turn,
                key,
                probed && probed.kind !== "abstain"
                    ? { answer: probed.answer, ...(probed.stale ? { stale: probed.stale } : {}) }
                    : null,
            );
            const mismatchesBefore = arm.main.scriptMismatches;
            const forwardedBefore = arm.main.records.filter(
                (r) => r.caller !== "main_scripted",
            ).length;
            const hostPid = arm.hostPid();
            if (opts.stallAt.has(key) && hostPid !== undefined) {
                process.kill(hostPid, "SIGSTOP");
                setTimeout(() => process.kill(hostPid, "SIGCONT"), opts.stallMs);
                log(`stalled the daemon for ${opts.stallMs}ms before ${key}`);
            }
            const timeout = turn.kind === "probe" ? 900_000 : 600_000;
            const result = await arm.prompt(turn.user, timeout);
            const harness = treeStats(arm.harnessPid());
            const host = procStats(arm.hostPid());
            const forwarded =
                arm.main.records.slice(0).filter((r) => r.caller !== "main_scripted").length -
                forwardedBefore;
            const fact = turn.probe
                ? world.facts.find((f) => f.id === turn.probe?.factId)
                : undefined;
            const row = {
                arm: arm.spec.name,
                session: session.index,
                turn: turn.index,
                kind: turn.kind,
                ms: Math.round(result.ms),
                error: result.error ?? null,
                scriptMismatch: arm.main.scriptMismatches - mismatchesBefore,
                forwardedMainCalls: forwarded,
                harnessRss: harness?.rss ?? null,
                harnessCpuMs: harness?.cpuMs ?? null,
                hostRss: host?.rss ?? null,
                hostCpuMs: host?.cpuMs ?? null,
                ...(fact
                    ? {
                          factId: fact.id,
                          factKind: fact.kind,
                          scope: turn.probe?.scope,
                          expected: fact.answer,
                          stale: fact.stale ?? null,
                          answer: result.answer.slice(0, 2000),
                          grade: grade(fact, result.answer),
                      }
                    : {}),
                ts: Date.now(),
            };
            appendFileSync(turnsFile, `${JSON.stringify(row)}\n`);
            if (turn.kind === "probe")
                log(
                    `probe ${key} ${fact?.kind} -> ${row.grade} (${Math.round(result.ms)}ms) ${result.answer.slice(0, 80).replace(/\n/g, " ")}`,
                );
            if (result.error) log(`turn ${key} error: ${result.error.slice(0, 300)}`);
            if (turn.index % 25 === 0)
                log(
                    `turn ${key} ${Math.round(result.ms)}ms rss=${Math.round((harness?.rss ?? 0) / 1e6)}MB host=${Math.round((host?.rss ?? 0) / 1e6)}MB`,
                );
            await Bun.sleep(turn.kind === "probe" ? 500 : opts.paceMs);
        }
        arm.setTurn(null, null);
        await arm.closeSession();
        const disk = arm.diskBytes();
        appendFileSync(
            join(outDir, "sessions.jsonl"),
            `${JSON.stringify({ arm: arm.spec.name, session: session.index, disk, host: procStats(arm.hostPid()), ts: Date.now() })}\n`,
        );
        log(`session ${session.index} closed disk=${JSON.stringify(disk)}`);
        await Bun.sleep(opts.sessionGapMs);
    }
    if (arm.host) {
        try {
            const status = await arm.host.primaryStatus(
                arm.sessionId() ?? "",
                arm.ctx.workdir,
                "status",
            );
            writeFileSync(join(outDir, "host-status.json"), JSON.stringify(status, null, 2));
        } catch (error) {
            log(`status failed: ${String(error).slice(0, 200)}`);
        }
        writeFileSync(join(outDir, "host.log"), arm.host.hostLog());
    }
    await arm.stop();
    log("done");
}

async function main(): Promise<void> {
    const opts = parseArgs(process.argv.slice(2));
    mkdirSync(opts.out, { recursive: true });
    if (!opts.sandbox)
        console.warn("arms run unsandboxed: agents can read other arms and the answer key");
    const world = buildWorld(opts.seed, opts.tier);
    writeFileSync(
        join(opts.out, "world.json"),
        JSON.stringify({ ...world, files: Object.keys(world.files).length }, null, 1),
    );
    writeFileSync(
        join(opts.out, "options.json"),
        JSON.stringify({ ...opts, stallAt: [...opts.stallAt] }, null, 2),
    );
    const results = await Promise.allSettled(
        opts.arms.map(async (name) => {
            const spec = specOf(name);
            const root = armRoot(join(opts.out, "arms"), name);
            const workdir = join(root, "work");
            prepareWorkdir(world, workdir);
            const outDir = armRoot(join(opts.out, "results"), name);
            const callsFile = join(outDir, "calls.jsonl");
            const arm = makeArm(spec, {
                root,
                resultsDir: outDir,
                sandboxDir: opts.sandbox ? opts.out : "",
                enforceWindow: opts.enforceWindow,
                workdir,
                fixtureBin: opts.fixtureBin,
                onCall: (record: CallRecord) =>
                    appendFileSync(callsFile, `${JSON.stringify(record)}\n`),
            });
            try {
                await runArm(arm, world, opts, outDir);
                return name;
            } catch (error) {
                console.error(`[${name}] failed: ${String(error)}`);
                await arm.stop().catch(() => undefined);
                throw error;
            }
        }),
    );
    for (const [i, r] of results.entries()) {
        console.log(
            `${opts.arms[i]}: ${r.status}${r.status === "rejected" ? ` ${String(r.reason).slice(0, 500)}` : ""}`,
        );
    }
    const summary = readFileSync(join(opts.out, "options.json"), "utf8");
    console.log(`out: ${opts.out}\n${summary}`);
}

await main();
