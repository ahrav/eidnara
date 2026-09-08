import { describe, expect, test } from "bun:test";
import { badgeTextColor, readableTextColorOn } from "./badge-contrast";

describe("badgeTextColor (AFT parity with #186 safety net)", () => {
    const accent = { r: 0.6, g: 0.5, b: 0.9, a: 1 };

    test("opaque distinct background is used verbatim as the label", () => {
        const background = { r: 0.05, g: 0.05, b: 0.07, a: 1 }; // near-black dark theme
        expect(badgeTextColor(accent, background)).toBe(background);
    });

    test("light theme: background is used verbatim too (near-white label inverse)", () => {
        const background = { r: 1, g: 1, b: 1, a: 1 }; // ~3.2:1 against the accent
        expect(badgeTextColor(accent, background)).toBe(background);
    });

    test("transparent background (alpha 0) falls back to a visible pick (#186)", () => {
        const transparent = { r: 0, g: 0, b: 0, a: 0 };
        const result = badgeTextColor(accent, transparent);
        expect(result).not.toBe(transparent);
        expect(result).toBe(readableTextColorOn(accent));
    });

    test("background ~= accent falls back to a visible pick", () => {
        const sameAsAccent = { r: 0.6, g: 0.5, b: 0.9, a: 1 };
        const result = badgeTextColor(accent, sameAsAccent);
        expect(result).toBe(readableTextColorOn(accent));
    });

    test("opaque background below 3:1 against the accent falls back even when channels differ", () => {
        // Channels differ by 0.07 but contrast is only ~1.3:1; white on the accent is ~12.6:1.
        const darkAccent = { r: 0.2, g: 0.2, b: 0.2, a: 1 };
        const nearbyBackground = { r: 0.27, g: 0.27, b: 0.27, a: 1 };
        const result = badgeTextColor(darkAccent, nearbyBackground);
        expect(result).not.toBe(nearbyBackground);
        expect(result).toBe("#ffffff");
    });

    test("translucent background is judged after compositing onto the accent", () => {
        // Raw black vs 0.5 gray is ~5.3:1, but half-alpha black renders as 0.25 gray at ~2.6:1.
        const grayAccent = { r: 0.5, g: 0.5, b: 0.5, a: 1 };
        const halfBlack = { r: 0, g: 0, b: 0, a: 0.5 };
        const result = badgeTextColor(grayAccent, halfBlack);
        expect(result).not.toBe(halfBlack);
        expect(result).toBe(readableTextColorOn(grayAccent));
    });

    test("missing alpha is treated as opaque", () => {
        const background = { r: 0.05, g: 0.05, b: 0.07 };
        expect(badgeTextColor(accent, background)).toBe(background);
    });
});

describe("readableTextColorOn", () => {
    test("dark accent gets white text", () => {
        expect(readableTextColorOn({ r: 0.1, g: 0.1, b: 0.3 })).toBe("#ffffff");
        expect(readableTextColorOn({ r: 0, g: 0, b: 0 })).toBe("#ffffff");
    });

    test("light accent gets black text", () => {
        // White text does not meet the contrast threshold on this accent.
        expect(readableTextColorOn({ r: 0.9, g: 0.9, b: 0.7 })).toBe("#000000");
        expect(readableTextColorOn({ r: 1, g: 1, b: 1 })).toBe("#000000");
    });

    test("mid-tone orange accent prefers white (white-bias, matches sibling badges)", () => {
        // readableTextColorOn prefers white when it meets the bold-text contrast threshold, even if black has higher contrast.
        expect(readableTextColorOn({ r: 0.69, g: 0.455, b: 0.188 })).toBe("#ffffff");
        expect(readableTextColorOn({ r: 0.741, g: 0.482, b: 0.2 })).toBe("#ffffff");
    });

    test("pure green is treated as light (white fails the contrast bar)", () => {
        // White text does not meet the contrast threshold on this green.
        expect(readableTextColorOn({ r: 0, g: 1, b: 0 })).toBe("#000000");
    });

    test("medium-light gray gets black text (white is only ~2.1:1)", () => {
        // Luminance ~0.448: white contrast ~2.1:1 fails the 3:1 bar; black contrast ~10:1.
        expect(readableTextColorOn({ r: 0.7, g: 0.7, b: 0.7 })).toBe("#000000");
    });

    test("pure blue is treated as dark (low luma weight)", () => {
        // Blue's low perceived brightness requires light text.
        expect(readableTextColorOn({ r: 0, g: 0, b: 1 })).toBe("#ffffff");
    });

    test("does not depend on the (possibly transparent) background alpha", () => {
        const a = readableTextColorOn({ r: 0.2, g: 0.2, b: 0.2 });
        const b = readableTextColorOn({ r: 0.2, g: 0.2, b: 0.2 });
        expect(a).toBe(b);
        expect(a).toBe("#ffffff");
    });
});
