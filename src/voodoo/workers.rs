//! Drawing on worker threads. The card's commands (triangles and
//! fastfills) are jobs, each with everything it draws with; batches of
//! them go to every worker, and each worker draws the screen rows it owns
//! (row `r` belongs to worker `r % N`). Every worker takes the jobs in
//! order, so a pixel sees the triangles in the order the card got them,
//! and the picture is the same for any number of workers: save states,
//! rewind and tests stay deterministic.
//!
//! Frame buffer writes that come while jobs are queued are jobs too, so
//! they land in their turn without waiting, and so does texture data
//! Glide downloads over a texture queued triangles read: its bytes are a
//! batch of their own, which one worker writes once every worker has
//! finished what came before and before any draws what comes after. The
//! emulator waits for the workers (`flush`) before anything that reads
//! what they draw: frame buffer reads, other texture writes the queued
//! triangles may read, the pixel counters, and a save state. The queue
//! has a limit (`MAX_BATCHES`, `MAX_BYTES`): past it the emulator waits
//! for a batch to finish, so what such a read waits for stays short.
//! With no workers (the browser, a single core) jobs run at once.

use super::Voodoo;
use super::mem::Vram;
use super::raster::{self, RasterState, Stats, TriParams};
use super::setup::render_triangle;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

/// Something drawn. Not boxed: a job is made for every triangle.
#[allow(clippy::large_enum_variant)]
pub enum Job {
    /// A triangle with `texcount` texture units: its vertices in pixels
    /// and its parameters.
    Triangle { st: RasterState, p: TriParams, verts: [(f32, f32); 3], texcount: usize },
    /// A fastfill of rows `y0..y1`, columns `x0..x1`, with the dither
    /// pattern of its colour.
    Fastfill { st: RasterState, dither: [u16; 16], x0: i32, x1: i32, y0: i32, y1: i32 },
    /// Words frame buffer writes left, in order.
    Pixels { fb: Vram, words: Vec<Word> },
    /// Rows `rows` of the buffer at word `base`, `rowpixels` words each,
    /// filled with `value` from word `columns.0` to `columns.1` of each
    /// (a Voodoo2's SGRAM fill).
    Rows {
        fb: Vram,
        base: usize,
        rowpixels: usize,
        rows: (u32, u32),
        columns: (usize, usize),
        value: u16,
    },
    /// Bytes put into the memory from byte `at`, between the jobs before
    /// them and those after: rows are no use here, as every worker's
    /// triangles read the textures (see `worker`). The worker that owns
    /// row 0 writes them.
    Bytes { fb: Vram, at: usize, bytes: Vec<u8> },
    /// A job that fails, for the tests.
    #[cfg(test)]
    Panic,
    /// A job that waits for a gate to open, for the tests.
    #[cfg(test)]
    Hold(Arc<(Mutex<bool>, Condvar)>),
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
                let draw = raster::pipeline(raster::variant(st, *texcount), *texcount);
                let mut row = |y: i32, x0: i32, x1: i32| {
                    if !owns(raster::screen_y(st, y, flip)) {
                        return;
                    }
                    draw(st, p, y, x0, x1, stipple, stats);
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
            Job::Rows {
                fb,
                base,
                rowpixels,
                rows,
                columns,
                value,
            } => {
                for row in rows.0..rows.1 {
                    if owns(row as i32) {
                        let start = base + row as usize * rowpixels;
                        let end = (start + columns.1).min(fb.len());
                        fb.fill((start + columns.0).min(end)..end, *value);
                    }
                }
            }
            Job::Bytes { fb, at, bytes } => {
                if owns(0) {
                    fb.write_bytes(*at, bytes);
                }
            }
            #[cfg(test)]
            Job::Panic => panic!("a worker failed"),
            #[cfg(test)]
            Job::Hold(gate) => {
                let mut open = gate.0.lock().unwrap();
                while !*open {
                    open = gate.1.wait(open).unwrap();
                }
            }
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
    /// The bytes of `Job::Bytes` in `batches`.
    bytes: usize,
    /// How many times the emulator found the queue full.
    full: u64,
    /// What the workers counted.
    stats: Stats,
    shutdown: bool,
    /// A worker's panic, handed on to the emulator.
    panic: Option<Box<dyn std::any::Any + Send>>,
}

impl Queue {
    /// Whether the emulator has to wait before it queues more. Batches
    /// count from the slowest worker's; a panic counts as finished, and
    /// reaches the emulator at the next `flush`.
    fn full(&self) -> bool {
        let oldest = self.done.iter().copied().min().unwrap_or(self.last);
        (self.last - oldest > MAX_BATCHES || self.bytes > MAX_BYTES) && !self.shutdown
    }
}

/// The bytes of `Job::Bytes` a batch holds.
fn bytes_of(batch: &[Job]) -> usize {
    let len = |job: &Job| match job {
        Job::Bytes { bytes, .. } => bytes.len(),
        _ => 0,
    };
    batch.iter().map(len).sum()
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

/// Batches the workers may have before the emulator waits for them.
/// Myth downloads over a texture in use 46 times a game frame, each a
/// batch of bytes and one of what came before, and the workers finish
/// such a pair in about 0.26 ms: 64 batches are about 8 ms of work,
/// well inside a host frame, and most of what a game frame gives them
/// when they keep up. Full batches of triangles reach it at 8,192
/// triangles, where the workers are behind anyway. The limit counts
/// batches, not time: 64 of the largest triangles take longer.
const MAX_BATCHES: u64 = 64;

/// Bytes of texture data the queue may hold before the emulator waits:
/// about 20 game frames of Myth's. Against a program that downloads a
/// card's memory many times over while triangles use it.
const MAX_BYTES: usize = 4 << 20;

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
                bytes: 0,
                full: 0,
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
        let mut pending = self.pending.lock().unwrap();
        if matches!(job, Job::Bytes { .. }) {
            let batch = std::mem::take(&mut *pending);
            drop(pending);
            self.publish(batch);
            self.publish(vec![job]);
            return;
        }
        // Frame buffer writes go out at once: they come in big jobs.
        let now = matches!(job, Job::Pixels { .. });
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
        queue.bytes += bytes_of(&batch);
        queue.batches.push_back((number, Arc::new(batch)));
        self.shared.work.notify_all();
        // The memory ends up the same however long this takes, and the
        // emulated clock doesn't see it.
        if queue.full() {
            queue.full += 1;
            while queue.full() {
                queue = self.shared.idle.wait(queue).unwrap();
            }
        }
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

    /// Give out what is pending, without waiting for it to be drawn.
    pub fn give_out(&self) {
        if self.workers == 0 {
            return;
        }
        let batch = std::mem::take(&mut *self.pending.lock().unwrap());
        self.publish(batch);
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

    /// How many times the emulator waited for room in the queue.
    pub fn full_waits(&self) -> u64 {
        self.shared.queue.lock().unwrap().full
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
            self.shared.idle.notify_all();
        }
        #[cfg(not(target_arch = "wasm32"))]
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Voodoo {
    /// Put texture data, which queued jobs may read or draw into, into a
    /// Banshee's or Voodoo3's memory from byte `at` on in its turn among
    /// them: triangles before it draw with the old texture, those after
    /// it with the new one, and the emulator goes on. False if the
    /// emulator has to write it itself: the workers have finished, which
    /// `flush` then finds out at no cost, there are none, or the OpenGL
    /// renderer is recording the memory.
    pub(crate) fn banshee_texture_queue(&self, at: usize, data: &[u8]) -> bool {
        if self.recording() || !self.pool.busy() {
            return false;
        }
        let job = Job::Bytes {
            fb: self.fbi.ram.clone(),
            at,
            bytes: data.to_vec(),
        };
        self.submit_outside(job, None);
        true
    }
}

/// Worker `k` of `n`: draw its rows of every batch, in order. Bytes
/// (`Job::Bytes`) are read by every worker's triangles, so the workers meet
/// around them: worker 0 waits for the others to finish the batch before,
/// writes the bytes, and the others wait for that.
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
        let bytes = matches!(batch.first(), Some(Job::Bytes { .. }));
        while bytes
            && !queue.shutdown
            && if k == 0 {
                queue.done.iter().any(|&d| d + 1 < number)
            } else {
                queue.done[0] < number
            }
        {
            queue = shared.idle.wait(queue).unwrap();
        }
        if queue.shutdown {
            return;
        }
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
            if let Some((_, batch)) = queue.batches.pop_front() {
                queue.bytes -= bytes_of(&batch);
            }
        }
        shared.idle.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_land_between_the_jobs_around_them() {
        let pool = Pool::new(4);
        let fb = Vram::new(0x1000);
        let word = |row, value| Job::Pixels {
            fb: fb.clone(),
            words: vec![Word {
                row,
                at: row as u32,
                value,
            }],
        };
        for i in 0..2000u16 {
            for row in 0..16 {
                pool.submit(word(row, i));
            }
            let bytes = [i as u8 ^ 0x5A, (i >> 8) as u8].repeat(16);
            pool.submit(Job::Bytes {
                fb: fb.clone(),
                at: 0,
                bytes,
            });
            for row in 8..16 {
                pool.submit(word(row, !i));
            }
            pool.flush();
            for row in 0..8 {
                assert_eq!(fb.get(row), i ^ 0x5A, "row {} of pass {}", row, i);
            }
            for row in 8..16 {
                assert_eq!(fb.get(row), !i, "row {} of pass {}", row, i);
            }
        }
    }

    #[test]
    fn a_pool_with_bytes_queued_shuts_down() {
        let pool = Pool::new(3);
        let fb = Vram::new(16);
        for _ in 0..100 {
            pool.submit(Job::Bytes {
                fb: fb.clone(),
                at: 0,
                bytes: vec![1; 16],
            });
        }
        drop(pool);
    }

    #[test]
    fn a_panic_before_bytes_leaves_the_workers_meeting() {
        let pool = Pool::new(3);
        let fb = Vram::new(16);
        pool.submit(Job::Panic);
        pool.submit(Job::Bytes {
            fb: fb.clone(),
            at: 0,
            bytes: vec![7; 16],
        });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.flush()));
        assert!(result.is_err(), "the panic reaches flush");
        assert_eq!(fb.get(0), 0x0707);
    }

    type Gate = Arc<(Mutex<bool>, Condvar)>;

    fn open(gate: &Gate) {
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    }

    /// A card whose workers wait at a gate.
    fn held_card() -> (Voodoo, Gate) {
        let v = Voodoo::with_workers(crate::voodoo::Board::Voodoo3, 2);
        let gate = Gate::default();
        v.submit_outside(Job::Hold(gate.clone()), None);
        v.pool.give_out();
        (v, gate)
    }

    fn queue_bytes(v: &Voodoo, at: usize, bytes: Vec<u8>) {
        let fb = v.fbi.ram.clone();
        v.submit_outside(Job::Bytes { fb, at, bytes }, None);
    }

    #[test]
    fn texture_data_over_queued_data_comes_after_it() {
        let (mut v, gate) = held_card();
        queue_bytes(&v, 0x1000, vec![2; 64]);
        queue_bytes(&v, 0, vec![7; 2]);
        let end = v.fbi.ram.bytes() as u32;
        // Over those bytes it queues, elsewhere it goes in at once, and
        // past the end of the memory each part does as its memory asks.
        v.banshee_texture_download(0x1020, &[3; 64]);
        v.banshee_texture_download(0x2000, &[4; 64]);
        v.banshee_texture_download(end - 2, &[5; 4]);
        let held = [0x1020, 0x2000, end as usize - 1, 0].map(|at| v.fbi.ram.byte(at));
        open(&gate);
        v.catch_up();
        assert_eq!(held, [0, 4, 5, 0]);
        assert_eq!(v.fbi.ram.byte(0x1000), 2);
        assert_eq!(v.fbi.ram.byte(0x1020), 3);
        assert_eq!(v.fbi.ram.byte(0x105F), 3);
        assert_eq!(v.fbi.ram.byte(0), 5);
    }

    /// What the workers finished stays in the footprint until something
    /// looks: the data doesn't queue behind nothing.
    #[test]
    fn texture_data_goes_in_at_once_when_the_workers_have_finished() {
        let mut v = Voodoo::with_workers(crate::voodoo::Board::Voodoo3, 2);
        queue_bytes(&v, 0x1000, vec![2; 64]);
        while v.pool.busy() {
            std::thread::yield_now();
        }
        assert!(!v.spare(0x1000, 0x1040), "the footprint still has them");
        v.banshee_texture_download(0x1000, &[3; 64]);
        assert_eq!(v.fbi.ram.byte(0x1000), 3);
        assert!(!v.pool.busy());
    }

    /// The emulator stops at the batches and queued bytes allowed, with
    /// the workers held at the first job, and goes on when they do.
    #[test]
    fn a_full_queue_holds_the_emulator_until_the_workers_go_on() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        // With the held job, the 64th batch of bytes reaches the first
        // limit, and the 5th quarter of the bytes allowed the second.
        for (size, stops_after) in [(1, MAX_BATCHES as usize - 1), (MAX_BYTES / 4, 4)] {
            let pool = Pool::new(2);
            let fb = Vram::new(MAX_BYTES / 2);
            let gate = Gate::default();
            let submitted = AtomicUsize::new(0);
            let total = 3 * (stops_after + 1);
            std::thread::scope(|s| {
                s.spawn(|| {
                    pool.submit(Job::Hold(gate.clone()));
                    pool.give_out();
                    for _ in 0..total {
                        let (fb, bytes) = (fb.clone(), vec![1; size]);
                        pool.submit(Job::Bytes { fb, at: 0, bytes });
                        submitted.fetch_add(1, Ordering::SeqCst);
                    }
                });
                while pool.full_waits() == 0 {
                    std::thread::yield_now();
                }
                let reached = submitted.load(Ordering::SeqCst);
                open(&gate);
                assert_eq!(reached, stops_after);
            });
            assert_eq!(submitted.load(Ordering::SeqCst), total);
            pool.flush();
            assert_eq!(pool.shared.queue.lock().unwrap().bytes, 0);
        }
    }

    /// The worker reads a word to write half of it: the emulator can't
    /// write the other half meanwhile.
    #[test]
    fn texture_data_beside_queued_data_in_a_word_comes_after_it() {
        let (mut v, gate) = held_card();
        queue_bytes(&v, 0x1000, vec![2; 3]);
        v.banshee_texture_download(0x1003, &[3]);
        v.banshee_texture_download(0x1004, &[4]);
        let held = [v.fbi.ram.byte(0x1003), v.fbi.ram.byte(0x1004)];
        open(&gate);
        v.catch_up();
        assert_eq!(held, [0, 4]);
        assert_eq!(v.fbi.ram.get(0x1002 / 2), 0x0302);
    }

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
