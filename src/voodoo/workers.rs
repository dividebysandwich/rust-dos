//! Drawing on worker threads. The card's commands (triangles and
//! fastfills) are jobs, each with everything it draws with; batches of
//! them go to every worker, and each worker draws the screen rows it owns
//! (row `r` belongs to worker `r % N`). Every worker takes the jobs in
//! order, so a pixel sees the triangles in the order the card got them,
//! and the picture is the same for any number of workers: save states,
//! rewind and tests stay deterministic.
//!
//! Frame buffer writes that come while jobs are queued are jobs too, so
//! they land in their turn without waiting. The emulator waits for the
//! workers (`flush`) before anything that reads what they draw: frame
//! buffer reads, texture writes the queued triangles may read, the pixel
//! counters, and a save state. With no workers (the browser, a single
//! core) jobs run at once.

use super::mem::Vram;
use super::raster::{self, RasterState, Stats, TriParams};
use super::setup::render_triangle;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

/// Something drawn.
pub enum Job {
    /// A triangle with `texcount` texture units: its vertices in pixels
    /// and its parameters.
    Triangle { st: RasterState, p: TriParams, verts: [(f32, f32); 3], texcount: usize },
    /// A fastfill of rows `y0..y1`, columns `x0..x1`, with the dither
    /// pattern of its colour.
    Fastfill { st: RasterState, dither: [u16; 16], x0: i32, x1: i32, y0: i32, y1: i32 },
    /// Words frame buffer writes left, in order.
    Pixels { fb: Vram, words: Vec<Word> },
    /// A job that fails, for the tests.
    #[cfg(test)]
    Panic,
}

/// A word of the frame buffer written: the buffer row it is on (the
/// worker that owns the row writes it, as it draws the triangles there),
/// its index in the frame buffer, and its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Word {
    pub row: u16,
    pub at: u32,
    pub value: u16,
}

impl Job {
    /// Draw all of it, here.
    pub fn run_all(&self, stipple: &mut u32, stats: &mut Stats) {
        self.run(|_| true, stipple, stats);
    }

    /// Draw the rows `owns` accepts. `stipple` is the stipple register,
    /// which only a triangle drawn whole changes.
    fn run(&self, owns: impl Fn(i32) -> bool, stipple: &mut u32, stats: &mut Stats) {
        match self {
            Job::Triangle { st, p, verts, texcount } => {
                let flip = st.fbz_mode & (1 << 17) != 0;
                let mut row = |y: i32, x0: i32, x1: i32| {
                    if !owns(raster::screen_y(st, y, flip)) {
                        return;
                    }
                    match texcount {
                        0 => raster::scanline::<0>(st, p, y, x0, x1, stipple, stats),
                        1 => raster::scanline::<1>(st, p, y, x0, x1, stipple, stats),
                        _ => raster::scanline::<2>(st, p, y, x0, x1, stipple, stats),
                    }
                };
                render_triangle(*verts, &mut row);
            }
            Job::Fastfill { st, dither, x0, x1, y0, y1 } => {
                let flip = st.fbz_mode & (1 << 17) != 0;
                for y in *y0..*y1 {
                    if owns(raster::screen_y(st, y, flip)) {
                        raster::fastfill_row(st, dither, y, *x0, *x1, stats);
                    }
                }
            }
            Job::Pixels { fb, words } => {
                for w in words {
                    if owns(w.row as i32) && (w.at as usize) < fb.len() {
                        fb.set(w.at as usize, w.value);
                    }
                }
            }
            #[cfg(test)]
            Job::Panic => panic!("a worker failed"),
        }
    }
}

struct Queue {
    /// Batches not yet drawn by every worker, with their numbers.
    batches: VecDeque<(u64, Arc<Vec<Job>>)>,
    /// The number of the last batch given out.
    last: u64,
    /// The last batch each worker finished.
    done: Vec<u64>,
    /// What the workers counted.
    stats: Stats,
    shutdown: bool,
    /// A worker's panic, handed on to the emulator.
    panic: Option<Box<dyn std::any::Any + Send>>,
}

struct Shared {
    queue: Mutex<Queue>,
    /// New work, or shut down.
    work: Condvar,
    /// A worker finished a batch.
    idle: Condvar,
}

