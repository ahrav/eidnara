import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";

const CLOCK_TICKS_PER_SECOND = 100;

let pageBytes: number | undefined;

function pageSize(): number {
    if (pageBytes === undefined) {
        const reported = Number(spawnSync("getconf", ["PAGESIZE"], { encoding: "utf8" }).stdout);
        pageBytes = Number.isFinite(reported) && reported > 0 ? reported : 4096;
    }
    return pageBytes;
}

/** Fields of `/proc/<pid>/stat` after the parenthesized command name, which may contain spaces. */
function statFields(pid: number): string[] | null {
    try {
        const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
        return stat.slice(stat.lastIndexOf(")") + 2).split(" ");
    } catch {
        return null;
    }
}

/** `/proc/<pid>/stat` fields 14 and 15 contain utime and stime; field 24 contains rss in pages. */
export function procStats(pid: number | undefined): { rss: number; cpuMs: number } | null {
    if (!pid) return null;
    const fields = statFields(pid);
    if (!fields) return null;
    const ticks = Number(fields[11]) + Number(fields[12]);
    return {
        rss: Number(fields[21]) * pageSize(),
        cpuMs: (ticks * 1000) / CLOCK_TICKS_PER_SECOND,
    };
}

/**
 * `/proc/<pid>/task/<tid>/children` lists each thread's direct children, so the walk reads one
 * children file per thread of each process in the tree. If `CHILDREN_FILES` is false, the walk
 * shares one `/proc` parent scan.
 */
const CHILDREN_FILES = existsSync(`/proc/${process.pid}/task/${process.pid}/children`);

function childrenOf(pid: number, parents: () => Map<number, number[]>): number[] {
    if (!CHILDREN_FILES) return parents().get(pid) ?? [];
    let tasks: string[];
    try {
        tasks = readdirSync(`/proc/${pid}/task`);
    } catch {
        return [];
    }
    const out: number[] = [];
    for (const tid of tasks) {
        let listed: string;
        try {
            listed = readFileSync(`/proc/${pid}/task/${tid}/children`, "utf8");
        } catch {
            continue;
        }
        for (const child of listed.split(" ")) {
            if (child.trim().length > 0) out.push(Number(child));
        }
    }
    return out;
}

function scanParents(): Map<number, number[]> {
    const parents = new Map<number, number[]>();
    for (const entry of readdirSync("/proc")) {
        if (!/^\d+$/.test(entry)) continue;
        const fields = statFields(Number(entry));
        if (!fields) continue;
        const ppid = Number(fields[1]);
        const siblings = parents.get(ppid);
        if (siblings) siblings.push(Number(entry));
        else parents.set(ppid, [Number(entry)]);
    }
    return parents;
}

function lazyParents(): () => Map<number, number[]> {
    let parents: Map<number, number[]> | undefined;
    return () => {
        parents ??= scanParents();
        return parents;
    };
}

export function descendants(pid: number): number[] {
    const parents = lazyParents();
    const out: number[] = [];
    const queue = [pid];
    for (let next = queue.shift(); next !== undefined; next = queue.shift()) {
        for (const child of childrenOf(next, parents)) {
            out.push(child);
            queue.push(child);
        }
    }
    return out;
}

export function treeStats(pid: number | undefined): { rss: number; cpuMs: number } | null {
    if (!pid) return null;
    const own = procStats(pid);
    if (!own) return null;
    for (const child of descendants(pid)) {
        const s = procStats(child);
        if (s) {
            own.rss += s.rss;
            own.cpuMs += s.cpuMs;
        }
    }
    return own;
}

function ownerOf(pid: number): number | undefined {
    try {
        return statSync(`/proc/${pid}`).uid;
    } catch {
        return undefined;
    }
}

/** Every process in the tree under `rootPid`, including the root, that the invoking user owns. */
export function ownedProcesses(rootPid: number): number[] {
    const uid = process.getuid?.();
    const parents = lazyParents();
    const out: number[] = [];
    const queue = [rootPid];
    for (let next = queue.shift(); next !== undefined; next = queue.shift()) {
        if (ownerOf(next) === uid) out.push(next);
        queue.push(...childrenOf(next, parents));
    }
    return out;
}

/** The process exists and has not become a zombie. */
export function running(pid: number): boolean {
    const fields = statFields(pid);
    return fields !== null && fields[0] !== "Z";
}

function signal(pid: number, name: NodeJS.Signals): void {
    try {
        process.kill(pid, name);
    } catch {}
}

async function waitStopped(pids: number[], timeoutMs: number): Promise<boolean> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
        if (!pids.some(running)) return true;
        if (Date.now() >= deadline) return false;
        await Bun.sleep(50);
    }
}

export async function stopOwnedTree(rootPid: number, graceMs: number): Promise<void> {
    const targets = ownedProcesses(rootPid);
    for (const pid of targets) signal(pid, "SIGTERM");
    if (await waitStopped(targets, graceMs)) return;
    for (const pid of targets) signal(pid, "SIGKILL");
    await waitStopped(targets, 2_000);
}

/**
 * Stops `pid` and returns once it is in the stopped state, then resumes it after `ms`. Signal
 * delivery is asynchronous, so the caller's next request reaches a stopped daemon only after the
 * state is observed. A process that exited in between needs no resume, and the pending resume
 * never holds the runner open.
 */
export async function stallFor(pid: number, ms: number): Promise<void> {
    process.kill(pid, "SIGSTOP");
    const deadline = Date.now() + 2_000;
    while (!/^[Tt]$/.test(statFields(pid)?.[0] ?? "")) {
        if (Date.now() >= deadline) throw new Error(`process ${pid} did not stop within 2s`);
        await Bun.sleep(5);
    }
    setTimeout(() => signal(pid, "SIGCONT"), ms).unref();
}
