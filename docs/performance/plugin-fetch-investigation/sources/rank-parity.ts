// Ranks a dumped `kernel.read` body with the plugin's own matcher so a
// server-side ranker can be compared against it id by id.
// Usage: bun rank-parity.ts <read-body.json> <k> <query>...
import { searchKernelMemoryRows } from "../../../../packages/opencode-plugin/src/tools/eidnara-search/kernel-memory-search.ts";

const [path, kText, ...queries] = process.argv.slice(2);
if (!path || !kText) throw new Error("usage: rank-parity.ts <read-body.json> <k> <query>...");
const body = JSON.parse(await Bun.file(path).text());
const rows = body.rows.map((row: any) => ({
    ...row,
    object: { ...row.object, domain_id: "memory" },
}));
for (const query of queries) {
    const hits = searchKernelMemoryRows({ rows, query, limit: Number(kText), nowMs: 0 }) ?? [];
    console.log(`RANK ${JSON.stringify(query)} k=${hits.length} ids=${hits.map((hit) => hit.objectId).join(",")}`);
}
const samples = ["İstanbul", "STRAẞE", "ΣΊΣΥΦΟΣ", "ǅemal", "ＡＢＣ", "Ⅸ", "ﬀ", "ᾈ", "ΑΣ", "İ", "K", "ẞ", "ΌΣ ΟΣ", "ΠΡΟΣ"];
for (const sample of samples) {
    const lowered = sample.toLowerCase();
    console.log(`LOWER ${JSON.stringify(sample)} -> ${JSON.stringify(lowered)} ${Buffer.from(lowered, "utf8").toString("hex")}`);
}
