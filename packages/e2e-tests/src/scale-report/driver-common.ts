import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import os from "node:os";

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
