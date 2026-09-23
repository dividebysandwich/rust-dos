use iced_x86::Register;
use rust_dos::cpu::{Cpu, CpuFlags, CpuState};
use rust_dos::interrupts::{int21, int33};
use rust_dos::mcb::{self, FIRST_MCB_SEG, MCB_M, MCB_Z, walk};
use std::fs;
use std::path::PathBuf;

/// Fresh directory with the given files under target/.
fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_tsr").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

fn int21(cpu: &mut Cpu, ah: u8) {
    cpu.set_reg8(Register::AH, ah);
    int21::handle(cpu);
}

fn ivt(cpu: &Cpu, vector: usize) -> (u16, u16) {
    (cpu.bus.read_16(vector * 4 + 2), cpu.bus.read_16(vector * 4))
}

fn set_ivt(cpu: &mut Cpu, vector: usize, seg: u16, off: u16) {
    cpu.bus.write_16(vector * 4, off);
    cpu.bus.write_16(vector * 4 + 2, seg);
}

#[test]
fn list_of_lists_points_at_first_mcb_and_dpb() {
    let base = scratch("lol", &[]);
    let mut cpu = Cpu::new(base);
    int21(&mut cpu, 0x52);
    let lol = cpu.get_physical_addr(cpu.es, cpu.bx);
    assert_eq!(cpu.bus.read_16(lol - 2), FIRST_MCB_SEG);

    // The first DPB is C:, the only drive mounted.
    let dpb = cpu.get_physical_addr(cpu.bus.read_16(lol + 2), cpu.bus.read_16(lol));
    assert_eq!(cpu.bus.read_8(dpb), 2);
    assert_eq!(cpu.bus.read_8(lol + 0x21), 26); // LASTDRIVE
    let nul: Vec<u8> = (0..8).map(|i| cpu.bus.read_8(lol + 0x2C + i)).collect();
    assert_eq!(&nul, b"NUL     ");
    assert_eq!(cpu.bus.read_16(lol + 0x22), 0xFFFF); // end of the driver chain
}

#[test]
fn psp_functions_get_and_set_current_psp() {
    let base = scratch("psp", &[]);
    let mut cpu = Cpu::new(base);
    cpu.current_psp = 0x1234;
    for ah in [0x51, 0x62] {
        cpu.bx = 0;
        int21(&mut cpu, ah);
        assert_eq!(cpu.bx, 0x1234, "AH={:02X}", ah);
    }
    cpu.bx = 0x2000;
    int21(&mut cpu, 0x50);
    assert_eq!(cpu.current_psp, 0x2000);
}

#[test]
fn release_from_keeps_resident_blocks_below() {
    let mut cpu = Cpu::new(scratch("release", &[]));
    let bus = &mut cpu.bus;
    mcb::init_empty(bus);
    let tsr = mcb::alloc(bus, 0x1000, 0x40).unwrap();
    let app = mcb::alloc(bus, 0x2000, 0x100).unwrap();
    let _data = mcb::alloc(bus, 0x2000, 0x10).unwrap();

    // Everything from the app's MCB up is freed; the TSR stays.
    let end = mcb::release_from(bus, app - 1).unwrap();
    assert_eq!(end, app - 1);
    let chain = walk(bus);
    assert_eq!(chain.len(), 2);
    assert_eq!(
        (chain[0].0, chain[0].1.owner, chain[0].1.signature),
        (tsr - 1, 0x1000, MCB_M)
    );
    assert!(chain[1].1.is_free() && chain[1].1.signature == MCB_Z);

    // Once the TSR freed itself the free memory starts at the first MCB.
    mcb::free(bus, tsr).unwrap();
    assert_eq!(mcb::release_from(bus, end), Some(FIRST_MCB_SEG));
    assert_eq!(walk(bus).len(), 1);
}

