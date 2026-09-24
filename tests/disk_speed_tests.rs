//! Emulated disk speed and disk noises: DOS and BIOS disk services take the
//! time the drive's speed setting says, with the machine running on, and
//! the drives make their noises meanwhile.

mod fatimage;

use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::disk::MountOptions;
use rust_dos::diskio::{DiskSettings, DiskSpeed, NoiseMode};
use rust_dos::exec::{self, NoHook};
use std::fs;
use std::path::Path;

/// A machine at 1000 instructions per ms with C: in `dir/c`, holding
/// DATA.BIN of 61440 bytes.
fn machine(dir: &Path, settings: DiskSettings) -> Cpu {
    fs::create_dir_all(dir.join("c")).unwrap();
    fs::write(dir.join("c/DATA.BIN"), vec![0x42u8; 61440]).unwrap();
    let mut cpu = Cpu::new(dir.join("c"));
    cpu.bus.set_cycles_per_ms(1000);
    cpu.bus.set_disk_settings(settings);
    cpu
}

/// Run `code` at 2000:0000 (with data at 2000:0100 on) until it halts at
/// its end, with interrupts on. Returns the emulated time it took, in
/// microseconds.
fn run(cpu: &mut Cpu, code: &[u8], data: &[u8]) -> u64 {
    cpu.bus.load_bytes(0x20000, code);
    cpu.bus.load_bytes(0x20100, data);
    cpu.set_cs(0x2000);
    cpu.set_ds(0x2000);
    cpu.set_ip(0);
    cpu.set_ss(0x3000);
    cpu.set_sp(0x0400);
    cpu.set_cpu_flag(CpuFlags::IF, true);
    let start = cpu.bus.clock.now_micros();
    while cpu.ip() != code.len() as u16 || cpu.cs() != 0x2000 {
        let end = cpu.bus.clock.icount + 1000;
        cpu.bus.start_batch(end);
        exec::run_batch(cpu, &mut NoHook, false);
        assert!(cpu.bus.clock.now_micros() - start < 5_000_000, "the code ends");
    }
    cpu.bus.clock.now_micros() - start
}

/// MOV AX,3D00h; MOV DX,0100h; INT 21h; MOV BX,AX; MOV AX,5000h;
/// MOV DS,AX; XOR DX,DX; MOV CX,F000h; MOV AH,3Fh; INT 21h; HLT: open
/// C:\DATA.BIN and read 61440 bytes of it.
const OPEN_AND_READ: [u8; 25] = [
    0xB8, 0x00, 0x3D, 0xBA, 0x00, 0x01, 0xCD, 0x21, 0x89, 0xC3, 0xB8, 0x00, 0x50, 0x8E, 0xD8, 0x31, 0xD2, 0xB9, 0x00,
    0xF0, 0xB4, 0x3F, 0xCD, 0x21, 0xF4,
];

#[test]
fn slow_hard_disks_take_their_time() {
    let dir = fatimage::scratch("slow_hdd");
    let slow = DiskSettings { hard_disk_speed: DiskSpeed::Slow, ..Default::default() };
    let mut cpu = machine(&dir, slow);
    let ticks = cpu.bus.read_32(0x046C);
    let took = run(&mut cpu, &OPEN_AND_READ, b"C:\\DATA.BIN\0");
    assert_eq!(cpu.ax(), 0xF000);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cpu.sp(), 0x0400, "the waits leave the stack as it was");
    // 1024 bytes for the open and 61440 read, at 600 KB/s.
    let expected = (1024 + 61440) * 1_000_000 / (600 * 1024);
    assert!((expected..expected + 3000).contains(&took), "took {} us, expected {}", took, expected);
    // The timer went on meanwhile.
    assert!(cpu.bus.read_32(0x046C) - ticks >= 1);
}

#[test]
fn maximum_speed_takes_no_time() {
    let dir = fatimage::scratch("max_hdd");
    let mut cpu = machine(&dir, DiskSettings::default());
    let took = run(&mut cpu, &OPEN_AND_READ, b"C:\\DATA.BIN\0");
    assert_eq!(cpu.ax(), 0xF000);
    assert!(took < 1000, "took {} us", took);
}

