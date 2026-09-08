import { describe, expect, it } from "bun:test";
import { capBodyToGithubLimit, extractRecentErrors, MAX_GITHUB_BODY_BYTES } from "./issue-body";

describe("extractRecentErrors", () => {
    it("matches the documented sessionLog error shapes", () => {
        const log = [
            "2026-05-20 12:00:00 [INFO] transform completed in 42ms",
            "2026-05-20 12:00:01 [INFO] historian: 12 compartments published; 0 failed", // telemetry, NOT an error
            "2026-05-20 12:00:02 transform failed: SQLITE_BUSY",
            "2026-05-20 12:00:03 historian prompt failed: connection refused",
            "2026-05-20 12:00:04 Error: Connection reset",
            "2026-05-20 12:00:05 TypeError: cannot read property 'foo' of undefined",
            "2026-05-20 12:00:06 EMERGENCY: aborting session ses_abc",
            "2026-05-20 12:00:07 some other info line",
            "2026-05-20 12:00:08 caught exception during cleanup",
        ].join("\n");

        const matches = extractRecentErrors(log, 20);

        // `extractRecentErrors` excludes `0 failed` telemetry.
        expect(matches).toContain("2026-05-20 12:00:02 transform failed: SQLITE_BUSY");
        expect(matches).toContain(
            "2026-05-20 12:00:03 historian prompt failed: connection refused",
        );
        expect(matches).toContain("2026-05-20 12:00:04 Error: Connection reset");
        expect(matches).toContain(
            "2026-05-20 12:00:05 TypeError: cannot read property 'foo' of undefined",
        );
        expect(matches).toContain("2026-05-20 12:00:06 EMERGENCY: aborting session ses_abc");
        expect(matches).toContain("2026-05-20 12:00:08 caught exception during cleanup");
        expect(matches).not.toContain(
            "2026-05-20 12:00:01 [INFO] historian: 12 compartments published; 0 failed",
        );
        expect(matches).not.toContain("2026-05-20 12:00:07 some other info line");
    });

    it("matches V8 stack-trace frames", () => {
        const log = [
            "Error: thing broke",
            "    at SomeFn (file:///foo.ts:42:5)",
            "    at processTransform (file:///bar.ts:13:9)",
            "    at file:///baz.ts:7:1",
        ].join("\n");

        const matches = extractRecentErrors(log, 20);
        expect(matches.length).toBe(4);
    });

    it("matches async, constructor, and aliased V8 frames", () => {
        const frames = [
            "    at async runCommand (file:///x.ts:1:2)",
            "    at new Historian (file:///y.ts:3:4)",
            "    at Server.emit [as emit] (node:events:1:2)",
            "    at async Promise.all (index 0)",
            "    at Array.map (<anonymous>)",
        ];
        const noise = "    at the moment nothing else is logged";

        const matches = extractRecentErrors([...frames, noise].join("\n"), 20);
        expect(matches).toEqual(frames);
    });

    it("matches `failed` regardless of the punctuation that follows it", () => {
        const log = [
            "[2026-05-20T12:00:00.000Z] ses_abc failed to send notification: ECONNREFUSED",
            "[2026-05-20T12:00:01.000Z] ses_abc rust transform failed; serving the input unchanged: boom",
            "[2026-05-20T12:00:02.000Z] historian cleanup failed (wrapup) for ses_abc",
            "[2026-05-20T12:00:03.000Z] apply failed=true",
            "[2026-05-20T12:00:04.000Z] historian: 12 compartments published; 0 failed",
            "[2026-05-20T12:00:05.000Z] historian: 3 published, 0  failed",
            "[2026-05-20T12:00:06.000Z] totals: 4 failed; 2 ok",
            "[2026-05-20T12:00:07.000Z] (0 failed)",
        ].join("\n");

        const matches = extractRecentErrors(log, 20);

        expect(matches).toEqual([
            "[2026-05-20T12:00:00.000Z] ses_abc failed to send notification: ECONNREFUSED",
            "[2026-05-20T12:00:01.000Z] ses_abc rust transform failed; serving the input unchanged: boom",
            "[2026-05-20T12:00:02.000Z] historian cleanup failed (wrapup) for ses_abc",
            "[2026-05-20T12:00:03.000Z] apply failed=true",
        ]);
    });

    it("matches failures whose subject ends in a digit", () => {
        const lines = [
            "[historian] openai/gpt-5 failed: timeout; 1 fallback(s) left",
            "[historian] openai/gpt-5 failed",
            "attempt 2 failed: connection reset",
            "job-2026-05-20 failed: quota",
            "ses_abc123 failed: SQLITE_BUSY",
        ];

        expect(extractRecentErrors(lines.join("\n"), 20)).toEqual(lines);
    });

    it("matches lowercase `error:` labels", () => {
        const lines = [
            "[rpc] handler error: ctx.status => boom",
            "[rpc] sidebar-snapshot error: Error: nope",
            "TypeError: cannot read property 'foo' of undefined",
        ];
        const noise = "SQLITE_ERROR is the code name, not an error label";

        expect(extractRecentErrors([...lines, noise].join("\n"), 20)).toEqual(lines);
    });

    it("returns matches in chronological order", () => {
        const log = [
            "transform failed: first error",
            "info noise",
            "transform failed: second error",
            "info noise",
            "transform failed: third error",
        ].join("\n");

        const matches = extractRecentErrors(log, 10);
        expect(matches).toEqual([
            "transform failed: first error",
            "transform failed: second error",
            "transform failed: third error",
        ]);
    });

    it("caps at the requested limit (newest-first selection, oldest-first output)", () => {
        const lines: string[] = [];
        for (let i = 0; i < 50; i += 1) {
            lines.push(`transform failed: error ${i}`);
        }
        const matches = extractRecentErrors(lines.join("\n"), 5);
        expect(matches.length).toBe(5);
        expect(matches[0]).toBe("transform failed: error 45");
        expect(matches[4]).toBe("transform failed: error 49");
    });

    it("returns empty array when no errors found", () => {
        const log = ["info line 1", "info line 2", "transform completed in 42ms"].join("\n");
        expect(extractRecentErrors(log, 20)).toEqual([]);
    });

    it("handles empty input gracefully", () => {
        expect(extractRecentErrors("", 20)).toEqual([]);
    });
});

