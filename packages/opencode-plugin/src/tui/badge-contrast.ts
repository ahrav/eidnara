type Color = { r: number; g: number; b: number; a?: number };

/** WCAG 2 requires a 3:1 contrast ratio for large or bold text. */
const MIN_LABEL_CONTRAST = 3;

const WHITE_LUMINANCE = 1;

function srgbChannelToLinear(c: number): number {
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function relativeLuminance(bg: Color): number {
    return (
        0.2126 * srgbChannelToLinear(bg.r) +
        0.7152 * srgbChannelToLinear(bg.g) +
        0.0722 * srgbChannelToLinear(bg.b)
    );
}

function contrastRatio(a: number, b: number): number {
    const [lighter, darker] = a >= b ? [a, b] : [b, a];
    return (lighter + 0.05) / (darker + 0.05);
}

function compositeOver(fg: Color, bg: Color): Color {
    const alpha = fg.a ?? 1;
    return {
        r: fg.r * alpha + bg.r * (1 - alpha),
        g: fg.g * alpha + bg.g * (1 - alpha),
        b: fg.b * alpha + bg.b * (1 - alpha),
    };
}

/**
 * Prefers white whenever it clears the 3:1 bar, even when black would contrast more.
 */
export function readableTextColorOn(bg: Color): string {
    const whiteContrast = contrastRatio(WHITE_LUMINANCE, relativeLuminance(bg));
    return whiteContrast >= MIN_LABEL_CONTRAST ? "#ffffff" : "#000000";
}

/**
 * A translucent `background` is composited over `accent` before the contrast check; a fully transparent background has 1:1 contrast with `accent`.
 */
export function badgeTextColor<T extends Color>(accent: T, background: T): T | string {
    const rendered = compositeOver(background, accent);
    if (
        contrastRatio(relativeLuminance(rendered), relativeLuminance(accent)) >= MIN_LABEL_CONTRAST
    ) {
        return background;
    }
    return readableTextColorOn(accent);
}
