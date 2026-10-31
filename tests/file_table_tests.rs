//! DOS's file tables in memory: each process's handles in the job file
//! table of its PSP, and the System File Table they refer to, which the List
//! of Lists points to and Windows reads.

use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::interrupts::handle_hle;
use std::fs;
use std::path::PathBuf;

/// A machine that runs a COM program (RET) from `dir`, with DATA.TXT
/// holding "0123456789" there.
fn machine(name: &str) -> Cpu {
    let dir = PathBuf::from("target/test_file_tables").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("P.COM"), [0xC3]).unwrap();
    fs::write(dir.join("DATA.TXT"), b"0123456789").unwrap();
    let mut cpu = Cpu::new(dir);
    assert!(cpu.load_executable("P.COM", None));
    cpu
}

/// INT 21h with AX, BX, CX and DX, the strings and buffers in the running
/// program's segment: AX, or the error code with CF set.
fn dos(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) -> Result<u16, u16> {
    let psp = cpu.current_psp;
    cpu.set_ds(psp);
    cpu.set_ax(ax);
    cpu.set_bx(bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    handle_hle(cpu, 0x21);
    if cpu.get_cpu_flag(CpuFlags::CF) { Err(cpu.ax()) } else { Ok(cpu.ax()) }
}

/// Open `name` with access `mode`: the handle.
fn open(cpu: &mut Cpu, name: &str, mode: u8) -> Result<u16, u16> {
    let at = cpu.current_psp as usize * 16 + 0x200;
    cpu.bus.load_bytes(at, format!("{}\0", name).as_bytes());
    dos(cpu, 0x3D00 | mode as u16, 0, 0, 0x200)
}

/// Read `count` bytes from `handle` into the program's segment at 300h.
fn read(cpu: &mut Cpu, handle: u16, count: u16) -> Result<Vec<u8>, u16> {
    let n = dos(cpu, 0x3F00, handle, count, 0x300)?;
    let at = cpu.current_psp as usize * 16 + 0x300;
    Ok((0..n as usize).map(|i| cpu.bus.read_8(at + i)).collect())
}

/// The job file table slot of `handle` in the PSP `psp`.
fn slot(cpu: &Cpu, psp: u16, handle: u16) -> u8 {
    let base = psp as usize * 16;
    let at = cpu.bus.read_16(base + 0x36) as usize * 16 + cpu.bus.read_16(base + 0x34) as usize;
    cpu.bus.read_8(at + handle as usize)
}

/// The linear address of a far pointer's target.
fn far(cpu: &Cpu, at: usize) -> usize {
    cpu.bus.read_16(at + 2) as usize * 16 + cpu.bus.read_16(at) as usize
}

/// The blocks of the System File Table, from the List of Lists: where each
/// is and how many entries it has.
fn sft_blocks(cpu: &mut Cpu) -> Vec<(usize, u16)> {
    dos(cpu, 0x5200, 0, 0, 0).unwrap();
    let lol = cpu.es() as usize * 16 + cpu.bx() as usize;
    let mut block = far(cpu, lol + 4);
    let mut blocks = vec![(block, cpu.bus.read_16(block + 4))];
    while cpu.bus.read_16(block) != 0xFFFF {
        block = far(cpu, block);
        blocks.push((block, cpu.bus.read_16(block + 4)));
        assert!(blocks.len() < 10, "the chain ends");
    }
    blocks
}

/// The linear address of SFT entry `sft`, as DOS finds it in its blocks.
fn sft_entry(cpu: &mut Cpu, sft: u8) -> usize {
    let mut n = sft as usize;
    for (block, count) in sft_blocks(cpu) {
        if n < count as usize {
            return block + 6 + n * 0x3B;
        }
        n -= count as usize;
    }
    panic!("no entry {sft}");
}

/// The number of handles referring to SFT entry `sft`.
fn refs(cpu: &mut Cpu, sft: u8) -> u16 {
    let entry = sft_entry(cpu, sft);
    cpu.bus.read_16(entry)
}

#[test]
fn a_program_starts_with_the_standard_handles() {
    let mut cpu = machine("standard");
    let psp = cpu.current_psp;
    let base = psp as usize * 16;
    assert_eq!(cpu.bus.read_16(base + 0x32), 20);
    assert_eq!((cpu.bus.read_16(base + 0x34), cpu.bus.read_16(base + 0x36)), (0x18, psp));
    // stdin, stdout and stderr on CON, then AUX and PRN.
    let slots: Vec<u8> = (0..6).map(|h| slot(&cpu, psp, h)).collect();
    assert_eq!(slots, [1, 1, 1, 0, 2, 0xFF]);
    assert_eq!(open(&mut cpu, "DATA.TXT", 0), Ok(5));
}

#[test]
fn windows_finds_the_size_of_an_entry_from_the_names_of_con() {
    // KRNL386 opens CON five times and looks for three of its names in a
    // row at the same distance, in the first 512 KB.
    let mut cpu = machine("con_scan");
    for expected in 5..10 {
        assert_eq!(open(&mut cpu, "CON", 0), Ok(expected));
    }
    let names: Vec<usize> = (0..0x80000 - 11)
        .filter(|&at| (0..11).all(|i| cpu.bus.read_8(at + i) == b"CON        "[i]))
        .collect();
    assert_eq!(names.len(), 6, "the standard CON and the five opened");
    let size = names.windows(3).find(|w| w[1] - w[0] == w[2] - w[1]).map(|w| w[1] - w[0]);
    assert_eq!(size, Some(0x3B));
    // Their entries: one handle each, a character device, the owner.
    let entry = names[5] - 0x20;
    assert_eq!(cpu.bus.read_16(entry), 1);
    assert_eq!(cpu.bus.read_16(entry + 0x05), 0x80D3);
    assert_eq!(cpu.bus.read_16(entry + 0x31), cpu.current_psp);
}

#[test]
fn the_table_is_in_two_blocks_as_in_dos() {
    // DOS's own five entries and the rest of FILES= after them, below
    // 512 KB. VSHARE.386 stops walking at the block before the last.
    let mut cpu = machine("blocks");
    let blocks = sft_blocks(&mut cpu);
    let counts: Vec<u16> = blocks.iter().map(|&(_, count)| count).collect();
    assert_eq!(counts, [5, 122]);
    let (last, count) = blocks[1];
    assert_eq!(cpu.bus.read_32(last), 0xFFFF_FFFF);
    assert!(last + 6 + count as usize * 0x3B < 0x80000);
    // The entries of the second block are where DOS finds them.
    let h = open(&mut cpu, "DATA.TXT", 0).unwrap();
    for _ in 0..3 {
        dos(&mut cpu, 0x4500, h, 0, 0).unwrap();
        open(&mut cpu, "DATA.TXT", 0).unwrap();
    }
    let sft = slot(&cpu, cpu.current_psp, 11);
    assert!(sft >= 5, "an entry in the second block");
    let entry = sft_entry(&mut cpu, sft);
    assert_eq!(cpu.bus.read_16(entry), 1);
    assert_eq!(&(0..11).map(|i| cpu.bus.read_8(entry + 0x20 + i)).collect::<Vec<u8>>(), b"DATA    TXT");
    // INT 2Fh AX=1216h gives the same address.
    cpu.set_ax(0x1216);
    cpu.set_bx(sft as u16);
    handle_hle(&mut cpu, 0x2F);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cpu.es() as usize * 16 + cpu.di() as usize, entry);
}

