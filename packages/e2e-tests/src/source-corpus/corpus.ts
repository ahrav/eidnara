import { createHash } from "node:crypto";
import { join, relative, resolve, sep } from "node:path";

const { readdirSync, readFileSync } = process.getBuiltinModule("node:fs");

export const SOURCE_CORPUS_SHA256 =
    "940e47f1a69126fee50848c75a0fdfd5b77328239c53c554f6991abd66b61be6";

export const SOURCE_CORPUS_DIR = resolve(
    import.meta.dir,
    "../../../../crates/daemon/testdata/corpus",
);

export interface CorpusFile {
    readonly path: string;
    readonly bytes: Uint8Array;
}

function comparePaths(a: CorpusFile, b: CorpusFile): number {
    return Buffer.compare(Buffer.from(a.path), Buffer.from(b.path));
}

export function corpusFiles(dir: string = SOURCE_CORPUS_DIR): CorpusFile[] {
    const files: CorpusFile[] = [];
    const pending = [dir];
    while (pending.length > 0) {
        const current = pending.pop() as string;
        for (const entry of readdirSync(current, { withFileTypes: true })) {
            const full = join(current, entry.name);
            if (entry.isSymbolicLink()) {
                throw new Error(`${full} is a symlink; corpus fixtures are regular files`);
            }
            if (entry.isDirectory()) {
                pending.push(full);
                continue;
            }
            const path = relative(dir, full).split(sep).join("/");
            if (path !== "README.md") files.push({ path, bytes: readFileSync(full) });
        }
    }
    return files.sort(comparePaths);
}

export function corpusDigest(files: readonly CorpusFile[]): string {
    const hash = createHash("sha256");
    for (const file of [...files].sort(comparePaths)) {
        hash.update(file.path);
        hash.update("\n");
        const bytes = Buffer.from(file.bytes);
        let start = 0;
        for (let at = bytes.indexOf("\r\n", start); at >= 0; at = bytes.indexOf("\r\n", start)) {
            hash.update(bytes.subarray(start, at));
            hash.update("\n");
            start = at + 2;
        }
        hash.update(bytes.subarray(start));
    }
    return hash.digest("hex");
}

export function verifyCorpusDigest(files: readonly CorpusFile[]): void {
    const actual = corpusDigest(files);
    if (actual !== SOURCE_CORPUS_SHA256) {
        throw new Error(`source corpus digest ${actual}, pinned ${SOURCE_CORPUS_SHA256}`);
    }
}
