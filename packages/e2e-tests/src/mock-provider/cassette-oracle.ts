/**
 * The client side of `eval_runner cassette-oracle`: one Rust process per cassette, spoken to over
 * line-delimited JSON. Rust owns the covered-field projection, every digest, the header allowlist,
 * the secret scan, and the file write; this module forwards requests and returns Rust's answers.
 * Digest strings that come back are compared as strings and never recomputed here.
 */

import { type ChildProcess, spawn } from "node:child_process";
import { attachStrictJsonlReader } from "../pi-runner/rpc-client";
import { waitForChildExit } from "../process-exit";
import { buildDaemonExample } from "../rust-runner/hermetic-host";

export type CassetteMode = "record" | "replay";

/** The request as the mock received it; the body travels as text so Rust decides whether it parses. */
export interface OracleRequest {
    path: string;
    headers: Record<string, string>;
    body_text: string;
}

/** What the mock produced for one request; frames are the exact bytes it served, in order. */
export interface RecordedResponse {
    status: number;
    content_type: string;
    frames: string[];
    /** The stream ended before `message_stop`; replay ends at the same frame. */
    aborted: boolean;
}

export interface CassetteHit {
    request_digest: string;
    response: RecordedResponse;
}

export interface CassetteMiss {
    turn: number;
    class: "ModelRequestChanged" | "ToolResultDrift";
    request_digest: string;
    nearest_recorded: string | null;
}

export type LookupOutcome = { hit: CassetteHit } | { miss: CassetteMiss };

export interface CloseReport {
    cases: number;
    misses: number;
    unconsumed: number;
    input_sha256: string | null;
}

/** A typed refusal from the oracle: `kind` is the Rust error variant name. */
export class CassetteRefused extends Error {
    constructor(
        readonly kind: string,
        readonly detail: string,
    ) {
        super(`cassette oracle refused: ${kind}: ${detail}`);
        this.name = "CassetteRefused";
    }
}

const CALL_TIMEOUT_MS = 30_000;

type Reply = { ok: Record<string, unknown> } | { error: { kind: string; detail: string } };

function isRecord(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object" && !Array.isArray(value);
}

/** The reply section named `name`, with every listed field checked against a type predicate. */
function section(
    ok: Record<string, unknown>,
    name: string,
    fields: Record<string, (value: unknown) => boolean>,
): Record<string, unknown> {
    const found = ok[name];
    if (!isRecord(found)) throw new Error(`cassette oracle reply lacks ${name}`);
    for (const [field, accepts] of Object.entries(fields)) {
        if (!accepts(found[field]))
            throw new Error(`cassette oracle ${name}.${field} is malformed`);
    }
    return found;
}

const isCount = (value: unknown) =>
    typeof value === "number" && Number.isInteger(value) && value >= 0;
const isText = (value: unknown) => typeof value === "string";
const isTextOrNull = (value: unknown) => value === null || isText(value);

/**
 * Serializes calls: each request line is answered by exactly one reply line, so a pending
 * promise is resolved by the next line read. A child exit, an unreadable line, or a timeout
 * fails the oracle for good and rejects every pending call.
 */
export class CassetteOracle {
    private readonly pending: Array<{
        settle: (reply: Reply) => void;
        fail: (error: Error) => void;
    }> = [];
    private failure: Error | null = null;
    private readonly stopReading: () => void;

    private constructor(private readonly child: ChildProcess) {
        if (!child.stdout) throw new Error("cassette oracle has no stdout");
        this.stopReading = attachStrictJsonlReader(child.stdout, (line) => this.onLine(line));
        const fail = (what: string) => (error: unknown) =>
            this.failAll(new Error(`cassette oracle ${what}: ${String(error)}`));
        child.once("error", fail("spawn"));
        child.once("close", (code, signal) => fail("exited")(`code ${code}, signal ${signal}`));
        child.stdin?.once("error", fail("stdin"));
    }

    static async start(): Promise<CassetteOracle> {
        const binary = await buildDaemonExample({
            example: "eval_runner",
            feature: "eval-runner",
            prebuiltEnv: "EIDNARA_E2E_EVAL_RUNNER_BIN",
        });
        // stderr is inherited, never captured: a child diagnostic must not travel into an Error.
        const child = spawn(binary, ["cassette-oracle"], { stdio: ["pipe", "pipe", "inherit"] });
        return new CassetteOracle(child);
    }

