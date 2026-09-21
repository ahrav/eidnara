/**
 * The frozen statistical reference for `crates/eval-core/src/statistics.rs`.
 *
 * This file shares no code with the Rust estimators. It computes every quantity in exact
 * BigInt rationals: the three gates and pair counts under the conservative censoring rule, the
 * one-way ANOVA intraclass correlation and the clustering unit it selects, the design-effect
 * deflated effective N, and the percentile cluster bootstrap driven by the pinned SHA-256 draw.
 * The Rust differential test recomputes `input_sha256` over the pretty-printed cases and
 * asserts every expected value, so an estimator change on either side is reviewed.
 *
 * Regenerate with `bun crates/eval-core/gen/gen-statistics-golden.ts`.
 */
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";

const generatorVersion = "statistics-reference-ts-v1";
const BOOTSTRAP_PROTOCOL = "eval-cluster-bootstrap/v1";

// Exact rationals in lowest terms with a positive denominator.
interface Ratio {
  numerator: bigint;
  denominator: bigint;
}
const abs = (value: bigint) => (value < 0n ? -value : value);
function gcd(a: bigint, b: bigint): bigint {
  while (b !== 0n) [a, b] = [b, a % b];
  return a === 0n ? 1n : a;
}
function ratio(numerator: bigint, denominator: bigint): Ratio {
  if (denominator < 0n) [numerator, denominator] = [-numerator, -denominator];
  const divisor = gcd(abs(numerator), denominator);
  return { numerator: numerator / divisor, denominator: denominator / divisor };
}
const sub = (a: Ratio, b: Ratio) =>
  ratio(a.numerator * b.denominator - b.numerator * a.denominator, a.denominator * b.denominator);
const add = (a: Ratio, b: Ratio) =>
  ratio(a.numerator * b.denominator + b.numerator * a.denominator, a.denominator * b.denominator);
const mul = (a: Ratio, b: Ratio) => ratio(a.numerator * b.numerator, a.denominator * b.denominator);
const div = (a: Ratio, b: Ratio) => ratio(a.numerator * b.denominator, a.denominator * b.numerator);
const cmp = (a: Ratio, b: Ratio) => {
  const left = a.numerator * b.denominator;
  const right = b.numerator * a.denominator;
  return left < right ? -1 : left > right ? 1 : 0;
};
const whole = (n: number | bigint) => ratio(BigInt(n), 1n);
const emit = (r: Ratio) => ({ numerator: Number(r.numerator), denominator: Number(r.denominator) });

type Arm = "pass" | "fail" | { censored: string };
interface Pair {
  pair_id: string;
  cluster: { family: string; world_seed: number };
  fresh: Arm;
  aged: Arm;
}
const isCensored = (arm: Arm) => typeof arm !== "string";
function counts(pairs: readonly Pair[]) {
  let b = 0;
  let c = 0;
  let agedPass = 0;
  let freshCensored = 0;
  let agedCensored = 0;
  for (const pair of pairs) {
    if (isCensored(pair.fresh)) freshCensored += 1;
    if (isCensored(pair.aged)) agedCensored += 1;
    if (pair.aged === "pass") agedPass += 1;
    if (pair.fresh !== "fail" && pair.aged !== "pass") b += 1;
    if (pair.fresh === "fail" && pair.aged === "pass") c += 1;
  }
  const n = pairs.length;
  return {
    n,
    b,
    c,
    aged_pass: agedPass,
    fresh_censored: freshCensored,
    aged_censored: agedCensored,
    quality_loss: emit(ratio(BigInt(b - c), BigInt(Math.max(n, 1)))),
    harm: emit(ratio(BigInt(b), BigInt(Math.max(n, 1)))),
    aged_pass_rate: emit(ratio(BigInt(agedPass), BigInt(Math.max(n, 1)))),
  };
}

