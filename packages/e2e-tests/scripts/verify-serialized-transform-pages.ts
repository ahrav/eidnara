import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "../../..");
const testName = "serialized_transform_corpus_preserves_host_admission_and_completion";
if (process.platform !== "linux") throw new Error("the direct host proof requires Linux");
const generated = spawnSync(process.execPath, ["scripts/serialized-transform-pages.ts"], {
    cwd: join(root, "packages/e2e-tests"),
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
});
if (generated.status !== 0)
    throw new Error(`corpus generation failed: ${generated.error ?? generated.stderr}`);
const scratch = mkdtempSync(join(tmpdir(), "serialized-transform-proof-"));
try {
    const corpusPath = join(scratch, "corpus.json");
    writeFileSync(corpusPath, generated.stdout);
    const test = spawnSync(
        "cargo",
        [
            "test",
            "--locked",
            "-p",
            "daemon",
            "--test",
            "serialized_transform_pages",
            "--features",
            "direct-host-fixture",
            "--",
            "--ignored",
            "--exact",
            testName,
            "--nocapture",
        ],
        {
            cwd: root,
            env: { ...process.env, EIDNARA_SERIALIZED_TRANSFORM_CORPUS: corpusPath },
            encoding: "utf8",
            maxBuffer: 4 * 1024 * 1024,
        },
    );
    process.stdout.write(test.stdout ?? "");
    process.stderr.write(test.stderr ?? "");
    if (
        test.status !== 0 ||
        !test.stdout?.includes(`test ${testName} ... ok`) ||
        !/test result: ok\. 1 passed; 0 failed; 0 ignored;/.test(test.stdout)
    ) {
        throw new Error(
            `host corpus did not pass its registered test: ${test.error ?? test.status}`,
        );
    }
} finally {
    rmSync(scratch, { recursive: true, force: true });
}
