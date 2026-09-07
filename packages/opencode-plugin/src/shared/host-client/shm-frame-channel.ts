import {
    isRingFullError,
    NativeChannel,
    type NativeProducerReservation,
    type NativeReceiveLease,
    type NativeSetupOptions,
    type ProducerCursor,
} from "@eidnara/shm-native";
import { armExpiryTimer, type Deadline } from "./deadline";
import { HostCallError } from "./errors";
import {
    BoundedFrameProducer,
    type ByteBudget,
    CopyCounter,
    type DirectFrameBody,
    type FrameChannelCloseReason,
    type FrameChannelHandlers,
    type FrameChannelStats,
    type FrameSendHooks,
    type FrameSendTicket,
    headerViolation,
    type OutboundFrame,
    type ProducerFrameHeader,
    ReceiveLease,
    type SetupFrameChannel,
} from "./frame-channel";
import {
    decodeHeader,
    type EnvelopeHeader,
    encodeHeader,
    HEADER_LEN,
    PROTOCOL_VERSION,
} from "./protocol";

class InboundFrameError extends Error {
    constructor(
        readonly reason: "protocol_violation" | "role_violation",
        message: string,
    ) {
        super(message);
    }
}

export interface ShmFrameChannelOptions {
    /** Injected only by unit tests; production attaches through `setup`. */
    nativeChannel?: NativeChannel;
    setup?: NativeSetupOptions;
    budget: ByteBudget;
    maxBodyLen: number;
    handlers: FrameChannelHandlers;
}

/** Frames delivered per readiness turn before the drain yields. */
const DRAIN_BATCH_FRAMES = 64;

/**
 * Microtasks run ahead of timers and I/O, so a peer that keeps the ring
 * non-empty would hold the event loop for as long as it publishes if every
 * batch re-armed as a microtask. The addon's readiness dispatch applies the
 * same budget.
 */
const DRAIN_MICROTASK_BUDGET = 16;

/** A full ring is backpressure, so callers may retry rather than fail the route. */
function ringFullError(cause: unknown): HostCallError {
    return new HostCallError(
        "not_sent",
        "shared-memory ring has no capacity for this frame",
        "ring_full",
        cause,
    );
}

/** The body was filled but not committed, so the frame provably never reached the ring. */
function deadlineExpiredError(cause?: unknown): HostCallError {
    return new HostCallError(
        "not_sent",
        "request deadline expired before publication",
        "deadline_expired",
        cause,
    );
}

export class ShmFrameChannel implements SetupFrameChannel {
    private native: NativeChannel | null;
    /** In-flight attachment shared by concurrent `start` callers. */
    private starting: Promise<void> | null = null;
    private readonly copies = new CopyCounter();
    private readinessStarted = false;
    private drainScheduled = false;
    private consecutiveMicrotaskDrains = 0;
    private closed = false;
    private readonly receiveLeases = new Set<ReceiveLease>();
    /** Producers whose budget charge is still held. */
    private readonly producers = new Set<BoundedFrameProducer>();
    private quarantinedBytes = 0;
    private heldBytes = 0;

    constructor(private readonly options: ShmFrameChannelOptions) {
        if (!options.nativeChannel && !options.setup) {
            throw new Error("shared-memory channel requires an attachment");
        }
        this.native = options.nativeChannel ?? null;
    }

    async start(deadline: Deadline): Promise<void> {
        if (this.closed) {
            throw new HostCallError("not_sent", "shared-memory channel closed");
        }
        if (this.native) return;
        // A second `start` while the first attachment is in flight joins it;
        // a separate `connectSetup` would leave one native channel unowned.
        // Each caller still waits no longer than its own deadline.
        this.starting ??= this.attach(deadline).finally(() => {
            this.starting = null;
        });
        await ShmFrameChannel.within(
            this.starting,
            deadline,
            "shared-memory setup deadline expired",
        );
    }

    /** Rejects once `deadline` passes; `work` keeps running for its other awaiters. */
    private static async within<T>(
        work: Promise<T>,
        deadline: Deadline,
        message: string,
    ): Promise<T> {
        let cancel = (): void => {};
        const expiry = new Promise<never>((_, reject) => {
            cancel = armExpiryTimer(deadline, () =>
                reject(new HostCallError("not_sent", message, "deadline_expired")),
            );
        });
        try {
            return await Promise.race([work, expiry]);
        } finally {
            cancel();
        }
    }