// One-way ANOVA ICC: (MSB - MSW) / (MSB + (m0 - 1) MSW), m0 the mean group size.
function icc(groups: readonly (readonly number[])[]): Ratio {
  const k = BigInt(groups.length);
  const values = groups.flat();
  const n = BigInt(values.length);
  const mean = ratio(BigInt(values.reduce((s, v) => s + v, 0)), n);
  let ssb: Ratio = whole(0);
  let ssw: Ratio = whole(0);
  for (const group of groups) {
    const m = BigInt(group.length);
    const groupMean = ratio(BigInt(group.reduce((s, v) => s + v, 0)), m);
    const between = sub(groupMean, mean);
    ssb = add(ssb, mul(whole(m), mul(between, between)));
    for (const value of group) {
      const within = sub(whole(value), groupMean);
      ssw = add(ssw, mul(within, within));
    }
  }
  const msb = div(ssb, whole(k - 1n));
  const msw = div(ssw, whole(n - k));
  // The unequal-group size correction n0 = (N - sum(n_i^2) / N) / (k - 1); the group size when balanced.
  const sumOfSquares = groups.reduce((s, g) => s + BigInt(g.length) ** 2n, 0n);
  const n0 = ratio(n * n - sumOfSquares, n * (k - 1n));
  return div(sub(msb, msw), add(msb, mul(sub(n0, whole(1)), msw)));
}

interface Observation {
  cluster: { family: string; world_seed: number };
  task: string;
  value: number;
}
// Groups sort by (family, then numeric seed), the order the Rust BTreeMap<ClusterKey, _> uses.
function groupBy(observations: readonly Observation[], key: (o: Observation) => string) {
  const groups = new Map<string, number[]>();
  for (const o of observations) {
    const list = groups.get(key(o)) ?? [];
    list.push(o.value);
    groups.set(key(o), list);
  }
  return [...groups.keys()].sort(compareKeys).map((k) => groups.get(k) ?? []);
}
const clusterKey = (family: string, seed: number) => `${family}\u0000${seed}`;
// Families compare by UTF-8 bytes, the order Rust's `String` uses; JS `<` compares UTF-16 code
// units, which disagrees for supplementary characters against private-use ones.
// The seed follows the last NUL, so a family name that itself contains NUL splits correctly.
function splitKey(key: string): [string, string] {
  const at = key.lastIndexOf("\u0000");
  return [key.slice(0, at), key.slice(at + 1)];
}
function compareKeys(left: string, right: string): number {
  const [lf, ls] = splitKey(left);
  const [rf, rs] = splitKey(right);
  if (lf !== rf) return Buffer.compare(Buffer.from(lf, "utf8"), Buffer.from(rf, "utf8"));
  return Number(ls) - Number(rs);
}
// The design effect is clamped at one, so deflation only ever shrinks N.
function pilot(observations: readonly Observation[], maxAffordableWorlds: number) {
  const byFamily = groupBy(observations, (o) => clusterKey(o.cluster.family, 0));
  const byWorld = groupBy(observations, (o) => clusterKey(o.cluster.family, o.cluster.world_seed));
  const iccFamily = icc(byFamily);
  const iccWorld = icc(byWorld);
  const threshold = ratio(1n, 20n);
  const family = cmp(iccFamily, threshold) > 0;
  const itemsAtMax = mul(ratio(BigInt(observations.length), BigInt(byWorld.length)), whole(maxAffordableWorlds));
  // Deflate at both nesting levels and keep the smaller, so a stronger finer-level correlation
  // is never discarded by selecting the coarser unit. Each affordable world lies in one family,
  // so at most that many family clusters are realized.
  const deflate = (clusters: number, icc: Ratio): Ratio => {
    const meanCluster = div(itemsAtMax, whole(clusters));
    const positiveIcc = cmp(icc, whole(0)) < 0 ? whole(0) : icc;
    const rawEffect = add(whole(1), mul(sub(meanCluster, whole(1)), positiveIcc));
    const designEffect = cmp(rawEffect, whole(1)) < 0 ? whole(1) : rawEffect;
    return div(itemsAtMax, designEffect);
  };
  const atFamily = deflate(Math.min(byFamily.length, maxAffordableWorlds), iccFamily);
  const atWorld = deflate(maxAffordableWorlds, iccWorld);
  return {
    icc_family: emit(iccFamily),
    icc_world_seed: emit(iccWorld),
    clustering_unit: family ? "family" : "world_seed",
    n_families: byFamily.length,
    n_worlds: byWorld.length,
    effective_n_at_max: emit(cmp(atFamily, atWorld) <= 0 ? atFamily : atWorld),
  };
}

