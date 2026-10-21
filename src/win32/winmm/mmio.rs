//! Multimedia files (mmio): buffered reading and writing of DOS files,
//! memory files and files of the program's own I/O procedures, and the
//! RIFF chunks they hold (mmioDescend, mmioAscend, mmioCreateChunk).
//!
//! The buffering follows Wine's mmio.c, which its tests check against
//! Windows: an MMIOINFO's buffer, its pointers and offsets, as
//! mmioGetInfo shows them and mmioAdvance moves them.
//!
//! A file's I/O procedure is the built-in DOS or memory one, or one of
//! the program's (mmioInstallIOProcA, or pIOProc in mmioOpenA's
//! MMIOINFO), which takes the messages in a call into the program. A
//! file of the program's procedure opened for reading is read whole
//! when it opens, through its MMIOM_READ, and is read from that copy
//! afterwards; one opened for writing gets each read, write and seek as
//! a message, unbuffered.

use super::*;
use crate::cpu::Cpu;
use crate::win32::task::{Next, Task};

/// FOURCC_DOS and FOURCC_MEM.
pub const FOURCC_DOS: u32 = u32::from_le_bytes(*b"DOS ");
pub const FOURCC_MEM: u32 = u32::from_le_bytes(*b"MEM ");
const FOURCC_RIFF: u32 = u32::from_le_bytes(*b"RIFF");
const FOURCC_LIST: u32 = u32::from_le_bytes(*b"LIST");

/// mmioOpenA's and the others' flags.
const MMIO_RWMODE: u32 = 0x0003;
const MMIO_READ: u32 = 0x0000;
const MMIO_WRITE: u32 = 0x0001;
const MMIO_READWRITE: u32 = 0x0002;
const MMIO_SHAREMODE: u32 = 0x0070;
const MMIO_DENYWRITE: u32 = 0x0020;
const MMIO_DENYREAD: u32 = 0x0030;
const MMIO_DENYNONE: u32 = 0x0040;
const MMIO_EXCLUSIVE: u32 = 0x0010;
const MMIO_CREATE: u32 = 0x1000;
const MMIO_PARSE: u32 = 0x0100;
const MMIO_DELETE: u32 = 0x0200;
const MMIO_EXIST: u32 = 0x4000;
const MMIO_ALLOCBUF: u32 = 0x0001_0000;
const MMIO_GETTEMP: u32 = 0x0002_0000;
const MMIO_DIRTY: u32 = 0x1000_0000;
/// mmioClose's: the file handle stays open.
const MMIO_FHOPEN: u32 = 0x0010;
/// mmioFlush's: the buffer empties.
const MMIO_EMPTYBUF: u32 = 0x0010;
/// mmioStringToFOURCCA's.
const MMIO_TOUPPER: u32 = 0x0010;
/// mmioDescend's.
const MMIO_FINDCHUNK: u32 = 0x0010;
const MMIO_FINDRIFF: u32 = 0x0020;
const MMIO_FINDLIST: u32 = 0x0040;
/// mmioCreateChunk's.
const MMIO_CREATERIFF: u32 = 0x0020;
const MMIO_CREATELIST: u32 = 0x0040;
/// mmioInstallIOProcA's.
const MMIO_INSTALLPROC: u32 = 0x0001_0000;
const MMIO_REMOVEPROC: u32 = 0x0002_0000;
const MMIO_FINDPROC: u32 = 0x0004_0000;
/// The size of the buffer MMIO_ALLOCBUF makes when none is asked for.
const MMIO_DEFAULTBUFFER: u32 = 8192;

/// The I/O procedures' messages.
const MMIOM_READ: u32 = 0;
const MMIOM_WRITE: u32 = 1;
const MMIOM_SEEK: u32 = 2;
const MMIOM_OPEN: u32 = 3;
const MMIOM_CLOSE: u32 = 4;
const MMIOM_RENAME: u32 = 6;
const MMIOM_USER: u32 = 0x8000;

/// The errors.
const MMIOERR_FILENOTFOUND: u32 = 257;
const MMIOERR_OUTOFMEMORY: u32 = 258;
const MMIOERR_CANNOTOPEN: u32 = 259;
const MMIOERR_CANNOTREAD: u32 = 261;
const MMIOERR_CANNOTWRITE: u32 = 262;
const MMIOERR_CANNOTSEEK: u32 = 263;
const MMIOERR_CHUNKNOTFOUND: u32 = 265;
const MMIOERR_UNBUFFERED: u32 = 266;
const MMIOERR_PATHNOTFOUND: u32 = 267;
const MMIOERR_ACCESSDENIED: u32 = 268;
const MMIOERR_SHARINGVIOLATION: u32 = 269;

/// MMIOINFO's size and the offsets of its fields.
const INFO_SIZE: u32 = 72;
const I_FLAGS: u32 = 0;
const I_FCC: u32 = 4;
const I_PROC: u32 = 8;
const I_ERROR: u32 = 12;
const I_CCH: u32 = 20;
const I_BUFFER: u32 = 24;
const I_NEXT: u32 = 28;
const I_END_READ: u32 = 32;
const I_END_WRITE: u32 = 36;
const I_DISK_OFFSET: u32 = 44;
const I_ADW_INFO: u32 = 48;

/// The bytes a program's procedure is asked for at a time while its
/// file is read whole.
const SLURP: u32 = 65_536;

/// What does a file's I/O.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Proc {
    /// The memory file the buffer is.
    #[default]
    Mem,
    /// A DOS file, by its handle (which adwInfo[0] has too).
    Dos { file: u32 },
    /// A file of the program's procedure read whole when it opened, and
    /// the place in it the procedure is at.
    Data { data: Vec<u8>, pos: u32 },
    /// A file of the program's procedure that gets each call as a
    /// message.
    Program,
}