    private async attach(deadline: Deadline): Promise<void> {
        const setup = this.options.setup;
        if (!setup) throw new Error("shared-memory setup is missing");
        if (deadline.remainingMs() <= 0) {
            throw new HostCallError(
                "not_sent",
                "shared-memory setup deadline expired",
                "deadline_expired",
            );
        }
        const native = await NativeChannel.connectSetup({
            ...setup,
            timeoutMs: Math.max(1, Math.ceil(deadline.remainingMs())),
        });
        if (this.closed) {
            native.close();
            throw new HostCallError("not_sent", "shared-memory channel closed");
        }
        this.native = native;
    }

    beginFrames(): void {
        if (this.readinessStarted) return;
        this.attached().startReadiness(
            () => this.drainReady(),
            (error) => {
                // The addon unregisters a readiness handler that threw and
                // never wakes it again, so the channel must not stay open.
                if (this.closed) return;
                this.failClose("protocol_violation", error);
            },
        );
        // Set after registration so a failed `startReadiness` stays retryable.
        this.readinessStarted = true;
    }

    produce(
        header: ProducerFrameHeader,
        body: DirectFrameBody,
        hooks?: FrameSendHooks,
        deadline?: Deadline,
    ): FrameSendTicket {
        if (this.closed) throw new HostCallError("not_sent", "shared-memory channel closed");
        this.assertBodyBounds(body.byteLength);
        // The native ring's fixed capacity is not the configured aggregate
        // cap: admission consults the shared budget so an over-cap body is
        // refused with `memory_cap`. The
        // charge covers the synchronous publication window and is returned
        // once the ring owns the bytes.
        const reservedBytes = HEADER_LEN + body.byteLength;
        this.admitPublication(reservedBytes);
        try {
            return this.publishFrame(header, body, hooks, deadline);
        } finally {
            this.releasePublication(reservedBytes);
        }
    }

    reserve(
        header: ProducerFrameHeader,
        capacity: number,
        hooks?: FrameSendHooks,
    ): BoundedFrameProducer {
        if (this.closed) throw new HostCallError("not_sent", "shared-memory channel closed");
        this.assertBodyBounds(capacity);
        // Reservations retain their capacity charge until publication or abort.
        // The capacity probe does not block the event loop; a full ring returns
        // retryable backpressure.
        const reservedBytes = HEADER_LEN + capacity;
        this.admitPublication(reservedBytes);
        let reservation: NativeProducerReservation;
        try {
            reservation = this.attached().reserve(capacity, 0);
        } catch (error) {
            this.releasePublication(reservedBytes);
            if (isRingFullError(error)) throw ringFullError(error);
            throw error;
        }
        let held = true;
        let charged = true;
        let abortError: unknown;
        let producer: BoundedFrameProducer | undefined;
        const releaseCharge = (): void => {
            if (!charged) return;
            charged = false;
            if (producer) this.producers.delete(producer);
            this.releasePublication(reservedBytes);
        };
        producer = new BoundedFrameProducer(
            reservation.segments,
            capacity,
            (_segments, exactLength) => ({
                publish: () => {
                    if (!held) throw new HostCallError("not_sent", "reservation released");
                    let published = false;
                    reservation.commit(
                        encodeHeader({ ...header, len: exactLength }),
                        exactLength,
                        () => {
                            published = true;
                            try {
                                hooks?.onPublish?.();
                            } catch {
                                // Send hooks cannot change publication.
                            }
                        },
                    );
                    held = false;
                    releaseCharge();
                    try {
                        hooks?.onComplete?.();
                    } catch {
                        // Send hooks cannot change completion.
                    }
                    return { cancel: () => !published };
                },
            }),
            (outcome) => {
                releaseCharge();
                if (outcome !== "quarantined") return;
                // The wrapper keeps an unconsumed token active, but this
                // producer is already inactive, so nothing can retry the
                // abort and the slot would stay reserved until close.
                const error =
                    abortError ??
                    new Error("producer alias revocation failed; storage quarantined");
                try {
                    this.retire(error);
                } catch {
                    // Reported through `onClosed("quarantined")`.
                }
                throw error;
            },
            () => {
                // The native abort detaches every producer view, so it is the alias revocation.
                if (!held) return "released";
                held = false;
                try {
                    reservation.abort();
                } catch (error) {
                    abortError = error;
                    throw error;
                }
                return "released";
            },
        );
        this.producers.add(producer);
        return producer;
    }

