//! The renderer's threads. A batch of draws is one piece of work that
//! every thread runs on the rows it owns: `Pool::run` hands a function
//! to the workers, runs it on the calling thread too, and comes back
//! when all have finished, so the function may borrow what the caller
//! holds, as with scoped threads. The threads stay between batches,
//! since a frame has many.

use std::sync::{Arc, Condvar, Mutex};

/// The function of a batch, as a pointer that outlives no call of
/// `run`.
#[derive(Clone, Copy)]
struct Work(*const (dyn Fn(usize) + Sync));

// The function is `Sync`, and `run` keeps it alive until every worker
// has returned from it.
unsafe impl Send for Work {}

#[derive(Default)]
struct State {
    work: Option<Work>,
    /// Counts the batches, so a worker knows one from the last.
    batch: u64,
    /// Workers still running the batch.
    running: usize,
    panicked: bool,
    quit: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// A batch is there, or the pool is going.
    wake: Condvar,
    /// A worker finished the batch.
    done: Condvar,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // Nothing panics with the lock held.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct Pool {
    shared: Arc<Shared>,
    #[cfg(not(target_arch = "wasm32"))]
    threads: Vec<std::thread::JoinHandle<()>>,
    workers: usize,
}

impl Pool {
    /// A pool of `workers` threads beside the caller's. The browser has
    /// none.
    pub fn new(workers: usize) -> Self {
        let shared = Arc::new(Shared::default());
        #[cfg(target_arch = "wasm32")]
        let workers = {
            let _ = workers;
            0
        };
        #[cfg(not(target_arch = "wasm32"))]
        let threads = (0..workers)
            .map(|k| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name(format!("toadforce-{k}"))
                    .spawn(move || worker(&shared, k + 1))
                    .expect("a renderer thread")
            })
            .collect();
        Pool {
            shared,
            #[cfg(not(target_arch = "wasm32"))]
            threads,
            workers,
        }
    }

    /// The number of workers a machine should have: the cores but the
    /// emulator's, at most 3, which with the emulator's own thread is
    /// four drawing (`RUST_DOS_TOADFORCE_THREADS` overrides it, for
    /// benchmarks).
    pub fn default_workers() -> usize {
        let set = std::env::var("RUST_DOS_TOADFORCE_THREADS");
        if let Some(n) = set.ok().and_then(|v| v.parse().ok()) {
            return n;
        }
        std::thread::available_parallelism().map_or(0, |n| n.get().saturating_sub(1).min(3))
    }

    /// The threads that draw: the workers and the caller.
    pub fn shares(&self) -> usize {
        self.workers + 1
    }

    /// Run `work(k)` for every `k` below `shares()`, share 0 on this
    /// thread, and return when all are done. A panic in any comes back
    /// as a panic here, after the others have finished.
    pub fn run(&self, work: &(dyn Fn(usize) + Sync)) {
        if self.workers == 0 {
            return work(0);
        }
        // The lifetime is erased for the workers; `Wait` below doesn't
        // let this call end, by return or by panic, while one has it.
        let erased: *const (dyn Fn(usize) + Sync + 'static) =
            unsafe { std::mem::transmute(work as *const (dyn Fn(usize) + Sync)) };
        {
            let mut state = self.shared.lock();
            state.work = Some(Work(erased));
            state.batch += 1;
            state.running = self.workers;
            state.panicked = false;
        }
        self.shared.wake.notify_all();

        struct Wait<'a>(&'a Shared);
        impl Drop for Wait<'_> {
            fn drop(&mut self) {
                let mut state = self.0.lock();
                while state.running > 0 {
                    state = self.0.done.wait(state).unwrap_or_else(|e| e.into_inner());
                }
                state.work = None;
            }
        }
        let wait = Wait(&self.shared);
        work(0);
        drop(wait);
        if self.shared.lock().panicked {
            panic!("a ToadForce renderer thread panicked");
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.lock().quit = true;
        self.shared.wake.notify_all();
        #[cfg(not(target_arch = "wasm32"))]
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn worker(shared: &Shared, share: usize) {
    let mut seen = 0;
    loop {
        let work = {
            let mut state = shared.lock();
            while !state.quit && state.batch == seen {
                state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            if state.quit {
                return;
            }
            seen = state.batch;
            state.work
        };
        let Some(Work(work)) = work else { continue };
        // `run` waits for this thread before the function goes.
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { (*work)(share) }));
        let mut state = shared.lock();
        state.panicked |= result.is_err();
        state.running -= 1;
        drop(state);
        shared.done.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn every_share_runs_once_a_batch_and_borrows_the_callers_data() {
        for workers in [0, 1, 3] {
            let pool = Pool::new(workers);
            assert_eq!(pool.shares(), workers + 1);
            let hits: Vec<AtomicUsize> = (0..pool.shares()).map(|_| AtomicUsize::new(0)).collect();
            for _ in 0..50 {
                pool.run(&|k| {
                    hits[k].fetch_add(1, Ordering::Relaxed);
                });
            }
            for hit in &hits {
                assert_eq!(hit.load(Ordering::Relaxed), 50);
            }
        }
    }

    #[test]
    fn a_workers_panic_comes_back_after_the_others_finish() {
        let pool = Pool::new(2);
        let finished = AtomicUsize::new(0);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pool.run(&|k| {
                if k == 1 {
                    panic!("share 1 fails");
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
                finished.fetch_add(1, Ordering::Relaxed);
            })
        }));
        assert!(result.is_err());
        assert_eq!(finished.load(Ordering::Relaxed), 2);
        // The pool still works.
        pool.run(&|_| {
            finished.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(finished.load(Ordering::Relaxed), 5);
    }
}
