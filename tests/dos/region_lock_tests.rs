//! INT 21h AH=5Ch: locks of byte ranges of open files, as SHARE has them
//! (MS-DOS 4.0's GSHARE.ASM and LOCK.ASM), and Windows 95's VSHARE.

use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::interrupts::handle_hle;
use std::fs;
use std::path::PathBuf;

/// A machine that runs a COM program (RET) from `dir`, with DATA.TXT
/// holding "0123456789" there.
fn machine(name: &str) -> Box<Cpu> {
    let dir = PathBuf::from("target/test_region_locks").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("P.COM"), [0xC3]).unwrap();
    fs::write(dir.join("DATA.TXT"), b"0123456789").unwrap();
    let mut cpu = Cpu::new(dir);
    assert!(cpu.load_executable("P.COM", None));
    rust_dos::mcb::resize(&mut cpu.bus, cpu.current_psp, 0x1000).unwrap();
    cpu
}

/// INT 21h with AX, BX, CX and DX, buffers in the running program's
/// segment: AX, or the error code with CF set.
fn dos(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) -> Result<u16, u16> {
    let psp = cpu.current_psp;
    cpu.set_ds(psp);
    cpu.set_ax(ax);
    cpu.set_bx(bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    // Calls that succeed without touching CF, as AH=50h, leave it so.
    cpu.set_cpu_flag(CpuFlags::CF, false);
    handle_hle(cpu, 0x21);
    if cpu.get_cpu_flag(CpuFlags::CF) {
        Err(cpu.ax())
    } else {
        Ok(cpu.ax())
    }
}

/// Open DATA.TXT to read and write, sharing it with everyone: the handle.
fn open(cpu: &mut Cpu) -> u16 {
    let at = cpu.current_psp as usize * 16 + 0x200;
    cpu.bus.load_bytes(at, b"DATA.TXT\0");
    dos(cpu, 0x3D42, 0, 0, 0x200).unwrap()
}

/// AH=5Ch with AL `al`: `len` bytes from `offset` of `handle`.
fn lock_call(cpu: &mut Cpu, al: u8, handle: u16, offset: u32, len: u32) -> Result<(), u16> {
    cpu.set_si((len >> 16) as u16);
    cpu.set_di(len as u16);
    dos(
        cpu,
        0x5C00 | al as u16,
        handle,
        (offset >> 16) as u16,
        offset as u16,
    )
    .map(drop)
}

fn lock(cpu: &mut Cpu, handle: u16, offset: u32, len: u32) -> Result<(), u16> {
    lock_call(cpu, 0x00, handle, offset, len)
}

fn unlock(cpu: &mut Cpu, handle: u16, offset: u32, len: u32) -> Result<(), u16> {
    lock_call(cpu, 0x01, handle, offset, len)
}

/// Read `count` bytes of `handle` from `offset`.
fn read_at(cpu: &mut Cpu, handle: u16, offset: u16, count: u16) -> Result<Vec<u8>, u16> {
    dos(cpu, 0x4200, handle, 0, offset).unwrap();
    let n = dos(cpu, 0x3F00, handle, count, 0x300)?;
    let at = cpu.current_psp as usize * 16 + 0x300;
    Ok((0..n as usize).map(|i| cpu.bus.read_8(at + i)).collect())
}

/// Write `data` to `handle` at `offset`: how many bytes.
fn write_at(cpu: &mut Cpu, handle: u16, offset: u16, data: &[u8]) -> Result<u16, u16> {
    dos(cpu, 0x4200, handle, 0, offset).unwrap();
    let at = cpu.current_psp as usize * 16 + 0x300;
    cpu.bus.load_bytes(at, data);
    dos(cpu, 0x4000, handle, data.len() as u16, 0x300)
}

#[test]
fn a_locked_range_is_the_lockers_until_it_unlocks_it() {
    let mut cpu = machine("lock_unlock");
    let (mine, other) = (open(&mut cpu), open(&mut cpu));
    assert_eq!(lock(&mut cpu, mine, 2, 4), Ok(()));
    // No byte of it can be locked again, through the other open of the
    // file or through the same handle.
    assert_eq!(lock(&mut cpu, other, 5, 1), Err(0x21));
    assert_eq!(lock(&mut cpu, mine, 0, 3), Err(0x21));
    assert_eq!(
        lock(&mut cpu, other, 6, 4),
        Ok(()),
        "the bytes after it are free"
    );
    // The locker reads and writes the bytes; the other open doesn't, and
    // moves nothing.
    assert_eq!(read_at(&mut cpu, mine, 0, 6).unwrap(), b"012345");
    assert_eq!(read_at(&mut cpu, other, 0, 3), Err(0x21));
    assert_eq!(write_at(&mut cpu, other, 4, b"x"), Err(0x21));
    assert_eq!(
        read_at(&mut cpu, other, 0, 2).unwrap(),
        b"01",
        "bytes before the lock"
    );
    assert_eq!(write_at(&mut cpu, mine, 3, b"ab"), Ok(2));
    // Only the locker unlocks it, with its offset and length exactly.
    assert_eq!(unlock(&mut cpu, other, 2, 4), Err(0x21));
    assert_eq!(unlock(&mut cpu, mine, 2, 3), Err(0x21));
    assert_eq!(unlock(&mut cpu, mine, 0, 6), Err(0x21));
    assert_eq!(unlock(&mut cpu, mine, 2, 4), Ok(()));
    assert_eq!(unlock(&mut cpu, mine, 2, 4), Err(0x21), "no longer locked");
    assert_eq!(read_at(&mut cpu, other, 0, 6).unwrap(), b"012ab5");
    assert_eq!(lock(&mut cpu, other, 0, 6), Ok(()));
}

#[test]
fn errors_as_ms_dos_gives_them() {
    let mut cpu = machine("errors");
    let h = open(&mut cpu);
    // AL past 01h is an invalid function, before the handle is looked at.
    assert_eq!(lock_call(&mut cpu, 0x02, 0x00F0, 0, 1), Err(0x01));
    assert_eq!(lock(&mut cpu, 0x00F0, 0, 1), Err(0x06));
    assert_eq!(unlock(&mut cpu, 0x00F0, 0, 1), Err(0x06));
    // No bytes, or bytes past 4 GB: a lock violation, as SHARE answers.
    assert_eq!(lock(&mut cpu, h, 0, 0), Err(0x21));
    assert_eq!(lock(&mut cpu, h, 0xFFFF_FFFF, 2), Err(0x21));
    assert_eq!(
        lock(&mut cpu, h, 0xFFFF_FFFF, 1),
        Ok(()),
        "the last byte of 4 GB"
    );
    // A device has no sharing record to lock.
    let at = cpu.current_psp as usize * 16 + 0x200;
    cpu.bus.load_bytes(at, b"NUL\0");
    let nul = dos(&mut cpu, 0x3D02, 0, 0, 0x200).unwrap();
    assert_eq!(lock(&mut cpu, nul, 0, 1), Err(0x21));
}

#[test]
fn locks_past_the_end_leave_the_files_bytes_free() {
    // As Btrieve for Windows tells other programs it has a file open: two
    // bytes at 30000002h, far past its end.
    let mut cpu = machine("past_end");
    let (btrieve, other) = (open(&mut cpu), open(&mut cpu));
    assert_eq!(lock(&mut cpu, btrieve, 0x3000_0002, 2), Ok(()));
    assert_eq!(read_at(&mut cpu, other, 0, 100).unwrap(), b"0123456789");
    assert_eq!(write_at(&mut cpu, other, 10, b"!"), Ok(1));
    assert_eq!(lock(&mut cpu, other, 0x3000_0003, 1), Err(0x21));
}

#[test]
fn closing_the_file_frees_its_locks() {
    let mut cpu = machine("close");
    let (mine, other) = (open(&mut cpu), open(&mut cpu));
    lock(&mut cpu, mine, 0, 4).unwrap();
    // A duplicate refers to the same open file: closing one of the two
    // handles leaves the lock.
    let dup = dos(&mut cpu, 0x4500, mine, 0, 0).unwrap();
    dos(&mut cpu, 0x3E00, mine, 0, 0).unwrap();
    assert_eq!(lock(&mut cpu, other, 0, 4), Err(0x21));
    assert_eq!(
        unlock(&mut cpu, dup, 0, 4).and_then(|()| lock(&mut cpu, dup, 0, 4)),
        Ok(())
    );
    dos(&mut cpu, 0x3E00, dup, 0, 0).unwrap();
    assert_eq!(lock(&mut cpu, other, 0, 4), Ok(()));
    // The entry the closed file had, opened again, has no lock from
    // before.
    let again = open(&mut cpu);
    assert_eq!(
        lock(&mut cpu, again, 0, 4),
        Err(0x21),
        "the other open's lock"
    );
    assert_eq!(lock(&mut cpu, again, 4, 4), Ok(()));
}

#[test]
fn another_process_neither_uses_nor_unlocks_a_processs_lock() {
    // Its child shares the open file through an inherited handle, and is
    // another owner all the same.
    let mut cpu = machine("other_process");
    let parent = cpu.current_psp;
    let h = open(&mut cpu);
    lock(&mut cpu, h, 0, 4).unwrap();
    let child = parent + 0x100;
    cpu.set_si(child + 0x10);
    dos(&mut cpu, 0x5500, 0, 0, child).unwrap();
    assert_eq!(cpu.current_psp, child);
    assert_eq!(read_at(&mut cpu, h, 0, 2), Err(0x21));
    assert_eq!(unlock(&mut cpu, h, 0, 4), Err(0x21));
    dos(&mut cpu, 0x5000, parent, 0, 0).unwrap();
    assert_eq!(read_at(&mut cpu, h, 0, 2).unwrap(), b"01");
}

#[test]
fn a_process_that_ends_leaves_no_locks() {
    // The child's lock goes with it, though the file stays open in its
    // parent, which it inherited the handle from.
    let mut cpu = machine("exit");
    let parent = cpu.current_psp;
    let h = open(&mut cpu);
    cpu.set_sp(0xF000);
    let child = parent + 0x100;
    cpu.set_si(child + 0x10);
    dos(&mut cpu, 0x5500, 0, 0, child).unwrap();
    let base = child as usize * 16;
    cpu.bus.write_16(base + 0x0A, 0x0300);
    cpu.bus.write_16(base + 0x0C, 0x5000);
    lock(&mut cpu, h, 0, 4).unwrap();
    let sp = cpu.sp();
    cpu.set_sp(sp - 0x40);
    let _ = dos(&mut cpu, 0x4C00, 0, 0, 0);
    assert_eq!(cpu.current_psp, parent);
    assert_eq!(read_at(&mut cpu, h, 0, 2).unwrap(), b"01");
    assert_eq!(lock(&mut cpu, h, 0, 4), Ok(()));
}

#[test]
fn a_save_state_keeps_the_locks() {
    let mut cpu = machine("state");
    let (mine, other) = (open(&mut cpu), open(&mut cpu));
    lock(&mut cpu, mine, 0, 4).unwrap();
    let state = rust_dos::savestate::machine::save(&cpu);
    unlock(&mut cpu, mine, 0, 4).unwrap();
    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    assert_eq!(lock(&mut cpu, other, 0, 4), Err(0x21));
    assert_eq!(unlock(&mut cpu, mine, 0, 4), Ok(()));
    // A state with none loads none.
    let state = rust_dos::savestate::machine::save(&cpu);
    lock(&mut cpu, mine, 0, 4).unwrap();
    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    assert_eq!(lock(&mut cpu, other, 0, 4), Ok(()));
}
