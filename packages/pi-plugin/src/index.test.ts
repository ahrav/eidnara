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
    const eviction = {
        summary: "<session-history>m0</session-history>",
        firstKeptEntryId: "entry-9",
        tokensBefore: 1_000,
        details: { sequence: 3 },
    };

    it("answers with the eviction of the acknowledged boundary, or cancels without one", async () => {
        expect(PI_TRANSFORM_AVAILABLE).toBe(true);
        expect(
            await handlePiSessionBeforeCompact({ compactionOff: false, eviction: () => eviction }),
        ).toEqual({ compaction: eviction });
        expect(
            await handlePiSessionBeforeCompact({ compactionOff: false, eviction: () => undefined }),
        ).toEqual({ cancel: true });
        expect(await handlePiSessionBeforeCompact({ compactionOff: false })).toEqual({
            cancel: true,
        });
    });

    it("hands compaction back to Pi after a declined pass only when no eviction is available", async () => {
        expect(
            await handlePiSessionBeforeCompact({
                compactionOff: false,
                handBack: true,
                eviction: () => eviction,
            }),
        ).toEqual({ compaction: eviction });
        expect(
            await handlePiSessionBeforeCompact({
                compactionOff: false,
                handBack: true,
                eviction: () => undefined,
            }),
        ).toBeUndefined();
    });

    it("lets native compaction proceed in compaction-off mode", async () => {
        expect(
            await handlePiSessionBeforeCompact({ compactionOff: true, eviction: () => eviction }),
        ).toBeUndefined();
    });
});
