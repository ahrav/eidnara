import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { HostClient } from "@eidnara/opencode/shared/host-client";
import {
    connectionFilePath,
    coordinationDirPath,
} from "@eidnara/opencode/shared/host-lifecycle/paths";
import type { HarnessHost } from "../rust-harness";
import { SEARCH_ADMISSION_RECORDS } from "./daemon-examples";
import { buildDaemonExample } from "./hermetic-host";

export const PAYLOAD_DIR_ENV = "EIDNARA_E2E_PAYLOAD_DIR";

const LAUNCHER = "payload/bin/eidnara-host";
const BUNDLE_MANIFEST = "payload/model/gte-modernbert-base-f32/manifest.json";
const PAYLOAD_MANIFEST = "payload-manifest.json";
const MAX_LOG_BYTES = 16 * 1024;

export function missingPayloadFiles(payloadDir: string): string[] {
    return [LAUNCHER, BUNDLE_MANIFEST, PAYLOAD_MANIFEST].filter(
        (file) => !existsSync(join(payloadDir, file)),
    );
}

/** The digest `--payload-manifest-digest` carries: SHA-256 of the manifest without its final newline. */
export function payloadManifestDigest(payloadDir: string): string {
    const bytes = readFileSync(join(payloadDir, PAYLOAD_MANIFEST));
    const body = bytes.at(-1) === 0x0a ? bytes.subarray(0, -1) : bytes;
    return createHash("sha256").update(body).digest("hex");
}

/** The `search_admission` block of `session.status`: `admitted`, or `refused` with the gate's code. */
export interface SearchAdmission {
    state: string;
    reason?: string;
}

/** One request over a Context route bound to `projectRoot` on the host behind `connectionFile`; the route and the connection are closed before the answer returns. */
async function contextRequest(
    connectionFile: string,
    projectRoot: string,
    sessionId: string,
    body: Record<string, unknown>,
): Promise<unknown> {
    const identity = {
        project_root: resolve(projectRoot),
        harness: "opencode",
        session: sessionId,
    };
    const client = await HostClient.connect({
        connectionFile,
        identity,
        targetKind: "tool_provider",
    });
    try {
        const route = await client.routeOpen(
            { kind: "tool_provider", module_id: "context" },
            identity,
        );
        try {
            return await client.request(route, { ...body, session_id: sessionId });
        } finally {
            await client.closeRoute(route).catch(() => undefined);
        }
    } finally {
        await client.closeAsync().catch(() => undefined);
    }
}

/** The `search_admission` block `session.status` reports for a session of `projectRoot` on the host behind `connectionFile`. */
export async function searchAdmission(
    connectionFile: string,
    projectRoot: string,
    sessionId: string,
): Promise<SearchAdmission | undefined> {
    const status = (await contextRequest(connectionFile, projectRoot, sessionId, {
        method: "session.status",
    })) as { search_admission?: SearchAdmission } | null;
    return status?.search_admission;
}

/** The `progress` block of `search.lifecycle.status`: the projection's lag behind the kernel tip and the slice loop's rebuild outcomes. */
export interface SearchProgress {
    tip: number | null;
    checkpoint: number | null;
    lag_commits: number | null;
    rebuilds: { recorded: number; completed: number; renewed: number };
    blocked: string | null;
}

/** The `status` answer of `search.lifecycle.status`. */
export interface SearchLifecycleStatus {
    kind: "status";
    record: { state: string; cause?: string };
    admission: SearchAdmission;
    progress: SearchProgress;
}

/** Any other answer of `search.lifecycle.status`: a terminal from a daemon with no lifecycle owner or one whose progress read failed, or a kernel state. */
export interface SearchLifecycleRefusal {
    kind?: string;
    state?: unknown;
}

/** Whether `answer` is the `status` answer, which alone carries `record` and `progress`. */
export function isSearchLifecycleStatus(
    answer: SearchLifecycleStatus | SearchLifecycleRefusal,
): answer is SearchLifecycleStatus {
    return answer.kind === "status";
}

/** `search.lifecycle.status` of the host behind `connectionFile`, asked over a route bound to `projectRoot`. */
export async function searchLifecycleStatus(
    connectionFile: string,
    projectRoot: string,
    sessionId: string,
): Promise<SearchLifecycleStatus | SearchLifecycleRefusal> {
    return (await contextRequest(connectionFile, projectRoot, sessionId, {
        method: "search.lifecycle.status",
        project_root: resolve(projectRoot),
    })) as SearchLifecycleStatus | SearchLifecycleRefusal;
}