/// An open multimedia file: its MMIOINFO's fields, and the rest.
#[derive(Clone, Debug, Default)]
pub struct File {
    pub hmmio: u32,
    pub pid: u32,
    pub flags: u32,
    pub fcc: u32,
    pub io_proc: u32,
    pub cch: u32,
    pub buffer: u32,
    pub next: u32,
    pub end_read: u32,
    pub end_write: u32,
    pub buf_offset: u32,
    pub disk_offset: u32,
    pub adw_info: [u32; 3],
    /// Whether the buffer has the data from `buf_offset` on.
    pub loaded: bool,
    /// The file's size, for seeks past its end.
    pub size: u32,
    pub proc: Proc,
    /// For a file of the program's procedure: the block of the heap
    /// with the MMIOINFO it is passed, and room for what it reads.
    pub block: u32,
    /// What the procedure read of the file so far, as it opens.
    pub slurp: Vec<u8>,
}

/// A procedure mmioInstallIOProcA installed.
#[derive(Clone, Debug, Default)]
pub struct Installed {
    pub pid: u32,
    pub fcc: u32,
    pub proc: u32,
}

/// The multimedia files' state.
#[derive(Clone, Debug, Default)]
pub struct Mmio {
    pub files: Vec<File>,
    pub procs: Vec<Installed>,
}

fn mmio(cpu: &mut Cpu) -> &mut Mmio {
    &mut cpu.bus.win32.winmm.mmio
}

fn find(cpu: &Cpu, hmmio: u32) -> Option<usize> {
    cpu.bus
        .win32
        .winmm
        .mmio
        .files
        .iter()
        .position(|f| f.hmmio == hmmio && hmmio != 0)
}

/// Take the file `i` out of the list to work on it, and put it back.
fn with_file<T>(cpu: &mut Cpu, i: usize, f: impl FnOnce(&mut Cpu, &mut File) -> T) -> T {
    let mut file = std::mem::take(&mut mmio(cpu).files[i]);
    let result = f(cpu, &mut file);
    mmio(cpu).files[i] = file;
    result
}

/// A block of the process heap, or 0.
fn heap_alloc(cpu: &mut Cpu, size: u32) -> u32 {
    crate::win32::with_process(cpu, |p, bus| p.heap.alloc(&mut p.memory, bus, size)).unwrap_or(0)
}

fn heap_free(cpu: &mut Cpu, at: u32) {
    crate::win32::process(cpu).heap.free(at);
}

/// The DOS file's error as an MMIOERR.
fn open_error(error: u32) -> u32 {
    match error {
        2 => MMIOERR_FILENOTFOUND,
        3 => MMIOERR_PATHNOTFOUND,
        5 => MMIOERR_ACCESSDENIED,
        32 => MMIOERR_SHARINGVIOLATION,
        _ => MMIOERR_CANNOTOPEN,
    }
}

impl File {
    /// The I/O procedure reads up to `n` bytes to `to` in the program's
    /// memory: the count, or -1.
    fn proc_read(&mut self, cpu: &mut Cpu, to: u32, n: u32) -> i32 {
        // Before anything is read: `n` comes from the program's buffer.
        if !cpu.bus.guest_probe(to, n as usize, true) {
            return -1;
        }
        let bytes = match &mut self.proc {
            Proc::Mem | Proc::Program => return 0,
            Proc::Dos { file } => match crate::win32::kernel32::read_file(cpu, *file, n) {
                Ok(b) => b,
                Err(_) => return -1,
            },
            Proc::Data { data, pos } => {
                let start = (*pos as usize).min(data.len());
                let end = (start + n as usize).min(data.len());
                *pos = end as u32;
                data[start..end].to_vec()
            }
        };
        cpu.bus.guest_write_bytes(to, &bytes);
        self.disk_offset += bytes.len() as u32;
        bytes.len() as i32
    }

    /// The I/O procedure writes `n` bytes from `from`: the count, or -1.
    fn proc_write(&mut self, cpu: &mut Cpu, from: u32, n: u32) -> i32 {
        let Proc::Dos { file } = self.proc else {
            return if self.proc == Proc::Mem { 0 } else { -1 };
        };
        if !cpu.bus.guest_probe(from, n as usize, false) {
            return -1;
        }
        let mut bytes = vec![0; n as usize];
        cpu.bus.guest_read_bytes(from, &mut bytes);
        match crate::win32::kernel32::write_file(cpu, file, &bytes) {
            Ok(done) => {
                self.disk_offset += done;
                self.size = self.size.max(self.disk_offset);
                done as i32
            }
            Err(_) => -1,
        }
    }

    /// The I/O procedure seeks: the new place, or -1.
    fn proc_seek(&mut self, cpu: &mut Cpu, offset: i32, origin: u32) -> i32 {
        let at = match &mut self.proc {
            Proc::Mem | Proc::Program => return -1,
            Proc::Dos { file } => {
                match crate::win32::kernel32::seek_file(cpu, *file, offset as i64, origin) {
                    Ok(at) => at,
                    Err(_) => return -1,
                }
            }
            Proc::Data { data, pos } => {
                let base = match origin {
                    0 => 0,
                    1 => *pos as i64,
                    2 => data.len() as i64,
                    _ => return -1,
                };
                let to = base + offset as i64;
                if to < 0 {
                    return -1;
                }
                *pos = to as u32;
                to as u32
            }
        };
        self.disk_offset = at;
        at as i32
    }

    /// Write what the buffer has that the file doesn't (MMIO_Flush).
    fn flush(&mut self, cpu: &mut Cpu, flags: u32) -> u32 {
        if self.cch != 0 && self.fcc != FOURCC_MEM {
            if self.flags & MMIO_DIRTY != 0 {
                self.proc_seek(cpu, self.buf_offset as i32, 0);
                let n = self.next.wrapping_sub(self.buffer);
                self.proc_write(cpu, self.buffer, n);
            }
            if flags & MMIO_EMPTYBUF != 0 {
                self.next = self.buffer;
                self.end_read = self.buffer;
            }
        }
        self.flags &= !MMIO_DIRTY;
        MMSYSERR_NOERROR
    }

