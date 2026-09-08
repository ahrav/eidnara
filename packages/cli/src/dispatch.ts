import { createRequire } from "node:module";
import { isPromptCancelledError } from "./lib/prompts";

export interface CliDispatchDependencies {
    runDaemon: (args: string[]) => Promise<number>;
    stdout: (line: string) => void;
    stderr: (line: string) => void;
}

const defaultDependencies: CliDispatchDependencies = {
    runDaemon: async (args) => {
        const { runDaemonCommand } = await import("./commands/daemon");
        return runDaemonCommand(args);
    },
    stdout: (line) => console.log(line),
    stderr: (line) => console.error(line),
};

function getVersion(): string {
    const req = createRequire(import.meta.url);
    for (const relPath of ["../../package.json", "../package.json"]) {
        try {
            const pkg = req(relPath) as { version?: unknown };
            if (typeof pkg.version === "string" && pkg.version.length > 0) {
                return pkg.version;
            }
        } catch {}
    }
    return "0.0.0";
}

export function usageText(): string {
    return [
        "",
        "  Eidnara CLI",
        "  -----------",
        "",
        "  Commands:",
        "    setup            Interactive setup wizard",
        "    doctor           Check and fix configuration issues",
        "    daemon start     Start the managed eidnara-host",
        "    daemon stop      Stop the managed eidnara-host",
        "    daemon restart   Restart the managed eidnara-host as one transaction",
        "    daemon status    Show lifecycle and readiness state without mutation",
        "    daemon doctor    Run read-only lifecycle diagnostics",
        "",
        "  Daemon output:",
        "    --json            Emit one eidnara.daemon/v1 JSON object",
        "",
        "  Doctor options:",
        "    doctor --force   Repair configuration conflicts",
        "    doctor --issue   Collect diagnostics and open a GitHub issue",
        "",
        "  Harness selection:",
        "    --harness opencode    Target OpenCode only",
        "    --harness pi          Target Pi only",
        "    --harness omp         Target Oh My Pi (OMP) only",
        "    (default: auto-detect, prompt if multiple installed)",
        "",
        "  Usage:",
        "    eidnara setup",
        "        # add --dry-run to preview the wizard without writing any files",
        "    eidnara doctor",
        "    eidnara doctor --issue",
        "    eidnara daemon status --json",
        "",
    ].join("\n");
}

const HELP_FLAGS: ReadonlySet<string> = new Set(["--help", "-h"]);
const SETUP_FLAGS: ReadonlySet<string> = new Set(["--dry-run", ...HELP_FLAGS]);
const DOCTOR_FLAGS: ReadonlySet<string> = new Set(["--force", "--issue", ...HELP_FLAGS]);

/** Tokens in `argv` that `allowed` does not name. `--harness` consumes the next token, so that token is never reported. */
function unknownArguments(argv: string[], allowed: ReadonlySet<string>): string[] {
    const unknown: string[] = [];
    for (let i = 0; i < argv.length; i++) {
        const token = argv[i];
        if (token === "--harness") {
            i++;
            continue;
        }
        if (!allowed.has(token)) unknown.push(token);
    }
    return unknown;
}

export async function dispatchCli(
    argv: string[] = process.argv.slice(2),
    dependencies: CliDispatchDependencies = defaultDependencies,
): Promise<number> {
    if (argv.length === 0 || argv[0] === "--help" || argv[0] === "-h" || argv[0] === "help") {
        dependencies.stdout(usageText());
        return 0;
    }

    if (argv[0] === "--version" || argv[0] === "-v") {
        dependencies.stdout(getVersion());
        return 0;
    }

    const command = argv[0];
    const rest = argv.slice(1);

    if (command === "setup" || command === "doctor") {
        // A removed action such as `doctor repair-db` must not run an ordinary check and exit 0.
        const unknown = unknownArguments(rest, command === "setup" ? SETUP_FLAGS : DOCTOR_FLAGS);
        if (unknown.length > 0) {
            const noun = unknown.length === 1 ? "argument" : "arguments";
            dependencies.stderr(`Unknown ${command} ${noun}: ${unknown.join(" ")}`);
            dependencies.stdout(usageText());
            return 1;
        }
        if (rest.some((token) => HELP_FLAGS.has(token))) {
            dependencies.stdout(usageText());
            return 0;
        }
    }

    try {
        if (command === "daemon") {
            return await dependencies.runDaemon(rest);
        }

        if (command === "setup") {
            const { runSetup } = await import("./commands/setup");
            return await runSetup(rest);
        }

        if (command === "doctor") {
            const { runDoctor } = await import("./commands/doctor");
            return await runDoctor({
                force: rest.includes("--force"),
                issue: rest.includes("--issue"),
                argv: rest,
            });
        }
    } catch (error) {
        if (isPromptCancelledError(error)) return 0;
        throw error;
    }

    dependencies.stderr(`Unknown command: ${command}`);
    dependencies.stdout(usageText());
    return 1;
}
