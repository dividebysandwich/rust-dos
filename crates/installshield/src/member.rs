//! Members expanded as they are read, and checked once they have all been:
//!
//! - A cabinet member's stored bytes may be obfuscated, and a compressed
//!   one is read in the form the set was opened with: length-prefixed
//!   chunks, each inflated on its own and expanding to 64 KB at most, or
//!   marker-delimited chunks, read as one raw deflate stream. An empty
//!   stored block (`00 00 FF FF`) is a valid deflate block, and the block
//!   after it starts on the next byte, so the stream reads through the
//!   chunks' ends without looking for them, and those four bytes inside a
//!   chunk's data are read as data. (Unshield splits the bytes at each
//!   `00 00 FF FF` instead, which also splits a chunk that holds them.)
//! - An InstallShield 3 member is its stored bytes, or PKWARE DCL data
//!   that ends with the end code.
//!
//! The declared size bounds the output: a decoder is asked for that many
//! bytes and one more. The stored bytes bound the input.

use crate::explode::Exploder;
use crate::{Compression, Error, ReadSeek, Result, invalid};
use flate2::{Decompress, FlushDecompress, Status};
use md5::{Digest, Md5};
use std::io::{Read, Seek, SeekFrom, Write};

/// The most a length-prefixed chunk expands to.
const CHUNK_LIMIT: usize = 64 * 1024;
/// The empty stored block that ends each marker-delimited chunk, as its
/// last four bytes read most significant first.
const MARKER: u32 = 0x0000_ffff;
const CHUNKS_HINT: &str = " The set was opened to read length-prefixed chunks; a set whose compressed data has no \
     chunk lengths (what Unshield reads with -O) is opened with the marker-delimited form.";

/// Where a cabinet member's stored bytes are: a part in a volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Segment {
    pub(crate) volume: u32,
    pub(crate) offset: u64,
    pub(crate) length: u64,
}

#[derive(Clone, Debug)]
pub(crate) enum Source {
    Volumes(Vec<Segment>),
    /// A file beside the header, `stored` bytes long when the set opened.
    Outside {
        path: String,
        stored: u64,
    },
}

/// How to read one cabinet file.
#[derive(Clone, Debug)]
pub(crate) struct Plan {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) compressed: bool,
    pub(crate) obfuscated: bool,
    pub(crate) md5: Option<[u8; 16]>,
    pub(crate) source: Source,
}

/// A member's stored bytes, read part after part and deobfuscated.
struct Stored<'a> {
    segments: Vec<Segment>,
    open: &'a dyn Fn(u32) -> Result<Box<dyn ReadSeek>>,
    stream: Option<(u32, Box<dyn ReadSeek>)>,
    next: usize,
    left: u64,
    read: u64,
    total: u64,
    obfuscated: bool,
    seed: u32,
    last_four: u32,
}

impl Stored<'_> {
    fn remaining(&self) -> u64 {
        self.total - self.read
    }

    /// Fills `buf`, which must not be longer than what remains.
    fn fill(&mut self, buf: &mut [u8], path: &str) -> Result<()> {
        let mut filled = 0;
        while filled < buf.len() {
            if self.left == 0 {
                let Some(&segment) = self.segments.get(self.next) else {
                    return invalid(format!(
                        "InstallShield member '{}' needs more than its {} stored bytes.",
                        path, self.total
                    ));
                };
                self.next += 1;
                if self
                    .stream
                    .as_ref()
                    .is_none_or(|(v, _)| *v != segment.volume)
                {
                    self.stream = Some((segment.volume, (self.open)(segment.volume)?));
                }
                if let Some((_, stream)) = self.stream.as_mut() {
                    stream.seek(SeekFrom::Start(segment.offset))?;
                }
                self.left = segment.length;
            }
            let want = (buf.len() - filled).min(self.left.min(usize::MAX as u64) as usize);
            let Some((_, stream)) = self.stream.as_mut() else {
                unreachable!("a segment is open")
            };
            let n = stream.read(&mut buf[filled..filled + want])?;
            if n == 0 {
                return invalid(format!(
                    "InstallShield member '{}' ends early: the file holding its stored bytes is shorter than when the set was opened.",
                    path
                ));
            }
            filled += n;
            self.left -= n as u64;
        }
        self.read += buf.len() as u64;
        if self.obfuscated {
            for byte in buf.iter_mut() {
                let value = *byte ^ 0xd5;
                let value = value.rotate_right(2);
                *byte = value.wrapping_sub((self.seed % 0x47) as u8);
                self.seed = self.seed.wrapping_add(1);
            }
        }
        for &b in &buf[buf.len().saturating_sub(4)..] {
            self.last_four = (self.last_four << 8) | b as u32;
        }
        Ok(())
    }
}

