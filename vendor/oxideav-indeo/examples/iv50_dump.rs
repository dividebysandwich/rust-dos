//! Decode the IV50 video stream of an AVI file to raw planar frames.
//!
//! Usage: `iv50_dump IN.avi OUT.yuv [--yuv410p]`
//!
//! Each frame is written at the size in the AVI's BITMAPINFOHEADER as
//! the Y plane, then V, then U (the order the vendor DLL keeps its
//! planes in), chroma at `ceil(W / 4)` by `ceil(H / 4)`. With
//! `--yuv410p` the chroma order is U then V instead, as ffmpeg writes
//! yuv410p. A frame that fails to decode repeats the previous picture.
//! The decode time of the whole stream, without file I/O, is printed on
//! stdout.

use std::time::Instant;

use oxideav_indeo::indeo5::{Iv50Decoder, Iv50Plane};

/// Read a little-endian u32.
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

/// The chunks of a RIFF list body: (id, data offset, size).
fn chunks(d: &[u8], start: usize, end: usize) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos + 8 <= end.min(d.len()) {
        let id: [u8; 4] = d[pos..pos + 4].try_into().unwrap();
        let size = u32_at(d, pos + 4) as usize;
        out.push((id, pos + 8, size));
        pos += 8 + size + (size & 1);
    }
    out
}

struct Avi {
    width: usize,
    height: usize,
    frames: Vec<Vec<u8>>,
}

/// Find the first video stream and collect its chunks from the first
/// `movi` list, in file order.
fn parse_avi(d: &[u8]) -> Avi {
    assert!(
        &d[0..4] == b"RIFF" && &d[8..12] == b"AVI ",
        "not an AVI file"
    );
    let mut vid: Option<usize> = None;
    let (mut width, mut height) = (0, 0);
    let mut movi = None;
    for (id, off, size) in chunks(d, 12, d.len()) {
        if &id == b"LIST" && &d[off..off + 4] == b"hdrl" {
            let mut stream = 0;
            for (id2, off2, size2) in chunks(d, off + 4, off + size) {
                if &id2 != b"LIST" || &d[off2..off2 + 4] != b"strl" {
                    continue;
                }
                let mut is_video = false;
                for (id3, off3, _) in chunks(d, off2 + 4, off2 + size2) {
                    if &id3 == b"strh" {
                        is_video = &d[off3..off3 + 4] == b"vids";
                    }
                    if &id3 == b"strf" && is_video && vid.is_none() {
                        vid = Some(stream);
                        width = u32_at(d, off3 + 4) as usize;
                        height = (u32_at(d, off3 + 8) as i32).unsigned_abs() as usize;
                    }
                }
                stream += 1;
            }
        } else if &id == b"LIST" && &d[off..off + 4] == b"movi" && movi.is_none() {
            movi = Some((off + 4, off + size));
        }
    }
    let vid = vid.expect("no video stream");
    let prefix = format!("{vid:02}");
    let mut frames = Vec::new();
    fn walk(d: &[u8], s: usize, e: usize, prefix: &[u8], frames: &mut Vec<Vec<u8>>) {
        for (id, off, size) in chunks(d, s, e) {
            if &id == b"LIST" {
                walk(d, off + 4, off + size, prefix, frames);
            } else if &id[..2] == prefix && (&id[2..] == b"dc" || &id[2..] == b"db") {
                frames.push(d[off..(off + size).min(d.len())].to_vec());
            }
        }
    }
    let (s, e) = movi.expect("no movi list");
    walk(d, s, e, prefix.as_bytes(), &mut frames);
    Avi {
        width,
        height,
        frames,
    }
}

/// Copy a plane into `out` at `w` by `h`, padding with zeros.
fn put(out: &mut Vec<u8>, p: Option<Iv50Plane>, w: usize, h: usize) {
    for r in 0..h {
        for c in 0..w {
            let v = match p {
                Some(p) if r < p.height && c < p.width => p.data[r * p.stride + c],
                _ => 0,
            };
            out.push(v);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: iv50_dump IN.avi OUT.yuv [--yuv410p]");
        std::process::exit(2);
    }
    let yuv410p = args.iter().any(|a| a == "--yuv410p");
    let data = std::fs::read(&args[1]).expect("read AVI");
    let avi = parse_avi(&data);
    let (w, h) = (avi.width, avi.height);
    let (cw, ch) = (w.div_ceil(4), h.div_ceil(4));
    let mut dec = Iv50Decoder::new();
    let mut out = Vec::with_capacity(avi.frames.len() * (w * h + 2 * cw * ch));
    let mut failed = 0;
    let mut elapsed = 0.0f64;
    for (i, f) in avi.frames.iter().enumerate() {
        let t = Instant::now();
        let res = dec.decode(f);
        elapsed += t.elapsed().as_secs_f64();
        if let Err(e) = res {
            failed += 1;
            eprintln!("frame {i}: {e}");
        }
        put(&mut out, dec.y(), w, h);
        if yuv410p {
            put(&mut out, dec.u(), cw, ch);
            put(&mut out, dec.v(), cw, ch);
        } else {
            put(&mut out, dec.v(), cw, ch);
            put(&mut out, dec.u(), cw, ch);
        }
    }
    std::fs::write(&args[2], &out).expect("write output");
    println!(
        "decoded {} failed {} decode_ms {:.1}",
        avi.frames.len() - failed,
        failed,
        elapsed * 1000.0
    );
}
