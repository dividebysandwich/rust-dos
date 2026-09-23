//! Reader for the MOO test format, version 1.1
//! (https://github.com/dbalsom/moo/blob/main/doc/moo_format_v1.md).
//!
//! A MOO file is a sequence of chunks (4-byte ASCII id, u32 length, payload),
//! little-endian throughout: a `MOO ` header, optional `META` and file-wide
//! register masks (`RMSK`/`RM32`), then one `TEST` chunk per test. Unknown
//! chunks are skipped by their length, as the spec asks. The bus cycle
//! traces (`CYCL`) are not decoded.

/// Number of registers in the 32-bit register file (`RG32`).
pub const REG_COUNT: usize = 20;

// Register slots, in `RG32` bitmask order.
pub const CR0: usize = 0;
pub const CR3: usize = 1;
pub const EAX: usize = 2;
pub const EBX: usize = 3;
pub const ECX: usize = 4;
pub const EDX: usize = 5;
pub const ESI: usize = 6;
pub const EDI: usize = 7;
pub const EBP: usize = 8;
pub const ESP: usize = 9;
pub const CS: usize = 10;
pub const DS: usize = 11;
pub const ES: usize = 12;
pub const FS: usize = 13;
pub const GS: usize = 14;
pub const SS: usize = 15;
pub const EIP: usize = 16;
pub const EFLAGS: usize = 17;
pub const DR6: usize = 18;
pub const DR7: usize = 19;

pub const REG_NAMES: [&str; REG_COUNT] = [
    "cr0", "cr3", "eax", "ebx", "ecx", "edx", "esi", "edi", "ebp", "esp", "cs", "ds", "es", "fs",
    "gs", "ss", "eip", "eflags", "dr6", "dr7",
];

/// The 16-bit register file (`REGS`/`RMSK`) in bitmask order, mapped to
/// `RG32` slots.
const REGS16_SLOTS: [usize; 14] = [
    EAX, EBX, ECX, EDX, CS, SS, DS, ES, ESP, EBP, ESI, EDI, EIP, EFLAGS,
];

/// A register file, or a set of register masks: the registers named in
/// `present` have a value in `values`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Regs {
    pub present: u32,
    pub values: [u32; REG_COUNT],
}

impl Regs {
    pub fn get(&self, slot: usize) -> Option<u32> {
        (self.present & (1 << slot) != 0).then(|| self.values[slot])
    }

    fn set(&mut self, slot: usize, value: u32) {
        self.present |= 1 << slot;
        self.values[slot] = value;
    }
}

/// An exception or interrupt the test executed (`EXCP`).
#[derive(Clone, Copy, Debug)]
pub struct Exception {
    pub number: u8,
    /// Address of the FLAGS image pushed to the stack.
    pub flag_addr: u32,
}

/// One test, decoded into reusable buffers.
#[derive(Default)]
pub struct Test {
    pub index: u32,
    pub name: String,
    pub bytes: Vec<u8>,
    pub initial: Regs,
    pub initial_ram: Vec<(u32, u8)>,
    /// Only the registers the instruction changed.
    pub final_regs: Regs,
    /// Undefined-bit masks for this test only (1 = defined, compare).
    pub final_mask: Regs,
    /// The bytes the instruction changed.
    pub final_ram: Vec<(u32, u8)>,
    pub exception: Option<Exception>,
    pub hash: [u8; 20],
}

impl Test {
    fn clear(&mut self) {
        self.index = 0;
        self.name.clear();
        self.bytes.clear();
        self.initial = Regs::default();
        self.initial_ram.clear();
        self.final_regs = Regs::default();
        self.final_mask = Regs::default();
        self.final_ram.clear();
        self.exception = None;
        self.hash = [0; 20];
    }

    /// The register's value after the test: the final state lists only
    /// registers that changed.
    pub fn expected(&self, slot: usize) -> Option<u32> {
        self.final_regs.get(slot).or(self.initial.get(slot))
    }

