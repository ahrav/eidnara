import { describe, expect, it } from "bun:test";
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    Arm,
    armSpec,
    inheritedHarnessEnv,
    OpencodeArm,
    PiArm,
    type PromptResult,
    piOutcome,
} from "./arms";
import { descendants } from "./procs";

describe("Pi arm process", () => {
    it("keeps ambient secrets out of the agent's inherited environment", () => {
        const env = inheritedHarnessEnv({
            PATH: "/usr/bin",
            LANG: "C.UTF-8",
            GITHUB_TOKEN: "ghp_real",
            ANTHROPIC_API_KEY: "sk-real",
            OPENAI_API_KEY: "sk-real",
            DATABASE_URL: "postgres://u:p@h/db",
            AWS_SESSION_TOKEN: "real",
            EIDNARA_LAUNCH_NONCE: "n",
            NODE_ENV: "test",
        });
        expect(env).toEqual({ PATH: "/usr/bin", LANG: "C.UTF-8" });
    });
});

describe("arm names", () => {
    it("maps the five supported names to their specs", () => {
        expect(["pi-off", "pi-on", "pi-onraw", "oc-off", "oc-on"].map(armSpec)).toEqual([
            { name: "pi-off", harness: "pi", eidnara: false, stripClosureTemperature: false },
            { name: "pi-on", harness: "pi", eidnara: true, stripClosureTemperature: true },
            { name: "pi-onraw", harness: "pi", eidnara: true, stripClosureTemperature: false },
            { name: "oc-off", harness: "opencode", eidnara: false, stripClosureTemperature: false },
            { name: "oc-on", harness: "opencode", eidnara: true, stripClosureTemperature: true },
        ]);
    });

    it("rejects a name outside the supported set", () => {
        expect(() => armSpec("pi-of")).toThrow(/pi-of/);
        expect(() => armSpec("")).toThrow();
    });
});

class FailingTeardownArm extends Arm {
    async openSession(): Promise<void> {}
    async prompt(): Promise<PromptResult> {
        return { answer: "", ms: 0 };
    }
    async closeSession(): Promise<void> {
        throw new Error("harness still running");
    }
    harnessPid(): number | undefined {
        return undefined;
    }
    harnessDataDirs(): string[] {
        return [];
    }
    sessionId(): string | null {
        return null;
    }
}

describe("host liveness", () => {
    const ctx = {
        root: "/nonexistent/arm",
        resultsDir: "/nonexistent/results",
        workdir: "/nonexistent/work",
        sandboxDir: "",
        enforceWindow: false,
        onCall: () => {},
        fixtureBin: "/nonexistent/fixture",
    };
    class DeadHostArm extends FailingTeardownArm {
        harnessPid(): number | undefined {
            return process.pid;
        }
        hostPid(): number | undefined {
            return 2 ** 22 + 1;
        }
    }

    it("fails an Eidnara arm whose host is gone and leaves an off arm alone", () => {
        expect(() => new DeadHostArm(armSpec("pi-on"), ctx).assertAlive("0:1")).toThrow(
            /host.*0:1/,
        );
        expect(() => new DeadHostArm(armSpec("pi-off"), ctx).assertAlive("0:1")).not.toThrow();
    });

    it("treats a zombie harness as gone", async () => {
        // The background `sleep 0.1` exits while `sh` waits on the foreground `sleep 30`, so it
        // stays a zombie child of `sh` until then.
        const sh = spawn("/bin/sh", ["-c", "sleep 0.1 & exec sleep 30"], { stdio: "ignore" });
        try {
            await Bun.sleep(400);
            const zombie = descendants(sh.pid as number).find((pid) => {
                const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
                return stat.slice(stat.lastIndexOf(")") + 2).split(" ")[0] === "Z";
            });
            expect(zombie).toBeDefined();
            class ZombieHarnessArm extends FailingTeardownArm {
                harnessPid(): number | undefined {
                    return zombie;
                }
            }
            expect(() => new ZombieHarnessArm(armSpec("pi-off"), ctx).assertAlive("0:1")).toThrow(
                /harness.*0:1/,
            );
        } finally {
            sh.kill("SIGKILL");
        }
    });

    it("fails any arm whose harness is gone", () => {
        class DeadHarnessArm extends FailingTeardownArm {
            harnessPid(): number | undefined {
                return 2 ** 22 + 1;
            }
        }
        expect(() => new DeadHarnessArm(armSpec("pi-off"), ctx).assertAlive("0:1")).toThrow(
            /harness.*0:1/,
        );
    });
});