/// Where expanded bytes go, counted and hashed.
struct Sink<'a> {
    out: &'a mut dyn Write,
    hash: Option<Md5>,
    produced: u64,
    size: u64,
    path: &'a str,
}

impl Sink<'_> {
    fn put(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 > self.size - self.produced {
            return invalid(format!(
                "InstallShield member '{}' expands past its declared size of {} bytes.",
                self.path, self.size
            ));
        }
        if let Some(hash) = self.hash.as_mut() {
            hash.update(bytes);
        }
        self.out.write_all(bytes)?;
        self.produced += bytes.len() as u64;
        Ok(())
    }
}

/// Expand the cabinet file `plan` into `out`, opening its volumes (or its
/// file beside the header) with `open`. Returns its size.
pub(crate) fn decode_cabinet(
    plan: &Plan,
    compression: Compression,
    open: &dyn Fn(u32) -> Result<Box<dyn ReadSeek>>,
    out: &mut dyn Write,
) -> Result<u64> {
    let segments = match &plan.source {
        Source::Volumes(segments) => segments.clone(),
        Source::Outside { stored, .. } => vec![Segment {
            volume: 0,
            offset: 0,
            length: *stored,
        }],
    };
    let total = segments.iter().map(|s| s.length).sum();
    let mut stored = Stored {
        segments,
        open,
        stream: None,
        next: 0,
        left: 0,
        read: 0,
        total,
        obfuscated: plan.obfuscated,
        seed: 0,
        last_four: 0,
    };
    let path = plan.path.as_str();
    let mut sink = Sink {
        out,
        hash: plan.md5.map(|_| Md5::new()),
        produced: 0,
        size: plan.size,
        path,
    };
    if !plan.compressed {
        let mut buf = vec![0u8; CHUNK_LIMIT];
        while sink.produced < plan.size {
            let n = (plan.size - sink.produced).min(CHUNK_LIMIT as u64) as usize;
            if n as u64 > stored.remaining() {
                return invalid(format!(
                    "InstallShield member '{}' needs more than its {} stored bytes.",
                    path, total
                ));
            }
            stored.fill(&mut buf[..n], path)?;
            sink.put(&buf[..n])?;
        }
        if stored.remaining() != 0 {
            return invalid(format!(
                "InstallShield member '{}' has data left after its declared size of {} bytes.",
                path, plan.size
            ));
        }
    } else {
        match compression {
            Compression::Chunks => decode_chunks(&mut stored, &mut sink)?,
            Compression::Markers => decode_markers(&mut stored, &mut sink)?,
        }
    }
    if let (Some(hash), Some(md5)) = (sink.hash.take(), plan.md5)
        && hash.finalize().as_slice() != md5
    {
        return invalid(format!(
            "InstallShield member '{}' doesn't match the MD5 the cabinet header records.",
            path
        ));
    }
    Ok(sink.produced)
}

