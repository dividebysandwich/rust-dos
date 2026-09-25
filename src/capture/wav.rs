//! Sound recordings: the mixed output as a WAV file, 16-bit stereo at 44.1
//! kHz. The header's sizes are brought up to date every second or so, so a
//! recording cut short still plays up to there.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

pub const RATE: u32 = crate::opl::RATE;
const CHANNELS: u16 = 2;
const HEADER: u32 = 44;

pub struct WavWriter {
    file: BufWriter<File>,
    /// Sample bytes written so far.
    data: u32,
    /// Sample bytes at the last header update.
    written: u32,
}

impl WavWriter {
    pub fn create(path: &Path) -> Result<Self, String> {
        let file = File::create(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let mut writer = Self { file: BufWriter::new(file), data: 0, written: 0 };
        writer.header().map_err(|e| e.to_string())?;
        Ok(writer)
    }

    /// The RIFF header for the samples written so far.
    fn header(&mut self) -> std::io::Result<()> {
        let block = CHANNELS as u32 * 2;
        let file = &mut self.file;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(b"RIFF")?;
        file.write_all(&(HEADER - 8 + self.data).to_le_bytes())?;
        file.write_all(b"WAVEfmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?; // PCM
        file.write_all(&CHANNELS.to_le_bytes())?;
        file.write_all(&RATE.to_le_bytes())?;
        file.write_all(&(RATE * block).to_le_bytes())?;
        file.write_all(&(block as u16).to_le_bytes())?;
        file.write_all(&16u16.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&self.data.to_le_bytes())?;
        file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    /// Add interleaved stereo samples.
    pub fn write(&mut self, samples: &[i16]) -> Result<(), String> {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.file.write_all(&bytes).map_err(|e| e.to_string())?;
        self.data = self.data.saturating_add(bytes.len() as u32);
        if self.data - self.written >= RATE * 4 {
            self.written = self.data;
            self.header().map_err(|e| e.to_string())?;
            self.file.flush().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Finish the file. Returns the seconds recorded.
    pub fn finish(mut self) -> Result<f64, String> {
        self.header().map_err(|e| e.to_string())?;
        self.file.flush().map_err(|e| e.to_string())?;
        Ok(self.data as f64 / (RATE * CHANNELS as u32 * 2) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_has_the_sizes() {
        let path = std::env::temp_dir().join(format!("rust-dos-wav-{}.wav", std::process::id()));
        let mut wav = WavWriter::create(&path).unwrap();
        wav.write(&[1, -1, 2, -2]).unwrap();
        wav.write(&[3, -3]).unwrap();
        let seconds = wav.finish().unwrap();
        assert!((seconds - 3.0 / RATE as f64).abs() < 1e-9);
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(bytes.len(), 44 + 12);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 36 + 12);
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 44_100);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 12);
        assert_eq!(i16::from_le_bytes(bytes[46..48].try_into().unwrap()), -1);
    }
}
