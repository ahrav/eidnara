import { describe, expect, it } from "bun:test";

import { isSystemDirective, removeSystemReminders } from "./system-directive";

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

    it("rejects other text", () => {
        expect(isSystemDirective("[SYSTEM DIRECTIVE: OTHER]")).toBe(false);
        expect(isSystemDirective("hello")).toBe(false);
    });
});