fn decode_chunks(stored: &mut Stored, sink: &mut Sink) -> Result<()> {
    let path = sink.path;
    let mut chunk = vec![0u8; u16::MAX as usize];
    let mut buf = vec![0u8; CHUNK_LIMIT + 1];
    let read_chunk_bytes = |stored: &mut Stored, buf: &mut [u8]| -> Result<()> {
        if buf.len() as u64 > stored.remaining() {
            return invalid(format!(
                "InstallShield member '{}' has a compressed chunk that runs past its {} stored bytes.{}",
                path, stored.total, CHUNKS_HINT
            ));
        }
        stored.fill(buf, path)
    };
    let mut next_chunk = |stored: &mut Stored, buf: &mut [u8]| -> Result<usize> {
        let mut prefix = [0u8; 2];
        read_chunk_bytes(stored, &mut prefix)?;
        let length = u16::from_le_bytes(prefix) as usize;
        if length == 0 {
            return invalid(format!(
                "InstallShield member '{}' has a compressed chunk of length zero.{}",
                path, CHUNKS_HINT
            ));
        }
        read_chunk_bytes(stored, &mut chunk[..length])?;
        let count = inflate_chunk(&chunk[..length], buf).map_err(|e| {
            Error::Invalid(format!(
                "InstallShield member '{}' has a chunk that doesn't inflate ({}).{}",
                path, e, CHUNKS_HINT
            ))
        })?;
        if count > CHUNK_LIMIT {
            return invalid(format!(
                "InstallShield member '{}' has a compressed chunk that expands past {} bytes.{}",
                path, CHUNK_LIMIT, CHUNKS_HINT
            ));
        }
        Ok(count)
    };
    while sink.produced < sink.size {
        if stored.remaining() == 0 {
            return invalid(format!(
                "InstallShield member '{}' ends after {} of its {} bytes.",
                path, sink.produced, sink.size
            ));
        }
        let count = next_chunk(stored, &mut buf)?;
        sink.put(&buf[..count])?;
    }
    // Chunks can follow the last byte (a member of no bytes can still hold
    // one); any that expand to something overrun it.
    while stored.remaining() > 0 {
        if next_chunk(stored, &mut buf)? != 0 {
            return invalid(format!(
                "InstallShield member '{}' expands past its declared size of {} bytes.",
                path, sink.size
            ));
        }
    }
    Ok(())
}

/// Inflates one raw deflate chunk into `out`, which is one byte longer
/// than a chunk may expand to. A chunk may end with a final block or
/// without one (after a flush), as a stream reader that stops at the end of
/// its input takes it.
fn inflate_chunk(input: &[u8], out: &mut [u8]) -> std::result::Result<usize, String> {
    let mut d = Decompress::new(false);
    loop {
        let (before_in, before_out) = (d.total_in(), d.total_out());
        let status = d
            .decompress(
                &input[before_in as usize..],
                &mut out[before_out as usize..],
                FlushDecompress::None,
            )
            .map_err(|e| e.to_string())?;
        let made = d.total_out() as usize;
        if status == Status::StreamEnd || made == out.len() {
            return Ok(made);
        }
        if d.total_in() == before_in && d.total_out() == before_out {
            return Ok(made);
        }
    }
}

fn decode_markers(stored: &mut Stored, sink: &mut Sink) -> Result<()> {
    let path = sink.path;
    let mut d = Decompress::new(false);
    let mut input = vec![0u8; CHUNK_LIMIT];
    let (mut start, mut end) = (0usize, 0usize);
    let mut out = vec![0u8; CHUNK_LIMIT];
    let mut ended = false;
    loop {
        if start == end && stored.remaining() > 0 {
            let n = stored.remaining().min(input.len() as u64) as usize;
            stored.fill(&mut input[..n], path)?;
            (start, end) = (0, n);
        }
        // Up to the declared size and one byte more.
        let want = (sink.size - sink.produced + 1).min(out.len() as u64) as usize;
        let (before_in, before_out) = (d.total_in(), d.total_out());
        let status = d.decompress(&input[start..end], &mut out[..want], FlushDecompress::None).map_err(|e| {
            Error::Invalid(format!(
                "InstallShield member '{}' has marker-delimited compressed data that doesn't inflate, within its first {} stored bytes ({}).",
                path, stored.read, e
            ))
        })?;
        let consumed = (d.total_in() - before_in) as usize;
        let made = (d.total_out() - before_out) as usize;
        start += consumed;
        sink.put(&out[..made])?;
        if status == Status::StreamEnd {
            ended = true;
            break;
        }
        if consumed == 0 && made == 0 && start == end && stored.remaining() == 0 {
            break;
        }
        if consumed == 0 && made == 0 && start < end {
            return invalid(format!(
                "InstallShield member '{}' has marker-delimited data the decoder stops in.",
                path
            ));
        }
    }
    if sink.produced < sink.size {
        return invalid(format!(
            "InstallShield member '{}' ends after {} of its {} bytes.",
            path, sink.produced, sink.size
        ));
    }
    // The form has no final block: a decoder that stopped on one left the
    // bytes after it undecoded.
    if ended {
        return invalid(format!(
            "InstallShield member '{}' has marker-delimited compressed data that ends with a final deflate block, which the form doesn't use.",
            path
        ));
    }
    if stored.total > 0 && (stored.total < 4 || stored.last_four != MARKER) {
        return invalid(format!(
            "InstallShield member '{}' has marker-delimited compressed data that doesn't end with the 00 00 FF FF marker.",
            path
        ));
    }
    Ok(())
}

