/**
 *
 */
import type { HarnessAdapter } from "../adapters/types";
import { resolveAdaptersForCommand } from "../lib/harness-select";
import { intro, isPromptCancelledError, log, note, outro } from "../lib/prompts";
import { runSetup as runOmpSetup } from "./setup-omp";
import { runSetup as runOpenCodeSetup } from "./setup-opencode";
import { runSetup as runPiSetup } from "./setup-pi";

export async function runSetup(argv: string[]): Promise<number> {
    const dryRun = argv.includes("--dry-run");
    intro(dryRun ? "Eidnara setup (dry run)" : "Eidnara setup");

    // A misspelled safety flag such as `--dryrun` must not fall through to real writes.
    const unknown = unknownSetupArguments(argv);
    if (unknown.length > 0) {
        log.error(`Unknown option${unknown.length > 1 ? "s" : ""}: ${unknown.join(", ")}`);
        log.message("Supported options: --dry-run, --harness <opencode|pi|omp>");
        outro("Setup stopped — correct the command arguments and try again.");
        return 1;
    }

    let adapters: HarnessAdapter[];
    try {
        adapters = await resolveAdaptersForCommand(argv, {
            allowMulti: false,
            verb: "setup",
        });
    } catch (error) {
        if (isPromptCancelledError(error)) throw error;
        log.error(error instanceof Error ? error.message : String(error));
        outro("Setup stopped — correct the command arguments and try again.");
        return 1;
    }

    if (adapters.length === 0) {
        outro("No harness selected. Nothing to do.");
        return 0;
    }

    let anyFailure = false;
    for (const adapter of adapters) {
        log.step(`Configuring ${adapter.displayName} (${adapter.pluginPackageName})…`);

        const code = await dispatchSetup(adapter, dryRun);
        if (code !== 0) {
            anyFailure = true;
            continue;
        }
        if (!dryRun) printNextSteps(adapter);
    }

    if (anyFailure) {
        outro("Setup finished with warnings — see above.");
        return 1;
    }
    outro(dryRun ? "Dry run done — no changes were made." : "Done.");
    return 0;
}

/** A `--`-prefixed harness value stays unconsumed so the harness parser reports its missing-value error. */
export function unknownSetupArguments(argv: readonly string[]): string[] {
    const unknown: string[] = [];
    for (let index = 0; index < argv.length; index++) {
        const argument = argv[index];
        if (argument === "--dry-run") continue;
        if (argument === "--harness") {
            const value = argv[index + 1];
            if (value !== undefined && !value.startsWith("--")) index++;
            continue;
        }
        unknown.push(argument);
    }
    return unknown;
}

async function dispatchSetup(adapter: HarnessAdapter, dryRun: boolean): Promise<number> {
    switch (adapter.kind) {
        case "opencode":
            return runOpenCodeSetup(dryRun);
        case "pi":
            return runPiSetup({ dryRun });
        case "omp":
            return runOmpSetup({ dryRun });
    }
}

function printNextSteps(adapter: HarnessAdapter): void {
    if (adapter.kind === "opencode") {
        note(
            [
                "Restart OpenCode (or reload your session) so the plugin loads.",
                "Verify with: eidnara doctor",
            ].join("\n"),
            "Next steps",
        );
        return;
    }
    if (adapter.kind === "pi") {
        note(
            [
                "Restart your Pi session so the extension registers.",
                "Verify with: eidnara doctor --harness pi",
            ].join("\n"),
            "Next steps",
        );
    }
    if (adapter.kind === "omp") {
        note(
            [
                "Restart OMP (or run /reload-plugins) so Eidnara registers.",
                "Verify with: eidnara doctor --harness omp",
            ].join("\n"),
            "Next steps",
        );
    }
}
