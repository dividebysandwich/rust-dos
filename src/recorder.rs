use std::fs::File;
use std::io::BufWriter;
use gif::{Encoder, Frame, Repeat};
use std::time::{Instant, Duration};
use chrono::Local;

pub struct ScreenRecorder {
    is_recording: bool,
    width: u16,
    height: u16,
    encoder: Option<Encoder<BufWriter<File>>>,
    last_frame_time: Instant,
    frame_delay: Duration,
}

impl ScreenRecorder {
    pub fn new(width: u32, height: u32, fps: u64) -> Self {
        Self {
            is_recording: false,
            width: width as u16,
            height: height as u16,
            encoder: None,
            last_frame_time: Instant::now(),
            frame_delay: Duration::from_millis(1000 / fps),
        }
    }

    pub fn is_active(&self) -> bool {
        self.is_recording
    }

    pub fn toggle(&mut self) {
        if self.is_recording {
            self.stop();
        } else {
            self.start();
        }
    }

    fn start(&mut self) {
        let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S");
        let filename = format!("rust-dos_capture_{}.gif", timestamp);
        
        println!("[RECORDER] Started recording to {}", filename);
        
        let file = File::create(&filename).expect("Failed to create record file");
        let writer = BufWriter::new(file);
        
        // Initialize GIF Encoder
        let mut encoder = Encoder::new(writer, self.width, self.height, &[]).unwrap();
        encoder.set_repeat(Repeat::Infinite).unwrap();
        
        self.encoder = Some(encoder);
        self.is_recording = true;
        self.last_frame_time = Instant::now();
    }

    fn stop(&mut self) {
        println!("[RECORDER] Stopped recording.");
        self.encoder = None; // Dropping the encoder flushes and writes the file trailer
        self.is_recording = false;
    }

    pub fn capture(&mut self, pixels: &[u8]) {
        if !self.is_recording { return; }
        
        if self.last_frame_time.elapsed() >= self.frame_delay {
            if let Some(enc) = &mut self.encoder {
                // Create a frame from the RGB pixels
                // Map RGB24 SDL2 buffer directly to GIF RGB
                let mut frame = Frame::from_rgb(self.width, self.height, pixels);
                
                // Delay is in units of 10ms
                frame.delay = (self.frame_delay.as_millis() / 10) as u16;
                
                if let Err(e) = enc.write_frame(&frame) {
                    println!("[RECORDER] Error writing frame: {}", e);
                }
            }
            self.last_frame_time = Instant::now();
        }
    }
}