#[test]
fn tsr_started_from_shell_stays_resident() {
    // TSR.COM and APP.COM just loop; the test drives the DOS calls.
    let base = scratch(
        "resident",
        &[("TSR.COM", &[0xEB, 0xFE]), ("APP.COM", &[0xEB, 0xFE])],
    );
    let mut cpu = Cpu::new(base);
    cpu.load_shell();

    assert!(cpu.load_executable("TSR.COM", None));
    let tsr = cpu.current_psp;
    assert_eq!(tsr, FIRST_MCB_SEG + 1);
    cpu.bus.write_8(cpu.get_physical_addr(tsr, 0x1F0), 0x5A); // resident data
    set_ivt(&mut cpu, 0x08, tsr, 0x0180); // hooked into the TSR
    set_ivt(&mut cpu, 0x16, 0x8000, 0x0000); // hooked outside it

    cpu.dx = 0x20;
    cpu.set_reg8(Register::AL, 0);
    int21(&mut cpu, 0x31);
    assert_eq!(cpu.state, CpuState::RebootShell);
    assert_eq!(cpu.resident_end, tsr + 0x20);
    cpu.load_shell();

    assert_eq!(ivt(&cpu, 0x08), (tsr, 0x0180));
    assert_eq!(ivt(&cpu, 0x16).0, 0xF000);
    let chain = walk(&cpu.bus);
    assert_eq!((chain[0].1.owner, chain[0].1.size), (tsr, 0x20));
    assert!(chain[1].1.is_free() && chain[1].1.is_last());

    // The next program loads above the TSR and leaves it intact.
    assert!(cpu.load_executable("APP.COM", None));
    assert_eq!(cpu.current_psp, tsr + 0x21);
    assert_eq!(cpu.cs, tsr + 0x21);
    assert_eq!(cpu.bus.read_8(cpu.get_physical_addr(tsr, 0x1F0)), 0x5A);
    assert_eq!(ivt(&cpu, 0x08), (tsr, 0x0180));

    // It exits: memory above the TSR is free again.
    cpu.set_reg8(Register::AL, 0);
    int21(&mut cpu, 0x4C);
    cpu.load_shell();
    assert_eq!(cpu.transient_segment(), tsr + 0x21);
    assert_eq!(walk(&cpu.bus).len(), 2);
}

#[test]
fn mouse_handler_is_far_called_and_registers_survive() {
    let mut cpu = Cpu::new(scratch("mouse", &[]));
    // Handler at 2000:0000 stores AX, CX, DX, clobbers AX and returns with RETF.
    let handler = [
        0x2E, 0xA3, 0x00, 0x01, // mov cs:[0100],ax
        0x2E, 0x89, 0x0E, 0x02, 0x01, // mov cs:[0102],cx
        0x2E, 0x89, 0x16, 0x04, 0x01, // mov cs:[0104],dx
        0xB8, 0x34, 0x12, // mov ax,1234h
        0xCB, // retf
    ];
    for (i, b) in handler.iter().enumerate() {
        cpu.bus.write_8(0x20000 + i, *b);
    }
    cpu.ax = 0x000C;
    cpu.cx = 0x0001; // motion
    cpu.es = 0x2000;
    cpu.dx = 0x0000;
    int33::handle(&mut cpu);

    // Interrupted code at 3000:0010 with its own registers.
    cpu.cs = 0x3000;
    cpu.ip = 0x0010;
    cpu.ss = 0x4000;
    cpu.sp = 0x0100;
    cpu.ax = 0xAAAA;
    cpu.cx = 0xCCCC;
    cpu.dx = 0xDDDD;
    cpu.set_cpu_flag(CpuFlags::IF, true);
    cpu.set_cpu_flag(CpuFlags::CF, true);
    cpu.bus.mouse.set_position(100, 50);

    assert!(rust_dos::mouse::deliver_callback(&mut cpu));
    // Not re-entered while the handler runs.
    cpu.bus.mouse.set_position(120, 60);
    assert!(!rust_dos::mouse::deliver_callback(&mut cpu));

    cpu.last_timer_tick = cpu.bus.start_time.elapsed().as_millis();
    for _ in 0..100 {
        if cpu.cs == 0x3000 {
            break;
        }
        cpu.step();
    }
    assert_eq!(
        (cpu.cs, cpu.ip, cpu.ss, cpu.sp),
        (0x3000, 0x0010, 0x4000, 0x0100)
    );
    assert_eq!((cpu.ax, cpu.cx, cpu.dx), (0xAAAA, 0xCCCC, 0xDDDD));
    assert!(cpu.get_cpu_flag(CpuFlags::IF) && cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cpu.bus.read_16(0x20100), 0x0001);
    assert_eq!(
        (cpu.bus.read_16(0x20102), cpu.bus.read_16(0x20104)),
        (100, 50)
    );

    // Done, so the pending motion is delivered now.
    assert!(rust_dos::mouse::deliver_callback(&mut cpu));
}
