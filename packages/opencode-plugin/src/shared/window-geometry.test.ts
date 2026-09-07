import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { resolveLimit, resolveOutputReserve } from "./models-dev-cache";
import {
    deriveWindowGeometry,
    formatWindowDerivationLine,
    parseWindowOverlay,
    placeholderFilteredOutput,
    readWindowOverlayFile,
    resolveWindowOverlayFacts,
    scalarizeFact,
    type WindowOverlay,
    type WindowOverlayFact,
} from "./window-geometry";

const fixture = JSON.parse(
    readFileSync(new URL("./__fixtures__/window-overlay.v1.json", import.meta.url), "utf8"),
) as unknown;
const parsedFixture = parseWindowOverlay(fixture).overlay as WindowOverlay;
const tempDirs: string[] = [];

function fact(
    value: WindowOverlayFact["value"],
    overrides: Partial<WindowOverlayFact> = {},
): WindowOverlayFact {
    return {
        value,
        grade: "measured",
        units: "provider",
        boundary: "Observed",
        source_ref: "test",
        observed_at: "2026-08-13T00:00:00Z",
        ...overrides,
    };
}

function overlay(
    provider: string,
    model: string,
    facts: Record<string, WindowOverlayFact>,
): WindowOverlay {
    return {
        schema: "fusiform-window-overlay/v1",
        generated_at: "2026-08-13T00:00:00Z",
        minted_provider_ids: [],
        cells: [{ provider_id: provider, model_id: model, facts }],
    };
}

afterEach(() => {
    for (const path of tempDirs.splice(0)) rmSync(path, { recursive: true, force: true });
});

