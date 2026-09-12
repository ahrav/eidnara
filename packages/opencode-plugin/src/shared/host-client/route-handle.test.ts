import { describe, expect, test } from "bun:test";
import {
    assertBelongsToConnection,
    belongsToConnection,
    createRouteHandle,
    newConnectionToken,
    RouteHandle,
    StaleRouteHandleError,
} from "./route-handle";

describe("RouteHandle connection ownership", () => {
    test("a handle belongs only to the connection whose token created it", () => {
        const token = newConnectionToken();
        const other = newConnectionToken();
        const handle = createRouteHandle(7, 77, token);
        expect(handle.channel).toBe(7);
        expect(handle.epoch).toBe(77);
        expect(Object.isFrozen(handle)).toBe(true);
        expect(belongsToConnection(handle, token)).toBe(true);
        expect(() => assertBelongsToConnection(handle, token)).not.toThrow();

        expect(belongsToConnection(handle, other)).toBe(false);
        expect(belongsToConnection(handle, undefined as unknown as object)).toBe(false);

        // Callers match on the error's code and message, so the shape is part of the contract.
        let caught: unknown;
        try {
            assertBelongsToConnection(handle, other);
        } catch (error) {
            caught = error;
        }
        expect(caught).toBeInstanceOf(StaleRouteHandleError);
        const stale = caught as StaleRouteHandleError;
        expect(stale.name).toBe("StaleRouteHandleError");
        expect(stale.code).toBe("stale_route_handle");
        expect(stale.message).toBe("route handle (7, 77) is not live on the current connection");
        expect(stale.handle).toBe(handle);
    });

    test("a handle built with `new RouteHandle` never passes, even against a nullish token", () => {
        // A handle that no `createRouteHandle` call registered has no owner. The guard must
        // still fail closed when the caller's token is nullish through a type escape.
        const forged = new RouteHandle(7, 77);
        const nullishToken = undefined as unknown as object;
        expect(belongsToConnection(forged, newConnectionToken())).toBe(false);
        expect(belongsToConnection(forged, nullishToken)).toBe(false);
        expect(() => assertBelongsToConnection(forged, newConnectionToken())).toThrow(
            StaleRouteHandleError,
        );
        expect(() => assertBelongsToConnection(forged, nullishToken)).toThrow(
            StaleRouteHandleError,
        );
    });

    test("rejects out-of-range channel and epoch values", () => {
        expect(() => new RouteHandle(0, 1)).toThrow(RangeError);
        expect(() => new RouteHandle(0x1_0000, 1)).toThrow(RangeError);
        expect(() => new RouteHandle(1, 0)).toThrow(RangeError);
        expect(() => new RouteHandle(1, 0x1_0000_0000)).toThrow(RangeError);
        expect(() => new RouteHandle(1.5, 1)).toThrow(RangeError);
        expect(() => createRouteHandle(0, 1, newConnectionToken())).toThrow(RangeError);
        expect(new RouteHandle(0xffff, 0xffff_ffff)).toEqual(
            expect.objectContaining({ channel: 0xffff, epoch: 0xffff_ffff }),
        );
    });
});