    /// Use `buffer` of `cch` bytes, or one of the heap's (MMIO_SetBuffer).
    fn set_buffer(&mut self, cpu: &mut Cpu, buffer: u32, cch: u32) -> u32 {
        if self.flush(cpu, 0) != MMSYSERR_NOERROR {
            return MMIOERR_CANNOTWRITE;
        }
        if self.flags & MMIO_ALLOCBUF != 0 {
            if self.buffer != 0 {
                heap_free(cpu, self.buffer);
            }
            self.buffer = 0;
            self.flags &= !MMIO_ALLOCBUF;
        }
        if buffer != 0 {
            self.buffer = buffer;
        } else if cch != 0 {
            let at = heap_alloc(cpu, cch);
            if at == 0 {
                return MMIOERR_OUTOFMEMORY;
            }
            self.buffer = at;
            self.flags |= MMIO_ALLOCBUF;
        } else {
            self.buffer = 0;
        }
        self.cch = cch;
        self.next = self.buffer;
        self.end_read = self.buffer;
        self.end_write = self.buffer.wrapping_add(cch);
        self.buf_offset = self.disk_offset;
        self.loaded = false;
        MMSYSERR_NOERROR
    }

    /// Load the buffer from where the file is (MMIO_GrabNextBuffer):
    /// what was read.
    fn grab_next(&mut self, cpu: &mut Cpu, for_read: bool) -> i32 {
        self.buf_offset = self.disk_offset;
        self.next = self.buffer;
        self.end_read = self.buffer;
        self.end_write = self.buffer.wrapping_add(self.cch);
        self.loaded = true;
        if !for_read {
            return self.cch as i32;
        }
        let size = self.proc_read(cpu, self.buffer, self.cch);
        if size > 0 {
            self.end_read = self.end_read.wrapping_add(size as u32);
        } else {
            self.loaded = false;
        }
        size
    }

    /// mmioRead's work: up to `n` bytes to `to`.
    fn read(&mut self, cpu: &mut Cpu, mut to: u32, mut n: u32) -> i32 {
        if self.buffer == 0 {
            return self.proc_read(cpu, to, n);
        }
        let mut count = 0u32;
        if self.next != self.end_read {
            let mut have = self.end_read.wrapping_sub(self.next) as i32;
            if have as i64 > n as i64 || have < 0 {
                have = n as i32;
            }
            let have = have as u32;
            copy(cpu, self.next, to, have);
            self.next = self.next.wrapping_add(have);
            to = to.wrapping_add(have);
            n -= have;
            count = have;
        }
        if n > 0 && self.fcc != FOURCC_MEM && self.cch > 0 {
            while n > 0 {
                let size = self.grab_next(cpu, true);
                if size <= 0 {
                    break;
                }
                let size = (size as u32).min(n);
                copy(cpu, self.buffer, to, size);
                self.next = self.next.wrapping_add(size);
                to = to.wrapping_add(size);
                n -= size;
                count += size;
            }
        }
        count as i32
    }

    /// mmioWrite's work: `n` bytes from `from`.
    fn write(&mut self, cpu: &mut Cpu, mut from: u32, mut n: u32) -> i32 {
        if self.cch == 0 {
            let count = self.proc_write(cpu, from, n);
            self.buf_offset = self.disk_offset;
            return count;
        }
        let mut written = 0u32;
        while n > 0 {
            if self.next != self.end_write {
                let mut room = self.end_write.wrapping_sub(self.next) as i32;
                if room as i64 > n as i64 || room < 0 {
                    room = n as i32;
                }
                let room = room as u32;
                copy(cpu, from, self.next, room);
                self.next = self.next.wrapping_add(room);
                from = from.wrapping_add(room);
                n -= room;
                written += room;
                self.flags |= MMIO_DIRTY;
            } else if self.fcc == FOURCC_MEM {
                // A memory file doesn't grow.
                break;
            }
            if self.next == self.end_write {
                if self.fcc == FOURCC_MEM {
                    break;
                }
                self.flush(cpu, MMIO_EMPTYBUF);
                self.grab_next(cpu, false);
            } else {
                break;
            }
        }
        written as i32
    }

    /// mmioSeek's work: the new place, or -1.
    fn seek(&mut self, cpu: &mut Cpu, offset: i32, origin: u32) -> i32 {
        if self.buffer == 0 {
            return self.proc_seek(cpu, offset, origin);
        }
        let to = match origin {
            0 => offset as i64,
            1 => {
                self.buf_offset as i64 + self.next.wrapping_sub(self.buffer) as i64 + offset as i64
            }
            2 => {
                let end = if self.fcc == FOURCC_MEM {
                    self.cch
                } else {
                    self.size
                };
                end as i64 + offset as i64
            }
            _ => return -1,
        };
        if to < 0 {
            return -1;
        }
        let to = to as u32;
        if to != 0 && to >= self.size && self.fcc != FOURCC_MEM {
            self.flush(cpu, 0);
            self.buf_offset = to;
            self.end_read = self.buffer;
            self.end_write = self.buffer.wrapping_add(self.cch);
            if self.flags & MMIO_RWMODE == MMIO_READ {
                self.disk_offset = self.size;
            }
        } else if self.cch > 0
            && (to < self.buf_offset || to >= self.buf_offset + self.cch || !self.loaded)
        {
            if self.fcc == FOURCC_MEM {
                return -1;
            }
            self.flush(cpu, 0);
            let base = to / self.cch * self.cch;
            if self.proc_seek(cpu, base as i32, 0) == -1 {
                return -1;
            }
            self.grab_next(cpu, true);
        }
        self.next = self.buffer.wrapping_add(to - self.buf_offset);
        to as i32
    }

    /// Write its MMIOINFO to `at`.
    fn write_info(&self, cpu: &mut Cpu, at: u32) {
        let words = [
            self.flags,
            self.fcc,
            self.io_proc,
            0,
            0,
            self.cch,
            self.buffer,
            self.next,
            self.end_read,
            self.end_write,
            self.buf_offset,
            self.disk_offset,
            self.adw_info[0],
            self.adw_info[1],
            self.adw_info[2],
            0,
            0,
            self.hmmio,
        ];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        cpu.bus.guest_write_bytes(at, &bytes);
    }

    /// Take the fields a program's procedure may change from its
    /// MMIOINFO block.
    fn read_info(&mut self, cpu: &mut Cpu, at: u32) {
        self.disk_offset = cpu.bus.guest_read_32(at + I_DISK_OFFSET);
        for k in 0..3 {
            self.adw_info[k] = cpu.bus.guest_read_32(at + I_ADW_INFO + 4 * k as u32);
        }
    }
}

