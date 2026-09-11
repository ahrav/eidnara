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

    test("translucent accent is judged after compositing onto the background", () => {
        // A transparent accent renders the badge white, so white label text would vanish.
        const transparentAccent = { r: 0, g: 0, b: 0, a: 0 };
        const white = { r: 1, g: 1, b: 1, a: 1 };
        const result = badgeTextColor(transparentAccent, white);
        expect(result).not.toBe(white);
        expect(result).toBe("#000000");
    });

    test("missing alpha is treated as opaque", () => {
        const background = { r: 0.05, g: 0.05, b: 0.07 };
        expect(badgeTextColor(accent, background)).toBe(background);
    });
});

describe("readableTextColorOn", () => {
    test("picks white on dark and white-biased accents and black where white fails the 3:1 bar", () => {
        const cases: Array<[{ r: number; g: number; b: number }, string, string]> = [
            [{ r: 0.1, g: 0.1, b: 0.3 }, "#ffffff", "dark navy"],
            [{ r: 0, g: 0, b: 0 }, "#ffffff", "black"],
            [{ r: 0.2, g: 0.2, b: 0.2 }, "#ffffff", "dark gray"],
            // Blue's low perceived brightness requires light text.
            [{ r: 0, g: 0, b: 1 }, "#ffffff", "pure blue"],
            // White is preferred once it meets the bold-text contrast threshold, even when black has higher contrast.
            [{ r: 0.69, g: 0.455, b: 0.188 }, "#ffffff", "mid-tone orange"],
            [{ r: 0.741, g: 0.482, b: 0.2 }, "#ffffff", "mid-tone orange, lighter"],
            // White text does not meet the contrast threshold on these accents.
            [{ r: 0.9, g: 0.9, b: 0.7 }, "#000000", "light cream"],
            [{ r: 1, g: 1, b: 1 }, "#000000", "white"],
            [{ r: 0, g: 1, b: 0 }, "#000000", "pure green"],
            // Luminance ~0.448: white contrast ~2.1:1 fails the 3:1 bar; black contrast ~10:1.
            [{ r: 0.7, g: 0.7, b: 0.7 }, "#000000", "medium-light gray"],
        ];
        for (const [accent, expected, label] of cases) {
            expect(readableTextColorOn(accent), label).toBe(expected);
        }
    });
});
