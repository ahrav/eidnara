/**
 * The history decay curve, restated in TypeScript from the decay ladder's constants.
 *
 * The delivery campaign uses it to choose how many newer rows put a published case segment at a
 * target tier, and to tell natural selection from budget-guard demotion: a served tier equal to
 * the curve tier at the pass's history budget is natural, and a sparser one is guard pressure. A
 * change to the ladder's constants fails the campaign's natural rows until this restatement
 * matches it again.
 */

import type { ServedTier } from "./corpus";

/** Half-life in segment positions for importance 50 at pressure 1. */
const H50 = 24;
/** Importance points that double the half-life. */
const DOUBLING = 25;
/** Lower bounds of P2, P3, P4, and P5 in half-lives. */
const BOUNDARIES = [0.201, 0.729, 1.322, 2.587] as const;
/** Estimated tokens of one segment at P1 through P4. */
const TIER_COST = [322, 109, 35, 20] as const;
const PRESSURE_FLOOR = 0.1;
/** Newest rows whose importance moves the pressure; older rows add no cost. */
const PRESSURE_WINDOW = 249;
const TIERS = ["p1", "p2", "p3", "p4", "p5"] as const;

/** The rendered tier of a row at 1-based `position` from the newest, with no anchor overlap. */
export function tierAt(position: number, importance: number, pressure: number): ServedTier {
    return TIERS[tierIndex(position, importance, pressure)] ?? "p5";
}

function tierIndex(position: number, importance: number, pressure: number): number {
    const clamped = Math.min(100, Math.max(1, importance));
    const halfLife = (H50 * 2 ** ((clamped - 50) / DOUBLING)) / Math.max(PRESSURE_FLOOR, pressure);
    const z = (Math.max(position, 1) - 1) / halfLife;
    const index = BOUNDARIES.findIndex((bound) => z < bound);
    return index < 0 ? BOUNDARIES.length : index;
}

/** The pressure a positive budget puts on rows whose importances run newest first. */
export function budgetPressure(importancesNewestFirst: readonly number[], budget: number): number {
    if (budget <= 0) return 1;
    let natural = 0;
    for (const [i, importance] of importancesNewestFirst.slice(0, PRESSURE_WINDOW).entries()) {
        natural += TIER_COST[tierIndex(i + 1, importance, 1)] ?? 0;
    }
    return Math.max(PRESSURE_FLOOR, natural / budget);
}

/** The rows of a campaign session, newest first: `newer` rows, the case, then `older` rows. */
function caseRows(caseImportance: number, newer: number, older: number, rest: number): number[] {
    return [...Array<number>(newer).fill(rest), caseImportance, ...Array<number>(older).fill(rest)];
}

/** The curve tier of the row at 1-based `position` from the newest, under `budget`. */
export function curveTier(
    importancesNewestFirst: readonly number[],
    position: number,
    budget: number,
): ServedTier {
    const importance = importancesNewestFirst[position - 1];
    if (importance === undefined) throw new Error(`no row at position ${position}`);
    return tierAt(position, importance, budgetPressure(importancesNewestFirst, budget));
}

/**
 * The inclusive newer-row ranges at which the case segment serves each tier under `budget`,
 * with every other row at `rest` importance, searched up to `limit` newer rows.
 */
export function tierWindows(
    caseImportance: number,
    older: number,
    rest: number,
    budget: number,
    limit = 400,
): Map<ServedTier, { first: number; last: number }> {
    const windows = new Map<ServedTier, { first: number; last: number }>();
    for (let newer = 0; newer <= limit; newer += 1) {
        const tier = curveTier(caseRows(caseImportance, newer, older, rest), newer + 1, budget);
        const window = windows.get(tier);
        if (window) window.last = newer;
        else windows.set(tier, { first: newer, last: newer });
    }
    return windows;
}