/// Copy `n` bytes of the program's memory from `from` to `to`.
fn copy(cpu: &mut Cpu, from: u32, to: u32, n: u32) {
    let mut bytes = vec![0; n as usize];
    cpu.bus.guest_read_bytes(from, &mut bytes);
    cpu.bus.guest_write_bytes(to, &bytes);
}

/// The procedure of `fcc` the process `pid` installed, if it did.
fn installed(cpu: &Cpu, pid: u32, fcc: u32) -> Option<u32> {
    cpu.bus
        .win32
        .winmm
        .mmio
        .procs
        .iter()
        .rev()
        .find(|p| p.pid == pid && p.fcc == fcc)
        .map(|p| p.proc)
}

/// The FOURCC of the procedure a name such as "GAME.PAK+INTRO.WAV" asks
/// for: the extension before the '+', in capitals.
fn parse_extension(name: &str) -> Option<u32> {
    let (before, _) = name.split_once('+')?;
    let ext = before.rsplit_once('.')?.1;
    if ext.is_empty() || ext.len() > 4 || ext.contains(['\\', '/']) {
        return None;
    }
    let mut cc = [b' '; 4];
    for (i, b) in ext.bytes().enumerate() {
        cc[i] = b.to_ascii_uppercase();
    }
    Some(u32::from_le_bytes(cc))
}

/// mmioOpenA(name, info, flags).
pub fn mmio_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name_at, info, flags] = args else {
        unreachable!()
    };
    let name = if name_at == 0 {
        None
    } else {
        Some(read_string(cpu, name_at, 260)?)
    };
    if info != 0 && !cpu.bus.guest_probe(info, INFO_SIZE as usize, true) {
        return None;
    }
    if flags & (MMIO_PARSE | MMIO_EXIST) != 0 {
        let Some(name) = name else {
            return Some(FALSE);
        };
        let full = crate::win32::kernel32::full_path(cpu, &name);
        if flags & MMIO_EXIST != 0 {
            let dos = crate::win32::kernel32::dos_path(&full);
            if !cpu.bus.disk.exists(&dos) {
                return Some(FALSE);
            }
        }
        if full.len() >= 128 {
            return Some(FALSE);
        }
        let mut bytes = full.into_bytes();
        bytes.push(0);
        cpu.bus.guest_write_bytes(name_at, &bytes);
        return Some(TRUE);
    }
    if flags & MMIO_GETTEMP != 0 {
        return Some(FALSE);
    }
    let read = |cpu: &mut Cpu, off: u32| {
        if info == 0 {
            0
        } else {
            cpu.bus.guest_read_32(info + off)
        }
    };
    let (mut fcc, mut io_proc) = (read(cpu, I_FCC), read(cpu, I_PROC));
    let (cch, buffer, info0) = (read(cpu, I_CCH), read(cpu, I_BUFFER), read(cpu, I_ADW_INFO));
    let pid = crate::win32::user32::current_pid(cpu);
    if fcc == 0 && io_proc == 0 {
        fcc = name
            .as_deref()
            .and_then(parse_extension)
            .unwrap_or(FOURCC_DOS);
        if fcc != FOURCC_DOS && fcc != FOURCC_MEM {
            match installed(cpu, pid, fcc) {
                Some(p) => io_proc = p,
                None => fcc = FOURCC_DOS,
            }
        }
    } else if io_proc == 0 && fcc != FOURCC_DOS && fcc != FOURCC_MEM {
        match installed(cpu, pid, fcc) {
            Some(p) => io_proc = p,
            None => {
                set_error_ret(cpu, info, MMIOERR_CANNOTOPEN);
                return Some(0);
            }
        }
    }
    let hmmio = new_handle(cpu);
    let mut file = File {
        hmmio,
        pid,
        flags,
        fcc,
        io_proc,
        ..File::default()
    };
    let cch = if flags & MMIO_ALLOCBUF != 0 && cch == 0 {
        MMIO_DEFAULTBUFFER
    } else {
        cch
    };
    if io_proc != 0 {
        file.adw_info[0] = info0;
        return open_program(cpu, file, name_at, info, buffer, cch);
    }
    let error = file.set_buffer(cpu, buffer, cch);
    if error != MMSYSERR_NOERROR {
        set_error_ret(cpu, info, error);
        return Some(0);
    }
    if fcc == FOURCC_MEM && file.flags & MMIO_ALLOCBUF == 0 {
        file.loaded = true;
    }
    file.adw_info[0] = info0;
    let error = if fcc == FOURCC_MEM {
        // The memory is all there to read, unless the file is new.
        if flags & MMIO_CREATE == 0 {
            file.end_read = file.end_write;
        }
        MMSYSERR_NOERROR
    } else {
        open_dos(cpu, &mut file, name.as_deref())
    };
    set_error_ret(cpu, info, error);
    if error != MMSYSERR_NOERROR {
        if file.flags & MMIO_ALLOCBUF != 0 {
            heap_free(cpu, file.buffer);
        }
        return Some(0);
    }
    mmio(cpu).files.push(file);
    Some(hmmio)
}

/// Set wErrorRet of the MMIOINFO at `info`, if there is one.
fn set_error_ret(cpu: &mut Cpu, info: u32, error: u32) {
    if info != 0 {
        cpu.bus.guest_write_32(info + I_ERROR, error);
    }
}

