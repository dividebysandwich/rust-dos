//! Decodes the intro of Grom (2002), INTRO.AVI, 640x480 IV32, and checks
//! the pictures against FFmpeg's. Skips (passes) when the file or ffmpeg
//! isn't there. INDEO3_AVI names another file, INDEO3_FRAMES how many
//! frames to check (300 by default); run in release mode for timings.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rust_dos_indeo3::{Decoder, PixelFormat};

const INTRO: &str =
    r"D:\rust-dos-games\Grom (2002)\installed\c\PROGRAM FILES\GROM\RUN\DATA\MOVIE\INTRO.AVI";

/// The video frames of stream 0, `00dc` or `00db` chunks of the `movi`
/// list, in file order.
fn video_chunks(data: &[u8]) -> Vec<&[u8]> {
    fn walk<'a>(mut d: &'a [u8], in_movi: bool, out: &mut Vec<&'a [u8]>) {
        while d.len() >= 8 {
            let id = &d[..4];
            let size = u32::from_le_bytes(d[4..8].try_into().unwrap()) as usize;
            let body = &d[8..(8 + size).min(d.len())];
            if (id == b"RIFF" || id == b"LIST") && body.len() >= 4 {
                let movi = &body[..4] == b"movi";
                walk(&body[4..], in_movi || movi, out);
            } else if in_movi && (id == b"00dc" || id == b"00db") {
                out.push(body);
            }
            d = &d[(8 + size + (size & 1)).min(d.len())..];
        }
    }
    let mut out = Vec::new();
    walk(data, false, &mut out);
    out
}

/// A sync frame: its bitstream header says 16 bytes, and it has no
/// picture for FFmpeg to give.
fn is_sync(frame: &[u8]) -> bool {
    frame.len() >= 24 && (u32::from_le_bytes(frame[20..24].try_into().unwrap()) + 7) >> 3 == 16
}

fn ffmpeg(path: &PathBuf, frames: usize, pix_fmt: &str) -> Option<Vec<u8>> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-frames:v", &frames.to_string()])
        .args([
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-pix_fmt",
            pix_fmt,
            "-",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

#[test]
fn intro_matches_ffmpeg() {
    let path = PathBuf::from(std::env::var("INDEO3_AVI").unwrap_or(INTRO.into()));
    let Ok(data) = std::fs::read(&path) else {
        eprintln!("skipped: {} isn't there", path.display());
        return;
    };
    let n: usize = std::env::var("INDEO3_FRAMES").map_or(300, |v| v.parse().unwrap());
    let chunks = video_chunks(&data);
    let n = n.min(chunks.len());
    let (Some(ref_yuv), Some(ref_bgr)) = (ffmpeg(&path, n, "yuv410p"), ffmpeg(&path, n, "bgr24"))
    else {
        eprintln!("skipped: ffmpeg didn't run");
        return;
    };

    let (w, h) = (640, 480);
    let yuv_size = w * h + 2 * (w / 4) * (h / 4);
    let mut d = Decoder::new(w as u32, h as u32).unwrap();
    let mut bgr = vec![0; w * h * 3];
    let mut decode_time = Duration::ZERO;
    let mut convert_time = Duration::ZERO;
    let (mut shown, mut max_diff, mut diff_sum) = (0, 0u8, 0u64);
    for (i, chunk) in chunks[..n].iter().enumerate() {
        let t = Instant::now();
        d.decode(chunk).unwrap_or_else(|e| panic!("frame {i}: {e}"));
        decode_time += t.elapsed();
        if chunk.is_empty() || is_sync(chunk) {
            continue;
        }

        let yuv = d.yuv410().concat();
        let want = &ref_yuv[shown * yuv_size..][..yuv_size];
        assert!(yuv == want, "frame {i}: YUV differs from FFmpeg's");

        let t = Instant::now();
        d.write_picture(PixelFormat::Bgr24, false, w * 3, &mut bgr)
            .unwrap();
        convert_time += t.elapsed();
        let want = &ref_bgr[shown * w * h * 3..][..w * h * 3];
        for (&a, &b) in bgr.iter().zip(want) {
            let diff = a.abs_diff(b);
            max_diff = max_diff.max(diff);
            diff_sum += diff as u64;
        }
        shown += 1;
    }
    assert_eq!(
        shown * yuv_size,
        ref_yuv.len(),
        "FFmpeg gave another number of pictures"
    );

    let per = |t: Duration| t.as_secs_f64() * 1000.0 / n as f64;
    eprintln!(
        "{n} frames, {shown} pictures: YUV equal to FFmpeg's; BGR24 against FFmpeg's \
         differs by at most {max_diff}, {:.3} on average; decode {:.3} ms a frame, \
         BGR24 conversion {:.3} ms",
        diff_sum as f64 / (shown * w * h * 3) as f64,
        per(decode_time),
        per(convert_time),
    );
}

/// Real frames damaged at random give errors or pictures, never a panic.
#[test]
fn damaged_intro_frames_dont_panic() {
    let path = PathBuf::from(std::env::var("INDEO3_AVI").unwrap_or(INTRO.into()));
    let Ok(data) = std::fs::read(&path) else {
        eprintln!("skipped: {} isn't there", path.display());
        return;
    };
    let chunks = video_chunks(&data);
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut d = Decoder::new(640, 480).unwrap();
    for i in 0..400 {
        let chunk = chunks[(rand() as usize) % chunks.len().min(300)];
        let mut f = chunk.to_vec();
        if i % 8 == 0 {
            f.truncate((rand() as usize) % f.len());
        }
        for _ in 0..1 + rand() % 8 {
            // The first 16 bytes hold the checksum; damage what follows.
            let j = 16 + (rand() as usize) % (f.len().max(17) - 16);
            if let Some(b) = f.get_mut(j) {
                *b = rand() as u8;
            }
        }
        let _ = d.decode(&f);
    }
}
