//! The built-in Windows's Win16: Windows 3.1 programs (NE files) started
//! from the DOS prompt. The programs are made here byte by byte, small
//! enough to check one thing of the loader each.

use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Where the NE header starts in the programs made here.
const NE: usize = 0x40;
/// The alignment shift of their segments: 512-byte sectors.
const SHIFT: u16 = 9;

/// A Windows 3.1 program with one code segment, `code`, with the segment
/// flags `code_flags`, and an automatic data segment for its stack. It
/// starts at the code's first byte.
fn program(code: &[u8], code_flags: u16) -> Vec<u8> {
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
    // resident names (the module's name), no module references, the
    // imported names (just their leading 0) and an empty entry table.
    let seg_table = 0x40;
    let res_names = seg_table + 2 * 8;
    let names = b"\x01T\x00\x00\x00";
    let mod_refs = res_names + names.len();
    let imp_names = mod_refs;
    let entry_table = imp_names + 1;
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
    w(&mut b, 0x14, 0);
    w(&mut b, 0x16, 1);
    w(&mut b, 0x18, 0);
    w(&mut b, 0x1A, 2);
    w(&mut b, 0x1C, 2);
    w(&mut b, 0x22, seg_table as u16);
    w(&mut b, 0x24, res_names as u16);
    w(&mut b, 0x26, res_names as u16);
    w(&mut b, 0x28, mod_refs as u16);
    w(&mut b, 0x2A, imp_names as u16);
    w(&mut b, 0x32, SHIFT);
    b[NE + 0x36] = 2;
    w(&mut b, 0x3E, 0x030A);

    // Segment 1: the code, in the second sector. Segment 2: the data,
    // with nothing in the file.
    let seg = |b: &mut Vec<u8>, i: usize, sector: u16, len: u16, flags: u16, alloc: u16| {
        let at = seg_table + 8 * i;
        w(b, at, sector);
        w(b, at + 2, len);
        w(b, at + 4, flags);
        w(b, at + 6, alloc);
    };
    seg(
        &mut b,
        0,
        1,
        code.len() as u16,
        code_flags,
        code.len() as u16,
    );
    seg(&mut b, 1, 0, 0, 0x0C01, 0x100);
    b[NE + res_names..NE + res_names + names.len()].copy_from_slice(names);

    b.extend_from_slice(code);
    b
}

/// Code that makes its own selector 32-bit, as Metal Marines' 32-bit
/// segment does, and then ends with exit code 20h if it runs as 32-bit
/// code, or 10h if it still runs as 16-bit. The IRET that ends the INT
/// 31h loads CS again, so the code after it is 32-bit at once; the game
/// checks that the same way, with the carry of ADD AX,AX.
fn sets_its_own_d_bit() -> Vec<u8> {
    vec![
        0x8C, 0xD0, // mov ax,ss
        0x8E, 0xC0, // mov es,ax
        0x83, 0xEC, 0x08, // sub sp,8
        0x8B, 0xFC, // mov di,sp
        0x8C, 0xCB, // mov bx,cs
        0xB8, 0x0B, 0x00, // mov ax,000Bh: get the descriptor
        0xCD, 0x31, // int 31h
        0x26, 0x80, 0x4D, 0x06, 0x40, // or byte es:[di+6],40h: the D bit
        0xB8, 0x0C, 0x00, // mov ax,000Ch: set the descriptor
        0xCD, 0x31, // int 31h
        0x33, 0xC0, // xor (e)ax,(e)ax
        0xB4, 0x80, // mov ah,80h
        0x03, 0xC0, // add (e)ax,(e)ax: a carry only in 16-bit code
        0xB0, 0x10, // mov al,10h
        0x72, 0x02, // jc over the next
        0xB0, 0x20, // mov al,20h
        0xB4, 0x4C, // mov ah,4Ch
        0xCD, 0x21, // int 21h: end with AL
    ]
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
/// to its end; its exit code and the log.
fn run(test: &str, program: Vec<u8>) -> (u8, String) {
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
    for _ in 0..200 {
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

#[test]
fn a_segment_flagged_32_bit_loads_as_16_bit_and_can_make_itself_32_bit() {
    let (code, log) = run("seg32", program(&sets_its_own_d_bit(), 0x2C10));
    assert!(!log.contains("32-bit segments"), "{log}");
    assert_eq!(code, 0x20, "{log}");
}
