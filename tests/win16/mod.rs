//! Windows 3.1 programs (NE files) made byte by byte for the tests of the
//! built-in Windows's Win16, and a machine that runs one from the shell.
//! A program has one code segment, with the calls of system functions
//! its code makes relocated by ordinal, and an automatic data segment
//! for its stack.

#![allow(dead_code)]

use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Where the NE header starts in the programs made here.
const NE: usize = 0x40;
/// The alignment shift of their segments: 512-byte sectors.
const SHIFT: u16 = 9;
/// A code segment's flags: movable, with level 3, with relocations
/// when it has calls to relocate.
pub const CODE: u16 = 0x0C10;
const SEG_RELOCINFO: u16 = 0x0100;

/// A code segment's bytes, and the far calls in it to relocate.
#[derive(Default)]
pub struct Code {
    pub bytes: Vec<u8>,
    /// The modules called, in the order of the module references.
    modules: Vec<String>,
    /// Each far call's module reference (from 1), ordinal and the offset
    /// of its address.
    calls: Vec<(u16, u16, u16)>,
}

impl Code {
    pub fn new() -> Code {
        Code::default()
    }

    /// Where the next byte goes.
    pub fn here(&self) -> u16 {
        self.bytes.len() as u16
    }

    pub fn emit(&mut self, bytes: &[u8]) -> &mut Code {
        self.bytes.extend_from_slice(bytes);
        self
    }

    /// `push imm16`.
    pub fn push(&mut self, value: u16) -> &mut Code {
        let [lo, hi] = value.to_le_bytes();
        self.emit(&[0x68, lo, hi])
    }

    /// `push cs` and `push imm16`: a far pointer to `offset` in this
    /// segment.
    pub fn push_far(&mut self, offset: u16) -> &mut Code {
        self.emit(&[0x0E]).push(offset)
    }

    /// `call far` the function `ordinal` of `module`.
    pub fn call(&mut self, module: &str, ordinal: u16) -> &mut Code {
        let index = match self.modules.iter().position(|m| m == module) {
            Some(i) => i,
            None => {
                self.modules.push(module.to_string());
                self.modules.len() - 1
            }
        };
        let at = self.here() + 1;
        self.calls.push((index as u16 + 1, ordinal, at));
        self.emit(&[0x9A, 0xFF, 0xFF, 0x00, 0x00])
    }

    /// End the program with AL as its exit code: `mov ah,4Ch`, `int 21h`.
    pub fn exit_al(&mut self) -> &mut Code {
        self.emit(&[0xB4, 0x4C, 0xCD, 0x21])
    }

