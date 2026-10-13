//! An order-preserving map over a small persistent worker pool. Each output keeps its input's
//! position, so callers observe the same sequence as a serial map.

use std::sync::OnceLock;

/// Items each worker takes at least, so a short input stays on the calling thread.
const MIN_ITEMS_PER_WORKER: usize = 24;
/// Workers one map uses at most.
const MAX_WORKERS: usize = 4;

/// The pool, or `None` when the host offers one core or the pool cannot start; the map then
/// runs on the calling thread.
fn pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(MAX_WORKERS);
        if threads < 2 {
            return None;
        }
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|index| format!("eidnara-map-{index}"))
            .build()
            .ok()
    })
    .as_ref()
}

/// `map` applied to every item in order. Each worker's chunk shares one `S`, built by
/// `S::default`; a worker's panic, redacted as a handler's is, resumes on the calling thread.
pub(crate) fn map_with<T, S, R, F>(items: Vec<T>, map: F) -> Vec<R>
where
    T: Send,
    S: Default,
    R: Send,
    F: Fn(&mut S, T) -> R + Sync,
{
    let run = |chunk: Vec<T>| {
        let mut state = S::default();
        chunk
            .into_iter()
            .map(|item| map(&mut state, item))
            .collect::<Vec<R>>()
    };
    let pool = pool()
        .filter(|pool| items.len() >= 2 * MIN_ITEMS_PER_WORKER && pool.current_num_threads() > 1);
    let Some(pool) = pool else {
        return run(items);
    };
    let workers = (items.len() / MIN_ITEMS_PER_WORKER).min(pool.current_num_threads());
    let chunk = items.len().div_ceil(workers);
    let mut chunks = Vec::with_capacity(workers);
    let mut rest = items;
    while rest.len() > chunk {
        let tail = rest.split_off(chunk);
        chunks.push(rest);
        rest = tail;
    }
    chunks.push(rest);
    let mut parts: Vec<Option<Vec<R>>> = chunks.iter().map(|_| None).collect();
    let run = &run;
    pool.scope(|scope| {
        for (chunk, part) in chunks.into_iter().zip(parts.iter_mut()) {
            scope.spawn(move |_| *part = Some(host_runtime::redact_panics(|| run(chunk))));
        }
    });
    parts.into_iter().flatten().flatten().collect()
}

/// Mapping stops after the batch in flight finishes when `consume` returns `false`.
pub(crate) fn map_until<T, S, R, F, C>(items: Vec<T>, map: F, mut consume: C)
where
    T: Send,
    S: Default,
    R: Send,
    F: Fn(&mut S, T) -> R + Sync,
    C: FnMut(R) -> bool,
{
    let Some(pool) = pool() else {
        let mut state = S::default();
        for item in items {
            if !consume(map(&mut state, item)) {
                return;
            }
        }
        return;
    };
    let mut batch = MIN_ITEMS_PER_WORKER * pool.current_num_threads();
    let mut items = items.into_iter();
    let mut current = map_with(items.by_ref().take(batch).collect(), &map);
    while !current.is_empty() {
        batch = batch.saturating_mul(2);
        let upcoming = items.by_ref().take(batch).collect::<Vec<T>>();
        let mut mapped = Vec::new();
        let map = &map;
        let consumed_all = pool.in_place_scope(|scope| {
            if !upcoming.is_empty() {
                let mapped = &mut mapped;
                scope.spawn(move |_| {
                    *mapped = host_runtime::redact_panics(|| map_with(upcoming, map));
                });
            }
            current.into_iter().all(&mut consume)
        });
        if !consumed_all {
            return;
        }
        current = mapped;
    }
}

/// `a` on the calling thread and `b` on a pool worker, both returned once both finish; without a
/// pool, `a` then `b` on the calling thread. A panic in either, redacted on the worker as a
/// handler's is, resumes on the calling thread after both have stopped.
pub(crate) fn join<A, B, RA, RB>(a: A, b: B) -> (RA, RB)
where
    A: FnOnce() -> RA + Send,
    B: FnOnce() -> RB + Send,
    RA: Send,
    RB: Send,
{
    let Some(pool) = pool() else {
        let left = a();
        return (left, b());
    };
    let mut right = None;
    let left = pool.in_place_scope(|scope| {
        scope.spawn(|_| right = Some(host_runtime::redact_panics(b)));
        a()
    });
    (left, right.expect("the scope joins its spawned half"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_input_order_on_every_split() {
        for len in [0, 1, 23, 24, 47, 48, 97, 301, 1000] {
            let items = (0..len).collect::<Vec<usize>>();
            let out = map_with::<_, usize, _, _>(items, |calls, item| {
                *calls += 1;
                item * 3
            });
            assert_eq!(
                out,
                (0..len).map(|item| item * 3).collect::<Vec<_>>(),
                "len {len}"
            );
        }
    }

    #[test]
    fn gives_each_worker_its_own_state() {
        let out = map_with::<_, Vec<usize>, _, _>((0..300).collect(), |seen, item| {
            seen.push(item);
            seen.windows(2).all(|pair| pair[0] + 1 == pair[1])
        });
        assert!(out.into_iter().all(|contiguous| contiguous));
    }

    fn mapped_until(len: usize, stop: usize) -> (Vec<usize>, usize) {
        let mapped = std::sync::atomic::AtomicUsize::new(0);
        let mut consumed = Vec::new();
        map_until::<_, (), _, _, _>(
            (0..len).collect(),
            |(), item| {
                mapped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                item * 3
            },
            |output| {
                consumed.push(output);
                consumed.len() < stop
            },
        );
        (consumed, mapped.into_inner())
    }

    #[test]
    fn map_until_hands_over_outputs_in_order_until_the_consumer_stops() {
        for len in [0, 1, 47, 48, 95, 96, 97, 301, 1000] {
            for stop in [1, 2, 47, 48, 96, 97, 300, usize::MAX] {
                let (consumed, _) = mapped_until(len, stop);
                let expected = (0..len.min(stop)).map(|item| item * 3).collect::<Vec<_>>();
                assert_eq!(consumed, expected, "len {len} stop {stop}");
            }
        }
    }

    #[test]
    fn map_until_maps_little_past_an_early_stop() {
        let first_batch = MIN_ITEMS_PER_WORKER * MAX_WORKERS;
        for stop in [1, 10, 96, 97, 200, 500] {
            let (consumed, mapped) = mapped_until(10_000, stop);
            assert_eq!(consumed.len(), stop);
            assert!(
                mapped <= 4 * stop + 3 * first_batch,
                "stop {stop} mapped {mapped} of 10000, over four times the consumed plus three opening batches"
            );
        }
    }

    #[test]
    fn join_returns_both_halves() {
        let (left, right) = join(|| (0..1000).sum::<u64>(), || "right".to_string());
        assert_eq!((left, right.as_str()), (499_500, "right"));
    }

    #[test]
    fn join_resumes_either_half_panic_on_the_caller() {
        assert!(std::panic::catch_unwind(|| join(|| (), || panic!("right half"))).is_err());
        assert!(std::panic::catch_unwind(|| join(|| panic!("left half"), || ())).is_err());
    }

    #[test]
    fn resumes_a_worker_panic_on_the_caller() {
        let caught = std::panic::catch_unwind(|| {
            map_with::<_, (), _, _>((0..300).collect::<Vec<usize>>(), |(), item| {
                assert!(item != 299, "the last chunk panics");
                item
            })
        });
        assert!(caught.is_err());
    }
}
