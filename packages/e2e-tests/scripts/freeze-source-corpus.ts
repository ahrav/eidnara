/**
 * Usage:
 *   bun packages/e2e-tests/scripts/freeze-source-corpus.ts --run <store-dir>
 *   bun packages/e2e-tests/scripts/freeze-source-corpus.ts --from <store-dir>
 *
 * `--run` drives the scripted OpenCode and Pi mixes against the mock provider and the direct-host
 * fixture, copies each harness's own store into `<store-dir>`, and freezes it. `--from` freezes the
 * stores a previous `--run` left in `<store-dir>`; the same stores yield the same fixture bytes.
 * Either mode writes `crates/daemon/testdata/corpus/sessions/<closure>/mix.json` and rewrites the
 * corpus digest pins in Rust and TypeScript. OpenCode must be the pinned 1.18.22 on `PATH`.
 */

import { Database } from "bun:sqlite";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";

import {
    freezeOpenCode,
    freezePi,
    OPENCODE_CLOSURE,
    PI_CLOSURE,
    type ScratchRoot,
} from "../src/corpus-sessions/freeze";
import { openCodeWorkspace, startOpenCode, startPi } from "../src/corpus-sessions/harnesses";
import { runOpenCodeMix } from "../src/corpus-sessions/opencode-mix";
import { runPiMix } from "../src/corpus-sessions/pi-mix";
import { corpusDigest, corpusFiles, SOURCE_CORPUS_DIR } from "../src/source-corpus/corpus";

const { copyFileSync, mkdirSync, readFileSync, realpathSync, writeFileSync } =
    process.getBuiltinModule("node:fs");

const REPO = resolve(import.meta.dir, "../../..");
const RUST_PIN = join(REPO, "crates/daemon/src/source_corpus.rs");
const TS_PIN = join(REPO, "packages/e2e-tests/src/source-corpus/corpus.ts");

interface StoreManifest {
    opencode: { sessionId: string; roots: ScratchRoot[] };
    pi: { roots: ScratchRoot[] };
}

async function runOpenCode(store: string): Promise<StoreManifest["opencode"]> {
    const h = await startOpenCode(true);
    try {
        const { root, sessionId } = await openCodeWorkspace(h);
        await runOpenCodeMix(h, sessionId, root);
        const db = new Database(join(h.env.dataDir, "opencode", "opencode.db"));
        db.prepare("VACUUM INTO ?").run(join(store, "opencode.db"));
        db.close();
        return {
            sessionId,
            roots: [
                { path: realpathSync(root), as: "/corpus/ripgrep" },
                { path: root, as: "/corpus/ripgrep" },
                { path: realpathSync(join(h.env.workdir, "..")), as: "/corpus/opencode" },
            ],
        };
    } finally {
        await h.dispose();
    }
}

async function runPi(store: string): Promise<StoreManifest["pi"]> {
    const pi = await startPi(true);
    try {
        await runPiMix(pi.harness);
        copyFileSync(pi.file, join(store, "pi-session.jsonl"));
        return {
            roots: [
                { path: pi.project, as: "/corpus/ripgrep" },
                { path: pi.root, as: "/corpus/pi" },
            ],
        };
    } finally {
        await pi.close();
    }
}

function writePins(digest: string): void {
    const rust = readFileSync(RUST_PIN, "utf8").replace(
        /(CORPUS_SHA256: &str =\s*")[0-9a-f]{64}"/,
        `$1${digest}"`,
    );
    writeFileSync(RUST_PIN, rust);
    const ts = readFileSync(TS_PIN, "utf8").replace(
        /(SOURCE_CORPUS_SHA256 =\s*")[0-9a-f]{64}"/,
        `$1${digest}"`,
    );
    writeFileSync(TS_PIN, ts);
}

function freeze(store: string, manifest: StoreManifest): void {
    const db = new Database(join(store, "opencode.db"), { readonly: true });
    const opencode = freezeOpenCode(db, manifest.opencode.sessionId, manifest.opencode.roots);
    db.close();
    const pi = freezePi(readFileSync(join(store, "pi-session.jsonl"), "utf8"), manifest.pi.roots);
    for (const [closure, text] of [
        [OPENCODE_CLOSURE, opencode],
        [PI_CLOSURE, pi],
    ] as const) {
        const dir = join(SOURCE_CORPUS_DIR, "sessions", closure);
        mkdirSync(dir, { recursive: true });
        writeFileSync(join(dir, "mix.json"), text);
    }
    const digest = corpusDigest(corpusFiles());
    writePins(digest);
    console.log(`source corpus digest ${digest}`);
}

const { values } = parseArgs({ options: { run: { type: "string" }, from: { type: "string" } } });
const store = values.run ?? values.from;
if (!store || (values.run && values.from)) {
    console.error("usage: freeze-source-corpus.ts (--run | --from) <store-dir>");
    process.exit(2);
}
mkdirSync(store, { recursive: true });
const manifestPath = join(store, "store.json");
if (values.run) {
    const manifest: StoreManifest = { opencode: await runOpenCode(store), pi: await runPi(store) };
    writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
}
let manifest: StoreManifest;
try {
    manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as StoreManifest;
} catch (error) {
    console.error(`${manifestPath}: ${String(error)}`);
    process.exit(2);
}
freeze(store, manifest);
process.exit(0);