    send(frame: OutboundFrame, hooks?: FrameSendHooks): FrameSendTicket {
        return this.produce(
            frame.header,
            {
                byteLength: frame.body.byteLength,
                fill: (cursor) => {
                    this.copies.record();
                    cursor.write(frame.body);
                },
            },
            hooks,
        );
    }

    sendControl(header: EnvelopeHeader): void {
        if (this.closed) return;
        // Control frames cannot wait in a queue; a full ring or an exhausted
        // byte budget exhausts control capacity and closes the channel.
        try {
            this.produce(header, { byteLength: 0, fill: () => {} });
        } catch (error) {
            if (
                error instanceof HostCallError &&
                (error.code === "ring_full" || error.code === "memory_cap")
            ) {
                this.failClose("control_exhausted", error);
                return;
            }
            throw error;
        }
    }

    async flush(_deadline: Deadline): Promise<void> {}

    close(): void {
        this.retire(undefined);
    }

    /** `quarantine` or a sweep failure skips the native close after reporting the error. */
    private retire(quarantine: unknown): void {
        if (this.closed) return;
        this.closed = true;
        this.releaseAll(quarantine);
    }

    private releaseAll(quarantine: unknown): void {
        let quarantineError = quarantine;
        // Each abort runs the reservation's release, which returns its budget
        // charge even when the native abort throws.
        for (const producer of [...this.producers]) {
            try {
                producer.abort();
            } catch (error) {
                quarantineError ??= error;
            }
        }
        for (const lease of [...this.receiveLeases]) {
            try {
                lease.release();
            } catch (error) {
                quarantineError ??= error;
            }
        }
        if (quarantineError !== undefined) {
            // Alias state is uncertain: unmapping under a live view would
            // trade a bounded leak for a use-after-free, so the native close
            // is withheld and the quarantine is reported.
            this.options.handlers.onClosed("quarantined", quarantineError);
            throw quarantineError;
        }
        if (!this.native) return;
        try {
            this.native.close();
        } catch (error) {
            this.options.handlers.onClosed("quarantined", error);
            throw error;
        }
    }

    isClosed(): boolean {
        return this.closed;
    }

    stats(): FrameChannelStats {
        return {
            readerHeldBytes: 0,
            queueHeldBytes: this.heldBytes,
            queuedDataFrames: 0,
            queuedControlFrames: 0,
            readPaused: false,
            activeTimers: 0,
            activeReceiveLeases: this.receiveLeases.size,
            quarantinedBytes: this.quarantinedBytes,
            ownedAdapterCopies: this.copies.copies,
        };
    }

    private attached(): NativeChannel {
        if (!this.native) {
            throw new HostCallError("not_sent", "shared-memory channel is not started");
        }
        return this.native;
    }

    /**
     * The configured frame limit and integer validity are enforced before
     * any budget charge or native call: a non-safe length (`NaN`, negative,
     * fractional) would poison `ByteBudget.used`, and the shared-memory
     * path must reject the same over-limit bodies TCP rejects.
     */
    private assertBodyBounds(byteLength: number): void {
        if (
            !Number.isSafeInteger(byteLength) ||
            byteLength < 0 ||
            byteLength > this.options.maxBodyLen
        ) {
            throw new RangeError("producer capacity is outside frame bounds");
        }
    }

    private publishFrame(
        header: ProducerFrameHeader,
        body: DirectFrameBody,
        hooks?: FrameSendHooks,
        deadline?: Deadline,
    ): FrameSendTicket {
        if (this.closed) throw new HostCallError("not_sent", "shared-memory channel closed");
        if (deadline?.isExpired()) throw deadlineExpiredError();
        let published = false;
        // `fill` runs caller code, so the deadline is re-checked after it; a throw there aborts the reservation before publication.
        // `expiredBeforePublish` preserves deadline classification when the addon rewraps the callback error.
        let expiredBeforePublish = false;
        try {
            this.attached().produce(
                encodeHeader({ ...header, len: body.byteLength }),
                body.byteLength,
                (cursor: ProducerCursor) => {
                    body.fill(cursor);
                    if (deadline?.isExpired()) {
                        expiredBeforePublish = true;
                        throw deadlineExpiredError();
                    }
                },
                () => {
                    published = true;
                    try {
                        hooks?.onPublish?.();
                    } catch {
                        // Send hooks cannot change publication.
                    }
                },
                0,
            );
        } catch (error) {
            if (expiredBeforePublish) {
                throw error instanceof HostCallError ? error : deadlineExpiredError(error);
            }
            if (isRingFullError(error)) throw ringFullError(error);
            throw error;
        }
        try {
            hooks?.onComplete?.();
        } catch {
            // Send hooks cannot change completion.
        }
        return { cancel: () => !published };
    }