describe("Fusiform overlay v1", () => {
    test("scalarizes every tagged union conservatively", () => {
        expect(scalarizeFact({ kind: "stated", value: 272_000 })).toBe(272_000);
        expect(scalarizeFact({ kind: "bracket", at_least: 340_000, below: 372_001 })).toBe(340_000);
        expect(scalarizeFact({ kind: "bracket", below: 372_001 })).toBeUndefined();
        expect(scalarizeFact({ kind: "unknown", why: "never_measured" })).toBeUndefined();
    });

    test("preserves a half-open bracket and derives exactly from at_least", () => {
        const halfOpen = parseWindowOverlay({
            schema: "fusiform-window-overlay/v1",
            generated_at: "2026-08-13T00:00:00Z",
            minted_provider_ids: [],
            cells: [
                {
                    provider_id: "openai-chatgpt-oauth",
                    model_id: "gpt-5.6-sol",
                    facts: {
                        "window.advertised": fact({ kind: "stated", value: 272_000 }),
                        "window.enforced": fact({ kind: "bracket", at_least: 340_000 }),
                    },
                },
            ],
        }).overlay;
        const parsedValue = halfOpen?.cells[0]?.facts["window.enforced"]?.value;
        expect(parsedValue).toEqual({ kind: "bracket", at_least: 340_000 });
        expect(parsedValue && "below" in parsedValue).toBe(false);
        const halfOpenResult = deriveWindowGeometry(
            "openai-chatgpt-oauth",
            "gpt-5.6-sol",
            { context: 272_000, output: 128_000 },
            {
                overlay: resolveWindowOverlayFacts("openai-chatgpt-oauth", "gpt-5.6-sol", halfOpen),
            },
        );
        const statedResult = deriveWindowGeometry(
            "openai-chatgpt-oauth",
            "gpt-5.6-sol",
            { context: 272_000, output: 128_000 },
            {
                overlay: resolveWindowOverlayFacts(
                    "openai-chatgpt-oauth",
                    "gpt-5.6-sol",
                    overlay("openai-chatgpt-oauth", "gpt-5.6-sol", {
                        "window.enforced": fact({ kind: "stated", value: 340_000 }),
                    }),
                ),
            },
        );
        expect(halfOpenResult).toEqual(statedResult);
    });

    test("specific cells override wildcard facts per fact", () => {
        const data: WindowOverlay = {
            schema: "fusiform-window-overlay/v1",
            generated_at: "2026-08-13T00:00:00Z",
            minted_provider_ids: [],
            cells: [
                {
                    provider_id: "ollama-cloud",
                    model_id: "*",
                    facts: {
                        "output.enforced": fact({ kind: "stated", value: 65_536 }),
                        geometry: fact({ kind: "stated", value: "shared_upfront" }),
                    },
                },
                {
                    provider_id: "ollama-cloud",
                    model_id: "deepseek-v4",
                    facts: {
                        "output.enforced": fact({ kind: "stated", value: 32_000 }),
                    },
                },
            ],
        };
        const facts = resolveWindowOverlayFacts("ollama-cloud", "deepseek-v4", data)?.facts;
        expect(scalarizeFact(facts?.["output.enforced"]?.value as never)).toBe(32_000);
        expect(facts?.geometry?.value).toEqual({ kind: "stated", value: "shared_upfront" });
    });

    test("an exact tagged cell beats its tag-less fallback in either file order", () => {
        const tagged = {
            provider_id: "ollama-cloud",
            model_id: "gemma3:27b",
            facts: { "window.enforced": fact({ kind: "stated", value: 131_072 }) },
        };
        const bare = {
            provider_id: "ollama-cloud",
            model_id: "gemma3",
            facts: { "window.enforced": fact({ kind: "stated", value: 400_000 }) },
        };
        for (const cells of [
            [tagged, bare],
            [bare, tagged],
        ]) {
            const data: WindowOverlay = { ...overlay("x", "y", {}), cells };
            const facts = resolveWindowOverlayFacts("ollama-cloud", "gemma3:27b", data)?.facts;
            expect(scalarizeFact(facts?.["window.enforced"]?.value as never)).toBe(131_072);
        }
    });

    test("the requested provider beats a mapped alias in either file order", () => {
        const requested = {
            provider_id: "openai-codex",
            model_id: "gpt-5.6-sol",
            facts: { "window.enforced": fact({ kind: "stated", value: 272_000 }) },
        };
        const alias = {
            provider_id: "openai",
            model_id: "gpt-5.6-sol",
            facts: { "window.enforced": fact({ kind: "stated", value: 400_000 }) },
        };
        for (const cells of [
            [requested, alias],
            [alias, requested],
        ]) {
            const data: WindowOverlay = { ...overlay("x", "y", {}), cells };
            const facts = resolveWindowOverlayFacts("openai-codex", "gpt-5.6-sol", data)?.facts;
            expect(scalarizeFact(facts?.["window.enforced"]?.value as never)).toBe(272_000);
        }
    });

    test("a bracket geometry fact invalidates the cell instead of selecting static geometry", () => {
        const parsed = parseWindowOverlay(
            overlay("google", "gemini-3.5-flash", {
                geometry: fact({ kind: "bracket", at_least: 1 }),
            }),
        );
        expect(parsed.badCells).toBe(1);
        expect(parsed.overlay?.cells).toEqual([]);
        for (const value of [
            { kind: "stated", value: "separate" },
            { kind: "unknown", why: "never_measured" },
        ] as const) {
            const valid = parseWindowOverlay(
                overlay("google", "gemini-3.5-flash", { geometry: fact(value) }),
            );
            expect(valid.badCells).toBe(0);
            expect(valid.overlay?.cells).toHaveLength(1);
        }
    });

    test("refuses an unrecognized schema before considering familiar cells", () => {
        const v2 = {
            ...(fixture as Record<string, unknown>),
            schema: "fusiform-window-overlay/v2",
        };
        const parsed = parseWindowOverlay(v2);
        expect(parsed.overlay).toBeUndefined();
        expect(parsed.refusal).toContain("unrecognized schema");
    });

    test("skips bad cells once and keeps valid cells", () => {
        const badCell = {
            provider_id: "bad",
            model_id: "model",
            facts: { "window.enforced": { value: { kind: "stated", value: 1_000_000 } } },
        };
        const parsed = parseWindowOverlay({
            ...(fixture as Record<string, unknown>),
            cells: [badCell, ...(fixture as { cells: unknown[] }).cells],
        });
        expect(parsed.badCells).toBe(1);
        expect(parsed.overlay?.cells.length).toBe(parsedFixture.cells.length);
    });

    test("rejects window facts outside the sane limit bounds as bad cells", () => {
        const outOfRange = [
            { kind: "stated", value: 50_000_000 },
            { kind: "stated", value: 2_000 },
            { kind: "bracket", at_least: 50_000_000 },
        ] as const;
        for (const value of outOfRange) {
            const parsed = parseWindowOverlay(
                overlay("provider", "model", { "window.enforced": fact(value) }),
            );
            expect(parsed.badCells).toBe(1);
            expect(parsed.overlay?.cells).toEqual([]);
        }
        const inRange = parseWindowOverlay(
            overlay("provider", "model", {
                "window.enforced": fact({ kind: "stated", value: 3_000_000 }),
                "window.advertised": fact({ kind: "stated", value: 20_000 }),
            }),
        );
        expect(inRange.badCells).toBe(0);
        expect(inRange.overlay?.cells).toHaveLength(1);
    });

    test("an out-of-range overlay window file falls back to the catalog window", () => {
        const dir = mkdtempSync(join(tmpdir(), "window-overlay-test-"));
        tempDirs.push(dir);
        const path = join(dir, "overlay.json");
        writeFileSync(
            path,
            JSON.stringify(
                overlay("provider", "model", {
                    "window.enforced": fact({ kind: "stated", value: 50_000_000 }),
                }),
            ),
        );
        const logs: string[] = [];
        const loaded = readWindowOverlayFile(path, (message) => logs.push(message));
        expect(logs).toHaveLength(1);
        const result = deriveWindowGeometry(
            "provider",
            "model",
            { context: 200_000, output: 32_000 },
            { overlay: resolveWindowOverlayFacts("provider", "model", loaded) },
        );
        expect(result?.derivation.window).toBe(200_000);
        expect(result?.usableSoft).toBe(168_000);
    });

    test("a missing file is silent and a bad file logs one summary", () => {
        const dir = mkdtempSync(join(tmpdir(), "window-overlay-test-"));
        tempDirs.push(dir);
        const logs: string[] = [];
        expect(
            readWindowOverlayFile(join(dir, "missing.json"), (message) => logs.push(message)),
        ).toBeUndefined();
        expect(logs).toEqual([]);
        const path = join(dir, "bad.json");
        writeFileSync(path, JSON.stringify({ ...(fixture as object), schema: "v2" }));
        expect(readWindowOverlayFile(path, (message) => logs.push(message))).toBeUndefined();
        expect(logs).toHaveLength(1);
    });
});

