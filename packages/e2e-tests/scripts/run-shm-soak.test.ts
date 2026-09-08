import { describe, expect, it } from "bun:test";
import { join } from "node:path";

import { exitStatus, soakInvocation, USAGE } from "./run-shm-soak";

describe("shared-memory soak runner", () => {
    it("selects the checked short smoke without a duration override", () => {
        const invocation = soakInvocation(["--smoke"]);
        expect(invocation.command).toEqual([
            "cargo",
            "test",
            "--locked",
            "-p",
            "host-runtime",
            "--test",
            "shm_soak",
            "short_soak_keeps_fd_mapping_thread_and_rss_envelopes_bounded",
        ]);
        expect(invocation.environment).toEqual({});
    });

    it("selects the release soak and converts hours to seconds", () => {
        const invocation = soakInvocation(["--hours", "5"]);
        expect(invocation.command).toContain("--locked");
        expect(invocation.command).toContain("--release");
        expect(invocation.command).toContain(
            "long_soak_keeps_fd_mapping_thread_and_rss_envelopes_bounded",
        );
        expect(invocation.command.slice(-3)).toEqual(["--", "--ignored", "--exact"]);
        expect(invocation.environment).toEqual({ EIDNARA_SHM_SOAK_SECONDS: "18000" });
    });

    it("defaults to a duration the six-hour hosted-runner limit can hold", () => {
        expect(soakInvocation([]).environment).toEqual({ EIDNARA_SHM_SOAK_SECONDS: "18000" });
    });

    it("rejects missing, zero, negative, and nonnumeric durations", () => {
        for (const args of [["--hours"], ["--hours", "0"], ["--hours", "-1"], ["--hours", "x"]]) {
            expect(() => soakInvocation(args)).toThrow("--hours must be a positive number");
        }
    });

    it("rejects fractional durations that round down to zero seconds", () => {
        for (const hours of ["0.0001", "0.00013"]) {
            expect(() => soakInvocation(["--hours", hours])).toThrow(
                "--hours must resolve to at least one second",
            );
        }
        expect(soakInvocation(["--hours", "0.001"]).environment).toEqual({
            EIDNARA_SHM_SOAK_SECONDS: "4",
        });
    });

    it("rejects unknown arguments instead of falling back to the long soak", () => {
        for (const args of [["--help"], ["-h"], ["--hour", "5"], ["--smoke", "extra"]]) {
            expect(() => soakInvocation(args)).toThrow(/unknown argument: .*\n.*Usage/);
        }
    });

    it("rejects --smoke combined with --hours", () => {
        expect(() => soakInvocation(["--smoke", "--hours", "5"])).toThrow(
            "--smoke and --hours are mutually exclusive",
        );
    });

    it("prints usage and exits 0 for --help without spawning cargo", () => {
        const result = Bun.spawnSync({
            cmd: [process.execPath, join(import.meta.dir, "run-shm-soak.ts"), "--help"],
            stdout: "pipe",
            stderr: "pipe",
        });
        expect(result.exitCode).toBe(0);
        expect(result.stdout.toString()).toContain(USAGE);
    });

    it("exits 2 with the parse error for an unknown argument", () => {
        const result = Bun.spawnSync({
            cmd: [process.execPath, join(import.meta.dir, "run-shm-soak.ts"), "--hour", "5"],
            stdout: "pipe",
            stderr: "pipe",
        });
        expect(result.exitCode).toBe(2);
        expect(result.stderr.toString()).toContain("unknown argument: --hour");
    });

    it("reports a signal-terminated soak as a failure", () => {
        expect(exitStatus({ exitCode: null, signalCode: "SIGKILL" })).toBe(1);
        expect(exitStatus({ exitCode: 0 })).toBe(0);
        expect(exitStatus({ exitCode: 101 })).toBe(101);
    });
});
