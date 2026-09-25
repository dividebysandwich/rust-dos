use crate::cpu::{Cpu, CpuFlags};
use iced_x86::Register;

/// Extended memory above 1 MB, in KB.
fn extended_kb(cpu: &Cpu) -> usize {
    (cpu.bus.ram().len() >> 10).saturating_sub(1024)
}

/// Report success (CF clear, AH 0).
fn ok(cpu: &mut Cpu) {
    cpu.set_reg8(Register::AH, 0);
    cpu.set_cpu_flag(CpuFlags::CF, false);
}

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    let al = cpu.get_al();
    match ah {
        // A20 gate: disable, enable, query, and which methods exist
        // (keyboard controller and port 92h).
        0x24 => match al {
            0x00 | 0x01 => {
                cpu.bus.set_a20(al == 0x01);
                ok(cpu);
            }
            0x02 => {
                let a20 = cpu.bus.a20() as u8;
                ok(cpu);
                cpu.set_reg8(Register::AL, a20);
            }
            0x03 => {
                ok(cpu);
                cpu.set_bx(0x0003);
            }
            _ => unsupported(cpu),
        },
        // Extended memory size in KB. The XMS driver owns all of it, so
        // this reports none, as with HIMEM.SYS loaded: programs that took
        // the memory this way would overwrite XMS blocks.
        0x88 => {
            cpu.set_ax(0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        0x87 => block_move(cpu),
        // The BIOS's joystick: DX=0 reads the buttons into bits 4-7 of AL,
        // DX=1 the axes of joysticks A and B into AX, BX, CX and DX.
        0x84 if cpu.bus.joystick.present() && cpu.dx() <= 1 => {
            let mouse = &cpu.bus.mouse;
            if cpu.dx() == 0 {
                let switches = cpu.bus.joystick.bios_switches(mouse);
                cpu.set_reg8(Register::AL, switches);
            } else {
                let [ax, ay, bx, by] = cpu.bus.joystick.bios_axes(mouse);
                cpu.set_ax(ax);
                cpu.set_bx(ay);
                cpu.set_cx(bx);
                cpu.set_dx(by);
            }
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        // AX=E801h: memory between 1 and 16 MB in KB (AX, CX), and above
        // 16 MB in 64 KB blocks (BX, DX).
        0xE8 if al == 0x01 => {
            let kb = extended_kb(cpu);
            let below_16m = kb.min(15 * 1024) as u16;
            let above_16m = (kb.saturating_sub(15 * 1024) / 64) as u16;
            cpu.set_ax(below_16m);
            cpu.set_cx(below_16m);
            cpu.set_bx(above_16m);
            cpu.set_dx(above_16m);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        0x86 => {
            // Wait CX:DX microseconds of emulated time, with interrupts
            // enabled, as the BIOS does.
            let micros = ((cpu.cx() as u64) << 16) | (cpu.dx() as u64);
            let now = cpu.bus.clock.now_ticks();
            let until = *cpu.bios_wait_until.get_or_insert(now + micros * crate::timer::PIT_HZ / 1_000_000);
            if now >= until {
                cpu.bios_wait_until = None;
                cpu.set_cpu_flag(CpuFlags::CF, false);
            } else {
                cpu.hle_wait();
                let clock = &mut cpu.bus.clock;
                clock.deadline = clock.deadline.min(clock.icount_at(until));
            }
        }
        0xC0 => {
            // Get System Configuration
            // Return ES:BX pointing to Config Table (8 bytes)
            // We'll construct a dummy table at F000:E800 (Phys FE800)
            let table_seg = 0xF000;
            let table_off = 0xE800;
            let phys_addr = 0xFE800;

            // Byte 0-1: Length (8)
            cpu.bus.write_16(phys_addr, 0x0008);
            // Byte 2: Model, as at F000:FFFE (FC = AT, FF = Tandy 1000, FD
            // = PCjr)
            let model = cpu.bus.read_8(0xFFFFE);
            cpu.bus.write_8(phys_addr + 2, model);
            // Byte 3: Submodel (01 = AT)
            cpu.bus.write_8(phys_addr + 3, 0x01);
            // Byte 4: BIOS Revision (0)
            cpu.bus.write_8(phys_addr + 4, 0x00);
            // Byte 5: Feature Info 1: RTC, second 8259
            cpu.bus.write_8(phys_addr + 5, 0x60);
            // Byte 6-9: Reserved/Features
            cpu.bus.write_8(phys_addr + 6, 0x00);
            cpu.bus.write_8(phys_addr + 7, 0x00);

            cpu.set_es(table_seg);
            cpu.set_bx(table_off);
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        _ => unsupported(cpu),
    }
}

/// Unknown function: CF set, AH=86h ("not supported"), as a BIOS answers.
fn unsupported(cpu: &mut Cpu) {
    cpu.bus.log_string(&format!("[BIOS] Unhandled INT 15h AX={:04X}", cpu.ax()));
    cpu.set_reg8(Register::AH, 0x86);
    cpu.set_cpu_flag(CpuFlags::CF, true);
}

/// AH=87h: copy CX words between two physical addresses described by the
/// source and destination descriptors of the GDT at ES:SI.
fn block_move(cpu: &mut Cpu) {
    let gdt = cpu.get_physical_addr(cpu.es(), cpu.si());
    let base = |cpu: &Cpu, desc: usize| -> usize {
        let b = |i| cpu.bus.read_8(desc + i) as usize;
        b(2) | b(3) << 8 | b(4) << 16 | b(7) << 24
    };
    let source = base(cpu, gdt + 0x10);
    let dest = base(cpu, gdt + 0x18);
    let len = cpu.cx() as usize * 2;
    for i in 0..len {
        let byte = cpu.bus.read_8(source + i);
        cpu.bus.write_8(dest + i, byte);
    }
    ok(cpu);
    cpu.set_cpu_flag(CpuFlags::ZF, true);
}
