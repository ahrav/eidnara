/**
 *
 */

import { createHash } from "node:crypto";
import { chmodSync, closeSync, existsSync, mkdirSync, openSync, rmSync, writeSync } from "node:fs";
import { join, resolve } from "node:path";

export interface CaseWorkspace {
    /* */
    root: string;
    home: string;
    tmp: string;
    /** The case owns this store root. */
    store: string;
    diagnosticsDir: string;
    /* */
    storeNamespace: string;
}

export function createCaseWorkspace(
    parentDir: string,
    variantId: string,
    runNonce: string,
): CaseWorkspace {
    const suffix = `${variantId}-${runNonce.slice(0, 8)}`;
    // The root name is a digest, not the variant id: the direct host binds a Unix socket
    // under the case `TMPDIR`, and the whole path must stay under `SUN_LEN` (108 bytes).
    const rootName = `case-${createHash("sha256").update(suffix).digest("hex").slice(0, 12)}`;
    const root = resolve(parentDir, rootName);
    mkdirSync(root, { recursive: true, mode: 0o700 });
    chmodSync(root, 0o700); // mkdir mode is masked by umask; make 0o700 unconditional
    const sub = (name: string): string => {
        const path = join(root, name);
        mkdirSync(path, { mode: 0o700 });
        // chmodSync restores 0o700 after umask masks mkdirSync's mode.
        chmodSync(path, 0o700);
        return path;
    };
    return {
        root,
        home: sub("home"),
        tmp: sub("tmp"),
        store: sub("store"),
        diagnosticsDir: sub("diagnostics"),
        storeNamespace: `incident-${suffix}`,
    };
}

export function destroyCaseWorkspace(workspace: CaseWorkspace): void {
    rmSync(workspace.root, { recursive: true, force: true });
}

/**
 *
 *
 */
export const CASE_ENV_ALLOWLIST = [
    "PATH",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "TERM",
    "TZ",
    "USER",
    "LOGNAME",
    "SHELL",
    "RUSTUP_HOME",
    "EIDNARA_E2E_DIRECT_HOST_FIXTURE_BIN",
] as const;

/* */
export function buildCaseEnv(
    workspace: CaseWorkspace,
    baseEnv: Record<string, string | undefined> = process.env,
): Record<string, string> {
    const env: Record<string, string> = {};
    for (const key of CASE_ENV_ALLOWLIST) {
        const value = baseEnv[key];
        if (typeof value === "string") env[key] = value;
    }
    env.HOME = workspace.home;
    env.TMPDIR = workspace.tmp;
    env.TMP = workspace.tmp;
    env.TEMP = workspace.tmp;
    env.XDG_CONFIG_HOME = join(workspace.home, ".config");
    env.XDG_DATA_HOME = join(workspace.home, ".local", "share");
    env.XDG_STATE_HOME = join(workspace.home, ".local", "state");
    env.XDG_CACHE_HOME = join(workspace.home, ".cache");
    env.CARGO_HOME = join(workspace.home, ".cargo");
    // The rustup proxies resolve toolchains from `RUSTUP_HOME`, defaulting to `$HOME/.rustup`.
    // The relocated HOME has no toolchains, so a parent that relies on the default (CI runners
    // do) is forwarded the real path explicitly; the directory holds no credentials.
    if (env.RUSTUP_HOME === undefined && typeof baseEnv.HOME === "string") {
        const defaultRustupHome = join(baseEnv.HOME, ".rustup");
        if (existsSync(defaultRustupHome)) env.RUSTUP_HOME = defaultRustupHome;
    }
    return env;
}

/**
 * */
export function isLoopbackUrl(raw: string): boolean {
    let url: URL;
    try {
        url = new URL(raw);
    } catch {
        return false;
    }
    const hostname = url.hostname.replace(/^\[|\]$/g, "").toLowerCase();
    if (hostname === "localhost" || hostname === "::1") return true;
    return /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(hostname);
}

/**
 * */
export function assertLoopbackProviderEndpoints(endpoints: Record<string, string>): void {
    for (const [name, url] of Object.entries(endpoints)) {
        if (!isLoopbackUrl(url)) {
            throw new Error(
                `provider endpoint ${name} is not a loopback URL; refusing to run the case`,
            );
        }
    }
}

/**
 */
export class DiagnosticSink {
    bytesWritten = 0;
    truncated = false;
    #fd: number;
    #capBytes: number;
    #closed = false;

    constructor(path: string, capBytes: number) {
        if (!Number.isInteger(capBytes) || capBytes <= 0) {
            throw new Error("diagnostic sink cap must be a positive integer");
        }
        this.#fd = openSync(path, "wx", 0o600);
        this.#capBytes = capBytes;
    }

    write(chunk: Uint8Array | string): void {
        if (this.#closed) return;
        const buffer = typeof chunk === "string" ? Buffer.from(chunk, "utf8") : Buffer.from(chunk);
        const remaining = this.#capBytes - this.bytesWritten;
        if (buffer.length > remaining) this.truncated = true;
        if (remaining <= 0) return;
        const slice = buffer.subarray(0, Math.min(buffer.length, remaining));
        writeSync(this.#fd, slice);
        this.bytesWritten += slice.length;
    }

    close(): void {
        if (this.#closed) return;
        this.#closed = true;
        closeSync(this.#fd);
    }
}
