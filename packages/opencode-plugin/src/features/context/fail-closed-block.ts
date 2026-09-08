/** The command a fail-closed notice tells the operator to run. */
export const FAIL_CLOSED_DOCTOR_COMMAND = "eidnara doctor";

export type HookInitFailure = { type: "no_project" };

let lastHookInitFailure: HookInitFailure | null = null;

export function recordHookInitFailure(failure: HookInitFailure): void {
    lastHookInitFailure = failure;
}

export function clearHookInitFailure(): void {
    lastHookInitFailure = null;
}

export function getLastHookInitFailure(): HookInitFailure | null {
    return lastHookInitFailure;
}
