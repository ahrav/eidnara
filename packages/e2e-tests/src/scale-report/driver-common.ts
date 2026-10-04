import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import os from "node:os";

import type { PassOutcome, PassRow } from "./rows";

export function seedCoverage(
    evalRunnerBin: string,
    dataDir: string,
    sessionId: string,
    segments: number,
): void {
    const result = spawnSync(
        evalRunnerBin,
        [
            "scale-seed",
            "--state-root",
            dataDir,
            "--session",
            sessionId,
            "--segments",
            String(segments),
        ],
        { encoding: "utf8" },
    );
    if (result.status !== 0) throw new Error(`scale-seed failed: ${result.stderr}`);
}

export function command(cmd: string, args: string[]): string {
    const result = spawnSync(cmd, args, { encoding: "utf8" });
    return result.status === 0 ? result.stdout.trim() : "";
}

export function hostManifest(): Record<string, unknown> {
    const cpuModel =
        /model name\s*:\s*(.+)/.exec(readFileSync("/proc/cpuinfo", "utf8"))?.[1]?.trim() ??
        os.cpus()[0]?.model ??
        "unknown";
    const device = command("df", ["--output=source", os.tmpdir()]).split("\n").at(-1) ?? "";
    const rotational = command("lsblk", ["-ndo", "ROTA", device]);
    return {
        cpu_model: cpuModel,
        core_count: os.availableParallelism(),
        memory_bytes: os.totalmem(),
        kernel: os.release(),
        glibc: command("getconf", ["GNU_LIBC_VERSION"]) || "unknown",
        disk: rotational === "0" ? "ssd" : rotational === "1" ? "hdd" : "unknown",
    };
}

export function exchangedBytes(exchanged: readonly unknown[]): number {
    let bytes = 0;
    for (const value of exchanged) bytes += Buffer.byteLength(JSON.stringify(value) ?? "");
    return bytes;
}

/** The transform call itself ran out of time, after its request may have been sent. */
export function isDeadline(error: unknown): boolean {
    if (!(error instanceof Error)) return false;
    const code = (error as { code?: unknown }).code;
    return (
        (code === "ETIMEDOUT" && !/while queued/.test(error.message)) ||
        /request deadline expired after a possible send/.test(error.message)
    );
}

export function passOutcome(pass: { published: boolean; status?: unknown; error?: unknown }): {
    outcome: PassOutcome;
    refusal: PassRow["refusal"];
} {
    if (pass.published) return { outcome: "completed", refusal: null };
    if (isDeadline(pass.error)) return { outcome: "censored", refusal: null };
    if (pass.error !== undefined) return { outcome: "refused", refusal: "transport_error" };
    return {
        outcome: "refused",
        refusal: pass.status === undefined || pass.status === "ok" ? "declined" : "daemon_error",
    };
}
