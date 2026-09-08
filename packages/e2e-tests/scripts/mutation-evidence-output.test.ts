import { describe, expect, it } from "bun:test";
import { bunTestEvidence, cargoTestEvidence } from "./mutation-evidence-output";

const BUN_FAILURE = [
    "bun test v1.4.0 (34cbb9a40)",
    "",
    "tests/rust-ctx-reduce-roundtrip.test.ts:",
    '128 |             { OPENAI_API_KEY: "secret", PATH: "/poisoned" },',
    "129 |             `/proc/self/${anchor.source_path}`,",
    "error: expect(received).toBeGreaterThan(expected)",
    "",
    "Expected: > 0",
    "Received: 0",
    "",
    "      at <anonymous> (/local/home/someone/scratch/eidnara/packages/e2e-tests/tests/rust-ctx-reduce-roundtrip.test.ts:133:26)",
    "error: failed to open /home/runner/.local/share/opencode/log/2026.log: ENOENT",
    "error: queued ctx_reduce drop must be pending before the bust",
    "[eidnara] session ses_01H8 failed: Error: boom at /home/someone/.local/share/opencode/log/2026.log",
    "(fail) rust ctx_reduce round trip > queues the reduce row [412.43ms]",
    "",
    " 0 pass",
    " 1 fail",
    " 3 expect() calls",
    "Ran 1 test across 1 file. [2.31s]",
].join("\n");

const CARGO_FAILURE = [
    "   Compiling daemon v0.1.0 (/local/home/someone/scratch/eidnara/crates/daemon)",
    "    Finished `test` profile [unoptimized + debuginfo] target(s) in 50.39s",
    "     Running unittests src/lib.rs (/local/home/someone/scratch/eidnara/target/debug/deps/daemon-3a19fdb2358c051a)",
    "",
    "running 1 test",
    "test differential_goldens::dg_goldens_match_ts_wire_surface_and_gate_labels ... FAILED",
    "",
    "failures:",
    "",
    "---- differential_goldens::dg_goldens_match_ts_wire_surface_and_gate_labels stdout ----",
    "",
    "thread 'differential_goldens::dg_goldens_match_ts_wire_surface_and_gate_labels' (183794) panicked at crates/daemon/src/differential_goldens.rs:59:9:",
    "assertion `left == right` failed: wire drift in DG-1-bust-veto",
    '  left: [Object {"text": String("stable inputx")}]',
    ' right: [Object {"text": String("stable input")}]',
    "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
    "",
    "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 980 filtered out; finished in 0.00s",
].join("\n");

describe("mutation evidence output", () => {
    it("keeps Bun verdicts, assertion text, and counts while dropping paths and diagnostics", () => {
        const kept = bunTestEvidence(BUN_FAILURE);
        expect(kept).toBe(
            [
                "error: expect(received).toBeGreaterThan(expected)",
                "Expected: > 0",
                "Received: 0",
                "error: queued ctx_reduce drop must be pending before the bust",
                "(fail) rust ctx_reduce round trip > queues the reduce row [412.43ms]",
                " 0 pass",
                " 1 fail",
                " 3 expect() calls",
                "Ran 1 test across 1 file. [2.31s]",
            ].join("\n"),
        );
        expect(kept).not.toContain("/local/home");
        expect(kept).not.toContain("/home/");
        expect(kept).not.toContain("ses_01H8");
        expect(kept).not.toContain("failed to open");
        expect(kept).not.toMatch(/^\d+ \|/m);
    });

    it("keeps cargo verdicts and the assertion while dropping compile, target, and backtrace lines", () => {
        const kept = cargoTestEvidence(CARGO_FAILURE);
        expect(kept.split("\n")).toEqual([
            "test differential_goldens::dg_goldens_match_ts_wire_surface_and_gate_labels ... FAILED",
            "failures:",
            "thread 'differential_goldens::dg_goldens_match_ts_wire_surface_and_gate_labels' (183794) panicked at crates/daemon/src/differential_goldens.rs:59:9:",
            "assertion `left == right` failed: wire drift in DG-1-bust-veto",
            '  left: [Object {"text": String("stable inputx")}]',
            ' right: [Object {"text": String("stable input")}]',
            "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 980 filtered out; finished in 0.00s",
        ]);
        expect(kept).not.toContain("/local/home");
        expect(kept).not.toContain("RUST_BACKTRACE");
    });

    it("keeps a passing Bun run's verdict and counts", () => {
        const kept = bunTestEvidence(
            "bun test v1.4.0\n\ntests/x.test.ts:\n(pass) suite > case [1.00ms]\n\n 1 pass\n 0 fail\nRan 1 test across 1 file. [10.00ms]\n",
        );
        expect(kept).toBe(
            "(pass) suite > case [1.00ms]\n 1 pass\n 0 fail\nRan 1 test across 1 file. [10.00ms]",
        );
    });
});
