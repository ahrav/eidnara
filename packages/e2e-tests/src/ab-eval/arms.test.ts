import { describe, expect, it } from "bun:test";
import { inheritedHarnessEnv } from "./arms";

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