/// Open the DOS file `name` (or use the handle adwInfo[0] has, for no
/// name) as the DOS procedure's MMIOM_OPEN does.
fn open_dos(cpu: &mut Cpu, file: &mut File, name: Option<&str>) -> u32 {
    use crate::win32::kernel32;
    let handle = match name {
        Some(name) => {
            if file.flags & MMIO_DELETE != 0 {
                let dos = kernel32::dos_path(&kernel32::full_path(cpu, name));
                return match cpu.bus.disk.delete_file(&dos) {
                    Ok(()) => MMSYSERR_NOERROR,
                    Err(e) => open_error(e as u32),
                };
            }
            let access = match file.flags & MMIO_RWMODE {
                MMIO_WRITE => 0x4000_0000,
                MMIO_READWRITE => 0xC000_0000,
                _ => 0x8000_0000,
            };
            let share = match file.flags & MMIO_SHAREMODE {
                MMIO_EXCLUSIVE => 0,
                MMIO_DENYWRITE => 1,
                MMIO_DENYREAD => 2,
                MMIO_DENYNONE => 3,
                _ => 3,
            };
            let disposition = if file.flags & MMIO_CREATE != 0 { 2 } else { 3 };
            match kernel32::open_handle(cpu, name, access, share, disposition) {
                Ok(h) => h,
                Err(e) => return open_error(e),
            }
        }
        None if file.adw_info[0] != 0 => file.adw_info[0],
        None => return MMIOERR_CANNOTOPEN,
    };
    file.adw_info[0] = handle;
    file.proc = Proc::Dos { file: handle };
    let here = kernel32::seek_file(cpu, handle, 0, 1).unwrap_or(0);
    file.size = kernel32::seek_file(cpu, handle, 0, 2).unwrap_or(0);
    let _ = kernel32::seek_file(cpu, handle, here as i64, 0);
    file.disk_offset = here;
    file.buf_offset = here;
    MMSYSERR_NOERROR
}

/// The variables of a task of mmio (`step`): what it does, the file,
/// the MMIOINFO of mmioOpenA, and the call's arguments.
const T_OP: usize = 0;
const T_FILE: usize = 1;
const T_INFO: usize = 2;
const T_A: usize = 3;
const T_B: usize = 4;
const T_C: usize = 5;

/// What a task of mmio does.
const OP_OPEN: u32 = 0;
const OP_CLOSE: u32 = 1;
const OP_CALL: u32 = 2;
const OP_RENAME: u32 = 3;

/// The most of a file of the program's procedure that is read at its
/// opening: 64 MiB.
const SLURP_MAX: usize = 64 << 20;

/// Open a file of the program's procedure: a task that calls it with
/// MMIOM_OPEN, then, for reading, reads all of it.
fn open_program(
    cpu: &mut Cpu,
    mut file: File,
    name_at: u32,
    info: u32,
    buffer: u32,
    cch: u32,
) -> Option<u32> {
    // The block holds the procedure's MMIOINFO, then room for what it
    // reads.
    let block = heap_alloc(cpu, INFO_SIZE + SLURP);
    if block == 0 {
        set_error_ret(cpu, info, MMIOERR_OUTOFMEMORY);
        return Some(0);
    }
    file.block = block;
    file.proc = Proc::Program;
    file.write_info(cpu, block);
    let hmmio = file.hmmio;
    mmio(cpu).files.push(file);
    let vars = vec![OP_OPEN, hmmio, info, name_at, buffer, cch];
    crate::win32::task::start(cpu, crate::win32::task::MMIO, vars, 0)
}

/// The steps of mmio's tasks: calls of a program's I/O procedure.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let hmmio = t.var(T_FILE);
    let Some(i) = find(cpu, hmmio) else {
        return Next::Done(MMSYSERR_INVALHANDLE);
    };
    let (proc, block) = {
        let f = &mmio(cpu).files[i];
        (f.io_proc, f.block)
    };
    let (a, b, c) = (t.var(T_A), t.var(T_B), t.var(T_C));
    match (t.var(T_OP), t.step) {
        (OP_OPEN, 0) => Next::call(proc, &[block, MMIOM_OPEN, a, 0], 1),
        (OP_OPEN, 1) => {
            with_file(cpu, i, |cpu, f| f.read_info(cpu, block));
            if value != 0 {
                set_error_ret(cpu, t.var(T_INFO), value);
                close_file(cpu, i);
                return Next::Done(0);
            }
            if mmio(cpu).files[i].flags & MMIO_RWMODE != MMIO_READ {
                set_error_ret(cpu, t.var(T_INFO), MMSYSERR_NOERROR);
                return Next::Done(hmmio);
            }
            Next::call(proc, &[block, MMIOM_READ, block + INFO_SIZE, SLURP], 2)
        }
        (OP_OPEN, _) => {
            let n = (value as i32).clamp(0, SLURP as i32) as usize;
            if n > 0 && mmio(cpu).files[i].slurp.len() < SLURP_MAX {
                let mut bytes = vec![0; n];
                cpu.bus.guest_read_bytes(block + INFO_SIZE, &mut bytes);
                mmio(cpu).files[i].slurp.extend_from_slice(&bytes);
                return Next::call(proc, &[block, MMIOM_READ, block + INFO_SIZE, SLURP], 2);
            }
            finish_slurp(cpu, i, t.var(T_INFO), b, c)
        }
        (OP_CLOSE, 0) => Next::call(proc, &[block, MMIOM_CLOSE, a, 0], 1),
        (OP_RENAME, 0) => Next::call(proc, &[block, MMIOM_RENAME, a, b], 1),
        (OP_CLOSE | OP_RENAME, _) => {
            close_file(cpu, i);
            Next::Done(value)
        }
        (_, 0) => Next::call(proc, &[block, a, b, c], 1),
        _ => {
            with_file(cpu, i, |cpu, f| f.read_info(cpu, block));
            Next::Done(value)
        }
    }
}

/// The file of the program's procedure is read whole: it is read from
/// the copy from now on, with the buffer mmioOpenA was asked for.
fn finish_slurp(cpu: &mut Cpu, i: usize, info: u32, buffer: u32, cch: u32) -> Next {
    let hmmio = mmio(cpu).files[i].hmmio;
    let error = with_file(cpu, i, |cpu, f| {
        let data = std::mem::take(&mut f.slurp);
        f.size = data.len() as u32;
        f.disk_offset = 0;
        f.proc = Proc::Data { data, pos: 0 };
        f.set_buffer(cpu, buffer, cch)
    });
    set_error_ret(cpu, info, error);
    if error != MMSYSERR_NOERROR {
        close_file(cpu, i);
        return Next::Done(0);
    }
    Next::Done(hmmio)
}

/// Let go of the file `i`: its buffer, and the block of its procedure.
fn close_file(cpu: &mut Cpu, i: usize) {
    let f = mmio(cpu).files.remove(i);
    if f.flags & MMIO_ALLOCBUF != 0 && f.buffer != 0 {
        heap_free(cpu, f.buffer);
    }
    if f.block != 0 {
        heap_free(cpu, f.block);
    }
}

