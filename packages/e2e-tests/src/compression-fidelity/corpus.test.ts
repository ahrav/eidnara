import { afterEach, describe, expect, it } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
    COMPRESSION_FIDELITY_CORPUS_PATH,
    COMPRESSION_FIDELITY_CORPUS_SHA256,
    CorpusIdentityError,
    readCompressionFidelityCorpus,
} from "./corpus";

const RUST_OWNER = resolve(
    import.meta.dir,
    "../../../../crates/daemon/src/compression_fidelity_corpus.rs",
);

const committed = readFileSync(COMPRESSION_FIDELITY_CORPUS_PATH);
const scratch: string[] = [];

afterEach(() => {
    for (const dir of scratch.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function scratchDir(): string {
    const dir = mkdtempSync(join(tmpdir(), "cf-corpus-"));
    scratch.push(dir);
    return dir;
}

function copyWith(bytes: Buffer | string): string {
    const path = join(scratchDir(), "compression-fidelity.json");
    writeFileSync(path, bytes);
    return path;
}

describe("compression fidelity corpus reader", () => {
    it("accepts the exact committed bytes under the pin the Rust owner enforces", () => {
        const rust = readFileSync(RUST_OWNER, "utf8");
        const pin = /CORPUS_SHA256: &str =\s*"([0-9a-f]{64})"/.exec(rust)?.[1];
        expect(pin).toBe(COMPRESSION_FIDELITY_CORPUS_SHA256);

        const corpus = readCompressionFidelityCorpus();
        expect(corpus.cases.map((c) => c.id)).toEqual(["C1", "C2", "C3", "C4", "C5", "C6"]);
        expect(readCompressionFidelityCorpus(copyWith(committed))).toEqual(corpus);
    });

    it("rejects missing, stale, whitespace-only, same-length, and scenario-only copies", () => {
        const text = committed.toString("utf8");
        const older = JSON.parse(text);
        older.cases.pop();
        const edits: Record<string, string> = {
            "stale prebuilt copy": `${JSON.stringify(older, null, 2)}\n`,
            "whitespace only": text.replace("\n  ", "\n   "),
            "same length": text.replace("quick win", "quick wit"),
            "scenario only": text.replace('"abstention": "forbidden"', '"abstention": "permitted"'),
        };
        expect(Buffer.byteLength(edits["same length"] ?? "")).toBe(committed.length);
        for (const [label, edited] of Object.entries(edits)) {
            expect(edited, label).not.toBe(text);
            const read = () => readCompressionFidelityCorpus(copyWith(edited));
            expect(read, label).toThrow(CorpusIdentityError);
            expect(read, label).toThrow(/hashes to/);
        }
        const missing = join(scratchDir(), "absent.json");
        expect(() => readCompressionFidelityCorpus(missing)).toThrow(/unreadable/);
        expect(() => readCompressionFidelityCorpus(copyWith(""))).toThrow(/hashes to/);
    });

    it("exposes IDs, follow-ups, reviewed outputs, and review expectations only", () => {
        const corpus = readCompressionFidelityCorpus();
        expect(Object.keys(corpus)).toEqual(["cases"]);
        for (const fidelityCase of corpus.cases) {
            expect(Object.keys(fidelityCase).sort()).toEqual(["id", "scenarios", "sources"]);
            for (const source of fidelityCase.sources) {
                expect(Object.keys(source).sort()).toEqual(["id", "reviewedOutput"]);
            }
            for (const scenario of fidelityCase.scenarios) {
                expect(Object.keys(scenario).sort()).toEqual([
                    "abstention",
                    "expectations",
                    "followUp",
                    "forbidden",
                    "id",
                    "serving",
                    "source",
                ]);
                expect(Object.keys(scenario.followUp).sort()).toEqual(["id", "prompt"]);
                for (const key of Object.keys(scenario.serving)) {
                    expect(["path", "stage", "tier"]).toContain(key);
                }
                for (const expectation of scenario.expectations) {
                    expect(Object.keys(expectation).sort()).toEqual(["accepted", "obligation"]);
                }
            }
        }

        const raw = JSON.parse(committed.toString("utf8"));
        const hidden: string[] = [];
        for (const [index, fidelityCase] of raw.cases.entries()) {
            for (const [at, source] of fidelityCase.sources.entries()) {
                expect(corpus.cases[index]?.sources[at]?.reviewedOutput).toBe(
                    source.approved_example,
                );
                for (const message of [...source.messages, ...(source.successors ?? [])]) {
                    for (const part of message.parts) {
                        const text = part.text ?? part.state?.output ?? part.state?.error;
                        expect(text, part.id).toBeString();
                        hidden.push(text);
                    }
                }
            }
            for (const item of [
                ...fidelityCase.obligations,
                ...fidelityCase.allowed_losses,
                ...fidelityCase.forbidden_conclusions,
            ]) {
                hidden.push(item.statement);
                for (const span of item.evidence ?? []) hidden.push(span.text);
            }
            for (const scenario of fidelityCase.scenarios) hidden.push(scenario.description);
            for (const memory of fidelityCase.memory_examples ?? []) hidden.push(memory.text);
        }
        const exposed = JSON.stringify(corpus, (key, value) =>
            key === "reviewedOutput" ? undefined : value,
        );
        const leaks = (serialized: string) =>
            hidden.filter((text) => serialized.includes(JSON.stringify(text).slice(1, -1)));
        expect(hidden.length).toBeGreaterThan(100);
        expect(leaks(exposed)).toEqual([]);
        expect(leaks(JSON.stringify(raw))).toHaveLength(hidden.length);
    });

    it("rejects a pinned file whose exposed fields have the wrong shape", () => {
        const corpus = JSON.parse(committed.toString("utf8"));
        corpus.cases[0].scenarios[0].serving.path = "teleport";
        const bytes = `${JSON.stringify(corpus, null, 2)}\n`;
        const digest = new Bun.CryptoHasher("sha256").update(bytes).digest("hex");
        expect(() => readCompressionFidelityCorpus(copyWith(bytes), digest)).toThrow(
            /serving.path/,
        );
    });
});
