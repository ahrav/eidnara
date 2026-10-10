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

    async searchAdmission(
        projectRoot: string,
        sessionId: string,
    ): Promise<SearchAdmission | undefined> {
        const identity = {
            project_root: resolve(projectRoot),
            harness: "opencode",
            session: sessionId,
        };
        const client = await HostClient.connect({
            connectionFile: this.connectionFile,
            identity,
            targetKind: "tool_provider",
        });
        try {
            const route = await client.routeOpen(
                { kind: "tool_provider", module_id: "context" },
                identity,
            );
            try {
                const status = (await client.request(route, {
                    method: "session.status",
                    v: 1,
                    session_id: sessionId,
                })) as { search_admission?: SearchAdmission } | null;
                return status?.search_admission;
            } finally {
                await client.closeRoute(route).catch(() => undefined);
            }
        } finally {
            await client.closeAsync().catch(() => undefined);
        }
    }

    async waitForAdmitted(
        projectRoot: string,
        sessionId: string,
        timeoutMs: number,
    ): Promise<void> {
        const deadline = Date.now() + timeoutMs;
        let last: SearchAdmission | undefined;
        while (Date.now() < deadline) {
            last = await this.searchAdmission(projectRoot, sessionId);
            if (last?.state === "admitted") return;
            await Bun.sleep(500);
        }
        throw new Error(
            `search admission did not open within ${timeoutMs}ms: ${JSON.stringify(last ?? null)}\n${this.hostLog()}`,
        );
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
