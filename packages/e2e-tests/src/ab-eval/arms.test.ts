import { describe, expect, it } from "bun:test";
import { Arm, armSpec, inheritedHarnessEnv, type PromptResult } from "./arms";

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
