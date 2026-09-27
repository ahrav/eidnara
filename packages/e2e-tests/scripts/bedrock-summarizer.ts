#!/usr/bin/env bun
/**
 * The summarizer the direct-host fixture runs when `EIDNARA_FIXTURE_SUMMARIZER_COMMAND` names this
 * script: it reads `{"system", "prompt", "max_output_tokens"}` on stdin, asks the Bedrock model
 * `EIDNARA_STALE_SUMMARIZER_MODEL` through the AWS CLI's Converse call, and prints the answer.
 * Credentials come from the caller's AWS environment; nothing here stores them.
 */

import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const model = process.env.EIDNARA_STALE_SUMMARIZER_MODEL;
if (!model) throw new Error("EIDNARA_STALE_SUMMARIZER_MODEL is required");
const input = JSON.parse(await Bun.stdin.text()) as {
    system: string | null;
    prompt: string;
    max_output_tokens: number;
};
const dir = mkdtempSync(join(tmpdir(), "eidnara-summarizer-"));
try {
    const body = join(dir, "request.json");
    writeFileSync(
        body,
        JSON.stringify({
            modelId: model,
            system: input.system ? [{ text: input.system }] : [],
            messages: [{ role: "user", content: [{ text: input.prompt }] }],
            inferenceConfig: { maxTokens: input.max_output_tokens },
        }),
    );
    // A hung Converse call must not hold the fixture's firing open past the
    // daemon's completion wait: three bounded attempts end well inside it, and
    // a failed script exits non-zero, which the fixture reports as a typed
    // terminal error.
    for (let attempt = 1; ; attempt += 1) {
        const run = spawnSync(
            "aws",
            ["bedrock-runtime", "converse", "--cli-input-json", `file://${body}`],
            { encoding: "utf8", maxBuffer: 64 << 20, timeout: 120_000 },
        );
        if (run.status === 0) {
            const response = JSON.parse(run.stdout) as {
                output: { message: { content: Array<{ text?: string }> } };
            };
            process.stdout.write(response.output.message.content.map((c) => c.text ?? "").join(""));
            break;
        }
        if (attempt === 3) {
            throw new Error(
                `converse failed: ${run.error?.message ?? ""} ${(run.stderr ?? "").slice(-2000)}`,
            );
        }
        await Bun.sleep(10_000 * attempt);
    }
} finally {
    rmSync(dir, { recursive: true, force: true });
}
