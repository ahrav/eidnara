import { describe, expect, it } from "bun:test";
import { armSpec, inheritedHarnessEnv } from "./arms";

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