// The pinned draw: first 64 bits of sha256("<protocol>\n" + canonical key) mod clusters.
function draw(seed: number, replicate: number, drawIndex: number, clusters: number): number {
  const key = `{"draw":${drawIndex},"replicate":${replicate},"seed":"${seed}"}`;
  const hex = createHash("sha256").update(`${BOOTSTRAP_PROTOCOL}\n${key}`).digest("hex");
  return Number(BigInt(`0x${hex.slice(0, 16)}`) % BigInt(clusters));
}
function bootstrap(pairs: readonly Pair[], unit: "family" | "world_seed", seed: number, replicates: number) {
  const clusters = new Map<string, { n: number; diff: number }>();
  for (const pair of pairs) {
    const key = clusterKey(pair.cluster.family, unit === "family" ? 0 : pair.cluster.world_seed);
    const cell = clusters.get(key) ?? { n: 0, diff: 0 };
    const one = counts([pair]);
    cell.n += 1;
    cell.diff += one.b - one.c;
    clusters.set(key, cell);
  }
  const ordered = [...clusters.keys()].sort(compareKeys).map((k) => clusters.get(k) ?? { n: 0, diff: 0 });
  const statistics: Ratio[] = [];
  for (let replicate = 0; replicate < replicates; replicate += 1) {
    let n = 0;
    let diff = 0;
    for (let i = 0; i < ordered.length; i += 1) {
      const picked = ordered[draw(seed, replicate, i, ordered.length)] ?? { n: 0, diff: 0 };
      n += picked.n;
      diff += picked.diff;
    }
    statistics.push(ratio(BigInt(diff), BigInt(Math.max(n, 1))));
  }
  statistics.sort(cmp);
  // The 1/40 and 39/40 order statistics: the ceil(B/40)-th and ceil(39B/40)-th smallest.
  return {
    n_clusters: ordered.length,
    n_items: pairs.length,
    lower: emit(statistics[Math.ceil(replicates / 40) - 1] ?? whole(0)),
    upper: emit(statistics[replicates - Math.floor(replicates / 40) - 1] ?? whole(0)),
  };
}

// The three gates against a fixture profile: noninferiority is `quality_loss <= margin`
// (signed, so aged-better passes), harm is `harm <= bound`, floor is `aged_pass_rate >= floor`.
const profile = { noninferiority_margin: ratio(1n, 50n), harm_bound: ratio(1n, 10n), floor_threshold: ratio(7n, 10n) };
function gates(pairs: readonly Pair[]) {
  const c = counts(pairs);
  const asRatio = (r: { numerator: number; denominator: number }) => ratio(BigInt(r.numerator), BigInt(r.denominator));
  const verdict = (statistic: Ratio, bound: Ratio, passed: boolean) => ({ statistic: emit(statistic), bound: emit(bound), passed });
  const quality = asRatio(c.quality_loss);
  const harm = asRatio(c.harm);
  const floor = asRatio(c.aged_pass_rate);
  return {
    quality_loss: verdict(quality, profile.noninferiority_margin, cmp(quality, profile.noninferiority_margin) <= 0),
    harm: verdict(harm, profile.harm_bound, cmp(harm, profile.harm_bound) <= 0),
    floor: verdict(floor, profile.floor_threshold, cmp(floor, profile.floor_threshold) >= 0),
  };
}

