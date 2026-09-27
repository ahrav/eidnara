/**
 * Lives an `eval_runner stale-world` fact world through OpenCode, the Eidnara plugin, and the
 * daemon, and writes the capture `eval_runner stale-arms` reads:
 *
 *     bun scripts/stale-preference.ts --world <stale-world.json> --out <capture.json>
 *         [--context-limit <tokens>] [--tokens-per-turn <tokens>]
 *         [--summarizer-model <bedrock id>] [--summarizer-dump <file.jsonl>]
 */

import { readFileSync, renameSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { captureStaleWorld, DEFAULT_STALE_DRIVER, type FactWorld } from "../src/stale-preference";

const { values } = parseArgs({
    options: {
        world: { type: "string" },
        out: { type: "string" },
        "context-limit": { type: "string" },
        "tokens-per-turn": { type: "string" },
        "summarizer-model": { type: "string" },
        "summarizer-dump": { type: "string" },
    },
});
if (!values.world || !values.out) {
    throw new Error("usage: stale-preference.ts --world <file> --out <file>");
}
let world: FactWorld;
try {
    world = JSON.parse(readFileSync(values.world, "utf8")) as FactWorld;
} catch (error) {
    throw new Error(`--world ${values.world} is not a readable stale world: ${error}`);
}
function positive(flag: string, fallback: number): number {
    const value = values[flag as "context-limit" | "tokens-per-turn"];
    const parsed = value === undefined ? fallback : Number(value);
    if (!Number.isFinite(parsed) || parsed <= 0) throw new Error(`--${flag} must be positive`);
    return parsed;
}
const capture = await captureStaleWorld(world, {
    ...DEFAULT_STALE_DRIVER,
    modelContextLimit: positive("context-limit", DEFAULT_STALE_DRIVER.modelContextLimit),
    tokensPerTurn: positive("tokens-per-turn", DEFAULT_STALE_DRIVER.tokensPerTurn),
    summarizerModel: values["summarizer-model"],
    summarizerDump: values["summarizer-dump"],
    progress: (done, total) => {
        if (done % 25 === 0 || done === total) console.error(`${done}/${total}`);
    },
});
const staged = `${values.out}.staged`;
writeFileSync(staged, JSON.stringify(capture));
renameSync(staged, values.out);
console.log(
    JSON.stringify({ capture: values.out, requests: Object.keys(capture.requests).length }),
);
