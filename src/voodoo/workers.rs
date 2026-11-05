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
//! has limits (`MAX_JOBS`, `MAX_BATCHES`, `MAX_BYTES`): past one the
//! emulator waits for a batch to finish, so what such a read waits for
//! has a bound.
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
    /// The jobs in `batches`.
    jobs: usize,
    /// The bytes of `Job::Bytes` in `batches`.
    bytes: usize,
    /// How many times the emulator found the queue full.
    full: u64,
    /// What the workers counted.
    stats: Stats,
    shutdown: bool,
    /// A worker's panic, handed on to the emulator.
    panic: Option<Box<dyn std::any::Any + Send>>,
    /// Batches every worker has drawn, emptied, for the emulator to fill
    /// again (`Pool::take_pending`).
    spare: Vec<Vec<Job>>,
}

impl Queue {
    /// Whether the emulator has to wait before it queues more. Batches
    /// count from the slowest worker's; a panic counts as finished, and
    /// reaches the emulator at the next `flush`.
    fn full(&self) -> bool {
        let oldest = self.done.iter().copied().min().unwrap_or(self.last);
        let over =
            self.last - oldest > MAX_BATCHES || self.jobs > MAX_JOBS || self.bytes > MAX_BYTES;
        over && !self.shutdown
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
    /// The queue's `last`, read without its lock (`latest_batch`).
    given_out: std::sync::atomic::AtomicU64,
    #[cfg(not(target_arch = "wasm32"))]
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// How many jobs go out at a time.
const BATCH: usize = 128;

/// Emptied batches kept for the emulator to fill again: it fills one at
/// a time, and the workers hand them back as fast as they finish them.
const SPARE_BATCHES: usize = 8;

/// Jobs the workers may have before the emulator waits for them: what
/// a frame buffer read or a save state may have to wait for. 8,192
/// triangles are 64 full batches.
const MAX_JOBS: usize = 8192;

/// Batches the workers may have before the emulator waits for them.
/// Bytes (`Job::Bytes`) go out as a batch of their own, after one of
/// what came before, and the workers meet around them: Myth downloads
/// over a texture in use 46 times a game frame, and the workers finish
/// such a pair in about 0.26 ms. 512 batches are 256 such pairs, the
/// batches of five or so of Myth's game frames, and about 70 ms of the
/// workers' time: a frame buffer read behind a full queue of them waits
/// that long. The queue holds the frame after the one a swap shows
/// (`Voodoo::catch_up_shown`), so the program draws it while the
/// workers finish the one before: at 64 batches the emulator waited for
/// room within a frame instead, and Myth's emulation thread was busy 63%
/// of the time at 170,000 cycles, against 40% here.
const MAX_BATCHES: u64 = 512;

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
                jobs: 0,
                bytes: 0,
                full: 0,
                stats: Stats::default(),
                shutdown: false,
                panic: None,
                spare: Vec::new(),
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
            given_out: std::sync::atomic::AtomicU64::new(0),
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
            let batch = self.take_pending(&mut pending);
            drop(pending);
            self.publish(batch);
            self.publish(vec![job]);
            return;
        }
        // Frame buffer writes go out at once: they come in big jobs.
        let now = matches!(job, Job::Pixels { .. });
        pending.push(job);
        if now || pending.len() >= BATCH {
            let batch = self.take_pending(&mut pending);
            drop(pending);
            self.publish(batch);
        }
    }

    /// The jobs `pending` holds, to give out, leaving in their place a
    /// batch the workers have finished with (`Queue::spare`) or a new one
    /// with room for `BATCH`. One that grew from nothing would move its
    /// jobs each time it doubled. A few jobs, as go out before texture
    /// data (`Job::Bytes`), move to a batch of their own size, and
    /// `pending` keeps its room: the queue holds hundreds of such batches
    /// (`MAX_BATCHES`), and with room for `BATCH` each they would take
    /// about 100 KB apiece.
    fn take_pending(&self, pending: &mut Vec<Job>) -> Vec<Job> {
        if pending.is_empty() {
            return Vec::new();
        }
        if pending.len() < BATCH / 4 {
            // `mem::take` would hand out the room this keeps.
            #[allow(clippy::drain_collect)]
            let few = pending.drain(..).collect();
            pending.reserve(BATCH);
            return few;
        }
        let spare = self.shared.queue.lock().unwrap().spare.pop();
        std::mem::replace(pending, spare.unwrap_or_else(|| Vec::with_capacity(BATCH)))
    }

    fn publish(&self, batch: Vec<Job>) {
        if batch.is_empty() {
            return;
        }
        let mut queue = self.shared.queue.lock().unwrap();
        queue.last += 1;
        let number = queue.last;
        self.given_out.store(number, std::sync::atomic::Ordering::Relaxed);
        queue.jobs += batch.len();
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
        self.wait_for(self.mark());
    }

    /// Give out what is pending, without waiting for it to be drawn.
    pub fn give_out(&self) {
        if self.workers == 0 {
            return;
        }
        let batch = self.take_pending(&mut self.pending.lock().unwrap());
        self.publish(batch);
    }

    /// The batch the job submitted last is in, or goes out in: the next
    /// one while it waits, else the last given out.
    pub fn latest_batch(&self) -> u64 {
        let waiting = !self.pending.lock().unwrap().is_empty();
        self.given_out.load(std::sync::atomic::Ordering::Relaxed) + waiting as u64
    }

    /// The batch a job submitted now goes out in, unless it goes out on
    /// its own (`Job::Bytes`): the one after the last given out.
    pub fn next_batch(&self) -> u64 {
        self.given_out.load(std::sync::atomic::Ordering::Relaxed) + 1
    }

    /// Give out what is pending, and the number of the last batch given
    /// out: `wait_for` it waits for the jobs submitted so far.
    pub fn mark(&self) -> u64 {
        if self.workers == 0 {
            return 0;
        }
        self.give_out();
        self.shared.queue.lock().unwrap().last
    }

    /// Wait until every worker drew batch `number` (from `mark`) and those
    /// before it, leaving the later ones drawing. A worker's panic comes
    /// back here.
    pub fn wait_for(&self, number: u64) {
        if self.workers == 0 {
            return;
        }
        let mut queue = self.shared.queue.lock().unwrap();
        while queue.done.iter().any(|&d| d < number) {
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
        // The bytes may land in what the card shows when the picture is
        // made, which then waits for them.
        self.note_drawn(super::Place::Bytes(at as u32, data.len() as u64));
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
        // A batch every worker has drawn can go, emptied for the emulator
        // to fill again. Each worker lets go of its own here, under the
        // lock, so the queue holds the last one once all have drawn it.
        drop(batch);
        let oldest = queue.done.iter().copied().min().unwrap_or(0);
        while queue.batches.front().is_some_and(|(number, _)| *number <= oldest) {
            if let Some((_, batch)) = queue.batches.pop_front() {
                queue.jobs -= batch.len();
                queue.bytes -= bytes_of(&batch);
                if let Ok(mut jobs) = Arc::try_unwrap(batch)
                    && jobs.capacity() >= BATCH
                    && queue.spare.len() < SPARE_BATCHES
                {
                    jobs.clear();
                    queue.spare.push(jobs);
                }
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

    /// The emulator stops at the batches, jobs and queued bytes allowed,
    /// with the workers held at the first job, and goes on when they do.
    #[test]
    fn a_full_queue_holds_the_emulator_until_the_workers_go_on() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let fb = Vram::new(MAX_BYTES / 2);
        let bytes = |size: usize| {
            let fb = fb.clone();
            move || Job::Bytes {
                fb: fb.clone(),
                at: 0,
                bytes: vec![1; size],
            }
        };
        let row = || Job::Rows {
            fb: fb.clone(),
            base: 0,
            rowpixels: 1,
            rows: (0, 1),
            columns: (0, 1),
            value: 1,
        };
        // With the held job: the 512th batch of bytes reaches the batch
        // limit, the 8,192nd row (64 full batches) the job limit, and the
        // 5th quarter of the bytes allowed the byte limit.
        let cases: [(&(dyn Fn() -> Job + Sync), usize); 3] = [
            (&bytes(1), MAX_BATCHES as usize - 1),
            (&row, MAX_JOBS - 1),
            (&bytes(MAX_BYTES / 4), 4),
        ];
        for (job, stops_after) in cases {
            let pool = Pool::new(2);
            let gate = Gate::default();
            let submitted = AtomicUsize::new(0);
            let total = 3 * (stops_after + 1);
            std::thread::scope(|s| {
                s.spawn(|| {
                    pool.submit(Job::Hold(gate.clone()));
                    pool.give_out();
                    for _ in 0..total {
                        pool.submit(job());
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
            let queue = pool.shared.queue.lock().unwrap();
            assert_eq!((queue.jobs, queue.bytes), (0, 0));
        }
    }

    /// A few jobs given out before texture data go in a batch of their
    /// own size, and the pending batch keeps its room for the next ones.
    #[test]
    fn a_few_jobs_go_out_in_a_batch_of_their_size() {
        let pool = Pool::new(2);
        let fb = Vram::new(0x100);
        let mut pending = Vec::with_capacity(BATCH);
        pending.push(Job::Bytes {
            fb,
            at: 0,
            bytes: vec![1],
        });
        let batch = pool.take_pending(&mut pending);
        assert_eq!((batch.len(), batch.capacity()), (1, 1));
        assert!(pending.is_empty() && pending.capacity() >= BATCH);
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

    /// A full batch every worker has drawn comes back, emptied, for a
    /// later one to be filled in: the emulator doesn't allocate a batch
    /// each time it gives one out.
    #[test]
    fn drawn_batches_come_back_to_be_filled_again() {
        let pool = Pool::new(2);
        let fb = Vram::new(0x1000);
        let row = || Job::Rows {
            fb: fb.clone(),
            base: 0,
            rowpixels: 1,
            rows: (0, 1),
            columns: (0, 1),
            value: 1,
        };
        // The first batch grew from nothing; those after it have room for
        // a whole one, and come back.
        for _ in 0..3 * BATCH {
            pool.submit(row());
        }
        pool.flush();
        assert!(pool.pending.lock().unwrap().capacity() >= BATCH);
        let queue = pool.shared.queue.lock().unwrap();
        assert!(!queue.spare.is_empty(), "no batch came back");
        assert!(queue.spare.len() <= SPARE_BATCHES);
        let emptied = |b: &Vec<Job>| b.is_empty() && b.capacity() >= BATCH;
        assert!(queue.spare.iter().all(emptied));
    }
}