    async open(mode: CassetteMode, namespace: string, path: string): Promise<{ cases: number }> {
        const ok = await this.call({ op: "open", mode, namespace, path });
        return { cases: section(ok, "open", { cases: isCount }).cases as number };
    }

    async lookup(namespace: string, request: OracleRequest): Promise<LookupOutcome> {
        const ok = await this.call({ op: "lookup", namespace, request });
        if (isRecord(ok.hit)) {
            const hit = section(ok, "hit", { request_digest: isText, response: isRecord });
            return { hit: hit as unknown as CassetteHit };
        }
        const miss = section(ok, "miss", {
            turn: isCount,
            class: (value) => value === "ModelRequestChanged" || value === "ToolResultDrift",
            request_digest: isText,
            nearest_recorded: isTextOrNull,
        });
        return { miss: miss as unknown as CassetteMiss };
    }

    async record(
        namespace: string,
        request: OracleRequest,
        response: RecordedResponse,
    ): Promise<{ request_digest: string }> {
        const ok = await this.call({ op: "record", namespace, request, response });
        const record = section(ok, "record", { request_digest: isText });
        return { request_digest: record.request_digest as string };
    }

    /**
     * Writes the cassette in record mode (write-then-rename) and reports the run's counts;
     * `unconsumed` entries after a replay mean the run made fewer requests than the recording.
     * A recording that refused an entry has no file and closes with that refusal.
     */
    async close(): Promise<CloseReport> {
        const ok = await this.call({ op: "close" });
        const close = section(ok, "close", {
            cases: isCount,
            misses: isCount,
            unconsumed: isCount,
            input_sha256: isTextOrNull,
        });
        return close as unknown as CloseReport;
    }

    /** Ends the child: stdin closes, then SIGTERM, then SIGKILL if it lingers. */
    async stop(timeoutMs = 2_000): Promise<void> {
        this.stopReading();
        this.child.stdin?.end();
        if (await waitForChildExit(this.child, timeoutMs)) return;
        this.child.kill("SIGTERM");
        if (await waitForChildExit(this.child, timeoutMs)) return;
        this.child.kill("SIGKILL");
        await waitForChildExit(this.child, timeoutMs);
    }

    private onLine(line: string): void {
        const next = this.pending.shift();
        let parsed: unknown;
        try {
            parsed = JSON.parse(line);
        } catch (error) {
            this.failAll(new Error(`cassette oracle reply unreadable: ${String(error)}`));
            return;
        }
        const error = isRecord(parsed) ? parsed.error : undefined;
        if (!next) {
            this.failAll(new Error("cassette oracle sent an unsolicited reply"));
        } else if (isRecord(parsed) && isRecord(parsed.ok)) {
            next.settle({ ok: parsed.ok });
        } else if (isRecord(error) && isText(error.kind) && isText(error.detail)) {
            next.settle({ error: { kind: error.kind as string, detail: error.detail as string } });
        } else {
            this.failAll(new Error("cassette oracle reply is neither ok nor error"));
        }
    }

    private failAll(error: Error): void {
        if (this.failure) return;
        this.failure = error;
        for (const pending of this.pending.splice(0)) pending.fail(error);
    }

    private call(op: Record<string, unknown>): Promise<Record<string, unknown>> {
        return new Promise((resolve, reject) => {
            if (this.failure) return reject(this.failure);
            const stdin = this.child.stdin;
            if (!stdin || this.child.exitCode !== null) {
                return reject(new Error("cassette oracle is not running"));
            }
            const timer = setTimeout(() => {
                this.failAll(new Error(`cassette oracle call exceeded ${CALL_TIMEOUT_MS}ms`));
            }, CALL_TIMEOUT_MS);
            this.pending.push({
                settle: (reply) => {
                    clearTimeout(timer);
                    if ("error" in reply) {
                        reject(new CassetteRefused(reply.error.kind, reply.error.detail));
                    } else {
                        resolve(reply.ok);
                    }
                },
                fail: (error) => {
                    clearTimeout(timer);
                    reject(error);
                },
            });
            stdin.write(`${JSON.stringify(op)}\n`);
        });
    }
}
