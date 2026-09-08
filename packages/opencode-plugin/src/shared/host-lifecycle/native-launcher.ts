/**
 * Bounded native lifecycle subprocess invocation.
 *
 * Production launch maps the retained verified launcher descriptor to one
 * fixed child fd through Node/Bun stdio numeric mapping and spawns the
 * Linux descriptor exec path (`/proc/self/fd/<n>`) with `shell:false`, an
 * environment holding only the caller's data root as `XDG_DATA_HOME` plus
 * any explicit additions, and the startup envelope on stdin only. A dev/test
 * injection point spawns an explicit binary path instead (this repo's
 * cargo-built `eidnara-host`); production callers never take that branch
 * with untrusted input because the target is constructed by policy, not
 * configuration.
 *
 * Output handling is fail-closed: stdout must be exactly one v1 JSON object,
 * the exit code must agree with `ok`, and stderr is tainted — drained and
 * discarded, so it never reaches an error or result.
 */

import { type ChildProcess, spawn } from "node:child_process";
import * as path from "node:path";
import {
    ContractViolation,
    type DaemonResultV1,
    exitAgreesWithResult,
    parseDaemonResult,
} from "./contract";

/** The fixed collision-free child descriptor for the retained launcher. */
export const LAUNCHER_CHILD_FD = 3;

export type NativeLaunchTarget =
    | { kind: "retained-fd"; fd: number }
    /**
     * Dev/test injection point. `path` MUST be absolute: the child is spawned
     * with `cwd: "/"`, so a relative path is resolved against the filesystem
     * root rather than the caller's directory.
     */
    | { kind: "test-binary"; path: string };

export type NativeLifecycleCommand = "start" | "stop" | "restart" | "probe";

export type NativeLaunchFailureCode =
    | "spawn_failed"
    | "unsupported_platform"
    | "timeout"
    | "signal_exit"
    | "output_cap_exceeded"
    | "malformed_output"
    | "exit_disagreement"
    | "command_mismatch"
    | "usage_error";

/**
 * Whether a failure with this code was raised after `spawn` returned a child.
 * `timeout` is raised on both sides of the spawn, so that site states it explicitly.
 */
const CHILD_SPAWNED_BY_CODE: Readonly<Record<NativeLaunchFailureCode, boolean>> = {
    spawn_failed: false,
    unsupported_platform: false,
    usage_error: false,
    timeout: true,
    signal_exit: true,
    output_cap_exceeded: true,
    malformed_output: true,
    exit_disagreement: true,
    command_mismatch: true,
};

/** Typed launch failure. Never carries stdout/stderr bytes or raw paths. */
export class NativeLaunchError extends Error {
    /** A failure raised after the child existed leaves its effects unknown; one raised before it committed nothing. */
    readonly childSpawned: boolean;

    constructor(
        readonly code: NativeLaunchFailureCode,
        message: string,
        options: { childSpawned?: boolean } = {},
    ) {
        super(message);
        this.name = "NativeLaunchError";
        this.childSpawned = options.childSpawned ?? CHILD_SPAWNED_BY_CODE[code];
    }
}

export interface NativeLaunchOptions {
    command: NativeLifecycleCommand;
    /**
     * Exported to the child as `XDG_DATA_HOME`. The binary derives its data
     * directory only from `XDG_DATA_HOME` and `HOME`; `dataRoot` must be absolute.
     */
    dataRoot: string;
    /**
     * Dev/test staging source forwarded as `--payload-dir`. MUST be absolute:
     * the child runs with `cwd: "/"` and resolves it there.
     */
    payloadDir?: string;
    /** Parent-trusted canonical manifest digest for a production payload directory. */
    payloadManifestDigest?: string;
    /** JSON-serializable startup envelope written to stdin, or null. */
    envelope?: unknown;
    /** Wall-clock budget for the whole call, measured from entry. */
    deadlineMs: number;
    /** Additional child environment. `XDG_DATA_HOME` is owned by `dataRoot` and may not appear here. */
    env?: Record<string, string>;
    /** Host platform override for the retained-descriptor exec path; tests only. */
    platform?: NodeJS.Platform;
}

export interface NativeHarnessCandidate {
    manifest_sha256: string;
    source_roots: Record<string, string>;
}