describe("capBodyToGithubLimit", () => {
    /**
     * Tests that require truncation use enough log lines to exceed the requested budget.
     */
    function makeBody(opts: { logLineCount: number; lineSize?: number }): string {
        const lineSize = opts.lineSize ?? 80;
        const logLines: string[] = [];
        for (let i = 0; i < opts.logLineCount; i += 1) {
            // Line indexes identify content retained after truncation.
            const prefix = `LINE${String(i).padStart(6, "0")}: `;
            const padding = "x".repeat(Math.max(0, lineSize - prefix.length));
            logLines.push(prefix + padding);
        }

        return [
            "## Description",
            "Test description for the cap helper.",
            "",
            "## Environment",
            "- Plugin: v0.21.5",
            "",
            "## Recent errors (last 20, sanitized)",
            "```",
            "transform failed: critical error 1",
            "transform failed: critical error 2",
            "```",
            "",
            "## Log (last 400 lines, sanitized)",
            "```",
            logLines.join("\n"),
            "```",
        ].join("\n");
    }

    it("returns body unchanged when already within budget", () => {
        const body = makeBody({ logLineCount: 20 });
        const capped = capBodyToGithubLimit(body, 100_000);
        expect(capped).toBe(body);
    });

    it("truncates the main log section when body exceeds budget", () => {
        const body = makeBody({ logLineCount: 5000, lineSize: 200 });
        const originalBytes = Buffer.byteLength(body, "utf8");

        const capped = capBodyToGithubLimit(body, 60_000);
        const cappedBytes = Buffer.byteLength(capped, "utf8");

        expect(cappedBytes).toBeLessThanOrEqual(60_000);
        expect(cappedBytes).toBeLessThan(originalBytes);
    });

    it("preserves the Recent errors section after truncation", () => {
        const body = makeBody({ logLineCount: 5000, lineSize: 200 });
        const capped = capBodyToGithubLimit(body, 60_000);

        // The errors section survives truncation.
        expect(capped).toContain("## Recent errors (last 20, sanitized)");
        expect(capped).toContain("transform failed: critical error 1");
        expect(capped).toContain("transform failed: critical error 2");
    });

    it("inserts the truncation marker when log lines are dropped", () => {
        const body = makeBody({ logLineCount: 5000, lineSize: 200 });
        const capped = capBodyToGithubLimit(body, 60_000);

        expect(capped).toContain("[truncated for GitHub 64KB limit");
    });

    it("drops oldest log lines first (keeps newest)", () => {
        const body = makeBody({ logLineCount: 5000, lineSize: 200 });
        const capped = capBodyToGithubLimit(body, 60_000);

        expect(capped).toContain("LINE004999:");

        // The first log line (LINE000000) should be gone — it's the oldest.
        expect(capped).not.toContain("LINE000000:");
    });

    it("treats a log line that begins with a fence as content, not as the closing fence", () => {
        // A logged Error message can embed a Markdown code block with its newlines intact.
        const body = makeBody({ logLineCount: 5000, lineSize: 200 }).replace(
            "LINE000010: ",
            "```\nLINE000010: ",
        );
        const capped = capBodyToGithubLimit(body, 60_000);

        expect(Buffer.byteLength(capped, "utf8")).toBeLessThanOrEqual(60_000);
        expect(capped).toContain("LINE004999:");
        expect(capped).not.toContain("LINE000000:");
        expect(capped.endsWith("\n```")).toBe(true);
        expect(capped).not.toContain("[truncated further to fit GitHub body limit]");
    });

    it("anchors on the generated Log heading, not a copy pasted into the description", () => {
        const pastedReport = [
            "Here is what I saw last time:",
            "## Log (last 400 lines, sanitized)",
            "```",
            "old pasted line",
            "```",
        ].join("\n");
        const body = makeBody({ logLineCount: 5000, lineSize: 200 }).replace(
            "Test description for the cap helper.",
            pastedReport,
        );
        const capped = capBodyToGithubLimit(body, 60_000);

        expect(Buffer.byteLength(capped, "utf8")).toBeLessThanOrEqual(60_000);
        expect(capped).toContain("## Environment");
        expect(capped).toContain("- Plugin: v0.21.5");
        expect(capped).toContain("transform failed: critical error 2");
        expect(capped).toContain("old pasted line");
        expect(capped).toContain("LINE004999:");
        expect(capped).not.toContain("LINE000000:");
    });

    it("ignores a Log heading that appears inside the main log content", () => {
        // A logged Error message can carry a copied report, heading included.
        const body = makeBody({ logLineCount: 5000, lineSize: 200 }).replace(
            "LINE004990: ",
            "## Log (last 400 lines, sanitized)\nLINE004990: ",
        );
        const capped = capBodyToGithubLimit(body, 60_000);

        expect(Buffer.byteLength(capped, "utf8")).toBeLessThanOrEqual(60_000);
        expect(capped).toContain("LINE004999:");
        expect(capped).not.toContain("LINE000000:");
        expect(capped).toContain("[truncated for GitHub 64KB limit — older log lines dropped]");
        expect(capped).not.toContain("[truncated further to fit GitHub body limit]");
    });

    it("preserves the Description and Environment sections", () => {
        const body = makeBody({ logLineCount: 5000, lineSize: 200 });
        const capped = capBodyToGithubLimit(body, 60_000);

        expect(capped).toContain("## Description");
        expect(capped).toContain("Test description for the cap helper.");
        expect(capped).toContain("## Environment");
        expect(capped).toContain("- Plugin: v0.21.5");
    });

    it("uses MAX_GITHUB_BODY_BYTES as the default budget", () => {
        // The 5000-line log exceeds the default 60 KB budget.
        const body = makeBody({ logLineCount: 5000, lineSize: 80 });
        const capped = capBodyToGithubLimit(body);
        expect(Buffer.byteLength(capped, "utf8")).toBeLessThanOrEqual(MAX_GITHUB_BODY_BYTES);
    });

    it("hard-truncates the tail when non-log sections alone exceed the budget", () => {
        const hugeDescription = "x".repeat(80_000);
        const body = [
            "## Description",
            hugeDescription,
            "",
            "## Recent errors (last 20, sanitized)",
            "```",
            "transform failed: critical error",
            "```",
            "",
            "## Log (last 1 lines, sanitized)",
            "```",
            "tiny log",
            "```",
        ].join("\n");

        const capped = capBodyToGithubLimit(body, 10_000);
        expect(Buffer.byteLength(capped, "utf8")).toBeLessThanOrEqual(10_000);
        expect(capped).toContain("[truncated further to fit GitHub body limit]");
        // A single oversized line keeps its prefix rather than losing the whole line.
        expect(capped).toContain("## Description\nxxxx");
    });

    it("keeps every fence balanced wherever the final byte cut lands", () => {
        // Sweeping the description length walks the cut across the Recent errors
        // fences, the Log heading, and the Log fences one byte at a time.
        function build(descriptionLength: number): string {
            return [
                "## Description",
                "x".repeat(descriptionLength),
                "",
                "## Recent errors (last 20, sanitized)",
                "```",
                "transform failed: critical error",
                "```",
                "",
                "## Log (last 1 lines, sanitized)",
                "```",
                "tiny log",
                "```",
            ].join("\n");
        }
        const maxBytes = 2_000;
        const baseline = Buffer.byteLength(build(0), "utf8");
        for (let over = 1; over <= 160; over += 1) {
            const body = build(maxBytes + over - baseline);
            expect(Buffer.byteLength(body, "utf8")).toBe(maxBytes + over);

            const capped = capBodyToGithubLimit(body, maxBytes);
            const lines = capped.split("\n");
            const fenceLines = lines.filter((line) => line.startsWith("```")).length;

            expect(Buffer.byteLength(capped, "utf8")).toBeLessThanOrEqual(maxBytes);
            expect(capped).toContain("[truncated further to fit GitHub body limit]");
            expect(fenceLines % 2).toBe(0);
            expect(lines.some((line) => /^`{1,2}$/.test(line))).toBe(false);
        }
    });
    it("falls back to raw byte truncation when log heading is missing", () => {
        // Non-ASCII padding verifies that UTF-8 boundary handling preserves valid text.
        const body = `## Other\n${"ü".repeat(50_000)}\n## End`;
        const capped = capBodyToGithubLimit(body, 10_000);
        expect(Buffer.byteLength(capped, "utf8")).toBeLessThanOrEqual(10_000);
        expect(capped).toContain("[truncated for GitHub 64KB limit]");
    });
});
