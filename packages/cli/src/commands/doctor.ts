/**
 * `runDoctor` dispatches to the per-harness doctor selected by `--harness` or auto-detection.
 */
import type { HarnessAdapter } from "../adapters/types";
import { resolveAdaptersForCommand } from "../lib/harness-select";
import { log } from "../lib/prompts";
import { runDoctor as runOmpDoctor } from "./doctor-omp";
import { runDoctor as runOpenCodeDoctor } from "./doctor-opencode";
import { runDoctor as runPiDoctor } from "./doctor-pi";

export interface RunDoctorOptions {
    force?: boolean;
    issue?: boolean;
    argv?: string[];
}

export async function runDoctor(options: RunDoctorOptions): Promise<number> {
    const argv = options.argv ?? [];
    const adapters = await resolveAdaptersForCommand(argv, {
        allowMulti: true,
        verb: "diagnose",
    });

    if (adapters.length === 0) {
        log.warn("No harness selected.");
        return 0;
    }

    let anyFailure = false;
    for (const adapter of adapters) {
        log.step(`Running doctor for ${adapter.displayName}…`);
        const code = await dispatchDoctor(adapter, options);
        if (code !== 0) anyFailure = true;
    }
    return anyFailure ? 1 : 0;
}

async function dispatchDoctor(adapter: HarnessAdapter, options: RunDoctorOptions): Promise<number> {
    switch (adapter.kind) {
        case "opencode": {
            return runOpenCodeDoctor({
                force: options.force,
                issue: options.issue,
            });
        }
        case "pi":
            return runPiDoctor({
                force: options.force,
                issue: options.issue,
            });
        case "omp":
            return runOmpDoctor({
                force: options.force,
                issue: options.issue,
            });
    }
}
