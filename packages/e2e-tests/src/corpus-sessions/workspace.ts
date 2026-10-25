import { join } from "node:path";

import { materialize, readManifest } from "../source-corpus/materialize.ts";

const { writeFileSync } = process.getBuiltinModule("node:fs");

/** The recognized fake credential shape the cassette tests use; it marks the canary unit. */
export const CANARY = "sk-ant-api03-Zk9Qw2Lm7Pv4Rt8Zw1Yc6Nb3Hd5Kf0JgAbCdEfGh";

/** A 1x1 PNG, so a read of it returns an image. */
const PIXEL_PNG =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

/** Bytes an MCP or extension probe returns for the over-cap result kind: one byte past 64 KiB plus margin. */
export const OVERSIZE_BYTES = 70_000;

/**
 * Materializes the ripgrep manifest under `parent` and adds the working files the scripted mix
 * reads, edits, and captures. The added files stay uncommitted, so the root commit equals the
 * manifest's pinned root.
 */
export function prepareWorkspace(parent: string): string {
    const root = materialize(readManifest("ripgrep"), parent);
    writeFileSync(join(root, "canary.txt"), `token ${CANARY}\n`);
    writeFileSync(join(root, "data.json"), '{"status":"ok","items":[1,2,3]}\n');
    writeFileSync(join(root, "notes.txt"), "alpha line\n");
    writeFileSync(join(root, "pixel.png"), Buffer.from(PIXEL_PNG, "base64"));
    writeFileSync(join(root, "ARCHITECTURE.md"), "# Architecture\n\nOne crate per concern.\n");
    return root;
}