export interface NativeStartupEnvelope {
    schema: 1;
    opencode?: NativeHarnessCandidate;
    pi?: NativeHarnessCandidate;
    credentials?: Record<string, string>;
}

const MAX_STDOUT_BYTES = 256 * 1024;
const STDIO_FLUSH_GRACE_MS = 250;
/** The host rejects stdin envelopes larger than 64 KiB (`spawn.rs` `MAX_ENVELOPE_BYTES`). */
const MAX_ENVELOPE_BYTES = 64 * 1024;
/**
 * Largest delay `setTimeout` honors. Node and Bun coerce a larger value to 1ms
 * with a `TimeoutOverflowWarning`.
 */
const MAX_TIMER_DELAY_MS = 2_147_483_647;

/** The binary answers `probe` under the contracted read-only command name. */
function expectedResultCommand(command: NativeLifecycleCommand): DaemonResultV1["command"] {
    return command === "probe" ? "status" : command;
}

interface CollectedExit {
    exitCode: number | null;
    signal: NodeJS.Signals | null;
    /** Raw bytes: decoding is a validation step, not a convenience. */
    stdout: Buffer;
    timedOut: boolean;
    outputCapExceeded: boolean;
    /** A read error on stdout; the buffered bytes may be a prefix of what the child wrote. */
    stdoutReadFailed: boolean;
}

function retainedFdExecPath(platform: NodeJS.Platform): string | null {
    return platform === "linux" ? `/proc/self/fd/${LAUNCHER_CHILD_FD}` : null;
}

/** `deadlineAt` uses the `performance.now()` timebase. */
function collectChild(child: ChildProcess, deadlineAt: number): Promise<CollectedExit> {
    return new Promise((resolve, reject) => {
        let stdoutLen = 0;
        const stdoutChunks: Buffer[] = [];
        let timedOut = false;
        let outputCapExceeded = false;
        let settled = false;
        let stdioGrace: ReturnType<typeof setTimeout> | null = null;
        // Computed after `spawn` returned, so the synchronous spawn cost is charged to the budget.
        const timer = setTimeout(
            () => {
                timedOut = true;
                child.kill("SIGKILL");
            },
            Math.max(0, deadlineAt - performance.now()),
        );
        child.stdout?.on("data", (chunk: Buffer) => {
            stdoutLen += chunk.length;
            if (stdoutLen > MAX_STDOUT_BYTES) {
                // Its own flag, never `timedOut = false`: buffered stdout can
                // still arrive after the deadline timer's SIGKILL, and clearing
                // `timedOut` there would report a real deadline expiry as a
                // bare signal exit.
                outputCapExceeded = true;
                child.kill("SIGKILL");
                return;
            }
            stdoutChunks.push(chunk);
        });
        // Pipe read errors are emitted on the stream, not on the ChildProcess
        // `error` event; `close` still settles the promise.
        child.stdout?.on("error", () => {});
        child.stderr?.on("error", () => {});
        // Stderr is tainted diagnostics: drain it so a child writing more than
        // one pipe buffer cannot block, and discard every byte — it never
        // reaches an error or result. Draining keeps the read end open for the
        // child's whole run; closing it early would make the next child-side
        // write take EPIPE/SIGPIPE and turn a healthy run into a signal exit.
        child.stderr?.resume();
        child.on("error", (error) => {
            if (settled) return;
            settled = true;
            clearTimeout(timer);
            if (stdioGrace !== null) clearTimeout(stdioGrace);
            reject(new NativeLaunchError("spawn_failed", `native spawn failed: ${error.name}`));
        });
        // The `exit` handler waits STDIO_FLUSH_GRACE_MS before destroying the
        // pipes so inherited descriptors cannot delay `close` indefinitely. The
        // wait is capped by the remaining budget so `close` cannot land past the
        // caller's deadline.
        child.on("exit", () => {
            stdioGrace = setTimeout(
                () => {
                    child.stdout?.destroy();
                    child.stderr?.destroy();
                },
                Math.min(STDIO_FLUSH_GRACE_MS, Math.max(0, deadlineAt - performance.now())),
            );
        });
        child.on("close", (exitCode, signal) => {
            if (settled) return;
            settled = true;
            clearTimeout(timer);
            if (stdioGrace !== null) clearTimeout(stdioGrace);
            resolve({
                exitCode,
                signal,
                stdout: Buffer.concat(stdoutChunks),
                timedOut,
                outputCapExceeded,
                // `destroy(error)` records the stream error in `errored`, even when no `error` event is emitted.
                stdoutReadFailed: (child.stdout?.errored ?? null) !== null,
            });
        });
    });
}

