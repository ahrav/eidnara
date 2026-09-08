/** Upper bound on one provider request; a stalled response fails the measurement instead of hanging it. */
export const PROVIDER_REQUEST_TIMEOUT_MS = 120_000;

export function providerRequestSignal(): AbortSignal {
    return AbortSignal.timeout(PROVIDER_REQUEST_TIMEOUT_MS);
}
