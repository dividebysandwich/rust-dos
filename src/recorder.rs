use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use gif::{Encoder, Repeat};
use std::time::{Instant, Duration};

use crate::video::Frame;

pub struct ScreenRecorder {
    is_recording: bool,
    /// The file being recorded to, until its first frame fixes the size.
    pending: Option<BufWriter<File>>,
    /// The size of the recording: the size of the picture when it started.
    width: u16,
    height: u16,
    encoder: Option<Encoder<BufWriter<File>>>,
    last_frame_time: Instant,
    frame_delay: Duration,
}

impl ScreenRecorder {
    pub fn new(fps: u64) -> Self {
        Self {
            is_recording: false,
            pending: None,
            width: 0,
            height: 0,
            encoder: None,
            last_frame_time: Instant::now(),
            frame_delay: Duration::from_millis(1000 / fps),
        }
    }

    pub fn is_active(&self) -> bool {
        self.is_recording
    }

    /// Start recording to a new file in `dir`, or stop. Returns the file
    /// it started recording to.
    pub fn toggle(&mut self, dir: &Path) -> Result<Option<PathBuf>, String> {
        if self.is_recording {
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
        self.pending = Some(BufWriter::new(file));
        self.encoder = None;
        self.is_recording = true;
        self.last_frame_time = Instant::now();
        Ok(path)
    }

    fn stop(&mut self) {
        println!("[RECORDER] Stopped recording.");
        self.encoder = None; // Dropping the encoder flushes and writes the file trailer
        self.pending = None;
        self.is_recording = false;
    }

    /// Record a frame. The recording keeps the size of its first frame;
    /// frames of another size (the video mode changed) are scaled to it.
    pub fn capture(&mut self, frame: &Frame) {
        if !self.is_recording { return; }

        if let Some(writer) = self.pending.take() {
            self.width = frame.width as u16;
            self.height = frame.height as u16;
            let mut encoder = Encoder::new(writer, self.width, self.height, &[]).unwrap();
            encoder.set_repeat(Repeat::Infinite).unwrap();
            self.encoder = Some(encoder);
        }

        if self.last_frame_time.elapsed() >= self.frame_delay {
            let (w, h) = (self.width as usize, self.height as usize);
            let scaled;
            let pixels = if (frame.width as usize, frame.height as usize) == (w, h) {
                &frame.rgb
            } else {
                // Nearest neighbour.
                let mut rgb = Vec::with_capacity(w * h * 3);
                for y in 0..h {
                    let sy = y * frame.height as usize / h;
                    for x in 0..w {
                        let at = (sy * frame.width as usize + x * frame.width as usize / w) * 3;
                        rgb.extend_from_slice(&frame.rgb[at..at + 3]);
                    }
                }
                scaled = rgb;
                &scaled
            };
            if let Some(enc) = &mut self.encoder {
                // Create a frame from the RGB pixels
                let mut gif_frame = gif::Frame::from_rgb(self.width, self.height, pixels);

                // Delay is in units of 10ms
                gif_frame.delay = (self.frame_delay.as_millis() / 10) as u16;

                if let Err(e) = enc.write_frame(&gif_frame) {
                    println!("[RECORDER] Error writing frame: {}", e);
                }
            }
            self.last_frame_time = Instant::now();
        }
    }
}