/**
 * Run one native lifecycle command and return its validated v1 result.
 * Every non-conforming outcome — spawn failure, deadline kill, signal exit,
 * extra stdout bytes, unknown fields, exit/JSON disagreement, a result for a
 * different command, usage exit — is a typed {@link NativeLaunchError};
 * secrets, stderr text, and raw paths never ride on it.
 */
export async function runNativeLifecycle(
    target: NativeLaunchTarget,
    options: NativeLaunchOptions,
): Promise<DaemonResultV1> {
    // The budget covers the whole call, including pre-spawn work; the child receives the remaining time.
    const startedAt = performance.now();
    const args: string[] = [options.command];
    if (options.payloadDir !== undefined) {
        args.push("--payload-dir", options.payloadDir);
    }
    if (options.payloadManifestDigest !== undefined) {
        args.push("--payload-manifest-digest", options.payloadManifestDigest);
    }
    // Validated before anything is spawned, for the same reason the envelope is:
    // an exhausted or malformed budget must produce a typed error with no child
    // in flight. `setTimeout` coerces a nonpositive or non-finite delay to 1ms,
    // so without this a mutating lifecycle transaction would start and then be
    // SIGKILLed a millisecond later — filesystem and daemon effects from a call
    // that had no execution budget at all.
    if (
        !Number.isFinite(options.deadlineMs) ||
        options.deadlineMs <= 0 ||
        options.deadlineMs > MAX_TIMER_DELAY_MS
    ) {
        throw new NativeLaunchError(
            "usage_error",
            "native lifecycle deadline is not a positive duration within the timer bound",
        );
    }
    // Path inputs are checked here for the same reason, because the child is
    // spawned with `cwd: "/"`: a relative value silently changes meaning rather
    // than failing. `target/debug/eidnara-host` becomes `/target/debug/...` and
    // fails as a missing payload, and a first segment that collides with a real
    // root entry is worse — `bin/foo` resolves to `/bin/foo` and *executes the
    // wrong binary*, surfacing later as malformed output. `--payload-dir` has
    // the same hazard, staging from a directory the caller never named.
    if (target.kind === "test-binary" && !path.isAbsolute(target.path)) {
        throw new NativeLaunchError("usage_error", "native launch target path is not absolute");
    }
    if (options.payloadDir !== undefined && !path.isAbsolute(options.payloadDir)) {
        throw new NativeLaunchError("usage_error", "native payload directory is not absolute");
    }
    // `host_runtime::data_dir_path` ignores a relative `XDG_DATA_HOME` and answers `no_data_dir`.
    if (!path.isAbsolute(options.dataRoot)) {
        throw new NativeLaunchError("usage_error", "native lifecycle data root is not absolute");
    }
    if (options.env !== undefined && "XDG_DATA_HOME" in options.env) {
        throw new NativeLaunchError(
            "usage_error",
            "native lifecycle environment may not override the data root",
        );
    }
    const env: Record<string, string> = { ...options.env, XDG_DATA_HOME: options.dataRoot };
    // The envelope is serialized before anything is spawned: a value with no
    // JSON form must fail as a typed error with no child in flight, and the
    // stdin `error` listener below only sees stream errors, never a synchronous
    // throw from this call. `JSON.stringify` also answers `undefined` rather
    // than throwing for values that have no JSON form at all, such as a bare
    // function, so both outcomes are rejected the same way.
    let serializedEnvelope: string | undefined;
    if (options.envelope !== undefined && options.envelope !== null) {
        try {
            serializedEnvelope = JSON.stringify(options.envelope);
        } catch {
            throw new NativeLaunchError(
                "usage_error",
                "native startup envelope is not JSON-serializable",
            );
        }
        if (serializedEnvelope === undefined) {
            throw new NativeLaunchError(
                "usage_error",
                "native startup envelope is not JSON-serializable",
            );
        }
        if (Buffer.byteLength(serializedEnvelope, "utf8") > MAX_ENVELOPE_BYTES) {
            throw new NativeLaunchError(
                "usage_error",
                "native startup envelope exceeds the launcher's byte cap",
            );
        }
    }
    let child: ChildProcess;
    const stdio: Array<"pipe" | "ignore" | number> = ["pipe", "pipe", "pipe"];
    let executable: string;
    if (target.kind === "retained-fd") {
        stdio[LAUNCHER_CHILD_FD] = target.fd;
        const execPath = retainedFdExecPath(options.platform ?? process.platform);
        if (execPath === null) {
            throw new NativeLaunchError(
                "unsupported_platform",
                "no retained-descriptor exec path on this platform",
            );
        }
        executable = execPath;
    } else {
        executable = target.path;
    }
    // Serializing a large envelope can consume the budget on its own; spawning
    // a mutating command after that would give it a fresh full deadline.
    const deadlineAt = startedAt + options.deadlineMs;
    if (performance.now() >= deadlineAt) {
        throw new NativeLaunchError(
            "timeout",
            "native lifecycle deadline expired before the child was spawned",
            { childSpawned: false },
        );
    }
    try {
        child = spawn(executable, args, {
            shell: false,
            env,
            cwd: "/",
            stdio,
        });
    } catch {
        throw new NativeLaunchError("spawn_failed", "native spawn threw synchronously");
    }
    // A child that already exited or failed to exec makes this write raise
    // EPIPE/ERR_STREAM_DESTROYED on stdin. Without a listener that stream
    // `error` is an uncaught exception in the host, so an ordinary child-side
    // failure would crash the process instead of surfacing as NativeLaunchError.
    child.stdin?.on("error", () => {});
    if (serializedEnvelope === undefined) {
        child.stdin?.end();
    } else {
        child.stdin?.end(serializedEnvelope);
    }
    const collected = await collectChild(child, deadlineAt);
    if (collected.timedOut) {
        throw new NativeLaunchError("timeout", "native lifecycle command exceeded its deadline");
    }
    if (collected.outputCapExceeded) {
        throw new NativeLaunchError(
            "output_cap_exceeded",
            "native lifecycle command exceeded its stdout cap",
        );
    }
    if (collected.signal !== null) {
        throw new NativeLaunchError(
            "signal_exit",
            `native lifecycle command died on ${collected.signal}`,
        );
    }
    if (collected.exitCode === 2) {
        throw new NativeLaunchError(
            "usage_error",
            "native lifecycle command rejected its invocation",
        );
    }
    // A complete-looking JSON prefix buffered before a read error is not the whole result.
    if (collected.stdoutReadFailed) {
        throw new NativeLaunchError("malformed_output", "native output was not fully read");
    }
    // Decoded strictly. `Buffer.toString("utf8")` substitutes U+FFFD for an
    // invalid byte, so a corrupt byte inside an otherwise well-formed JSON
    // string would parse, validate, and be accepted as a conforming result
    // carrying a silently mangled value — a truncated or corrupted stream must
    // fail closed instead.
    let stdoutText: string;
    try {
        stdoutText = new TextDecoder("utf-8", { fatal: true }).decode(collected.stdout);
    } catch {
        throw new NativeLaunchError("malformed_output", "native output is not valid UTF-8");
    }
    let result: DaemonResultV1;
    try {
        result = parseDaemonResult(stdoutText);
    } catch (error) {
        if (error instanceof ContractViolation) {
            throw new NativeLaunchError("malformed_output", error.message);
        }
        throw new NativeLaunchError("malformed_output", "native output failed validation");
    }
    if (collected.exitCode === null || !exitAgreesWithResult(collected.exitCode, result)) {
        throw new NativeLaunchError(
            "exit_disagreement",
            "native exit code disagrees with the result object",
        );
    }
    // `parseDaemonResult` does not know which command was invoked, so a
    // successful `stop` result returned to a `start` call would otherwise be
    // reported as a started daemon.
    if (result.command !== expectedResultCommand(options.command)) {
        throw new NativeLaunchError(
            "command_mismatch",
            "native result names a different command than the one invoked",
        );
    }
    return result;
}