// Fixtures. The pair fixture is 6 families x 50 seeds; outcomes follow a fixed rule so the
// table is reproducible without randomness: family index and seed decide the cell.
const families = ["cargo", "tokio", "django", "git", "docs", "tests"];
const pairFixture: Pair[] = [];
for (const [f, family] of families.entries()) {
  for (let seed = 0; seed < 50; seed += 1) {
    const roll = (f * 7 + seed * 13) % 20;
    const fresh: Arm = roll % 4 === 0 ? "fail" : "pass";
    let aged: Arm = roll < 3 ? "fail" : roll === 3 ? { censored: "timeout" } : roll === 4 ? "pass" : fresh;
    if (roll === 5) aged = "pass";
    pairFixture.push({ pair_id: `${family}-${seed}`, cluster: { family, world_seed: seed }, fresh, aged });
  }
}
const gateFixtures = {
  "aged-better": {
    pairs: [
      ...Array.from({ length: 3 }, (_, i): Pair => ({ pair_id: `b${i}`, cluster: { family: "f", world_seed: i }, fresh: "pass", aged: "fail" })),
      ...Array.from({ length: 5 }, (_, i): Pair => ({ pair_id: `c${i}`, cluster: { family: "f", world_seed: i + 3 }, fresh: "fail", aged: "pass" })),
      ...Array.from({ length: 12 }, (_, i): Pair => ({ pair_id: `p${i}`, cluster: { family: "g", world_seed: i }, fresh: "pass", aged: "pass" })),
    ],
  },
  "equal-discordance": {
    pairs: [
      ...Array.from({ length: 4 }, (_, i): Pair => ({ pair_id: `b${i}`, cluster: { family: "f", world_seed: i }, fresh: "pass", aged: "fail" })),
      ...Array.from({ length: 4 }, (_, i): Pair => ({ pair_id: `c${i}`, cluster: { family: "f", world_seed: i + 4 }, fresh: "fail", aged: "pass" })),
      ...Array.from({ length: 12 }, (_, i): Pair => ({ pair_id: `p${i}`, cluster: { family: "g", world_seed: i }, fresh: "fail", aged: "fail" })),
    ],
  },
  "aged-worse": {
    pairs: [
      ...Array.from({ length: 5 }, (_, i): Pair => ({ pair_id: `b${i}`, cluster: { family: "f", world_seed: i }, fresh: "pass", aged: "fail" })),
      ...Array.from({ length: 15 }, (_, i): Pair => ({ pair_id: `p${i}`, cluster: { family: "g", world_seed: i }, fresh: "pass", aged: "pass" })),
    ],
  },
  "at-the-margin": {
    pairs: [
      { pair_id: "b0", cluster: { family: "f", world_seed: 0 }, fresh: "pass", aged: "fail" },
      ...Array.from({ length: 49 }, (_, i): Pair => ({ pair_id: `p${i}`, cluster: { family: "g", world_seed: i }, fresh: "pass", aged: "pass" })),
    ] as Pair[],
  },
  "censored-arms": {
    pairs: [
      { pair_id: "loss", cluster: { family: "f", world_seed: 0 }, fresh: "pass", aged: { censored: "max_tokens_out" } },
      { pair_id: "unknown", cluster: { family: "f", world_seed: 1 }, fresh: { censored: "hard_deadline_ms" }, aged: "pass" },
      { pair_id: "both", cluster: { family: "f", world_seed: 2 }, fresh: { censored: "timeout" }, aged: { censored: "max_tool_calls" } },
      { pair_id: "kept", cluster: { family: "g", world_seed: 0 }, fresh: "pass", aged: "pass" },
    ] as Pair[],
  },
};
// Pilot observations: 6 families x 20 seeds. The positive fixture adds a family effect large
// enough that the family ICC clears 1/20; the control has no family effect.
const pilotFixture = (familyEffect: boolean): Observation[] =>
  families.flatMap((family, f) =>
    Array.from({ length: 20 }, (_, seed) =>
      Array.from({ length: 3 }, (_, t) => ({
        cluster: { family, world_seed: seed },
        task: `t${t}`,
        value: (familyEffect ? f * 10 : 0) + ((seed * 7 + f + t * 3) % 5),
      })),
    ).flat(),
  );
