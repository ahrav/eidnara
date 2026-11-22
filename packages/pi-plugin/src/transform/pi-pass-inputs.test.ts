import { describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EidnaraConfigSchema } from "@eidnara/opencode/config/schema/eidnara";

import { resolvePiWindowGeometry } from "../pi-context-limit";
import { piPassFields, projectRelativeDirectory } from "./pi-pass-inputs";

type Json = Record<string, unknown>;

const config = EidnaraConfigSchema.parse({});
const geometry = resolvePiWindowGeometry({
    model: { provider: "faux", id: "faux-model", contextWindow: 200_000, maxTokens: 8_192 },
});

function assistant(stopReason: string, usage: Json, timestamp: number): Json {
    return { role: "assistant", content: [], stopReason, usage, timestamp };
}

function fields(
    messages: Json[],
    completedAtMs: (index: number) => number | undefined = () => undefined,
) {
    return piPassFields({
        config,
        geometry,
        providerId: "faux",
        modelKey: "faux/faux-model",
        systemPromptHash: "",
        reduceRegistered: true,
        todowriteRegistered: false,
        messages,
        completedAtMs,
        now: 2_000,
    });
}

const zero = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0 };

describe("Pi pass usage sample", () => {
    it("keeps the last valid response's pressure past an aborted or unreported response", () => {
        const valid = assistant(
            "stop",
            { input: 40_000, output: 10, cacheRead: 20_000, cacheWrite: 0, totalTokens: 60_010 },
            1_000,
        );
        for (const later of [
            assistant("aborted", zero, 1_500),
            assistant("aborted", { ...zero, input: 5, totalTokens: 5 }, 1_500),
            assistant("stop", zero, 1_500),
        ]) {
            const sent = fields([
                { role: "user", content: "a", timestamp: 900 },
                valid,
                { role: "user", content: "b", timestamp: 1_400 },
                later,
                { role: "user", content: "c", timestamp: 1_900 },
            ]);
            expect(sent.usage).toMatchObject({ current_total_input_tokens: 60_000 });
            expect(sent.prev_response_cache_usage).toEqual({
                cache_read_tokens: 20_000,
                cache_write_tokens: 0,
            });
        }
    });

    it("sends no usage when no response reports a valid count", () => {
        const sent = fields([
            { role: "user", content: "a", timestamp: 900 },
            assistant("aborted", zero, 1_000),
            { role: "user", content: "b", timestamp: 1_900 },
        ]);
        expect(sent.usage).toBeUndefined();
        expect(sent.prev_response_cache_usage).toBeUndefined();
    });
});

describe("Pi pass previous response time", () => {
    const valid = { input: 10, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 11 };

    it("reads the latest response's completion, not its stream start", () => {
        const messages = [
            { role: "user", content: "a", timestamp: 900 },
            assistant("stop", valid, 1_000),
            { role: "user", content: "b", timestamp: 1_900 },
        ];
        const sent = fields(messages, (index) => (index === 1 ? 1_700 : 1));
        expect(sent.prev_response_completed_at_ms).toBe(1_700);
    });

    it("sends none when the latest response's completion is unknown", () => {
        const messages = [
            { role: "user", content: "a", timestamp: 900 },
            assistant("stop", valid, 1_000),
            { role: "user", content: "b", timestamp: 1_900 },
        ];
        expect(fields(messages).prev_response_completed_at_ms).toBeUndefined();
        expect(fields(messages, () => Number.NaN).prev_response_completed_at_ms).toBeUndefined();
    });
});

describe("Pi pass working directory", () => {
    function checkout(): { base: string; project: string; app: string } {
        const base = mkdtempSync(join(tmpdir(), "eidnara-pi-cwd-"));
        const project = join(base, "project");
        const app = join(project, "packages", "app");
        mkdirSync(app, { recursive: true });
        mkdirSync(join(base, "project-two"));
        return { base, project, app };
    }

    it("is the session's directory beneath the project root, with / separators", () => {
        const { base, project, app } = checkout();
        try {
            expect(projectRelativeDirectory(project, project)).toBe("");
            expect(projectRelativeDirectory(project, app)).toBe("packages/app");
            expect(projectRelativeDirectory(`${project}/`, `${app}/`)).toBe("packages/app");
        } finally {
            rmSync(base, { recursive: true, force: true });
        }
    });

    it("names the physical directory of a working directory reached through a link", () => {
        const { base, project, app } = checkout();
        try {
            symlinkSync(app, join(base, "app-link"));
            symlinkSync(project, join(base, "project-link"));
            expect(projectRelativeDirectory(project, join(base, "app-link"))).toBe("packages/app");
            expect(projectRelativeDirectory(join(base, "project-link"), app)).toBe("packages/app");
        } finally {
            rmSync(base, { recursive: true, force: true });
        }
    });

    it("is absent for a directory outside the project root or one that does not resolve", () => {
        const { base, project } = checkout();
        try {
            expect(projectRelativeDirectory(project, base)).toBeUndefined();
            expect(projectRelativeDirectory(project, join(base, "project-two"))).toBeUndefined();
            expect(projectRelativeDirectory(project, join(project, "absent"))).toBeUndefined();
        } finally {
            rmSync(base, { recursive: true, force: true });
        }
    });

    it("is sent when the session's directory is known and left out otherwise", () => {
        const args = {
            config,
            geometry,
            providerId: "faux",
            modelKey: "faux/faux-model",
            systemPromptHash: "",
            reduceRegistered: true,
            todowriteRegistered: false,
            messages: [{ role: "user", content: "a", timestamp: 900 }],
            completedAtMs: () => undefined,
            now: 2_000,
        };
        expect(piPassFields({ ...args, workingDirectory: "" }).working_directory).toBe("");
        expect(piPassFields({ ...args, workingDirectory: "packages/app" }).working_directory).toBe(
            "packages/app",
        );
        expect("working_directory" in piPassFields(args)).toBe(false);
    });
});

describe("Pi pass capability fields", () => {
    it.each([
        [true, true],
        [true, false],
        [false, true],
        [false, false],
    ])("reports eidnara_reduce registered %p and todowrite registered %p", (reduce, todo) => {
        const sent = piPassFields({
            config,
            geometry,
            providerId: "faux",
            modelKey: "faux/faux-model",
            systemPromptHash: "",
            reduceRegistered: reduce,
            todowriteRegistered: todo,
            messages: [{ role: "user", content: "a", timestamp: 900 }],
            completedAtMs: () => undefined,
            now: 2_000,
        });
        expect(sent.tool_present).toBe(reduce);
        expect(sent.todo_tool_present).toBe(todo);
    });
});
