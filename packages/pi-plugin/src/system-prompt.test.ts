import { describe, expect, it } from "bun:test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    createPromptSurfaceGuidanceEpochCache,
    createPromptSurfaceRuntime,
} from "@eidnara/opencode/shared/prompt-surface-runtime";
import {
    clearPiSystemPromptSession,
    piSystemPromptStateFor,
    processSystemPromptForCache,
} from "./system-prompt";

function tempDir(prefix: string): string {
    return mkdtempSync(join(tmpdir(), prefix));
}

describe("Pi prompt-surface guidance epochs", () => {
    it("folds once when a preset/model epoch selects authored light", () => {
        const directory = tempDir("pi-prompt-epoch-");
        const sessionId = "ses-prompt-surface-epoch";
        const warnings: string[] = [];
        const runtime = createPromptSurfaceRuntime({
            userConfigDirectory: directory,
            warn: (warning) => warnings.push(warning),
        });
        const epochs = createPromptSurfaceGuidanceEpochCache(runtime);
        const config = {
            default: "full" as const,
            models: { "provider/light": "light" as const },
        };
        const render = (selection: ReturnType<typeof epochs.resolve>) =>
            `Base prompt\npreset=${selection.preset}\noverride=${selection.primaryOverride ?? ""}`;

        try {
            const firstSelection = epochs.resolve(sessionId, config, "provider/full");
            const firstPrompt = render(firstSelection);
            const first = processSystemPromptForCache({
                sessionId,
                systemPrompt: firstPrompt,
                isCacheBusting: false,
                promptSurfacePreset: firstSelection.preset,
            });
            expect(first.hashChanged).toBe(false);
            expect(piSystemPromptStateFor(sessionId)?.systemPromptHash).toBe(first.currentHash);
            expect(piSystemPromptStateFor(sessionId)?.systemPromptTokens).toBeGreaterThan(0);

            for (let pass = 0; pass < 5; pass++) {
                const frozenSelection = epochs.resolve(sessionId, config, "provider/full");
                expect(frozenSelection.primaryOverride).toBeUndefined();
                const frozen = processSystemPromptForCache({
                    sessionId,
                    systemPrompt: render(frozenSelection),
                    isCacheBusting: false,
                    promptSurfacePreset: frozenSelection.preset,
                });
                expect(frozen.hashChanged).toBe(false);
                expect(frozen.currentHash).toBe(first.currentHash);
            }

            const changedSelection = epochs.resolve(sessionId, config, "provider/light");
            expect(changedSelection.preset).toBe("light");
            expect(changedSelection.primaryOverride).toBeUndefined();
            expect(render(changedSelection)).not.toBe(firstPrompt);
            const changed = processSystemPromptForCache({
                sessionId,
                systemPrompt: render(changedSelection),
                isCacheBusting: false,
                promptSurfacePreset: changedSelection.preset,
            });
            expect(changed.hashChanged).toBe(true);
            expect(changed.currentHash).not.toBe(first.currentHash);
            expect(piSystemPromptStateFor(sessionId)?.systemPromptHash).toBe(changed.currentHash);

            expect(warnings).toEqual([]);

            for (let pass = 0; pass < 5; pass++) {
                const stableSelection = epochs.resolve(sessionId, config, "provider/light");
                const stable = processSystemPromptForCache({
                    sessionId,
                    systemPrompt: render(stableSelection),
                    isCacheBusting: false,
                    promptSurfacePreset: stableSelection.preset,
                });
                expect(stable.hashChanged).toBe(false);
                expect(stable.currentHash).toBe(changed.currentHash);
            }
        } finally {
            clearPiSystemPromptSession(sessionId);
            expect(piSystemPromptStateFor(sessionId)).toBeUndefined();
        }
    });

    it("coalesces a midnight date and preset flip into one hash change", () => {
        const sessionId = "ses-midnight-preset";
        try {
            const first = processSystemPromptForCache({
                sessionId,
                systemPrompt: "Base prompt\nToday's date: Mon Jan 01 2024",
                isCacheBusting: false,
                promptSurfacePreset: "full",
            });
            expect(first.hashChanged).toBe(false);

            const changed = processSystemPromptForCache({
                sessionId,
                systemPrompt: "Base prompt\nToday's date: Tue Jan 02 2024",
                isCacheBusting: false,
                promptSurfacePreset: "light",
            });
            expect(changed.hashChanged).toBe(true);
            expect(changed.currentHash).not.toBe(first.currentHash);
            expect(changed.systemPrompt).toContain("Today's date: Tue Jan 02 2024");

            const stable = processSystemPromptForCache({
                sessionId,
                systemPrompt: "Base prompt\nToday's date: Tue Jan 02 2024",
                isCacheBusting: false,
                promptSurfacePreset: "light",
            });
            expect(stable.hashChanged).toBe(false);
            expect(stable.currentHash).toBe(changed.currentHash);
        } finally {
            clearPiSystemPromptSession(sessionId);
        }
    });

    it("freezes a date-only change on a cache-stable pass and advances it on a cache-busting pass", () => {
        const sessionId = "ses-date-freeze";
        try {
            const first = processSystemPromptForCache({
                sessionId,
                systemPrompt: "Base prompt\nToday's date: Mon Jan 01 2024",
                isCacheBusting: false,
            });
            expect(first.hashChanged).toBe(false);

            const frozen = processSystemPromptForCache({
                sessionId,
                systemPrompt: "Base prompt\nToday's date: Tue Jan 02 2024",
                isCacheBusting: false,
            });
            expect(frozen.hashChanged).toBe(false);
            expect(frozen.currentHash).toBe(first.currentHash);
            expect(frozen.systemPrompt).toContain("Today's date: Mon Jan 01 2024");

            const advanced = processSystemPromptForCache({
                sessionId,
                systemPrompt: "Base prompt\nToday's date: Tue Jan 02 2024",
                isCacheBusting: true,
            });
            expect(advanced.hashChanged).toBe(true);
            expect(advanced.systemPrompt).toContain("Today's date: Tue Jan 02 2024");
            expect(piSystemPromptStateFor(sessionId)?.systemPromptHash).toBe(advanced.currentHash);
        } finally {
            clearPiSystemPromptSession(sessionId);
        }
    });

    it("freezes every date line when the prompt repeats it", () => {
        const sessionId = "ses-date-multi";
        const first = "Header\nToday's date: Mon Jan 01 2024\nBody\nToday's date: Mon Jan 01 2024";
        const later = "Header\nToday's date: Tue Jan 02 2024\nBody\nToday's date: Tue Jan 02 2024";
        try {
            const initial = processSystemPromptForCache({
                sessionId,
                systemPrompt: first,
                isCacheBusting: false,
            });

            const frozen = processSystemPromptForCache({
                sessionId,
                systemPrompt: later,
                isCacheBusting: false,
            });
            expect(frozen.hashChanged).toBe(false);
            expect(frozen.currentHash).toBe(initial.currentHash);
            expect(frozen.systemPrompt).toBe(first);
            expect(piSystemPromptStateFor(sessionId)?.stickyDate).toBe(
                "Today's date: Mon Jan 01 2024",
            );
        } finally {
            clearPiSystemPromptSession(sessionId);
        }
    });

    it("freezes only the host date line when project docs mention the phrase", () => {
        const sessionId = "ses-date-prose";
        const dayOne = "Today's date: Mon Jan 01 2024";
        const dayTwo = "Today's date: Tue Jan 02 2024";
        const prose = "Today's date: comes from the host env block; do not ask the user for it.";
        const example = `The host writes a line like "${dayTwo}" inside <env>; never ask for the date.`;
        const render = (date: string) =>
            `${prose}\n<env>\n  ${date}\n  Platform: linux\n</env>\n${example}`;
        try {
            const initial = processSystemPromptForCache({
                sessionId,
                systemPrompt: render(dayOne),
                isCacheBusting: false,
            });
            expect(piSystemPromptStateFor(sessionId)?.stickyDate).toBe(dayOne);

            const frozen = processSystemPromptForCache({
                sessionId,
                systemPrompt: render(dayTwo),
                isCacheBusting: false,
            });
            expect(frozen.hashChanged).toBe(false);
            expect(frozen.currentHash).toBe(initial.currentHash);
            expect(frozen.systemPrompt).toBe(render(dayOne));
            expect(piSystemPromptStateFor(sessionId)?.stickyDate).toBe(dayOne);

            // Editing the prose is a content change: the hash moves and the date advances with it.
            const edited = processSystemPromptForCache({
                sessionId,
                systemPrompt: render(dayTwo).replace(prose, "Today's date: comes from <env>."),
                isCacheBusting: false,
            });
            expect(edited.hashChanged).toBe(true);
            expect(edited.systemPrompt).toContain(`<env>\n  ${dayTwo}\n`);
            expect(piSystemPromptStateFor(sessionId)?.stickyDate).toBe(dayTwo);
        } finally {
            clearPiSystemPromptSession(sessionId);
        }
    });

    it("stores the sticky date in the bounded session entry and clears it with the session", () => {
        const sessionId = "ses-date-entry";
        try {
            processSystemPromptForCache({
                sessionId,
                systemPrompt: "Base prompt\nToday's date: Mon Jan 01 2024",
                isCacheBusting: false,
            });
            expect(piSystemPromptStateFor(sessionId)?.stickyDate).toBe(
                "Today's date: Mon Jan 01 2024",
            );

            processSystemPromptForCache({
                sessionId,
                systemPrompt: "Base prompt\nToday's date: Tue Jan 02 2024",
                isCacheBusting: true,
            });
            expect(piSystemPromptStateFor(sessionId)?.stickyDate).toBe(
                "Today's date: Tue Jan 02 2024",
            );
        } finally {
            clearPiSystemPromptSession(sessionId);
            expect(piSystemPromptStateFor(sessionId)).toBeUndefined();
        }
    });
});
