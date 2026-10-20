use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gif::{Encoder, Repeat};

use crate::capture::zmbv::fitted;
use crate::video::Frame;

/// Frames waiting for the encoding thread. When it falls behind (a
/// high-colour picture takes it longer than a frame's time to quantise),
/// the frames past these are skipped, and the one before them stays on
/// screen for their time.
const QUEUED: usize = 2;

/// A captured frame for the encoding thread, and when it was captured.
struct Work {
    frame: Frame,
    at: Instant,
}

/// An animated GIF recording. The emulation thread hands over a copy of
/// each frame to record; a thread of its own converts it to 256 colours,
/// compresses it and writes it.
pub struct ScreenRecorder {
    work: Option<SyncSender<Work>>,
    /// The encoding threads: the recording's, and those of stopped
    /// recordings still writing the frames queued for them.
    threads: Vec<JoinHandle<Result<(), String>>>,
    last_frame_time: Instant,
    frame_delay: Duration,
}

impl ScreenRecorder {
    pub fn new(fps: u64) -> Self {
        Self {
            work: None,
            threads: Vec::new(),
            last_frame_time: Instant::now(),
            frame_delay: Duration::from_millis(1000 / fps),
        }
    }

    pub fn is_active(&self) -> bool {
        self.work.is_some()
    }

    /// Start recording to a new file in `dir`, or stop. Returns the file
    /// it started recording to. A stopped recording's thread goes on
    /// writing the frames queued for it (a high-colour frame takes it
    /// about 0.2 s) without holding up the caller; `finished` says when
    /// a recording failed.
    pub fn toggle(&mut self, dir: &Path) -> Result<Option<PathBuf>, String> {
        if self.is_active() {
            self.stop();
            Ok(None)
        } else {
            self.start(dir).map(Some)
        }
    }

    fn start(&mut self, dir: &Path) -> Result<PathBuf, String> {
        let path = crate::capture::capture_path(dir, "animation", "gif")?;
        let file = File::create(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
        println!("[RECORDER] Started recording to {}", path.display());
        let (work, frames) = mpsc::sync_channel(QUEUED);
        let last_delay = (self.frame_delay.as_millis() / 10) as u16;
        let thread = std::thread::Builder::new()
            .name("gif recorder".into())
            .spawn(move || encode(BufWriter::new(file), frames, last_delay))
            .map_err(|e| {
                let _ = std::fs::remove_file(&path);
                format!("Can't start the recording: {}", e)
            })?;
        self.work = Some(work);
        self.threads.push(thread);
        self.last_frame_time = Instant::now();
        Ok(path)
    }

    /// Stop recording without waiting: the thread writes the frames
    /// queued for it and the file's trailer once the channel closes.
    fn stop(&mut self) {
        self.work = None;
    }

    /// Why a recording failed, once its thread has finished. Called every
    /// frame: it takes at most one finished thread and never waits.
    pub fn finished(&mut self) -> Option<String> {
        let at = self.threads.iter().position(|t| t.is_finished())?;
        joined(self.threads.swap_remove(at)).err()
    }

    /// Record a frame. The recording keeps the size of its first frame;
    /// frames of another size (the video mode changed) are scaled to it.
    pub fn capture(&mut self, frame: &Frame) {
        let Some(work) = &self.work else { return };
        if self.last_frame_time.elapsed() < self.frame_delay {
            return;
        }
        let now = Instant::now();
        self.last_frame_time = now;
        match work.try_send(Work { frame: frame.clone(), at: now }) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            // The thread stopped: writing failed, which `finished` reports.
            Err(TrySendError::Disconnected(_)) => self.stop(),
        }
    }
}

/// Quitting while recording, or while a stopped recording is still being
/// written, waits for the files to be complete.
impl Drop for ScreenRecorder {
    fn drop(&mut self) {
        self.stop();
        for thread in self.threads.drain(..) {
            let _ = joined(thread);
        }
    }
}

/// Wait for an encoding thread and log how its recording ended.
fn joined(thread: JoinHandle<Result<(), String>>) -> Result<(), String> {
    let result = thread.join().unwrap_or_else(|_| Err("the recording thread failed".to_string()));
    match &result {
        Ok(()) => println!("[RECORDER] Finished writing the recording."),
        Err(e) => println!("[RECORDER] The recording failed: {}", e),
    }
    result.map_err(|e| format!("The animation recording failed: {}", e))
}

