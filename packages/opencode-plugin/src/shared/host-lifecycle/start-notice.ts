import { readFileSync } from "node:fs";
import { selectAwsSource } from "../host-client/aws-source";

const TRANSIENT_REASONS: ReadonlySet<string> = new Set([
    "starting",
    "stopping",
    "lifecycle_busy",
    "storage_starting",
    "kernel_starting",
    "local_embeddings_starting",
    "startup_timeout",
]);

const AWS_CONFIG_MAX_BYTES = 256 * 1024;

export function isPersistentStartRefusal(reason: string): boolean {
    return !TRANSIENT_REASONS.has(reason);
}

export function credentialProcessProfile(
    env: Readonly<Record<string, string | undefined>>,
): string | undefined {
    let source: ReturnType<typeof selectAwsSource>;
    let text: string;
    try {
        source = selectAwsSource(env);
        if (source.mode !== "profile") return undefined;
        const bytes = readFileSync(source.source.config_file);
        if (bytes.length > AWS_CONFIG_MAX_BYTES) return undefined;
        text = bytes.toString("utf8");
    } catch {
        return undefined;
    }
    const profile = source.source.profile;
    const headers = new Set(
        profile === "default" ? ["default", "profile default"] : [`profile ${profile}`],
    );
    let inSection = false;
    for (const raw of text.split(/\r?\n/)) {
        const line = raw.trim();
        const header = /^\[\s*(.+?)\s*\]$/.exec(line);
        if (header) {
            inSection = headers.has((header[1] as string).replace(/\s+/g, " "));
            continue;
        }
        if (inSection && /^credential_process\s*=/.test(line)) return profile;
    }
    return undefined;
}

export function managedStartNotice(
    reason: string,
    remediation: string | null,
    env: Readonly<Record<string, string | undefined>>,
): string {
    const lead = `Eidnara is off in this process: its daemon did not start (${reason}).`;
    const profile = reason === "harness_unavailable" ? credentialProcessProfile(env) : undefined;
    const hint = profile
        ? `AWS profile "${profile}" supplies credentials through credential_process. The daemon accepts SSO, role, and static-key profiles, or the AWS_* environment credentials when AWS_PROFILE is unset.`
        : reason === "unsupported_install_layout"
          ? "The daemon payload resolves from an npm install of the Eidnara package; install it with npm or `eidnara setup`."
          : remediation
            ? `Suggested fix: ${remediation.replaceAll("_", " ")}.`
            : "";
    return [lead, hint, "Run `eidnara doctor` for details."].filter(Boolean).join(" ");
}

/**
 * Each announcer delivers each persistent refusal reason once. Refusals received before listener
 * registration are queued for the first listener.
 */
export function createStartRefusalAnnouncer(
    env: () => Readonly<Record<string, string | undefined>>,
) {
    const announced = new Set<string>();
    const pending: string[] = [];
    let deliver: ((notice: string) => void) | undefined;
    return {
        refused(reason: string, remediation: string | null): void {
            if (!isPersistentStartRefusal(reason) || announced.has(reason)) return;
            announced.add(reason);
            const notice = managedStartNotice(reason, remediation, env());
            if (deliver) deliver(notice);
            else pending.push(notice);
        },
        listen(listener: (notice: string) => void): void {
            deliver = listener;
            for (const notice of pending.splice(0)) listener(notice);
        },
    };
}
