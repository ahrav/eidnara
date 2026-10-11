import { spawnSync } from "node:child_process";
import { appendFileSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { type Arm, armRoot, armSpec, makeArm } from "../src/ab-eval/arms";
import type { CallRecord } from "../src/ab-eval/gateway";
import { procStats, stallFor, treeStats } from "../src/ab-eval/procs";
import { claimRunDir, pointLatest, timestampedRunDir } from "../src/ab-eval/run-dir";
import { assertRunRootMaskable, sandboxAvailable, sharedKeep } from "../src/ab-eval/sandbox";
import { buildWorld, grade, PROJECT_SIZES, type World, writeRepo } from "../src/ab-eval/world";
import {
    cargoBuildExampleArgs,
    DIRECT_HOST_FIXTURE,
    isExecutableFile,
} from "../src/rust-runner/daemon-examples";

const REPO_ROOT = resolve(import.meta.dir, "../../..");

interface Options {
    tier: string;
    seed: number;
    arms: string[];
    out: string;
    /** True when `out` is the default timestamped directory, which `runs/latest` then names. */
    linkLatest: boolean;
    paceMs: number;
    sessionGapMs: number;
    fixtureBin: string;
    sandbox: boolean;
    enforceWindow: boolean;
    projectSize: string;
    /** `session:turn` keys before whose prompt the arm's daemon stops for `stallMs`. */
    stallAt: Set<string>;
    stallMs: number;
}

function parseArgs(argv: string[]): Options {
    const get = (name: string, fallback: string): string => {
        const i = argv.indexOf(`--${name}`);
        if (i < 0) return fallback;
        const value = argv[i + 1];
        if (value === undefined || value.startsWith("--"))
            throw new Error(`--${name} needs a value`);
        return value;
    };
    const num = (name: string, fallback: string): number => {
        const value = Number(get(name, fallback));
        if (!Number.isFinite(value) || value < 0) {
            throw new Error(`--${name} must be a non-negative number`);
        }
        return value;
    };
    const choice = <T extends string>(name: string, values: readonly T[], fallback: T): T => {
        const value = get(name, fallback);
        if (!values.includes(value as T)) {
            throw new Error(`--${name} ${JSON.stringify(value)}; accepted: ${values.join(", ")}`);
        }
        return value as T;
    };
    const out = get("out", "");
    const arms = get("arms", "pi-on,pi-off,oc-on,oc-off").split(",");
    const repeated = arms.find((name, i) => arms.indexOf(name) !== i);
    if (repeated !== undefined) throw new Error(`--arms names ${repeated} twice`);
    const sandbox = choice("sandbox", ["auto", "on", "off"], "auto");
    return {
        tier: get("tier", "xs"),
        seed: num("seed", "7"),
        arms,
        out: out ? resolve(out) : timestampedRunDir(join(tmpdir(), "ab-eval/runs")),
        linkLatest: !out,
        paceMs: num("pace-ms", "1500"),
        sessionGapMs: num("session-gap-ms", "30000"),
        fixtureBin: resolve(
            get("fixture-bin", join(REPO_ROOT, "target/release/examples/direct_host_fixture")),
        ),
        sandbox: sandbox === "auto" ? sandboxAvailable() : sandbox === "on",
        stallAt: new Set(
            get("stall-at", "")
                .split(",")
                .filter((key) => key.length > 0),
        ),
        stallMs: num("stall-ms", "6000"),
        enforceWindow: choice("enforce-window", ["on", "off"], "on") === "on",
        projectSize: choice("project-size", Object.keys(PROJECT_SIZES), "small"),
    };
}

function prepareWorkdir(world: World, dir: string): void {
    mkdirSync(dir, { recursive: true });
    writeRepo(world, dir);
    writeFileSync(join(dir, ".gitignore"), ".runs/\n");
    const init = spawnSync(
        "sh",
        [
            "-c",
            "git init -q . && git add -A && git -c user.email=ab@eval -c user.name=ab commit -qm init",
        ],
        { cwd: dir, encoding: "utf8" },
    );
    if (init.status !== 0) {
        throw new Error(
            `git setup of ${dir} failed (${init.status ?? init.error?.message}): ${init.stderr.trim()}`,
        );
    }
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
            const timeout = turn.kind === "probe" ? 900_000 : 600_000;
            arm.setTurn({
                key,
                turn,
                probe:
                    probed && probed.kind !== "abstain"
                        ? {
                              answer: probed.answer,
                              ...(probed.stale ? { stale: probed.stale } : {}),
                          }
                        : null,
                deadlineAt: Date.now() + timeout,
            });
            const mismatchesBefore = arm.main.scriptMismatches;
            const forwardedBefore = arm.main.forwardedCalls;
            const hostPid = arm.hostPid();
            if (opts.stallAt.has(key) && hostPid !== undefined) {
                stallFor(hostPid, opts.stallMs);
                log(`stalled the daemon for ${opts.stallMs}ms before ${key}`);
            }
            const result = await arm.prompt(turn.user, timeout);
            const harness = treeStats(arm.harnessPid());
            const host = procStats(arm.hostPid());
            const forwarded = arm.main.forwardedCalls - forwardedBefore;
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
        arm.setTurn(null);
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
    const specs = opts.arms.map(armSpec);
    if (specs.some((spec) => spec.eidnara) && !isExecutableFile(opts.fixtureBin)) {
        throw new Error(
            `${opts.fixtureBin} is not an executable file; build the release fixture with \`cargo ${cargoBuildExampleArgs(DIRECT_HOST_FIXTURE).join(" ")} --release\` or pass --fixture-bin`,
        );
    }
    const componentCount = PROJECT_SIZES[opts.projectSize];
    if (componentCount === undefined) throw new Error(`unknown --project-size ${opts.projectSize}`);
    const world = buildWorld(opts.seed, opts.tier, componentCount);
    const turnKeys = new Set(
        world.sessions.flatMap((s) => s.turns.map((t) => `${s.index}:${t.index}`)),
    );
    const missing = [...opts.stallAt].filter((key) => !turnKeys.has(key));
    if (missing.length > 0) {
        throw new Error(`--stall-at names turns the world does not have: ${missing.join(", ")}`);
    }
    claimRunDir(opts.out);
    if (opts.linkLatest) pointLatest(dirname(opts.out), opts.out);
    if (opts.sandbox) assertRunRootMaskable(realpathSync(opts.out), sharedKeep());
    else console.warn("arms run unsandboxed: agents can read other arms and the answer key");
    writeFileSync(
        join(opts.out, "world.json"),
        JSON.stringify({ ...world, files: Object.keys(world.files).length }, null, 1),
    );
    writeFileSync(
        join(opts.out, "options.json"),
        JSON.stringify({ ...opts, stallAt: [...opts.stallAt] }, null, 2),
    );
    const results = await Promise.allSettled(
        specs.map(async (spec) => {
            const name = spec.name;
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
    if (results.some((r) => r.status === "rejected")) process.exitCode = 1;
    const summary = readFileSync(join(opts.out, "options.json"), "utf8");
    console.log(`out: ${opts.out}\n${summary}`);
}

await main();