/// The encoding thread: each frame is written when the next arrives, which
/// says how long it stays on screen; the last one stays `last_delay`
/// hundredths of a second.
fn encode(writer: BufWriter<File>, frames: Receiver<Work>, last_delay: u16) -> Result<(), String> {
    let Ok(first) = frames.recv() else { return Ok(()) };
    let (width, height) = (first.frame.width as u16, first.frame.height as u16);
    let mut encoder = Encoder::new(writer, width, height, &[]).map_err(|e| e.to_string())?;
    encoder.set_repeat(Repeat::Infinite).map_err(|e| e.to_string())?;
    let mut shown = first;
    // Hundredths of a second written so far, from the first frame.
    let mut written: u64 = 0;
    let start = shown.at;
    loop {
        let next = frames.recv().ok();
        let delay = match &next {
            Some(next) => {
                let until = (next.at - start).as_millis() as u64 / 10;
                let delay = until.saturating_sub(written).min(u16::MAX as u64);
                // From the capture time, so that time a frame could not
                // be shown for (past the longest delay) is not carried
                // over to the next.
                written = until.max(written);
                delay as u16
            }
            None => last_delay,
        };
        let pixels = fitted(&shown.frame, width as usize, height as usize);
        let mut gif_frame = gif::Frame::from_rgb(width, height, &pixels);
        gif_frame.delay = delay;
        encoder.write_frame(&gif_frame).map_err(|e| e.to_string())?;
        match next {
            Some(next) => shown = next,
            // Through the trailer and the last flush, whose errors the
            // encoder's and the writer's drops would ignore.
            None => {
                let writer = encoder.into_inner().map_err(|e| e.to_string())?;
                writer.into_inner().map_err(|e| e.error().to_string())?;
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, shade: u8) -> Frame {
        Frame::from_rgb(width, height, vec![shade; (width * height * 3) as usize])
    }

    /// The frames of a GIF file: (width, height, delay, first pixel's
    /// palette colour).
    type GifFrame = (u16, u16, u16, [u8; 3]);

    fn frames_of(path: &Path) -> (u16, u16, Vec<GifFrame>) {
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = options.read_info(File::open(path).unwrap()).unwrap();
        let (w, h) = (decoder.width(), decoder.height());
        let mut frames = Vec::new();
        while let Some(f) = decoder.read_next_frame().unwrap() {
            frames.push((f.width, f.height, f.delay, [f.buffer[0], f.buffer[1], f.buffer[2]]));
        }
        (w, h, frames)
    }

    #[test]
    fn a_recording_holds_the_frames_at_the_first_size() {
        let dir = crate::test_dir::path("gif");
        let mut recorder = ScreenRecorder::new(1000);
        let path = recorder.toggle(&dir).unwrap().unwrap();
        assert!(recorder.is_active());
        for f in [frame(32, 16, 10), frame(64, 32, 200), frame(32, 16, 90)] {
            std::thread::sleep(Duration::from_millis(50));
            recorder.capture(&f);
        }
        assert_eq!(recorder.toggle(&dir).unwrap(), None);
        assert!(!recorder.is_active());
        // Stopping returns at once; the thread finishes the file on its own.
        let waited = Instant::now();
        while !recorder.threads.is_empty() {
            assert_eq!(recorder.finished(), None);
            assert!(waited.elapsed() < Duration::from_secs(10), "the recording was never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
        let (w, h, frames) = frames_of(&path);
        assert_eq!((w, h), (32, 16));
        assert_eq!(frames.len(), 3);
        // Each frame as big as the first, the second scaled down.
        assert!(frames.iter().all(|f| (f.0, f.1) == (32, 16)));
        assert_eq!(frames.iter().map(|f| f.3[0]).collect::<Vec<_>>(), [10, 200, 90]);
        // Each stays until the next was captured, 50 ms or more later (a
        // busy machine may sleep much longer); the last a frame's time of
        // the recorder.
        assert!(frames[..2].iter().all(|f| f.2 >= 5), "{:?}", frames);
        assert_eq!(frames[2].2, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