describe("window geometry", () => {
    test("filters placeholders independently in both directions", () => {
        expect(placeholderFilteredOutput(0, 1_000_000)).toBeUndefined();
        expect(placeholderFilteredOutput(1_000_000, 1_000_000)).toBeUndefined();
        expect(placeholderFilteredOutput(1_000_001, 1_000_000)).toBeUndefined();
        expect(placeholderFilteredOutput(65_536, 1_000_000)).toBe(65_536);
        expect(
            deriveWindowGeometry("xai", "mixed", {
                context: 500_000,
                input: 300_000,
                output: 500_000,
            })?.usableSoft,
        ).toBe(300_000);
    });

    test("fixture table yields the 65% execution bases from the spec", () => {
        const cases = [
            ["openai-chatgpt-oauth", "gpt-5.6", 272_000, 128_000, 200_200],
            ["anthropic", "claude-opus-5", 1_000_000, 128_000, 629_200],
            // Expected execution base: floor(0.65 * (1_048_576 - 32_000)) = 660_774, not 681_574.
            ["google", "gemini-3.5-flash", 1_048_576, 65_536, 660_774],
            ["xai", "grok-4.6", 500_000, 500_000, 304_200],
            ["ollama-cloud", "deepseek-v4-flash", 1_048_576, 384_000, 660_774],
            ["moonshotai", "kimi-k3", 1_048_576, 1_048_576, 660_774],
        ] as const;
        for (const [provider, model, context, output, expectedExecute] of cases) {
            const result = deriveWindowGeometry(
                provider,
                model,
                { context, output },
                {
                    overlay: resolveWindowOverlayFacts(provider, model, parsedFixture),
                },
            );
            expect(Math.floor((result?.usableSoft ?? 0) * 0.65)).toBe(expectedExecute);
        }
    });

    test("grok derives from geometry, window, and default output without an enforced output", () => {
        const facts = resolveWindowOverlayFacts("xai", "grok-4.6", parsedFixture);
        expect(facts?.facts["output.enforced"]).toBeUndefined();
        const result = deriveWindowGeometry(
            "xai",
            "grok-4.6",
            { context: 500_000, output: 500_000 },
            { overlay: facts },
        );
        expect(result?.geometry).toBe("shared_truncating");
        expect(result?.usableSoft).toBe(468_000);
    });

    test("kimi reserves its default output instead of placeholder advertised output", () => {
        const result = deriveWindowGeometry(
            "moonshotai",
            "kimi-k3",
            { context: 1_048_576, output: 1_048_576 },
            {
                overlay: resolveWindowOverlayFacts("moonshotai", "kimi-k3", parsedFixture),
            },
        );
        expect(result?.derivation.reserve).toBe(32_000);
        expect(result?.usableSoft).toBe(1_016_576);
    });

    test("a placeholder enforced output falls through to the next overlay output fact", () => {
        const data = overlay("provider", "model", {
            "window.enforced": fact({ kind: "stated", value: 200_000 }),
            "output.enforced": fact({ kind: "stated", value: 200_000 }),
            "output.default": fact({ kind: "stated", value: 16_000 }),
        });
        const result = deriveWindowGeometry(
            "provider",
            "model",
            { context: 200_000 },
            { overlay: resolveWindowOverlayFacts("provider", "model", data) },
        );
        expect(result?.derivation.reserve).toBe(16_000);
        expect(result?.derivation.reserveSource).toBe("output_catalog");
        expect(result?.usableSoft).toBe(184_000);
    });

    test("output_reserve forms override a pre-carved input and overlay output facts", () => {
        const data = overlay("openai-codex", "gpt-5.6-sol", {
            "output.enforced": fact({ kind: "stated", value: 68_000 }),
        });
        const cases = [
            16_384,
            { default: 16_384 },
            { default: 32_000, "openai-codex/gpt-5.6-sol": 16_384 },
        ] as const;

        for (const reserveConfig of cases) {
            const result = deriveWindowGeometry(
                "openai-codex",
                "gpt-5.6-sol",
                { context: 400_000, input: 272_000, output: 128_000 },
                {
                    overlay: resolveWindowOverlayFacts("openai-codex", "gpt-5.6-sol", data),
                    outputReserveOverride: resolveOutputReserve(
                        "openai-codex",
                        "gpt-5.6-sol",
                        reserveConfig,
                    ),
                },
            );
            expect(result?.usableSoft).toBe(272_000 - 16_384);
            expect(result?.derivation).toMatchObject({
                window: 272_000,
                reserve: 16_384,
                reserveSource: "output_config",
            });
        }
    });

    test("provider hook wins over overlay, while overlay beats catalog", () => {
        const data = overlay("provider", "model", {
            "window.enforced": fact({ kind: "stated", value: 300_000 }),
            "output.enforced": fact({ kind: "stated", value: 20_000 }),
        });
        const overlayOnly = deriveWindowGeometry(
            "provider",
            "model",
            { context: 200_000, output: 10_000 },
            { overlay: resolveWindowOverlayFacts("provider", "model", data) },
        );
        const hooked = deriveWindowGeometry(
            "provider",
            "model",
            { context: 200_000, output: 10_000 },
            {
                overlay: resolveWindowOverlayFacts("provider", "model", data),
                providerLimit: { context: 400_000, output: 30_000 },
            },
        );
        expect(overlayOnly?.derivation.window).toBe(300_000);
        expect(hooked?.derivation.window).toBe(400_000);
        expect(hooked?.derivation.reserve).toBe(30_000);
    });

    test("clamps an inversion and logs it", () => {
        const logs: string[] = [];
        const result = deriveWindowGeometry(
            "provider",
            "model",
            { context: 200_000, input: 190_000, output: 10_000 },
            {
                providerLimit: { context: 192_000, input: 190_000, output: 10_000 },
                log: (message) => logs.push(message),
            },
        );
        expect(result?.usableHard).toBe(result?.usableSoft);
        expect(logs).toHaveLength(1);
        expect(logs[0]).toContain("clamped");
    });

    test("a detected cap at or below the output keeps reserving output under an overlay", () => {
        const catalog = { context: 1_000_000, output: 128_000 };
        const overlayFacts = resolveWindowOverlayFacts("anthropic", "claude-opus-5", parsedFixture);
        const withoutOverlay = deriveWindowGeometry("anthropic", "claude-opus-5", catalog, {
            contextCap: 128_000,
        });
        const withOverlay = deriveWindowGeometry("anthropic", "claude-opus-5", catalog, {
            overlay: overlayFacts,
            contextCap: 128_000,
        });
        expect(withoutOverlay?.usableSoft).toBe(96_000);
        expect(withOverlay?.usableSoft).toBe(96_000);
        expect(withOverlay?.derivation.reserve).toBe(32_000);
        expect(withOverlay?.derivation.reserveSource).toBe("output_catalog");
        expect(withOverlay?.usableHard).toBe(128_000 - 4_096);
    });

    test("keeps no-overlay usableSoft byte-identical to legacy resolveLimit", () => {
        const corpus = [
            ["openai", "real", { context: 200_000, output: 32_000 }],
            ["openai", "zero", { context: 200_000, output: 0 }],
            ["xai", "placeholder", { context: 500_000, output: 500_000 }],
            ["google", "separate", { context: 1_000_000, output: 65_536 }],
            ["anthropic", "input", { context: 1_000_000, input: 900_000, output: 128_000 }],
        ] as const;
        for (const [provider, model, limit] of corpus) {
            const result = deriveWindowGeometry(provider, model, limit);
            expect(result?.usableSoft).toBe(resolveLimit(limit, provider, model));
        }
    });

    test("display percentage uses the same usableSoft scheduler base", () => {
        const result = deriveWindowGeometry("openai", "model", {
            context: 204_000,
            output: 30_600,
        }) as NonNullable<ReturnType<typeof deriveWindowGeometry>>;
        const input = 105_900;
        const schedulerPercentage = (input / result.usableSoft) * 100;
        expect(formatWindowDerivationLine(input, result)).toContain(
            `(${schedulerPercentage.toFixed(1)}%)`,
        );
    });
});
