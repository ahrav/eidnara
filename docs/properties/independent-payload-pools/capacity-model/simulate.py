#!/usr/bin/env python3
"""Deterministic capacity sketch, not a transport benchmark or safety proof.

Run: python3 docs/plans/independent-payload-pools/simulate.py
Units: microseconds and bytes. One direction, initially empty, 2s open-loop
trace, immediate would-block on admission failure, no retries, full drain.
Refusals have disjoint causes: class first when both resources are exhausted,
otherwise descriptor. Simultaneous exhaustion also has a diagnostic count.
Each 100-message cycle: 90 x 2KiB/2ms, 8 x 48KiB/10ms,
1 x 512KiB/30ms, 1 x 4MiB/100ms. One 32MiB payload at t=0 is held
through the arrival window. Sizes denote bodies; each allocation includes
21 header bytes. Classes are full-frame capacities; largest has one extra
4KiB page for a maximum body. No distribution fitting or random seeds.
Descriptor consumption takes fixed accept_us after publish. Returns take
return_us after the final reader releases. They do not consume descriptors.
FIFO mode isolates ordered payload reclamation, not current ring geometry.
No bandwidth, CPU, SQLite, JS GC, allocator, or wakeup performance is modeled.
"""

import heapq
import json
from collections import deque

KIB = 1024
MIB = KIB * KIB
HEADER_BYTES = 21
CLASSES = (4 * KIB, 64 * KIB, MIB, 8 * MIB, 64 * MIB + 4 * KIB)
COUNTS = (64, 16, 8, 2, 1)
HORIZON_US = 2_000_000