/// Expand an InstallShield 3 member: `stored_size` bytes at `offset` of
/// `archive`, stored or imploded, to `size` bytes in `out`.
pub(crate) fn decode_archive(
    archive: &mut dyn ReadSeek,
    path: &str,
    offset: u64,
    stored_size: u64,
    size: u64,
    compressed: bool,
    out: &mut dyn Write,
) -> Result<u64> {
    archive.seek(SeekFrom::Start(offset))?;
    let mut input = Input::new(archive, stored_size);
    let mut sink = Sink {
        out,
        hash: None,
        produced: 0,
        size,
        path,
    };
    let short = |path: &str| {
        Error::Invalid(format!(
            "InstallShield 3 archive member '{}' ends early: the archive is shorter than when it was opened.",
            path
        ))
    };
    if !compressed {
        let mut buf = vec![0u8; CHUNK_LIMIT];
        while sink.produced < size {
            let n = (size - sink.produced).min(buf.len() as u64) as usize;
            match input.next_block(&mut buf[..n])? {
                0 => return Err(short(path)),
                got => sink.put(&buf[..got])?,
            }
        }
        return Ok(sink.produced);
    }
    let mut exploder = Exploder::new();
    let mut buf = vec![0u8; CHUNK_LIMIT];
    let wrap = |e: Error, produced: u64| match e {
        Error::Invalid(m) => Error::Invalid(format!(
            "InstallShield 3 archive member '{}' doesn't decode after {} bytes: {}",
            path, produced, m
        )),
        e => e,
    };
    while sink.produced < size {
        let n = (size - sink.produced).min(buf.len() as u64) as usize;
        let got = exploder
            .read(&mut input, &mut buf[..n])
            .map_err(|e| wrap(e, exploder.produced()))?;
        if got == 0 {
            return invalid(format!(
                "InstallShield 3 archive member '{}' ends after {} of its {} bytes.",
                path, sink.produced, size
            ));
        }
        sink.put(&buf[..got])?;
    }
    // The end code must follow the last byte, and the stored data must end
    // with it: bits left in its byte are padding.
    let mut one = [0u8; 1];
    if exploder
        .read(&mut input, &mut one)
        .map_err(|e| wrap(e, exploder.produced()))?
        != 0
    {
        return invalid(format!(
            "InstallShield 3 archive member '{}' expands past its declared size of {} bytes.",
            path, size
        ));
    }
    if input.next_byte()?.is_some() {
        return invalid(format!(
            "InstallShield 3 archive member '{}' has stored data left after its end code.",
            path
        ));
    }
    Ok(sink.produced)
}

/// An InstallShield 3 member's stored bytes, read ahead.
pub(crate) struct Input<'a> {
    archive: &'a mut dyn ReadSeek,
    left: u64,
    buf: Vec<u8>,
    start: usize,
    end: usize,
}

impl<'a> Input<'a> {
    /// The `left` bytes from where `archive` is.
    pub(crate) fn new(archive: &'a mut dyn ReadSeek, left: u64) -> Input<'a> {
        Input {
            archive,
            left,
            buf: vec![0u8; 16 * 1024],
            start: 0,
            end: 0,
        }
    }

    /// The next stored byte, or None at the end of the member's data.
    pub(crate) fn next_byte(&mut self) -> Result<Option<u8>> {
        if self.start == self.end {
            let n = self.left.min(self.buf.len() as u64) as usize;
            if n == 0 {
                return Ok(None);
            }
            let got = self.archive.read(&mut self.buf[..n])?;
            if got == 0 {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "the archive is shorter than when it was opened",
                )));
            }
            self.left -= got as u64;
            (self.start, self.end) = (0, got);
        }
        self.start += 1;
        Ok(Some(self.buf[self.start - 1]))
    }

    /// Up to `out.len()` stored bytes; 0 at the end.
    fn next_block(&mut self, out: &mut [u8]) -> Result<usize> {
        if self.start < self.end {
            let n = out.len().min(self.end - self.start);
            out[..n].copy_from_slice(&self.buf[self.start..self.start + n]);
            self.start += n;
            return Ok(n);
        }
        let n = self.left.min(out.len() as u64) as usize;
        if n == 0 {
            return Ok(0);
        }
        let got = self.archive.read(&mut out[..n])?;
        self.left -= got as u64;
        Ok(got)
    }
}