#[test]
fn duplicated_handles_share_the_file_until_the_last_closes() {
    let mut cpu = machine("dup");
    let h = open(&mut cpu, "DATA.TXT", 0).unwrap();
    let sft = slot(&cpu, cpu.current_psp, h);
    let dup = dos(&mut cpu, 0x4500, h, 0, 0).unwrap();
    assert_eq!(slot(&cpu, cpu.current_psp, dup), sft);
    assert_eq!(refs(&mut cpu, sft), 2);
    // One position for both.
    assert_eq!(read(&mut cpu, h, 4).unwrap(), b"0123");
    assert_eq!(read(&mut cpu, dup, 2).unwrap(), b"45");
    let entry = sft_entry(&mut cpu, sft);
    assert_eq!(cpu.bus.read_32(entry + 0x15), 6, "the position in the entry");
    assert_eq!(dos(&mut cpu, 0x3E00, h, 0, 0), Ok(0x3E00));
    assert_eq!(slot(&cpu, cpu.current_psp, h), 0xFF);
    assert_eq!(read(&mut cpu, h, 1), Err(0x06));
    assert_eq!(read(&mut cpu, dup, 10).unwrap(), b"6789");
    dos(&mut cpu, 0x3E00, dup, 0, 0).unwrap();
    assert_eq!(refs(&mut cpu, sft), 0, "the entry is free");
    assert_eq!(read(&mut cpu, dup, 1), Err(0x06));
}

