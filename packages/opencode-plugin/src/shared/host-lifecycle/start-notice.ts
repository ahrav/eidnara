import {
    type AwsProfileSource,
    AwsSourceError,
    type AwsSourceSelection,
    selectAwsSource,
} from "../host-client/aws-source";
import { readRegularFileSync } from "../regular-file";
import { isDaemonReason, remediationForReason } from "./contract";

/**
 * A start that ran out of its budget and a compatibility probe that missed the policy budget after a successful start are both retried by the next demand.
 */
const RETRIED_TIMEOUT_REASONS: ReadonlySet<string> = new Set([
    "startup_timeout",
    "native_probe_unavailable",
]);

const AWS_CONFIG_MAX_BYTES = 256 * 1024;
const AWS_MAX_PROFILES = 5;

export function isPersistentStartRefusal(reason: string): boolean {
    if (RETRIED_TIMEOUT_REASONS.has(reason)) return false;
    return !(isDaemonReason(reason) && remediationForReason(reason) === "wait_and_retry");
}

const trimBlank = (text: string): string => text.replace(/^[ \t]+|[ \t]+$/g, "");

/** A section header key as `section_header` in `crates/host-runtime/src/model_execution/aws_profile.rs` reads it. */
function sectionKey(line: string): { prefix: string | null; name: string } | undefined {
    if (!line.startsWith("[")) return undefined;
    const comment = line.search(/[#;]/);
    const text = trimBlank(comment === -1 ? line : line.slice(0, comment));
    if (!text.endsWith("]")) return undefined;
    const raw = trimBlank(text.slice(1, -1));
    const split = raw.search(/[ \t]/);
    return split === -1
        ? { prefix: null, name: raw.trim() }
        : { prefix: raw.slice(0, split).trim(), name: raw.slice(split + 1).trim() };
}

type Profiles = Map<string, Map<string, string>>;

function profileName(
    key: { prefix: string | null; name: string },
    config: boolean,
): string | undefined {
    if (key.prefix === null) return config && key.name !== "default" ? undefined : key.name;
    return config && key.prefix === "profile" ? key.name : undefined;
}

function parseProfiles(text: string, config: boolean, into: Profiles): void {
    let section: Map<string, string> | undefined;
    for (const line of text.split(/\r?\n/)) {
        const key = sectionKey(line);
        if (key) {
            const name = profileName(key, config);
            if (name === undefined) {
                section = undefined;
                continue;
            }
            section = into.get(name) ?? new Map();
            into.set(name, section);
            continue;
        }
        const eq = line.indexOf("=");
        if (!section || eq === -1 || /^[ \t#;]/.test(line)) continue;
        const value = line.slice(eq + 1).replace(/[ \t][#;].*$/, "");
        section.set(trimBlank(line.slice(0, eq)), trimBlank(value));
    }
}

function readProfileFile(path: string): string {
    try {
        return readRegularFileSync(path, AWS_CONFIG_MAX_BYTES);
    } catch {
        return "";
    }
}

function credentialProcessIn(source: AwsProfileSource): string | undefined {
    const profiles: Profiles = new Map();
    parseProfiles(readProfileFile(source.config_file), true, profiles);
    parseProfiles(readProfileFile(source.credentials_file), false, profiles);
    const visited = new Set<string>();
    let name = source.profile;
    while (!visited.has(name) && visited.size < AWS_MAX_PROFILES) {
        visited.add(name);
        const profile = profiles.get(name);
        if (!profile) return undefined;
        if (profile.has("credential_process")) return name;
        const next = profile.get("source_profile");
        if (next === undefined || !profile.has("role_arn")) return undefined;
        name = next;
    }
    return undefined;
}

export function credentialProcessProfile(
    env: Readonly<Record<string, string | undefined>>,
): string | undefined {
    let selection: AwsSourceSelection;
    try {
        selection = selectAwsSource(env);
    } catch {
        return undefined;
    }
    return selection.mode === "profile" ? credentialProcessIn(selection.source) : undefined;
}

function harnessHint(env: Readonly<Record<string, string | undefined>>): string {
    let selection: AwsSourceSelection;
    try {
        selection = selectAwsSource(env);
    } catch (error) {
        return error instanceof AwsSourceError
            ? `The AWS source selection is not admissible (${error.field}: ${error.code}); AWS_PROFILE needs AWS_REGION and an absolute HOME.`
            : "";
    }
    if (selection.mode !== "profile") return "";
    const found = credentialProcessIn(selection.source);
    if (found === undefined) return "";
    const subject =
        found === selection.source.profile
            ? `AWS profile "${found}"`
            : `AWS profile "${found}", the source profile of "${selection.source.profile}",`;
    return `${subject} supplies credentials through credential_process. The daemon accepts SSO profiles, role profiles, including a role whose source profile holds static keys, or the AWS_* environment credentials when AWS_PROFILE is unset.`;
}

export function managedStartNotice(
    reason: string,
    remediation: string | null,
    env: Readonly<Record<string, string | undefined>>,
): string {
    const lead = `Eidnara is off in this process: its daemon did not start (${reason}).`;
    const suggested = remediation ? `Suggested fix: ${remediation.replaceAll("_", " ")}.` : "";
    const hint =
        reason === "harness_unavailable"
            ? harnessHint(env) || suggested
            : reason === "unsupported_install_layout"
              ? "The daemon payload resolves from an npm install of the Eidnara package; install it with npm or `eidnara setup`."
              : suggested;
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