/// Start a task of mmio for the file `hmmio`.
fn program_call(cpu: &mut Cpu, op: u32, hmmio: u32, a: u32, b: u32, c: u32) -> Option<u32> {
    let vars = vec![op, hmmio, 0, a, b, c];
    crate::win32::task::start(cpu, crate::win32::task::MMIO, vars, 0)
}

/// mmioClose(hmmio, flags).
pub fn mmio_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hmmio, flags] = args else {
        unreachable!()
    };
    let Some(i) = find(cpu, hmmio) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    with_file(cpu, i, |cpu, f| f.flush(cpu, 0));
    let f = &mmio(cpu).files[i];
    if f.block != 0 {
        // The program's procedure gets its MMIOM_CLOSE.
        return program_call(cpu, OP_CLOSE, hmmio, flags, 0, 0);
    }
    if let Proc::Dos { file } = f.proc
        && flags & MMIO_FHOPEN == 0
    {
        crate::win32::object::close(cpu, file);
    }
    close_file(cpu, i);
    Some(MMSYSERR_NOERROR)
}

/// mmioRead(hmmio, buffer, count): the count read, or -1.
pub fn mmio_read(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hmmio, to, n] = args else {
        unreachable!()
    };
    let Some(i) = find(cpu, hmmio) else {
        return Some(u32::MAX);
    };
    if (n as i32) < 0 {
        return Some(u32::MAX);
    }
    if !cpu.bus.guest_probe(to, n as usize, true) {
        return None;
    }
    if matches!(mmio(cpu).files[i].proc, Proc::Program) {
        return program_call(cpu, OP_CALL, hmmio, MMIOM_READ, to, n);
    }
    Some(with_file(cpu, i, |cpu, f| f.read(cpu, to, n)) as u32)
}

/// mmioWrite(hmmio, buffer, count): the count written, or -1.
pub fn mmio_write(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hmmio, from, n] = args else {
        unreachable!()
    };
    let Some(i) = find(cpu, hmmio) else {
        return Some(u32::MAX);
    };
    if (n as i32) < 0 {
        return Some(u32::MAX);
    }
    if !cpu.bus.guest_probe(from, n as usize, false) {
        return None;
    }
    if matches!(mmio(cpu).files[i].proc, Proc::Program) {
        return program_call(cpu, OP_CALL, hmmio, MMIOM_WRITE, from, n);
    }
    Some(with_file(cpu, i, |cpu, f| f.write(cpu, from, n)) as u32)
}

/// mmioSeek(hmmio, offset, origin): the new place, or -1.
pub fn mmio_seek(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hmmio, offset, origin] = args else {
        unreachable!()
    };
    let Some(i) = find(cpu, hmmio) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if matches!(mmio(cpu).files[i].proc, Proc::Program) {
        return program_call(cpu, OP_CALL, hmmio, MMIOM_SEEK, offset, origin);
    }
    Some(with_file(cpu, i, |cpu, f| f.seek(cpu, offset as i32, origin)) as u32)
}

/// mmioGetInfo(hmmio, info, flags).
pub fn mmio_get_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hmmio, at, _] = args else {
        unreachable!()
    };
    let Some(i) = find(cpu, hmmio) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(at, INFO_SIZE as usize, true) {
        return None;
    }
    let f = mmio(cpu).files[i].clone();
    f.write_info(cpu, at);
    Some(MMSYSERR_NOERROR)
}

/// mmioSetInfo(hmmio, info, flags): the buffer's pointers the program
/// moved, which have to stay in the buffer.
pub fn mmio_set_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hmmio, at, _] = args else {
        unreachable!()
    };
    let Some(i) = find(cpu, hmmio) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(at, INFO_SIZE as usize, false) {
        return None;
    }
    let next = cpu.bus.guest_read_32(at + I_NEXT);
    let end_read = cpu.bus.guest_read_32(at + I_END_READ);
    let end_write = cpu.bus.guest_read_32(at + I_END_WRITE);
    let flags = cpu.bus.guest_read_32(at + I_FLAGS);
    let f = &mut mmio(cpu).files[i];
    let range = f.buffer..=f.buffer.wrapping_add(f.cch);
    if ![next, end_read, end_write]
        .iter()
        .all(|p| range.contains(p))
    {
        return Some(MMSYSERR_INVALPARAM);
    }
    f.next = next;
    f.end_read = end_read;
    f.flags |= flags & MMIO_DIRTY;
    Some(MMSYSERR_NOERROR)
}

/// mmioAdvance(hmmio, info, flags): the buffer is written if it was
/// changed, and filled from the file for MMIO_READ.
pub fn mmio_advance(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hmmio, at, flags] = args else {
        unreachable!()
    };
    let Some(i) = find(cpu, hmmio) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if mmio(cpu).files[i].cch == 0 {
        return Some(MMIOERR_UNBUFFERED);
    }
    if flags != MMIO_READ && flags != MMIO_WRITE {
        return Some(MMSYSERR_INVALPARAM);
    }
    if at != 0 && !cpu.bus.guest_probe(at, INFO_SIZE as usize, true) {
        return None;
    }
    let result = with_file(cpu, i, |cpu, f| {
        if at != 0 {
            // In the buffer, as mmioSetInfo has it.
            let next = cpu.bus.guest_read_32(at + I_NEXT);
            if !(f.buffer..=f.buffer.wrapping_add(f.cch)).contains(&next) {
                return MMSYSERR_INVALPARAM;
            }
            f.next = next;
            f.flags |= cpu.bus.guest_read_32(at + I_FLAGS) & MMIO_DIRTY;
        }
        if f.fcc == FOURCC_MEM {
            // A memory file's buffer is the whole of it.
            return if flags == MMIO_READ {
                MMIOERR_CANNOTREAD
            } else {
                MMIOERR_CANNOTEXPAND
            };
        }
        if f.flush(cpu, MMIO_EMPTYBUF) != MMSYSERR_NOERROR {
            return MMIOERR_CANNOTWRITE;
        }
        f.grab_next(cpu, flags == MMIO_READ);
        if at != 0 {
            f.write_info(cpu, at);
        }
        MMSYSERR_NOERROR
    });
    Some(result)
}