    pub fn hash_hex(&self) -> String {
        self.hash.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn bytes_hex(&self) -> String {
        self.bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// File header (`MOO ` and `META`).
#[derive(Debug, Default)]
pub struct Header {
    pub version: (u8, u8),
    pub test_count: u32,
    pub cpu_id: String,
    pub mnemonic: String,
}

/// Walks the tests of a decompressed MOO file.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    pub header: Header,
    /// File-wide undefined-bit masks (1 = defined, compare).
    pub file_mask: Regs,
}

struct Chunk<'a> {
    id: [u8; 4],
    body: &'a [u8],
}

/// The chunk at `pos`, and the position after it.
fn chunk_at(buf: &[u8], pos: usize) -> Result<(Chunk<'_>, usize), String> {
    let header = buf
        .get(pos..pos + 8)
        .ok_or_else(|| format!("truncated chunk header at offset {pos}"))?;
    let id = [header[0], header[1], header[2], header[3]];
    let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    let body = buf.get(pos + 8..pos + 8 + len).ok_or_else(|| {
        format!(
            "chunk '{}' at offset {pos} runs past the end of the file",
            String::from_utf8_lossy(&id)
        )
    })?;
    Ok((Chunk { id, body }, pos + 8 + len))
}

/// Iterate over the chunks packed in `buf`.
fn for_each_chunk<'a>(
    buf: &'a [u8],
    mut f: impl FnMut(Chunk<'a>) -> Result<(), String>,
) -> Result<(), String> {
    let mut pos = 0;
    while pos < buf.len() {
        let (chunk, next) = chunk_at(buf, pos)?;
        f(chunk)?;
        pos = next;
    }
    Ok(())
}

fn u32_at(buf: &[u8], pos: usize) -> Result<u32, String> {
    buf.get(pos..pos + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| format!("truncated field at offset {pos}"))
}

fn u16_at(buf: &[u8], pos: usize) -> Result<u16, String> {
    buf.get(pos..pos + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| format!("truncated field at offset {pos}"))
}

/// Length-prefixed payload of `NAME` and `BYTS`.
fn counted(body: &[u8]) -> Result<&[u8], String> {
    let n = u32_at(body, 0)? as usize;
    body.get(4..4 + n)
        .ok_or_else(|| "truncated NAME/BYTS chunk".to_string())
}

/// Decode a `RG32`/`RM32` (32-bit) or `REGS`/`RMSK` (16-bit) chunk into
/// `regs`, on top of what it already holds.
fn read_regs(id: &[u8; 4], body: &[u8], regs: &mut Regs) -> Result<(), String> {
    match id {
        b"RG32" | b"RM32" => {
            let mask = u32_at(body, 0)?;
            let mut pos = 4;
            for slot in 0..REG_COUNT {
                if mask & (1 << slot) != 0 {
                    regs.set(slot, u32_at(body, pos)?);
                    pos += 4;
                }
            }
        }
        _ => {
            let mask = u16_at(body, 0)?;
            let mut pos = 2;
            for (bit, &slot) in REGS16_SLOTS.iter().enumerate() {
                if mask & (1 << bit) != 0 {
                    regs.set(slot, u16_at(body, pos)? as u32);
                    pos += 2;
                }
            }
        }
    }
    Ok(())
}

fn read_ram(body: &[u8], ram: &mut Vec<(u32, u8)>) -> Result<(), String> {
    let n = u32_at(body, 0)? as usize;
    let entries = body
        .get(4..4 + n * 5)
        .ok_or_else(|| "truncated RAM chunk".to_string())?;
    ram.extend(
        entries
            .chunks_exact(5)
            .map(|e| (u32::from_le_bytes([e[0], e[1], e[2], e[3]]), e[4])),
    );
    Ok(())
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Result<Self, String> {
        let (chunk, pos) = chunk_at(buf, 0)?;
        if &chunk.id != b"MOO " {
            return Err("not a MOO file (no 'MOO ' header chunk)".into());
        }
        let b = chunk.body;
        if b.len() < 12 {
            return Err("MOO header chunk too short".into());
        }
        let header = Header {
            version: (b[0], b[1]),
            test_count: u32_at(b, 4)?,
            cpu_id: String::from_utf8_lossy(&b[8..12]).trim().to_string(),
            mnemonic: String::new(),
        };
        Ok(Self {
            buf,
            pos,
            header,
            file_mask: Regs::default(),
        })
    }

    /// Decode the next test into `test`. Returns false after the last one.
    /// File-level chunks met on the way (`META`, masks) update the header
    /// and `file_mask`.
    pub fn next_into(&mut self, test: &mut Test) -> Result<bool, String> {
        while self.pos < self.buf.len() {
            let (chunk, next) = chunk_at(self.buf, self.pos)?;
            self.pos = next;
            match &chunk.id {
                b"TEST" => {
                    read_test(chunk.body, test)?;
                    return Ok(true);
                }
                b"META" => {
                    if let Some(m) = chunk.body.get(7..15) {
                        self.header.mnemonic = String::from_utf8_lossy(m).trim().to_string();
                    }
                }
                id @ (b"RM32" | b"RMSK") => read_regs(id, chunk.body, &mut self.file_mask)?,
                _ => {}
            }
        }
        Ok(false)
    }
}

fn read_test(body: &[u8], test: &mut Test) -> Result<(), String> {
    test.clear();
    test.index = u32_at(body, 0)?;
    let mut have_hash = false;
    for_each_chunk(&body[4..], |chunk| {
        match &chunk.id {
            b"NAME" => test
                .name
                .push_str(&String::from_utf8_lossy(counted(chunk.body)?)),
            b"BYTS" => test.bytes.extend_from_slice(counted(chunk.body)?),
            b"INIT" => for_each_chunk(chunk.body, |sub| {
                match &sub.id {
                    id @ (b"RG32" | b"REGS") => read_regs(id, sub.body, &mut test.initial)?,
                    b"RAM " => read_ram(sub.body, &mut test.initial_ram)?,
                    _ => {}
                }
                Ok(())
            })?,
            b"FINA" => for_each_chunk(chunk.body, |sub| {
                match &sub.id {
                    id @ (b"RG32" | b"REGS") => read_regs(id, sub.body, &mut test.final_regs)?,
                    id @ (b"RM32" | b"RMSK") => read_regs(id, sub.body, &mut test.final_mask)?,
                    b"RAM " => read_ram(sub.body, &mut test.final_ram)?,
                    _ => {}
                }
                Ok(())
            })?,
            b"EXCP" => {
                let number = *chunk.body.first().ok_or("empty EXCP chunk")?;
                test.exception = Some(Exception {
                    number,
                    flag_addr: u32_at(chunk.body, 1)?,
                });
            }
            b"HASH" => {
                let h = chunk.body.get(..20).ok_or("HASH chunk too short")?;
                test.hash.copy_from_slice(h);
                have_hash = true;
            }
            _ => {}
        }
        Ok(())
    })?;
    if !have_hash {
        return Err(format!("test {} has no HASH chunk", test.index));
    }
    Ok(())
}

/// Parse a 40-digit hex SHA-1 string.
pub fn parse_hash(hex: &str) -> Option<[u8; 20]> {
    let hex = hex.trim();
    if hex.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}
