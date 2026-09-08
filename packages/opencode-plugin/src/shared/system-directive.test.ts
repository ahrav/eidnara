import { describe, expect, it } from "bun:test";

import {
    isSystemDirective,
    removeSystemInjections,
    removeSystemReminders,
} from "./system-directive";

describe("removeSystemReminders", () => {
    it("drops a single reminder and trims", () => {
        expect(removeSystemReminders("keep <system-reminder>drop</system-reminder> words")).toBe(
            "keep  words",
        );
    });

    it("drops nested reminders through the outer closer", () => {
        expect(
            removeSystemReminders(
                "keep <system-reminder>drop <system-reminder>nested</system-reminder> tail</system-reminder> words",
            ),
        ).toBe("keep  words");
        expect(
            removeSystemReminders(
                "retained <system-reminder>drop <system-reminder>nested</system-reminder></system-reminder> text",
            ),
        ).toBe("retained  text");
    });

    it("matches tags case-insensitively", () => {
        expect(removeSystemReminders("a <SYSTEM-REMINDER>x</System-Reminder> b")).toBe("a  b");
    });

    it("drops an unmatched closer instead of echoing it", () => {
        expect(removeSystemReminders("a </system-reminder> b")).toBe("a  b");
    });

    it("drops everything after an unterminated opener", () => {
        expect(removeSystemReminders("a <system-reminder> b c")).toBe("a");
    });

    it("keeps astral characters outside reminders intact", () => {
        expect(removeSystemReminders("😀 <system-reminder>😈</system-reminder> 🎉")).toBe("😀  🎉");
    });

    it("locates tags correctly after characters whose lowercase form changes length", () => {
        // `İ` (U+0130) lowercases to two code units, so a lowercased copy would shift every offset.
        const prefix = "İ".repeat(40);
        expect(
            removeSystemReminders(`${prefix}<system-reminder>SECRET</system-reminder>AFTER`),
        ).toBe(`${prefix}AFTER`);
    });

    it("returns text without reminders unchanged apart from trimming", () => {
        expect(removeSystemReminders("  plain text  ")).toBe("plain text");
    });
});

describe("isSystemDirective", () => {
    it("recognizes the Eidnara directive prefix after leading whitespace", () => {
        expect(isSystemDirective("  [SYSTEM DIRECTIVE: EIDNARA do x]")).toBe(true);
    });

    it("recognizes the Oh My OpenCode and Oh My Claude directive prefixes", () => {
        expect(isSystemDirective("[SYSTEM DIRECTIVE: OH-MY-OPENCODE do x]")).toBe(true);
        expect(isSystemDirective("[SYSTEM DIRECTIVE: OH-MY-CLAUDE do x]")).toBe(true);
    });

    it("rejects text that does not start with the directive prefix", () => {
        expect(isSystemDirective("hello [SYSTEM DIRECTIVE: EIDNARA]")).toBe(false);
        expect(isSystemDirective("hello")).toBe(false);
    });
});

describe("removeSystemInjections", () => {
    it.each([
        "[SYSTEM DIRECTIVE: EIDNARA do x]",
        "[SYSTEM DIRECTIVE: OH-MY-OPENCODE do x]",
        "[Category+Skill Reminder] use the skill",
        "[EDIT ERROR - IMMEDIATE ACTION REQUIRED] fix",
        "[task CALL FAILED - IMMEDIATE RETRY REQUIRED] retry",
        "[task CALL FAILED] retry",
        "[EMERGENCY CONTEXT WINDOW WARNING] compact",
        "Unstable background agent appears idle",
        "**THE SUBAGENT JUST CLAIMED THIS TASK IS DONE. verify",
        "  \n[Category+Skill Reminder] after whitespace",
        "<!-- OMO_INTERNAL_INITIATOR -->",
        "<system-reminder>x</system-reminder>",
    ])("removes a notice-only text %j entirely", (text) => {
        expect(removeSystemInjections(text)).toBe("");
    });

    it.each([
        "please retry the task",
        "the agent appears idle",
        "I fixed the [EDIT ERROR] myself",
        "",
    ])("leaves %j unchanged", (text) => {
        expect(removeSystemInjections(text)).toBe(text.trim());
    });

    it.each([
        "Unstable background agent appears idle",
        "**THE SUBAGENT JUST CLAIMED THIS TASK IS DONE.",
        "[Category+Skill Reminder]",
        "[EMERGENCY CONTEXT WINDOW WARNING]",
    ])("removes an embedded notice %j through the next blank line", (marker) => {
        expect(removeSystemInjections(`authored\n\n${marker}\ntransport details`)).toBe("authored");
        expect(
            removeSystemInjections(`authored\n\n${marker}\ntransport details\n\nmore authored`),
        ).toBe("authored\n\nmore authored");
    });

    it("removes a directive header and body, keeping a list that continues past a blank line", () => {
        const text =
            "keep\n\n[SYSTEM DIRECTIVE: EIDNARA] do these:\n- one\n\n- two\n\nafter the directive";

        expect(removeSystemInjections(text)).toBe("keep\n\nafter the directive");
    });

    it("removes a directive with no closing bracket through the end of the text", () => {
        expect(removeSystemInjections("keep\n\n[SYSTEM DIRECTIVE: EIDNARA never closed")).toBe(
            "keep",
        );
    });

    it("removes a reminder that shares a part with authored text", () => {
        expect(
            removeSystemInjections(
                "Keep this authored request.\n\n<system-reminder>hidden transport</system-reminder>",
            ),
        ).toBe("Keep this authored request.");
    });

    it("removes several different injections from one text", () => {
        const text =
            "<!-- OMO_INTERNAL_INITIATOR -->\nfirst\n\n[task CALL FAILED] retry\nbody\n\nsecond\n\n[SYSTEM DIRECTIVE: OH-MY-OPENCODE x]\n\nthird";

        expect(removeSystemInjections(text)).toBe("first\n\nsecond\n\nthird");
    });

    it("removes many repeated notice blocks in linear time", () => {
        const block = "[task CALL FAILED - IMMEDIATE RETRY REQUIRED] retry\n\n";
        const text = `${"keep\n\n".repeat(1)}${block.repeat(20_000)}tail`;

        const started = performance.now();
        expect(removeSystemInjections(text)).toBe("keep\n\ntail");
        // 20,000 blocks distinguish a linear pass from one that copies the tail per block.
        expect(performance.now() - started).toBeLessThan(500);
    });
});