const MMIOERR_CANNOTEXPAND: u32 = 264;

/// mmioFlush(hmmio, flags).
pub fn mmio_flush(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hmmio, flags] = args else {
        unreachable!()
    };
    let Some(i) = find(cpu, hmmio) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    Some(with_file(cpu, i, |cpu, f| f.flush(cpu, flags)))
}

/// mmioSetBuffer(hmmio, buffer, size, flags).
pub fn mmio_set_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hmmio, buffer, cch, _] = args else {
        unreachable!()
    };
    let Some(i) = find(cpu, hmmio) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if matches!(mmio(cpu).files[i].proc, Proc::Program) {
        return Some(MMSYSERR_NOERROR);
    }
    Some(with_file(cpu, i, |cpu, f| f.set_buffer(cpu, buffer, cch)))
}

/// mmioSendMessage(hmmio, message, p1, p2): a message of the program's
/// own to the file's procedure.
pub fn mmio_send_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hmmio, msg, a, b] = args else {
        unreachable!()
    };
    let Some(i) = find(cpu, hmmio) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if msg < MMIOM_USER {
        return Some(MMSYSERR_INVALPARAM);
    }
    if matches!(mmio(cpu).files[i].proc, Proc::Program) {
        return program_call(cpu, OP_CALL, hmmio, msg, a, b);
    }
    Some(0)
}

/// Read the MMCKINFO at `at`: ckid, cksize, fccType, dwDataOffset,
/// dwFlags.
fn read_ck(cpu: &mut Cpu, at: u32) -> [u32; 5] {
    std::array::from_fn(|k| cpu.bus.guest_read_32(at + 4 * k as u32))
}

fn write_ck(cpu: &mut Cpu, at: u32, ck: &[u32; 5]) {
    for (k, &w) in ck.iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * k as u32, w);
    }
}

/// The file `i` for mmioDescend and the others, if it can seek: not a
/// file a program's procedure writes.
fn chunk_file(cpu: &Cpu, hmmio: u32) -> Result<usize, u32> {
    let i = find(cpu, hmmio).ok_or(MMSYSERR_INVALHANDLE)?;
    if matches!(cpu.bus.win32.winmm.mmio.files[i].proc, Proc::Program) {
        return Err(MMIOERR_CANNOTSEEK);
    }
    Ok(i)
}

/// mmioDescend(hmmio, ck, parent, flags): into the next chunk, or the
/// next one of the id or type asked for.
pub fn mmio_descend(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hmmio, ck_at, parent, flags] = args else {
        unreachable!()
    };
    let i = match chunk_file(cpu, hmmio) {
        Ok(i) => i,
        Err(e) => return Some(e),
    };
    if ck_at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(ck_at, 20, true) {
        return None;
    }
    if parent != 0 && !cpu.bus.guest_probe(parent, 20, false) {
        return None;
    }
    let mut ck = read_ck(cpu, ck_at);
    let parent = (parent != 0).then(|| read_ck(cpu, parent));
    let result = with_file(cpu, i, |cpu, f| {
        let mut old = f.seek(cpu, 0, 1) as u32;
        if let Some(p) = parent
            && (old < p[3] || old >= p[3].wrapping_add(p[1]))
        {
            return MMIOERR_CHUNKNOTFOUND;
        }
        let (mut want_id, mut want_type) = (0, 0);
        if flags & MMIO_FINDCHUNK != 0 {
            want_id = ck[0];
        }
        if flags & MMIO_FINDLIST != 0 {
            want_id = FOURCC_LIST;
            want_type = ck[2];
        }
        if flags & MMIO_FINDRIFF != 0 {
            want_id = FOURCC_RIFF;
            want_type = ck[2];
        }
        // The header is read through a scratch place in the MMCKINFO.
        loop {
            let n = f.read(cpu, ck_at, 12);
            let words = read_ck(cpu, ck_at);
            ck[0] = words[0];
            ck[1] = words[1];
            ck[2] = words[2];
            if n < 8 {
                // dwDataOffset stays the last chunk's.
                f.seek(cpu, old as i32, 0);
                return MMIOERR_CHUNKNOTFOUND;
            }
            ck[3] = old + 8;
            if (want_id == 0 || want_id == ck[0]) && (want_type == 0 || want_type == ck[2]) {
                break;
            }
            // The next chunk is further on, or there is none: a size
            // that wraps around would go back and look for ever.
            let next = ck[3] as u64 + ((ck[1] as u64 + 1) & !1);
            if next <= old as u64 || next > u32::MAX as u64 {
                f.seek(cpu, old as i32, 0);
                return MMIOERR_CHUNKNOTFOUND;
            }
            old = next as u32;
            f.seek(cpu, old as i32, 0);
        }
        ck[4] = 0;
        if ck[0] == FOURCC_RIFF || ck[0] == FOURCC_LIST {
            f.seek(cpu, ck[3] as i32 + 4, 0);
        } else {
            f.seek(cpu, ck[3] as i32, 0);
            ck[2] = 0;
        }
        MMSYSERR_NOERROR
    });
    write_ck(cpu, ck_at, &ck);
    Some(result)
}

/// mmioAscend(hmmio, ck, flags): out of the chunk, its size written if
/// it was made and written to.
pub fn mmio_ascend(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hmmio, ck_at, _] = args else {
        unreachable!()
    };
    let i = match chunk_file(cpu, hmmio) {
        Ok(i) => i,
        Err(e) => return Some(e),
    };
    if ck_at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(ck_at, 20, true) {
        return None;
    }
    let mut ck = read_ck(cpu, ck_at);
    let scratch = heap_alloc(cpu, 4);
    if scratch == 0 {
        return Some(MMIOERR_OUTOFMEMORY);
    }
    with_file(cpu, i, |cpu, f| {
        if ck[4] & MMIO_DIRTY != 0 {
            let here = f.seek(cpu, 0, 1) as u32;
            let size = here.wrapping_sub(ck[3]);
            if size != ck[1] {
                ck[1] = size;
                if size & 1 != 0 {
                    cpu.bus.guest_write_32(scratch, 0);
                    f.write(cpu, scratch, 1);
                }
                f.seek(cpu, ck[3] as i32 - 4, 0);
                cpu.bus.guest_write_32(scratch, size);
                f.write(cpu, scratch, 4);
            }
            ck[4] = 0;
        }
        f.seek(
            cpu,
            ck[3].wrapping_add(ck[1].wrapping_add(1) & !1) as i32,
            0,
        );
    });
    heap_free(cpu, scratch);
    write_ck(cpu, ck_at, &ck);
    Some(MMSYSERR_NOERROR)
}

