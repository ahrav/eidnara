import { afterEach, describe, expect, it, spyOn } from "bun:test";

import * as loggerModule from "@eidnara/opencode/shared/logger";

import { __test, handlePiSessionBeforeCompact, PI_TRANSFORM_AVAILABLE } from "./index";

afterEach(() => {
    __test.resetLoggedPiConfigDirs();
});

describe("Pi config load logging", () => {
    it("dedupes /cd config warnings per directory", () => {
        const logSpy = spyOn(loggerModule, "log").mockImplementation(() => undefined);
        try {
            __test.logPiConfigLoad({
                dir: "/tmp/project-a",
                loadedFromPaths: ["/tmp/project-a/.eidnara/eidnara.jsonc"],
                warnings: ["Ignoring history_summarizer.model from project config"],
                dedupe: true,
            });
            __test.logPiConfigLoad({
                dir: "/tmp/project-a",
                loadedFromPaths: ["/tmp/project-a/.eidnara/eidnara.jsonc"],
                warnings: ["Ignoring history_summarizer.model from project config"],
                dedupe: true,
            });
            __test.logPiConfigLoad({
                dir: "/tmp/project-b",
                loadedFromPaths: [],
                warnings: ["Ignoring execute_threshold_percentage from project config"],
                dedupe: true,
            });

            const messages = logSpy.mock.calls.map(([message]) => String(message));
            expect(
                messages.filter((message) => message.includes("config loaded from:")),
            ).toHaveLength(1);
            expect(
                messages.filter((message) =>
                    message.includes("config: no eidnara.jsonc found, using schema defaults"),
                ),
            ).toHaveLength(1);
            expect(
                messages.filter((message) =>
                    message.includes("Ignoring history_summarizer.model from project config"),
                ),
            ).toHaveLength(1);
            expect(
                messages.filter((message) =>
                    message.includes("Ignoring execute_threshold_percentage from project config"),
                ),
            ).toHaveLength(1);
        } finally {
            logSpy.mockRestore();
        }
    });
});

describe("Pi compaction gate", () => {
    it("runs compaction-off until a Pi context transform exists", async () => {
        // Cancelling `session_before_compact` without a transform leaves the session to overflow.
        expect(PI_TRANSFORM_AVAILABLE).toBe(false);
        expect(
            await handlePiSessionBeforeCompact({ compactionOff: true, ctx: {} }),
        ).toBeUndefined();
        expect(await handlePiSessionBeforeCompact({ compactionOff: false, ctx: {} })).toEqual({
            cancel: true,
        });
    });
});