#[test]
fn force_duplicate_points_standard_output_at_a_file() {
    let mut cpu = machine("force_dup");
    let at = cpu.current_psp as usize * 16 + 0x200;
    cpu.bus.load_bytes(at, b"OUT.TXT\0");
    let h = dos(&mut cpu, 0x3C00, 0, 0, 0x200).unwrap();
    assert_eq!(dos(&mut cpu, 0x4600, h, 1, 0), Ok(0x4600));
    dos(&mut cpu, 0x3E00, h, 0, 0).unwrap();
    // Standard output keeps the file open.
    let at = cpu.current_psp as usize * 16 + 0x300;
    cpu.bus.load_bytes(at, b"hi");
    assert_eq!(dos(&mut cpu, 0x4000, 1, 2, 0x300), Ok(2));
    dos(&mut cpu, 0x3E00, 1, 0, 0).unwrap();
    let dir = PathBuf::from("target/test_file_tables/force_dup");
    assert_eq!(fs::read(dir.join("OUT.TXT")).unwrap(), b"hi");
}

/// The tests below hold what MS-DOS 7.10 answers, from a test program run
/// on Windows 98's DOS booted from a floppy.
#[test]
fn ioctl_status_of_a_file_reads_ahead_and_keeps_the_position() {
    let mut cpu = machine("ioctl_status");
    let h = open(&mut cpu, "DATA.TXT", 2).unwrap();
    // AL=FFh with the next byte in AH, until the end, and at or past it
    // AL=00h and AH=1Ah.
    assert_eq!(dos(&mut cpu, 0x4406, h, 0, 0), Ok(0x30FF));
    assert_eq!(dos(&mut cpu, 0x4201, h, 0, 0), Ok(0), "the position stays");
    dos(&mut cpu, 0x4200, h, 0, 5).unwrap();
    assert_eq!(dos(&mut cpu, 0x4406, h, 0, 0), Ok(0x35FF));
    assert_eq!(read(&mut cpu, h, 5).unwrap(), b"56789");
    assert_eq!(dos(&mut cpu, 0x4406, h, 0, 0), Ok(0x1A00));
    dos(&mut cpu, 0x4200, h, 0, 20).unwrap();
    assert_eq!(dos(&mut cpu, 0x4406, h, 0, 0), Ok(0x1A00));
    assert_eq!(dos(&mut cpu, 0x4201, h, 0, 0), Ok(20), "the position stays");
    // A file is always ready for output.
    assert_eq!(dos(&mut cpu, 0x4407, h, 0, 0), Ok(0x01FF));
    // A handle open for writing only has nothing to read.
    let w = open(&mut cpu, "DATA.TXT", 1).unwrap();
    assert_eq!(dos(&mut cpu, 0x4406, w, 0, 0), Ok(0x1A00));
    assert_eq!(dos(&mut cpu, 0x4407, w, 0, 0), Ok(0x01FF));
    // No such handle.
    assert_eq!(dos(&mut cpu, 0x4406, 0x77, 0, 0), Err(6));
    assert_eq!(dos(&mut cpu, 0x4407, 0x77, 0, 0), Err(6));
}

#[test]
fn ioctl_status_of_the_console_is_the_keyboards() {
    let mut cpu = machine("ioctl_status_console");
    assert_eq!(dos(&mut cpu, 0x4406, 0, 0, 0), Ok(0x0000));
    assert_eq!(dos(&mut cpu, 0x4407, 1, 0, 0), Ok(0x01FF));
    rust_dos::keyboard::queue_keystroke(&mut cpu.bus, 0x1071);
    // Any handle on the console: the key stays where it was.
    assert_eq!(dos(&mut cpu, 0x4406, 1, 0, 0), Ok(0x71FF));
    assert_eq!(dos(&mut cpu, 0x4406, 0, 0, 0), Ok(0x71FF));
    assert_eq!(dos(&mut cpu, 0x0700, 0, 0, 0).map(|ax| ax & 0xFF), Ok(0x71));
    // The rest of a line a cooked read took doesn't count.
    cpu.con_pending.extend(b"cde\r\n");
    assert_eq!(dos(&mut cpu, 0x4406, 0, 0, 0), Ok(0x0000));
}