/// mmioCreateChunk(hmmio, ck, flags): a chunk's header at the file's
/// place, its size to be written by mmioAscend.
pub fn mmio_create_chunk(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hmmio, ck_at, flags] = args else {
        unreachable!()
    };
    let i = match chunk_file(cpu, hmmio) {
        Ok(i) => i,
        Err(e) => return Some(e),
    };
    if ck_at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(ck_at, 20, true) {
        return None;
    }
    let mut ck = read_ck(cpu, ck_at);
    if flags == MMIO_CREATELIST {
        ck[0] = FOURCC_LIST;
    } else if flags == MMIO_CREATERIFF {
        ck[0] = FOURCC_RIFF;
    }
    let result = with_file(cpu, i, |cpu, f| {
        let old = f.seek(cpu, 0, 1) as u32;
        ck[3] = old + 8;
        let size = if ck[0] == FOURCC_RIFF || ck[0] == FOURCC_LIST {
            12
        } else {
            8
        };
        ck[4] = MMIO_DIRTY;
        write_ck(cpu, ck_at, &ck);
        let n = f.write(cpu, ck_at, size);
        if n < size as i32 {
            f.seek(cpu, old as i32, 0);
            return MMIOERR_CANNOTWRITE;
        }
        MMSYSERR_NOERROR
    });
    write_ck(cpu, ck_at, &ck);
    Some(result)
}

/// mmioStringToFOURCCA(text, flags): its first four characters, padded
/// with spaces.
pub fn mmio_string_to_fourcc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, flags] = args else { unreachable!() };
    let text = cpu.bus.guest_read_asciiz(at, 4);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let mut cc = [b' '; 4];
    for (k, &b) in text.iter().enumerate() {
        cc[k] = if flags & MMIO_TOUPPER != 0 {
            b.to_ascii_uppercase()
        } else {
            b
        };
    }
    Some(u32::from_le_bytes(cc))
}

/// mmioInstallIOProcA(fcc, proc, flags): install, remove or find the
/// process's procedure for `fcc`.
pub fn mmio_install_io_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[fcc, proc, flags] = args else {
        unreachable!()
    };
    let pid = crate::win32::user32::current_pid(cpu);
    let m = mmio(cpu);
    if flags & MMIO_INSTALLPROC != 0 {
        if proc == 0 {
            return Some(0);
        }
        m.procs.push(Installed { pid, fcc, proc });
        return Some(proc);
    }
    if flags & MMIO_REMOVEPROC != 0 {
        let at = m
            .procs
            .iter()
            .rposition(|p| p.pid == pid && p.fcc == fcc && (proc == 0 || p.proc == proc));
        return Some(at.map_or(0, |k| m.procs.remove(k).proc));
    }
    if flags & MMIO_FINDPROC != 0 {
        return Some(installed(cpu, pid, fcc).unwrap_or(0));
    }
    Some(0)
}

/// mmioRenameA(old, new, info, flags): by the program's procedure the
/// MMIOINFO names, or as a DOS file.
pub fn mmio_rename(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[from_at, to_at, info, _] = args else {
        unreachable!()
    };
    let from = read_string(cpu, from_at, 260)?;
    let to = read_string(cpu, to_at, 260)?;
    if info != 0 {
        if !cpu.bus.guest_probe(info, INFO_SIZE as usize, false) {
            return None;
        }
        let fcc = cpu.bus.guest_read_32(info + I_FCC);
        let proc = cpu.bus.guest_read_32(info + I_PROC);
        let pid = crate::win32::user32::current_pid(cpu);
        let proc = if proc != 0 {
            Some(proc)
        } else if fcc != 0 && fcc != FOURCC_DOS && fcc != FOURCC_MEM {
            installed(cpu, pid, fcc)
        } else {
            None
        };
        if let Some(proc) = proc {
            // A file of its own for the call, which goes after it.
            let block = heap_alloc(cpu, INFO_SIZE);
            if block == 0 {
                return Some(MMIOERR_OUTOFMEMORY);
            }
            let hmmio = new_handle(cpu);
            let file = File {
                hmmio,
                pid,
                fcc,
                io_proc: proc,
                block,
                proc: Proc::Program,
                ..File::default()
            };
            file.write_info(cpu, block);
            mmio(cpu).files.push(file);
            return program_call(cpu, OP_RENAME, hmmio, from_at, to_at, 0);
        }
    }
    use crate::win32::kernel32;
    let from = kernel32::dos_path(&kernel32::full_path(cpu, &from));
    let to = kernel32::dos_path(&kernel32::full_path(cpu, &to));
    Some(match cpu.bus.disk.rename_file(&from, &to) {
        Ok(()) => MMSYSERR_NOERROR,
        Err(_) => MMIOERR_FILENOTFOUND,
    })
}

/// A process ended: its files and procedures go (DOS closes its files,
/// and the heap goes with the process).
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let m = mmio(cpu);
    m.files.retain(|f| f.pid != pid);
    m.procs.retain(|p| p.pid != pid);
}

crate::win32::state_variants!(Proc {
    Mem {},
    Dos { file },
    Data { data, pos },
    Program {},
});
crate::state_fields!(File {
    hmmio,
    pid,
    flags,
    fcc,
    io_proc,
    cch,
    buffer,
    next,
    end_read,
    end_write,
    buf_offset,
    disk_offset,
    adw_info,
    loaded,
    size,
    proc,
    block,
    slurp
});
crate::state_fields!(Installed { pid, fcc, proc });
crate::state_fields!(Mmio { files, procs });