/// The workers, or none.
pub struct Pool {
    shared: Arc<Shared>,
    /// Jobs not yet given out.
    pending: Mutex<Vec<Job>>,
    workers: usize,
    #[cfg(not(target_arch = "wasm32"))]
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// How many jobs go out at a time.
const BATCH: usize = 128;

impl Pool {
    /// `workers` threads, or none to draw every job at once.
    pub fn new(workers: usize) -> Self {
        #[cfg(target_arch = "wasm32")]
        let workers = {
            let _ = workers;
            0
        };
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                batches: VecDeque::new(),
                last: 0,
                done: vec![0; workers],
                stats: Stats::default(),
                shutdown: false,
                panic: None,
            }),
            work: Condvar::new(),
            idle: Condvar::new(),
        });
        #[cfg(not(target_arch = "wasm32"))]
        let threads = (0..workers)
            .map(|k| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name(format!("3dfx {}", k))
                    .spawn(move || worker(shared, k, workers))
                    .expect("starting a 3dfx render thread")
            })
            .collect();
        Self {
            shared,
            pending: Mutex::new(Vec::new()),
            workers,
            #[cfg(not(target_arch = "wasm32"))]
            threads,
        }
    }

    /// The number of workers a machine should have: all cores but the
    /// emulator's, at most 4 (`RUST_DOS_VOODOO_THREADS` overrides it, for
    /// benchmarks).
    pub fn default_workers() -> usize {
        if let Some(n) = std::env::var("RUST_DOS_VOODOO_THREADS").ok().and_then(|v| v.parse().ok()) {
            return n;
        }
        std::thread::available_parallelism().map_or(0, |n| n.get().saturating_sub(1).min(4))
    }

    pub fn workers(&self) -> usize {
        self.workers
    }

    /// Draw `job`: at once without workers, otherwise in its turn.
    pub fn submit(&self, job: Job) {
        if self.workers == 0 {
            let mut stats = Stats::default();
            let mut stipple = 0;
            job.run(|_| true, &mut stipple, &mut stats);
            self.shared.queue.lock().unwrap().stats.add(&stats);
            return;
        }
        // Frame buffer writes go out at once: they come in big jobs.
        let now = matches!(job, Job::Pixels { .. });
        let mut pending = self.pending.lock().unwrap();
        pending.push(job);
        if now || pending.len() >= BATCH {
            let batch = std::mem::take(&mut *pending);
            drop(pending);
            self.publish(batch);
        }
    }

    fn publish(&self, batch: Vec<Job>) {
        if batch.is_empty() {
            return;
        }
        let mut queue = self.shared.queue.lock().unwrap();
        queue.last += 1;
        let number = queue.last;
        queue.batches.push_back((number, Arc::new(batch)));
        self.shared.work.notify_all();
    }

    /// Give out what is pending and wait until everything is drawn. A
    /// worker's panic comes back here.
    pub fn flush(&self) {
        if self.workers == 0 {
            return;
        }
        let batch = std::mem::take(&mut *self.pending.lock().unwrap());
        self.publish(batch);
        let mut queue = self.shared.queue.lock().unwrap();
        while queue.done.iter().any(|&d| d < queue.last) {
            queue = self.shared.idle.wait(queue).unwrap();
        }
        if let Some(panic) = queue.panic.take() {
            drop(queue);
            std::panic::resume_unwind(panic);
        }
    }

    /// Whether jobs are waiting or being drawn.
    pub fn busy(&self) -> bool {
        if self.workers == 0 {
            return false;
        }
        if !self.pending.lock().unwrap().is_empty() {
            return true;
        }
        let queue = self.shared.queue.lock().unwrap();
        queue.done.iter().any(|&d| d < queue.last)
    }

    /// What the workers counted (after a `flush`, all of it).
    pub fn stats(&self) -> Stats {
        self.shared.queue.lock().unwrap().stats
    }

    pub fn reset_stats(&self) {
        self.shared.queue.lock().unwrap().stats = Stats::default();
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        {
            let mut queue = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            queue.shutdown = true;
            self.shared.work.notify_all();
        }
        #[cfg(not(target_arch = "wasm32"))]
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Worker `k` of `n`: draw its rows of every batch, in order.
#[cfg(not(target_arch = "wasm32"))]
fn worker(shared: Arc<Shared>, k: usize, n: usize) {
    let mut queue = shared.queue.lock().unwrap();
    loop {
        if queue.shutdown {
            return;
        }
        let done = queue.done[k];
        let Some((number, batch)) = queue.batches.iter().find(|(number, _)| *number > done).cloned() else {
            queue = shared.work.wait(queue).unwrap();
            continue;
        };
        drop(queue);
        let mut stats = Stats::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut stipple = 0;
            for job in batch.iter() {
                job.run(|row| row >= 0 && row as usize % n == k, &mut stipple, &mut stats);
            }
        }));
        queue = shared.queue.lock().unwrap();
        if let Err(panic) = result {
            queue.panic.get_or_insert(panic);
        }
        queue.stats.add(&stats);
        queue.done[k] = number;
        // A batch every worker has drawn can go.
        let oldest = queue.done.iter().copied().min().unwrap_or(0);
        while queue.batches.front().is_some_and(|(number, _)| *number <= oldest) {
            queue.batches.pop_front();
        }
        shared.idle.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workers_panic_comes_back_to_the_emulator() {
        let pool = Pool::new(2);
        pool.submit(Job::Panic);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.flush()));
        assert!(result.is_err(), "the panic reaches flush");
        // The workers go on, and the pool shuts down.
        pool.flush();
        assert!(!pool.busy());
    }
}