#[test]
fn a_disk_service_in_an_interrupt_handler_waits_on_its_own() {
    let dir = fatimage::scratch("nested");
    let slow = DiskSettings { hard_disk_speed: DiskSpeed::Slow, ..Default::default() };
    let mut cpu = machine(&dir, slow);
    // An INT 1Ch handler at 2000:0200 that seeks in the file (handle at
    // 2000:0300) and counts its calls at 2000:0302.
    let handler = [
        0x1E, 0x50, 0x53, 0x51, 0x52, // PUSH DS, AX, BX, CX, DX
        0xB8, 0x00, 0x20, 0x8E, 0xD8, // MOV AX,2000h; MOV DS,AX
        0xB8, 0x01, 0x42, // MOV AX,4201h
        0x8B, 0x1E, 0x00, 0x03, // MOV BX,[0300h]
        0x31, 0xC9, 0x31, 0xD2, // XOR CX,CX; XOR DX,DX
        0xCD, 0x21, // INT 21h
        0xFF, 0x06, 0x02, 0x03, // INC WORD [0302h]
        0x5A, 0x59, 0x5B, 0x58, 0x1F, 0xCF, // POP DX, CX, BX, AX, DS; IRET
    ];
    cpu.bus.load_bytes(0x20200, &handler);
    let code = [
        0x31, 0xC0, 0x8E, 0xC0, // XOR AX,AX; MOV ES,AX
        0x26, 0xC7, 0x06, 0x70, 0x00, 0x00, 0x02, // MOV WORD ES:[70h],0200h
        0x26, 0xC7, 0x06, 0x72, 0x00, 0x00, 0x20, // MOV WORD ES:[72h],2000h
        0xB8, 0x00, 0x3D, 0xBA, 0x00, 0x01, 0xCD, 0x21, // open C:\DATA.BIN
        0xA3, 0x00, 0x03, // MOV [0300h],AX
        0x89, 0xC3, 0xB8, 0x00, 0x50, 0x8E, 0xD8, 0x31, 0xD2, 0xB9, 0x00, 0xF0, 0xB4, 0x3F, 0xCD, 0x21, // read
        0xF4, // HLT
    ];
    cpu.bus.write_16(0x20302, 0);
    let took = run(&mut cpu, &code, b"C:\\DATA.BIN\0");
    assert_eq!(cpu.ax(), 0xF000);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cpu.sp(), 0x0400);
    let calls = cpu.bus.read_16(0x20302);
    assert!(calls >= 1, "the handler ran during the read");
    assert!(took >= 100_000, "took {} us", took);
}

#[test]
fn slow_floppies_read_sectors_slowly_and_noisily() {
    let dir = fatimage::scratch("slow_fdd");
    let image = fatimage::write(&dir, "disk.img", &fatimage::image(fatimage::FLOPPY_1440, None, &[], &[]));
    let slow = DiskSettings { floppy_disk_speed: DiskSpeed::Slow, floppy_disk_noise: NoiseMode::On, ..Default::default() };
    let mut cpu = machine(&dir, slow);
    cpu.bus.mount_drive(0, &image, MountOptions::default(), false).unwrap();
    cpu.bus.audio_catch_up();
    cpu.bus.audio_out.clear();
    // MOV AX,5000h; MOV ES,AX; XOR BX,BX; MOV AX,0212h; MOV CX,1;
    // XOR DX,DX; INT 13h; HLT: a track of 18 sectors.
    let code = [0xB8, 0x00, 0x50, 0x8E, 0xC0, 0x31, 0xDB, 0xB8, 0x12, 0x02, 0xB9, 0x01, 0x00, 0x31, 0xD2, 0xCD, 0x13, 0xF4];
    let took = run(&mut cpu, &code, &[]);
    assert_eq!((cpu.get_cpu_flag(CpuFlags::CF), cpu.ax()), (false, 0x0012));
    let expected = 18 * 512 * 1_000_000 / (30 * 1024);
    assert!((expected..expected + 3000).contains(&took), "took {} us, expected {}", took, expected);

    cpu.bus.audio_catch_up();
    let loudest = cpu.bus.audio_out.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    assert!(loudest > 100, "the drive was heard: {}", loudest);
}

#[test]
fn quiet_drives_stay_quiet() {
    let dir = fatimage::scratch("quiet");
    let mut cpu = machine(&dir, DiskSettings::default());
    cpu.bus.audio_catch_up();
    cpu.bus.audio_out.clear();
    run(&mut cpu, &OPEN_AND_READ, b"C:\\DATA.BIN\0");
    cpu.bus.audio_catch_up();
    assert!(cpu.bus.audio_out.iter().all(|&s| s == 0));
}

#[test]
fn a_program_waiting_for_its_disk_is_running() {
    let dir = fatimage::scratch("waiting");
    let mut cpu = machine(&dir, DiskSettings::default());
    cpu.set_cs(0x2000);
    cpu.set_ip(0x0100);
    cpu.set_ss(0x3000);
    cpu.set_sp(0x0400);
    // Early on, the deadline's second word is 0, as the shell's CS is.
    rust_dos::diskio::wait_before(&mut cpu, 1_000_000);
    assert_eq!((cpu.cs(), cpu.ip()), (0xF000, rust_dos::bios::IO_WAIT));
    assert!(!cpu.shell_idle());
}