    private admitPublication(bytes: number): void {
        if (this.options.budget.wouldExceed(bytes)) {
            throw new HostCallError(
                "not_sent",
                "aggregate connection memory cap would be exceeded",
                "memory_cap",
            );
        }
        this.options.budget.charge(bytes);
        this.heldBytes += bytes;
    }

    private releasePublication(bytes: number): void {
        this.heldBytes -= bytes;
        this.options.budget.release(bytes);
    }

    private drainReady(): void {
        if (this.closed) return;
        try {
            for (let frames = 0; frames < DRAIN_BATCH_FRAMES; frames += 1) {
                // `onFrame` can close the channel; return before polling its closed native handle.
                if (this.closed) return;
                if (
                    !this.attached().drainOne((nativeLease: NativeReceiveLease) => {
                        const header = decodeHeader(nativeLease.header);
                        const violation = headerViolation(header);
                        const structuralError =
                            header.ver !== PROTOCOL_VERSION
                                ? "unsupported protocol version"
                                : header.len !== nativeLease.byteLength
                                  ? "ring frame length mismatch"
                                  : header.len > this.options.maxBodyLen
                                    ? "ring frame exceeds configured body limit"
                                    : null;
                        if (structuralError !== null || violation !== null) {
                            nativeLease.release();
                            throw new InboundFrameError(
                                violation?.reason ?? "protocol_violation",
                                structuralError ?? violation?.detail ?? "invalid ring frame",
                            );
                        }
                        const segments = Array.from(
                            { length: nativeLease.segmentCount },
                            (_, index) => nativeLease.segment(index),
                        );
                        let lease: ReceiveLease;
                        lease = new ReceiveLease(
                            segments,
                            (outcome) => {
                                this.receiveLeases.delete(lease);
                                if (outcome === "quarantined") this.quarantinedBytes += header.len;
                                this.options.handlers.onLeaseReleased?.();
                                // A retained lease can quarantine after `onFrame` returned, where
                                // no drain catch retires the channel.
                                if (outcome === "quarantined" && !this.closed) {
                                    try {
                                        this.retire(
                                            new Error(
                                                "receive lease alias detachment failed; storage quarantined",
                                            ),
                                        );
                                    } catch {
                                        // `release()` throws the quarantine to its own caller.
                                    }
                                }
                            },
                            this.copies,
                            () => {
                                nativeLease.release();
                                return "released";
                            },
                        );
                        this.receiveLeases.add(lease);
                        try {
                            this.options.handlers.onFrame({ header, body: lease });
                        } catch (error) {
                            lease.release();
                            throw error;
                        }
                    })
                ) {
                    this.consecutiveMicrotaskDrains = 0;
                    // Readiness includes setup-socket closure. Check only after an empty drain so graceful Goodbye reaches dispatcher first.
                    if (this.attached().peerClosed()) this.failClose("eof", undefined);
                    return;
                }
            }
            this.scheduleDrain();
        } catch (error) {
            this.failClose(
                error instanceof InboundFrameError ? error.reason : "protocol_violation",
                error,
            );
        }
    }

    private scheduleDrain(): void {
        if (this.drainScheduled) return;
        this.drainScheduled = true;
        const resume = (): void => {
            this.drainScheduled = false;
            if (!this.closed) this.drainReady();
        };
        if (this.consecutiveMicrotaskDrains < DRAIN_MICROTASK_BUDGET) {
            this.consecutiveMicrotaskDrains += 1;
            queueMicrotask(resume);
        } else {
            this.consecutiveMicrotaskDrains = 0;
            setImmediate(resume);
        }
    }

    /** `closed` is set before `onClosed` so a publish from the callback is refused. */
    private failClose(reason: FrameChannelCloseReason, error: unknown): void {
        if (this.closed) return;
        this.closed = true;
        try {
            this.options.handlers.onClosed(reason, error);
        } catch {
            // Readiness callbacks have no caller to observe the throw.
        }
        try {
            this.releaseAll(undefined);
        } catch {
            // Quarantine is already surfaced through `onClosed("quarantined")`.
        }
    }
}
