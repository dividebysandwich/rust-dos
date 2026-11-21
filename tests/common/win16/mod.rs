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
use std::path::{Path, PathBuf};
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
    /// Other relocations of the segment.
    fixups: Vec<ne::Relocation>,
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

    /// The module reference of `module` (from 1, in the order modules
    /// are first named), for a call or a fixup of `Target::Ordinal`.
    pub fn module(&mut self, module: &str) -> u16 {
        let index = match self.modules.iter().position(|m| m == module) {
            Some(i) => i,
            None => {
                self.modules.push(module.to_string());
                self.modules.len() - 1
            }
        };
        index as u16 + 1
    }

    /// `call far` the function `ordinal` of `module`.
    pub fn call(&mut self, module: &str, ordinal: u16) -> &mut Code {
        let module = self.module(module);
        let at = self.here() + 1;
        self.calls.push((module, ordinal, at));
        self.emit(&[0x9A, 0xFF, 0xFF, 0x00, 0x00])
    }

    /// Relocate `at` as `source` (an `ne::ADDR_` value) to `target`, the
    /// place's chain ending there: what is at `at` is FFFFh.
    pub fn fixup(&mut self, at: u16, source: u8, target: ne::Target) -> &mut Code {
        self.fixups.push(ne::Relocation {
            source,
            additive: false,
            offset: at,
            target,
        });
        self
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
    program_with_resources(code, flags, &[])
}

/// `program`, with the resources `resources` (type, name, data),
/// moveable and discardable.
pub fn program_with_resources(
    code: &Code,
    flags: u16,
    resources: &[(ne::ResId, ne::ResId, &[u8])],
) -> Vec<u8> {
    let mut b = ne::Builder::new("T", ne::MULTIPLEDATA);
    for (kind, name, data) in resources {
        b.resource(kind.clone(), name.clone(), 0x1030, data);
    }
    for (i, m) in code.modules.iter().enumerate() {
        let number = b.import_module(m);
        assert_eq!(number as usize, i + 1, "the module reference of {m}");
    }
    let relocations = code
        .calls
        .iter()
        .map(|&(module, ordinal, offset)| ne::Relocation {
            source: ne::ADDR_FARPTR,
            additive: false,
            offset,
            target: ne::Target::Ordinal { module, ordinal },
        })
        .chain(code.fixups.iter().cloned())
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
    run_with(test, program, |_, _| {})
}

/// `run`, with `setup` given the machine and the test's folder first, to
/// mount drives or add files.
pub fn run_with(test: &str, program: Vec<u8>, setup: impl FnOnce(&mut Cpu, &Path)) -> (u8, String) {
    run_command(test, program, "T.EXE", "", setup)
}

/// `run_with`, with the program started as the prompt starts the
/// command `command` with the arguments `args` (exec::run_program).
pub fn run_command(
    test: &str,
    program: Vec<u8>,
    command: &str,
    args: &str,
    setup: impl FnOnce(&mut Cpu, &Path),
) -> (u8, String) {
    run_machine(test, program, command, args, None, setup)
}

/// `run`, on a machine with `memory_mb` MB of RAM.
pub fn run_in_memory(test: &str, program: Vec<u8>, memory_mb: usize) -> (u8, String) {
    run_machine(test, program, "T.EXE", "", Some(memory_mb), |_, _| {})
}

fn run_machine(
    test: &str,
    program: Vec<u8>,
    command: &str,
    args: &str,
    memory_mb: Option<usize>,
    setup: impl FnOnce(&mut Cpu, &Path),
) -> (u8, String) {
    run_prompts(test, program, command, args, memory_mb, 1, setup)
}

/// `run_with`, going on until the prompt has come back `prompts` times:
/// for what the shell runs after the program, which then gives the exit
/// code.
pub fn run_with_prompts(
    test: &str,
    program: Vec<u8>,
    prompts: usize,
    setup: impl FnOnce(&mut Cpu, &Path),
) -> (u8, String) {
    run_prompts(test, program, "T.EXE", "", None, prompts, setup)
}

fn run_prompts(
    test: &str,
    program: Vec<u8>,
    command: &str,
    args: &str,
    memory_mb: Option<usize>,
    prompts: usize,
    setup: impl FnOnce(&mut Cpu, &Path),
) -> (u8, String) {
    let base = scratch(test, &[("T.EXE", program)]);
    let mut cpu = match memory_mb {
        Some(mb) => Cpu::with_memory(base.clone(), mb),
        None => Cpu::new(base.clone()),
    };
    setup(&mut cpu, &base);
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    cpu.bus.log_hook = Some(Box::new(move |line: &str| {
        sink.lock().unwrap().push(line.to_string())
    }));
    cpu.load_shell();
    assert!(
        rust_dos::exec::run_program(&mut cpu, command, args, false),
        "{command} didn't start"
    );
    let mut ended = 0;
    for _ in 0..400 * prompts {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            ended += 1;
            if ended == prompts {
                break;
            }
        }
    }
    let ended = ended == prompts;
    let log = lines.lock().unwrap().join("\n");
    assert!(ended, "the program didn't end:\n{log}");
    (cpu.errorlevel, log)
}

/// A DOS program (a COM file) that writes C:\MARK.TXT and ends with 5:
/// the file holds what INT 2Fh AX=1600h gave it (Windows' version, as a
/// DOS box on Windows has it), then its command tail.
pub fn dos_marker() -> Vec<u8> {
    const NAME: u16 = 0x0140;
    const RESULT: u16 = 0x0150;
    let [name_lo, name_hi] = NAME.to_le_bytes();
    let [result_lo, result_hi] = RESULT.to_le_bytes();
    let mut code = vec![
        0xB8, 0x00, 0x16, // mov ax,1600h
        0xCD, 0x2F, // int 2Fh
        0xA3, result_lo, result_hi, // mov [result],ax
        0xB4, 0x3C, // mov ah,3Ch
        0x31, 0xC9, // xor cx,cx
        0xBA, name_lo, name_hi, // mov dx,name
        0xCD, 0x21, // int 21h
        0x72, 0x22, // jc fail
        0x89, 0xC3, // mov bx,ax
        0xB4, 0x40, // mov ah,40h
        0xB9, 0x02, 0x00, // mov cx,2
        0xBA, result_lo, result_hi, // mov dx,result
        0xCD, 0x21, // int 21h
        0xB4, 0x40, // mov ah,40h
        0x8A, 0x0E, 0x80, 0x00, // mov cl,[80h]
        0x30, 0xED, // xor ch,ch
        0xBA, 0x81, 0x00, // mov dx,81h
        0xCD, 0x21, // int 21h
        0xB4, 0x3E, // mov ah,3Eh
        0xCD, 0x21, // int 21h
        0xB8, 0x05, 0x4C, // mov ax,4C05h
        0xCD, 0x21, // int 21h
        0xB8, 0x06, 0x4C, // fail: mov ax,4C06h
        0xCD, 0x21, // int 21h
    ];
    code.resize((NAME - 0x100) as usize, 0x90);
    code.extend_from_slice(b"C:\\MARK.TXT\0");
    code.resize((RESULT - 0x100) as usize + 2, 0);
    code
}
