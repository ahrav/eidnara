import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

/** A record that does not parse fails the read, since a truncated file is a partial run. */
export function readJsonl<T>(path: string): T[] {
    if (!existsSync(path)) return [];
    const out: T[] = [];
    for (const [i, line] of readFileSync(path, "utf8").split("\n").entries()) {
        if (line.trim().length === 0) continue;
        try {
            out.push(JSON.parse(line) as T);
        } catch (error) {
            throw new Error(`${path}:${i + 1}: ${String(error)}`);
        }
    }
    return out;
}

export interface RunResults<Turn, Call> {
    /** The arms the run was asked for, sorted. */
    arms: string[];
    turnsByArm: Map<string, Turn[]>;
    callsByArm: Map<string, Call[]>;
}

/**
 * Reads a run directory's results. Every requested arm must have recorded as many turns as the
 * world holds and written `done.json` after its teardown; an arm that stopped early would
 * otherwise read as a completed run with smaller denominators.
 */
export function loadRun<Turn, Call>(out: string): RunResults<Turn, Call> {
    const options = JSON.parse(readFileSync(join(out, "options.json"), "utf8")) as {
        arms: string[];
    };
    const world = JSON.parse(readFileSync(join(out, "world.json"), "utf8")) as {
        sessions: { turns: unknown[] }[];
    };
    const expectedTurns = world.sessions.reduce((sum, s) => sum + s.turns.length, 0);
    const arms = [...options.arms].sort();
    const turnsByArm = new Map<string, Turn[]>();
    const callsByArm = new Map<string, Call[]>();
    const incomplete: string[] = [];
    for (const arm of arms) {
        const dir = join(out, "results", arm);
        const turns = readJsonl<Turn>(join(dir, "turns.jsonl"));
        turnsByArm.set(arm, turns);
        callsByArm.set(arm, readJsonl<Call>(join(dir, "calls.jsonl")));
        if (turns.length !== expectedTurns) {
            incomplete.push(`${arm} recorded ${turns.length}/${expectedTurns} turns`);
        } else if (!existsSync(join(dir, "done.json"))) {
            incomplete.push(`${arm} did not finish`);
        }
    }
    if (incomplete.length > 0) throw new Error(`${out}: incomplete arms: ${incomplete.join("; ")}`);
    return { arms, turnsByArm, callsByArm };
}