// Unbalanced worlds (one family has three worlds, another one) with a strong world effect.
const unbalancedFixture: Observation[] = [
  ...[0, 1, 2].flatMap((seed) => [0, 1].map((t) => ({ cluster: { family: "a", world_seed: seed }, task: `t${t}`, value: seed * 4 + t }))),
  ...[0, 1, 2, 3].map((t) => ({ cluster: { family: "b", world_seed: 9 }, task: `t${t}`, value: 20 + (t % 2) })),
];
// Every world internally constant: the world ICC is exactly one.
const constantFixture: Observation[] = [
  ...[0, 1].map((t) => ({ cluster: { family: "a", world_seed: 0 }, task: `t${t}`, value: 3 })),
  ...[0, 1].map((t) => ({ cluster: { family: "a", world_seed: 1 }, task: `t${t}`, value: 7 })),
  ...[0, 1].map((t) => ({ cluster: { family: "b", world_seed: 0 }, task: `t${t}`, value: 11 })),
];

const pilotCase = (id: string, observations: Observation[], maxAffordableWorlds: number) => ({
  id,
  kind: "icc_pilot",
  input: { max_affordable_worlds: maxAffordableWorlds, observations },
  expected: pilot(observations, maxAffordableWorlds),
});
const cases = [
  ...Object.entries(gateFixtures).map(([id, fixture]) => ({
    id: `gates-${id}`,
    kind: "pair_counts",
    input: { pairs: fixture.pairs },
    expected: { ...counts(fixture.pairs), gates: gates(fixture.pairs) },
  })),
  pilotCase("pilot-family-effect", pilotFixture(true), 40),
  pilotCase("pilot-no-family-effect", pilotFixture(false), 40),
  // Fewer affordable worlds than the pilot had: the clamp keeps effective N at the item count.
  pilotCase("pilot-fewer-affordable-worlds", pilotFixture(false), 2),
  pilotCase("pilot-family-effect-fewer-affordable-worlds-than-families", pilotFixture(true), 2),
  pilotCase("pilot-unbalanced-worlds", unbalancedFixture, 3),
  pilotCase("pilot-constant-worlds", constantFixture, 1),
  {
    id: "bootstrap-family-unit",
    kind: "cluster_bootstrap",
    input: { pairs: pairFixture, replicates: 400, seed: 7, unit: "family" },
    expected: bootstrap(pairFixture, "family", 7, 400),
  },
  {
    id: "bootstrap-world-seed-unit",
    kind: "cluster_bootstrap",
    input: { pairs: pairFixture, replicates: 400, seed: 7, unit: "world_seed" },
    expected: bootstrap(pairFixture, "world_seed", 7, 400),
  },
  {
    // At the minimum replicate count the 1/40 order statistic is the smallest replicate.
    id: "bootstrap-world-seed-unit-minimum-replicates",
    kind: "cluster_bootstrap",
    input: { pairs: pairFixture, replicates: 40, seed: 7, unit: "world_seed" },
    expected: bootstrap(pairFixture, "world_seed", 7, 40),
  },
];

// The Rust reader recomputes this hash over `serde_json::to_string_pretty` of the whole case
// array, expectations included, whose maps sort keys; so keys are sorted here before hashing,
// and a hand-edited expectation is caught.
type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
function sortKeys(value: unknown): Json {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    return Object.fromEntries(
      Object.keys(record)
        .sort()
        .map((k) => [k, sortKeys(record[k])]),
    );
  }
  if (value === null || typeof value === "boolean" || typeof value === "number" || typeof value === "string") {
    return value;
  }
  throw new Error(`not JSON: ${typeof value}`);
}
const canonical = (value: unknown): string => `${JSON.stringify(sortKeys(value), null, 2)}\n`;
const inputHash = createHash("sha256").update(canonical(cases)).digest("hex");
const golden = {
  schema: 1,
  provenance: {
    generator: "crates/eval-core/gen/gen-statistics-golden.ts",
    generator_version: generatorVersion,
    input_sha256: inputHash,
  },
  cases,
};
const outPath = join(dirname(import.meta.path), "../testdata/statistics-golden.json");
await Bun.write(outPath, canonical(golden));
console.log(`wrote ${outPath} (${cases.length} cases, input ${inputHash})`);