#[test]
fn ioctl_status_of_nul_aux_and_a_serial_port() {
    use rust_dos::serial::{PortType, SerialSettings};
    let mut cpu = machine("ioctl_status_devices");
    let nul = open(&mut cpu, "NUL", 2).unwrap();
    assert_eq!(dos(&mut cpu, 0x4406, nul, 0, 0), Ok(0x00FF));
    assert_eq!(dos(&mut cpu, 0x4407, nul, 0, 0), Ok(0x01FF));
    // AUX, the standard one or by name, with nothing on it.
    let aux = open(&mut cpu, "AUX", 2).unwrap();
    for handle in [3, aux] {
        assert_eq!(dos(&mut cpu, 0x4406, handle, 0, 0), Ok(0x0000));
        assert_eq!(dos(&mut cpu, 0x4407, handle, 0, 0), Ok(0x0100));
    }
    // A COM port with no hardware.
    let com3 = open(&mut cpu, "COM3", 2).unwrap();
    assert_eq!(dos(&mut cpu, 0x4406, com3, 0, 0), Ok(0x0000));
    assert_eq!(dos(&mut cpu, 0x4407, com3, 0, 0), Ok(0x0100));
    // A modem on COM1, which raises DSR once the program raises DTR:
    // ready for output, nothing to read yet.
    let modem = [
        PortType::Modem,
        PortType::Empty,
        PortType::Off,
        PortType::Off,
    ];
    cpu.bus.configure_serial(&SerialSettings {
        ports: modem,
        ..Default::default()
    });
    cpu.bus.io_write(0x3FC, 0x03);
    let com1 = open(&mut cpu, "COM1", 2).unwrap();
    assert_eq!(dos(&mut cpu, 0x4407, com1, 0, 0), Ok(0x01FF));
    assert_eq!(dos(&mut cpu, 0x4406, com1, 0, 0), Ok(0x0000));
    // Nothing on COM2, so no DSR: busy both ways.
    let com2 = open(&mut cpu, "COM2", 2).unwrap();
    assert_eq!(dos(&mut cpu, 0x4407, com2, 0, 0), Ok(0x0100));
    assert_eq!(dos(&mut cpu, 0x4406, com2, 0, 0), Ok(0x0000));
}

#[test]
fn ioctl_440a_gives_the_sft_flags_and_keeps_ax() {
    let mut cpu = machine("ioctl_remote");
    let h = open(&mut cpu, "DATA.TXT", 2).unwrap();
    let drive = cpu.bus.disk.get_current_drive() as u16;
    // Bit 15, remote, clear; bit 6, not written, set.
    assert_eq!(dos(&mut cpu, 0x440A, h, 0, 0xFFFF), Ok(0x440A));
    assert_eq!(cpu.dx(), 0x0040 | drive);
    // A device has no driver attributes in the high byte, unlike AX=4400h.
    dos(&mut cpu, 0x4400, 1, 0, 0).unwrap();
    assert_eq!(cpu.dx(), 0x80D3);
    assert_eq!(dos(&mut cpu, 0x440A, 1, 0, 0xFFFF), Ok(0x440A));
    assert_eq!(cpu.dx(), 0x00D3);
    let nul = open(&mut cpu, "NUL", 2).unwrap();
    dos(&mut cpu, 0x4400, nul, 0, 0).unwrap();
    assert_eq!(cpu.dx(), 0x80C4);
    dos(&mut cpu, 0x440A, nul, 0, 0xFFFF).unwrap();
    assert_eq!(cpu.dx(), 0x00C4);
    // Raw mode shows, as in AX=4400h.
    dos(&mut cpu, 0x4401, 1, 0, 0xF3).unwrap();
    dos(&mut cpu, 0x440A, 1, 0, 0xFFFF).unwrap();
    assert_eq!(cpu.dx(), 0x00F3);
    // No such handle: error 6, DX as it was.
    assert_eq!(dos(&mut cpu, 0x440A, 0x77, 0, 0x1234), Err(6));
    assert_eq!(cpu.dx(), 0x1234);
}

#[test]
fn a_console_write_clears_a_carry_the_caller_had_set() {
    let mut cpu = machine("console_write_carry");
    let con = open(&mut cpu, "CON", 2).unwrap();
    for handle in [1, 2, con] {
        for count in [2, 0] {
            cpu.set_cpu_flag(CpuFlags::CF, true);
            assert_eq!(dos(&mut cpu, 0x4000, handle, count, 0x200), Ok(count));
        }
    }
}