    /// Write `value` at `at`, as an operand found out later.
    pub fn patch(&mut self, at: u16, value: u16) {
        let at = at as usize;
        self.bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
}

/// The Windows 3.1 program whose code is `code`, its segment with the
/// flags `flags` (and relocations if it calls functions), starting at the
/// code's first byte.
pub fn program(code: &Code, flags: u16) -> Vec<u8> {
    let mut b = vec![0u8; 0x200];
    // The MZ header, with the new header at 40h. Like a linker's, it asks
    // DOS for all the memory there is (e_maxalloc FFFFh), so the stub
    // loads low, with a stack at its end.
    b[0..2].copy_from_slice(b"MZ");
    b[2..4].copy_from_slice(&0x200u16.to_le_bytes());
    b[4..6].copy_from_slice(&1u16.to_le_bytes());
    b[8..10].copy_from_slice(&4u16.to_le_bytes());
    b[0x0A..0x0C].copy_from_slice(&0x10u16.to_le_bytes());
    b[0x0C..0x0E].copy_from_slice(&0xFFFFu16.to_le_bytes());
    b[0x10..0x12].copy_from_slice(&0x2FEu16.to_le_bytes());
    b[0x18..0x1A].copy_from_slice(&0x40u16.to_le_bytes());
    b[0x3C..0x40].copy_from_slice(&(NE as u32).to_le_bytes());

    // The tables after the 40h-byte NE header: the segment table, the
    // resident names (the module's name), the module references, the
    // imported names (a 0, then the modules' names) and an empty entry
    // table.
    let seg_table = 0x40;
    let res_names = seg_table + 2 * 8;
    let names = b"\x01T\x00\x00\x00";
    let mod_refs = res_names + names.len();
    let imp_names = mod_refs + 2 * code.modules.len();
    let mut imported = vec![0u8];
    let mut refs = Vec::new();
    for m in &code.modules {
        refs.extend_from_slice(&(imported.len() as u16).to_le_bytes());
        imported.push(m.len() as u8);
        imported.extend_from_slice(m.as_bytes());
    }
    let entry_table = imp_names + imported.len();
    let tables = [names.as_slice(), &refs, &imported, &[0, 0]].concat();
    b[NE + res_names..NE + res_names + tables.len()].copy_from_slice(&tables);

    let w = |b: &mut Vec<u8>, at: usize, v: u16| {
        b[NE + at..NE + at + 2].copy_from_slice(&v.to_le_bytes());
    };
    b[NE..NE + 2].copy_from_slice(b"NE");
    b[NE + 2] = 5;
    b[NE + 3] = 10;
    w(&mut b, 0x04, entry_table as u16);
    w(&mut b, 0x06, 2);
    w(&mut b, 0x0C, 0x0302);
    w(&mut b, 0x0E, 2);
    w(&mut b, 0x10, 0x400);
    w(&mut b, 0x12, 0x800);
    w(&mut b, 0x16, 1);
    w(&mut b, 0x1A, 2);
    w(&mut b, 0x1C, 2);
    w(&mut b, 0x1E, code.modules.len() as u16);
    w(&mut b, 0x22, seg_table as u16);
    w(&mut b, 0x24, res_names as u16);
    w(&mut b, 0x26, res_names as u16);
    w(&mut b, 0x28, mod_refs as u16);
    w(&mut b, 0x2A, imp_names as u16);
    w(&mut b, 0x32, SHIFT);
    b[NE + 0x36] = 2;
    w(&mut b, 0x3E, 0x030A);

    // Segment 1: the code, in the second sector, its relocations after
    // it. Segment 2: the data, with nothing in the file.
    let seg = |b: &mut Vec<u8>, i: usize, sector: u16, len: u16, flags: u16, alloc: u16| {
        let at = seg_table + 8 * i;
        w(b, at, sector);
        w(b, at + 2, len);
        w(b, at + 4, flags);
        w(b, at + 6, alloc);
    };
    let len = code.bytes.len() as u16;
    let flags = if code.calls.is_empty() {
        flags
    } else {
        flags | SEG_RELOCINFO
    };
    seg(&mut b, 0, 1, len, flags, len);
    seg(&mut b, 1, 0, 0, 0x0C01, 0x100);

    b.extend_from_slice(&code.bytes);
    if !code.calls.is_empty() {
        b.extend_from_slice(&(code.calls.len() as u16).to_le_bytes());
        for &(module, ordinal, at) in &code.calls {
            // A far address, imported by ordinal.
            b.extend_from_slice(&[3, 1]);
            for v in [at, module, ordinal] {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    b
}

fn scratch(name: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let base = PathBuf::from("target/test_win16").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// Run `program` as T.EXE from the shell with the built-in Windows on,
/// to its end: its exit code and the log.
pub fn run(test: &str, program: Vec<u8>) -> (u8, String) {
    let mut cpu = Cpu::new(scratch(test, &[("T.EXE", program)]));
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    cpu.bus.log_hook = Some(Box::new(move |line: &str| {
        sink.lock().unwrap().push(line.to_string())
    }));
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    let mut ended = false;
    for _ in 0..400 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            ended = true;
            break;
        }
    }
    let log = lines.lock().unwrap().join("\n");
    assert!(ended, "the program didn't end:\n{log}");
    (cpu.errorlevel, log)
}