def trace(rate_per_s, hold_scale=1, burst=1):
    rows = [(0, 32 * MIB, HORIZON_US + 1_000_000)]
    for i in range(rate_per_s * 2):
        slot = i % 100
        size, hold = ((2 * KIB, 2_000) if slot < 90 else
                      (48 * KIB, 10_000) if slot < 98 else
                      (512 * KIB, 30_000) if slot == 98 else
                      (4 * MIB, 100_000))
        at = (i // burst) * burst * 1_000_000 // rate_per_s
        rows.append((at, size, hold * hold_scale))
    return rows


def simulate(rows, counts=COUNTS, depth=32, accept_us=100,
             return_us=100, fifo=False, horizon_us=HORIZON_US):
    assert depth > 0 and accept_us >= 0 and return_us >= 0
    assert len(counts) == len(CLASSES) and all(n > 0 for n in counts)
    free = list(counts)
    descriptors = []
    releases = []
    ordered = deque()
    completed = set()
    classes = {}
    held_bytes = peak_bytes = accepted = rejected = released = 0
    peak_slots = [0] * len(counts)
    peak_descriptors = 0
    class_refusals = [0] * len(counts)
    descriptor_refusals = both_exhausted = 0

    def recycle(identity):
        nonlocal held_bytes, released
        c = classes.pop(identity)
        free[c] += 1
        held_bytes -= CLASSES[c]
        released += 1

    def advance(now):
        while descriptors and descriptors[0] <= now:
            heapq.heappop(descriptors)
        while releases and releases[0][0] <= now:
            _, identity = heapq.heappop(releases)
            if fifo:
                completed.add(identity)
            else:
                recycle(identity)
        while fifo and ordered and ordered[0] in completed:
            identity = ordered.popleft()
            completed.remove(identity)
            recycle(identity)

    previous = -1
    for identity, (at, size, hold) in enumerate(rows):
        assert previous <= at < horizon_us and 0 <= size <= 64 * MIB
        assert hold >= 0
        previous = at
        advance(at)
        c = next(i for i, capacity in enumerate(CLASSES)
                 if size + HEADER_BYTES <= capacity)
        class_full = not free[c]
        descriptors_full = len(descriptors) == depth
        if class_full or descriptors_full:
            rejected += 1
            both_exhausted += int(class_full and descriptors_full)
            if class_full:
                class_refusals[c] += 1
            else:
                descriptor_refusals += 1
            continue
        accepted += 1
        free[c] -= 1
        classes[identity] = c
        if fifo:
            ordered.append(identity)
        held_bytes += CLASSES[c]
        heapq.heappush(descriptors, at + accept_us)
        heapq.heappush(releases, (at + accept_us + hold + return_us, identity))
        peak_bytes = max(peak_bytes, held_bytes)
        peak_descriptors = max(peak_descriptors, len(descriptors))
        peak_slots = [max(peak, n - f) for peak, n, f in
                      zip(peak_slots, counts, free)]
        assert 0 <= held_bytes <= sum(c * n for c, n in zip(CLASSES, counts))
        assert all(0 <= f <= n for f, n in zip(free, counts))
    advance(horizon_us)
    held_at_horizon = len(classes)
    last_return = max((at for at, _ in releases), default=horizon_us)
    advance(max(last_return, horizon_us))
    assert not classes and free == list(counts)
    assert accepted == released and accepted + rejected == len(rows)
    assert rejected == descriptor_refusals + sum(class_refusals)
    return dict(offered=len(rows), admitted=accepted, would_block=rejected,
                descriptor_refusals=descriptor_refusals,
                class_refusals=class_refusals, both_exhausted=both_exhausted,
                held_at_horizon=held_at_horizon, unfinished_after_drain=0,
                peak_slots=peak_slots, peak_descriptors=peak_descriptors,
                peak_mib=round(peak_bytes / MIB, 3),
                backing_mib=sum(c * n for c, n in zip(CLASSES, counts)) / MIB)


def check():
    rows = [(0, 32 * MIB, 100), (1, 1, 1), (3, 1, 1)]
    args = dict(counts=(1, 1, 1, 1, 1), depth=1, accept_us=0,
                return_us=0, horizon_us=10)
    assert simulate(rows, **args)['would_block'] == 0
    assert simulate(rows, fifo=True, **args)['would_block'] == 1
    result = simulate([(i * 10, 1, 20) for i in range(10)],
                      accept_us=0, return_us=0, horizon_us=100)
    assert result['peak_slots'][0] == 2
    assert simulate([], horizon_us=10)['offered'] == 0
    assert simulate([(0, 64 * MIB, 1)])['admitted'] == 1
    simultaneous = simulate([(0, 1, 100), (0, 1, 100)],
                            counts=(1, 1, 1, 1, 1), depth=1)
    assert simultaneous['both_exhausted'] == 1
    assert simultaneous['class_refusals'][0] == 1
    assert simultaneous['descriptor_refusals'] == 0


if __name__ == '__main__':
    check()
    print('Self-checks passed. Scenario results, not measured performance:')
    for label, rate, scale, burst, depth, accept, returns, counts, fifo in [
        ('base', 1000, 1, 1, 32, 100, 100, COUNTS, False),
        ('fifo-isolation', 1000, 1, 1, 32, 100, 100, COUNTS, True),
        ('rate-5000', 5000, 1, 1, 32, 100, 100, COUNTS, False),
        ('large-count-8', 5000, 1, 1, 32, 100, 100, (64, 16, 8, 8, 1), False),
        ('holds-10x', 1000, 10, 1, 32, 100, 100, COUNTS, False),
        ('burst-64-depth-8', 1000, 1, 64, 8, 100, 100, COUNTS, False),
        ('burst-64-depth-32', 1000, 1, 64, 32, 100, 100, COUNTS, False),
        ('burst-64-depth-128', 1000, 1, 64, 128, 100, 100, COUNTS, False),
        ('accept-10ms', 5000, 1, 1, 32, 10_000, 100, COUNTS, False),
        ('return-10ms', 1000, 1, 1, 32, 100, 10_000, COUNTS, False),
    ]:
        print(json.dumps(dict(scenario=label, **simulate(
            trace(rate, scale, burst), counts, depth, accept, returns, fifo))))