/** Polls `search.lifecycle.status` until the projection is Current, admitted, and at the kernel tip, and returns that status. */
export async function waitForSearchCaughtUp(
    host: { connectionFile: string; hostLog(): string },
    projectRoot: string,
    sessionId: string,
    timeoutMs: number,
): Promise<SearchLifecycleStatus> {
    const deadline = Date.now() + timeoutMs;
    let last: SearchLifecycleStatus | SearchLifecycleRefusal | undefined;
    while (Date.now() < deadline) {
        last = await searchLifecycleStatus(host.connectionFile, projectRoot, sessionId);
        if (
            isSearchLifecycleStatus(last) &&
            last.record.state === "current" &&
            last.admission.state === "admitted" &&
            last.progress.lag_commits === 0
        ) {
            return last;
        }
        await Bun.sleep(500);
    }
    throw new Error(
        `the search projection did not catch up within ${timeoutMs}ms: ${JSON.stringify(last ?? null)}\n${host.hostLog()}`,
    );
}

/** Polls `session.status` until the session's search admission reads `admitted`. */
export async function waitForSearchAdmitted(
    host: { connectionFile: string; hostLog(): string },
    projectRoot: string,
    sessionId: string,
    timeoutMs: number,
): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    let last: SearchAdmission | undefined;
    while (Date.now() < deadline) {
        last = await searchAdmission(host.connectionFile, projectRoot, sessionId);
        if (last?.state === "admitted") return;
        await Bun.sleep(500);
    }
    throw new Error(
        `search admission did not open within ${timeoutMs}ms: ${JSON.stringify(last ?? null)}\n${host.hostLog()}`,
    );
}

interface CommandResult {
    status: number | null;
    stdout: string;
    stderr: string;
}

export class PayloadHost implements HarnessHost {
    readonly connectionFile: string;
    private readonly launcher: string;

    private constructor(
        private readonly dataDir: string,
        private readonly payloadDir: string,
    ) {
        this.connectionFile = connectionFilePath(dataDir);
        this.launcher = join(payloadDir, LAUNCHER);
    }

    static start(dataDir: string, payloadDir: string): PayloadHost {
        const host = new PayloadHost(resolve(dataDir), resolve(payloadDir));
        const started = host.lifecycle([
            "start",
            "--payload-dir",
            host.payloadDir,
            "--payload-manifest-digest",
            payloadManifestDigest(host.payloadDir),
        ]);
        if (started.status !== 0 || !started.stdout.includes('"ok":true')) {
            const log = host.hostLog();
            // A daemon the launcher spawned before reporting failure is stopped here, since the harness receives no host to tear down.
            host.lifecycle(["stop"]);
            throw new Error(`payload host did not start: ${describe(started)}\n${log}`);
        }
        return host;
    }

    async installSearchAdmission(): Promise<void> {
        const generator = await buildDaemonExample(SEARCH_ADMISSION_RECORDS);
        const out = mkdtempSync(join(tmpdir(), "eidnara-admission-"));
        try {
            const written = spawnSync(generator, [join(this.payloadDir, BUNDLE_MANIFEST), out], {
                encoding: "utf8",
            });
            if (written.status !== 0) {
                throw new Error(`admission records were not written: ${describe(written)}`);
            }
            const installed = this.lifecycle([
                "install-search-admission",
                join(out, "runtime-manifest.json"),
                join(out, "campaign-evidence.json"),
            ]);
            if (installed.status !== 0 || installed.stdout.trim() !== "installed") {
                throw new Error(`admission records were not installed: ${describe(installed)}`);
            }
        } finally {
            rmSync(out, { recursive: true, force: true });
        }
    }

    async waitForAdmitted(
        projectRoot: string,
        sessionId: string,
        timeoutMs: number,
    ): Promise<void> {
        await waitForSearchAdmitted(this, projectRoot, sessionId, timeoutMs);
    }

    hostLog(): string {
        try {
            return readFileSync(
                join(coordinationDirPath(this.dataDir), "eidnara.log"),
                "utf8",
            ).slice(-MAX_LOG_BYTES);
        } catch {
            return "";
        }
    }

    async stop(): Promise<void> {
        const stopped = this.lifecycle(["stop"]);
        if (stopped.status !== 0) {
            throw new Error(`payload host did not stop: ${describe(stopped)}`);
        }
        rmSync(this.dataDir, { recursive: true, force: true });
    }

    private lifecycle(args: string[]): CommandResult {
        return spawnSync(this.launcher, args, {
            encoding: "utf8",
            env: {
                PATH: process.env.PATH ?? "/usr/bin:/bin",
                XDG_DATA_HOME: this.dataDir,
                EIDNARA_HOST_TEST_PHASE_CAP_MS: "30000",
            },
        });
    }
}

function describe(result: CommandResult): string {
    return `exit ${result.status}: ${result.stdout.trim()} ${result.stderr.trim()}`.slice(0, 4096);
}
