//! Windows 3.1 programs (NE files) made byte by byte for the tests of the
//! built-in Windows's Win16, and a machine that runs one from the shell.
//! A program, which ne::Builder writes, has one code segment, with the
//! calls of system functions its code makes relocated by ordinal, and
//! an automatic data segment for its stack.

#![allow(dead_code)]

use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos_ne as ne;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// A code segment's flags: movable, with level 3.
pub const CODE: u16 = ne::SEG_MOVEABLE | 0x0C00;

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
    let mut b = ne::Builder::new("T", ne::MULTIPLEDATA);
    let relocations = code
        .calls
        .iter()
        .map(|&(module, ordinal, offset)| ne::Relocation {
            source: ne::ADDR_FARPTR,
            additive: false,
            offset,
            target: ne::Target::Ordinal { module, ordinal },
        })
        .collect();
    let text = b.segment(ne::SegmentSpec {
        data: code.bytes.clone(),
        flags,
        alloc: 0,
        relocations,
    });
    let data = b.segment(ne::SegmentSpec {
        data: Vec::new(),
        flags: ne::SEG_DATA | 0x0C00,
        alloc: 0x100,
        relocations: Vec::new(),
    });
    for m in &code.modules {
        b.import_module(m);
    }
    b.auto_data(data, 0x400, 0x800);
    b.entry_point(text, 0);
    b.initial_stack(data, 0);
    b.build()
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
