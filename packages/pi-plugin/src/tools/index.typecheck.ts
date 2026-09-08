import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { KernelClientResolver } from "@eidnara/opencode/shared/kernel-client";
import { registerEidnaraTools } from "./index";

/**
 * Compile-time contract for `RegisterToolsOptions`: `tsc --noEmit` covers this
 * file while it excludes `*.test.ts`, so the `@ts-expect-error` line fails the
 * typecheck if `RegisterToolsOptions` ever accepts a `db` key again.
 * Nothing invokes this function; it exists only to be type-checked.
 */
export function assertRegisterToolsOptionsRejectsDb(
    pi: ExtensionAPI,
    kernelClient: KernelClientResolver,
): void {
    registerEidnaraTools(pi, {
        kernelClient,
        rustToolBackends: {},
        // @ts-expect-error `db` is not a RegisterToolsOptions key.
        db: {},
    });
}