#[test]
fn a_child_psp_inherits_handles_but_not_those_opened_not_to_be() {
    let mut cpu = machine("child_psp");
    let parent = cpu.current_psp;
    let shared = open(&mut cpu, "DATA.TXT", 0x00).unwrap();
    let private = open(&mut cpu, "DATA.TXT", 0x80).unwrap();
    let sft = slot(&cpu, parent, shared);
    // AH=55h: the child's PSP at DX, its memory ending at SI, and it runs
    // now.
    let child = parent + 0x100;
    cpu.set_si(child + 0x10);
    dos(&mut cpu, 0x5500, 0, 0, child).unwrap();
    assert_eq!(cpu.current_psp, child);
    let base = child as usize * 16;
    assert_eq!(cpu.bus.read_16(base + 0x16), parent);
    assert_eq!(cpu.bus.read_16(base + 0x02), child + 0x10, "the end of its memory, as SI says");
    assert_eq!(slot(&cpu, child, shared), sft);
    assert_eq!(slot(&cpu, child, private), 0xFF);
    assert_eq!(slot(&cpu, child, 1), 1, "standard output");
    assert_eq!(refs(&mut cpu, sft), 2);
    assert_eq!(read(&mut cpu, shared, 3).unwrap(), b"012");
    dos(&mut cpu, 0x3E00, shared, 0, 0).unwrap();
    // The parent's handle is still open, and at the child's position.
    dos(&mut cpu, 0x5000, parent, 0, 0).unwrap();
    assert_eq!(read(&mut cpu, shared, 3).unwrap(), b"345");
}

#[test]
fn a_bigger_handle_table_moves_out_of_the_psp() {
    let mut cpu = machine("handle_count");
    let psp = cpu.current_psp;
    let h = open(&mut cpu, "DATA.TXT", 0).unwrap();
    assert!(dos(&mut cpu, 0x6700, 40, 0, 0).is_ok());
    let base = psp as usize * 16;
    assert_eq!(cpu.bus.read_16(base + 0x32), 40);
    assert_ne!(cpu.bus.read_16(base + 0x36), psp);
    assert_eq!(read(&mut cpu, h, 2).unwrap(), b"01");
    // Room for more than 20 handles now.
    let handles: Vec<u16> = (0..30).map(|_| open(&mut cpu, "DATA.TXT", 0).unwrap()).collect();
    assert_eq!(handles.last(), Some(&35));
}

#[test]
fn a_psp_a_program_made_ends_back_in_its_parent_at_its_terminate_address() {
    // As Windows ends a task: its PSP made with AH=55h, then AH=4Ch with
    // it current.
    let mut cpu = machine("made_psp_exit");
    let parent = cpu.current_psp;
    let h = open(&mut cpu, "DATA.TXT", 0).unwrap();
    let sft = slot(&cpu, parent, h);
    cpu.set_sp(0xF000);
    let (ss, sp) = (cpu.ss(), cpu.sp());
    // The parent's last INT 21h call leaves its registers on its stack.
    let child = parent + 0x100;
    cpu.set_si(child + 0x10);
    cpu.set_bp(0x1234);
    dos(&mut cpu, 0x5500, 0xABCD, 0, child).unwrap();
    // Where the child returns to when it ends.
    let base = child as usize * 16;
    cpu.bus.write_16(base + 0x0A, 0x0300);
    cpu.bus.write_16(base + 0x0C, 0x5000);

    // The child runs on a stack of its own, and ends.
    cpu.set_sp(sp - 0x40);
    cpu.set_bp(0);
    assert_eq!(refs(&mut cpu, sft), 2);
    let _ = dos(&mut cpu, 0x4C00, 0, 0, 0);
    assert_eq!(cpu.current_psp, parent);
    assert_eq!((cpu.ax(), cpu.bx(), cpu.dx(), cpu.bp()), (0x5500, 0xABCD, child, 0x1234), "the parent's registers");
    assert_eq!((cpu.ss(), cpu.sp()), (ss, sp), "on the parent's stack");
    let frame = ss as usize * 16 + sp as usize;
    assert_eq!((cpu.bus.read_16(frame), cpu.bus.read_16(frame + 2)), (0x0300, 0x5000), "returning to the terminate address");
    assert_eq!(cpu.bus.read_32(0x22 * 4), 0x5000_0300, "INT 22h as the child had it");
    assert_eq!(refs(&mut cpu, sft), 1, "the child's handle closed, the parent's open");
    assert_eq!(read(&mut cpu, h, 2).unwrap(), b"01");
}