describe("Pi arm teardown", () => {
    it("reports a Pi process that survived its shutdown", async () => {
        const root = mkdtempSync(join(tmpdir(), "ab-pi-arm-"));
        const harness = spawn("/bin/sleep", ["30"], { stdio: "ignore" });
        try {
            const arm = new PiArm(armSpec("pi-off"), {
                root,
                resultsDir: root,
                workdir: root,
                sandboxDir: "",
                enforceWindow: false,
                onCall: () => {},
                fixtureBin: "/nonexistent/fixture",
            });
            (arm as unknown as { rpc: unknown }).rpc = {
                child: harness,
                shutdown: async () => {
                    throw new Error("Pi RPC process did not exit");
                },
            };
            await expect(arm.closeSession()).rejects.toThrow("did not exit");
        } finally {
            harness.kill("SIGKILL");
            rmSync(root, { recursive: true, force: true });
        }
    });
});

describe("arm teardown", () => {
    it("reports a harness that would not stop and still stops the gateways", async () => {
        const arm = new FailingTeardownArm(armSpec("pi-off"), {
            root: "/nonexistent/arm",
            resultsDir: "/nonexistent/results",
            workdir: "/nonexistent/work",
            sandboxDir: "",
            enforceWindow: false,
            onCall: () => {},
            fixtureBin: "/nonexistent/fixture",
        });
        const stopped: string[] = [];
        arm.main.stop = async () => {
            stopped.push("main");
        };
        arm.closure.stop = async () => {
            stopped.push("closure");
        };
        await expect(arm.stop()).rejects.toThrow("harness still running");
        expect(stopped.sort()).toEqual(["closure", "main"]);
    });
});

describe("OpenCode arm process", () => {
    it("drops a cached serve PID that now names another process", () => {
        const root = mkdtempSync(join(tmpdir(), "ab-oc-arm-"));
        try {
            const arm = new OpencodeArm(armSpec("oc-off"), {
                root,
                resultsDir: root,
                workdir: root,
                sandboxDir: "",
                enforceWindow: false,
                onCall: () => {},
                fixtureBin: "/nonexistent/fixture",
            });
            // A port no `opencode serve` listens on, and a live PID that is this test runner.
            (arm as unknown as { oc: unknown; servePid: number }).oc = { port: 65_431 };
            (arm as unknown as { servePid: number }).servePid = process.pid;
            expect(arm.harnessPid()).toBeUndefined();
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});

describe("Pi prompt outcome", () => {
    const text = (t: string, extra: Record<string, unknown> = {}) => ({
        role: "assistant",
        content: [{ type: "text", text: t }],
        ...extra,
    });

    it("reads the answer and the failure from the final assistant message", () => {
        expect(piOutcome([text("draft"), text("final", { stopReason: "stop" })])).toEqual({
            answer: "final",
        });
        expect(
            piOutcome([text("draft"), { role: "assistant", content: [], stopReason: "aborted" }]),
        ).toEqual({ answer: "", error: "aborted" });
        expect(
            piOutcome([
                text("draft"),
                { role: "assistant", content: [], stopReason: "error", errorMessage: "boom" },
            ]),
        ).toEqual({ answer: "", error: "boom" });
    });
});

describe("Pi RPC output", () => {
    it("fails a turn after which Pi wrote lines outside the RPC protocol", async () => {
        const root = mkdtempSync(join(tmpdir(), "ab-pi-arm-"));
        try {
            const arm = new PiArm(armSpec("pi-off"), {
                root,
                resultsDir: root,
                workdir: root,
                sandboxDir: "",
                enforceWindow: false,
                onCall: () => {},
                fixtureBin: "/nonexistent/fixture",
            });
            const malformed: string[] = [];
            (arm as unknown as { rpc: unknown }).rpc = {
                sendCommand: async () => {
                    malformed.push("plugin: unexpected stdout line");
                    return { success: true, data: {} };
                },
                waitForEvent: async () => ({ messages: [] }),
                getMalformedLines: () => malformed,
            };
            await expect(arm.prompt("hello", 1_000)).rejects.toThrow(/RPC/);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});
