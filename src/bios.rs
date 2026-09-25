//! The BIOS ROM at F000: the emulator service traps, the default interrupt
//! handlers, the machine identification bytes and the reset entry point,
//! and the interrupt vector table programs start with.

use crate::bus::Bus;
use crate::video::adapter::Adapter;
use crate::cpu::Cpu;

/// Vectors handled by emulator services (`FE 38 vv` traps). Their traps sit
/// four bytes apart from F000:1000 in this order; new vectors go at the end
/// because programs may remember the addresses of the older ones.
pub const HLE_VECTORS: [u8; 20] = [
    0x08, 0x09, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x1A, 0x20, 0x21, 0x2F, 0x33, 0x00,
    0x06, 0x25, 0x26, 0x67,
];
const TRAP_BASE: u16 = 0x1000;

/// Inline emulator services (`FE 39 nn`) used by the ROM code.
pub const SERVICE_XMS: u8 = 0x01;
pub const SERVICE_TIMER_TICK: u8 = 0x08;
/// The strategy and interrupt entries of the CD-ROM driver (MSCDEX).
pub const SERVICE_CD_STRATEGY: u8 = 0x15;
pub const SERVICE_CD_INTERRUPT: u8 = 0x16;
/// The VESA window function (WinFuncPtr).
pub const SERVICE_VBE_WINDOW: u8 = 0x17;
/// Waiting for slow disk access to end (`diskio::wait`).
pub const SERVICE_IO_WAIT: u8 = 0x18;
/// Esc, Tab, Up and Down at the DOS prompt: the line editing (`shell::edit_key`).
pub const SERVICE_SHELL_KEY: u8 = 0x19;
pub const SERVICE_POST: u8 = 0xF0;

/// Offsets in the F000 segment.
const TIMER_HANDLER: u16 = 0x1100;
const MASTER_EOI_HANDLER: u16 = 0x1110;
const SLAVE_EOI_HANDLER: u16 = 0x1120;
const IRQ9_HANDLER: u16 = 0x1130;
/// The XMS driver entry point (INT 2Fh AX=4310h).
pub const XMS_ENTRY: u16 = 0x1140;
/// The diskette parameter table INT 1Eh points to, for 1.44 MB drives.
pub const DISKETTE_PARAMS: u16 = 0x1150;
/// Where disk services wait for slow disk access to end, after the CD-ROM
/// driver's entries.
pub const IO_WAIT: u16 = 0x1190;
/// Where the IBM PC BIOS keeps its dummy interrupt handler (an IRET).
const IRET_HANDLER: u16 = 0xFF53;
const RESET_VECTOR: u16 = 0xFFF0;

const ROM: usize = 0xF0000;

fn far(offset: u16) -> u32 {
    (0xF000 << 16) | offset as u32
}

/// Where the Tandy 1000's BIOS has its name, whose first byte (21h, "!")
/// games look for: F000:C000.
const TANDY_SIGNATURE: u16 = 0xC000;
/// The Tandy's BIOS name, as DOSBox has it.
const TANDY_BIOS_NAME: &[u8] = b"!BIOS ROM version 02.00.00\r\nCompatibility Software\r\nCopyright (C) 1984,1985,1986,1987\r\nPhoenix Software Associates Ltd.\r\nand Tandy";

/// What says which machine this is, for the display adapter `bus` has:
/// the model byte at F000:FFFE (FFh a Tandy 1000, FDh a PCjr, FCh an AT),
/// the Tandy's BIOS name at F000:C000, and the base memory (BDA 0413h),
/// less the video memory at the top of 640 KB on a Tandy.
pub fn set_machine_id(bus: &mut Bus) {
    let adapter = bus.vga.adapter;
    let model = match adapter {
        Adapter::Tandy => 0xFF,
        Adapter::Pcjr => 0xFD,
        _ => 0xFC,
    };
    write_rom(bus, 0xFFFE, &[model, 0x00]);
    let mut name = [0u8; TANDY_BIOS_NAME.len()];
    if adapter == Adapter::Tandy {
        name.copy_from_slice(TANDY_BIOS_NAME);
    }
    write_rom(bus, TANDY_SIGNATURE, &name);
    let kb = crate::mcb::conventional_end(bus) / 64;
    bus.write_16(0x0413, kb);
}

fn write_rom(bus: &mut Bus, offset: u16, code: &[u8]) {
    bus.load_bytes(ROM + offset as usize, code);
}

/// The vector table the BIOS sets up: service traps, the timer and IRQ
/// handlers, and an IRET for everything else. Entries are segment:offset.
pub fn default_ivt() -> [u32; 256] {
    let mut ivt = [far(IRET_HANDLER); 256];
    for irq in 0x0A..=0x0F {
        ivt[irq] = far(MASTER_EOI_HANDLER);
    }
    for irq in 0x70..=0x77 {
        ivt[irq] = far(SLAVE_EOI_HANDLER);
    }
    ivt[0x71] = far(IRQ9_HANDLER);
    for (i, &vector) in HLE_VECTORS.iter().enumerate() {
        ivt[vector as usize] = far(TRAP_BASE + 4 * i as u16);
    }
    ivt[0x08] = far(TIMER_HANDLER);
    ivt[0x1E] = far(DISKETTE_PARAMS);
    // The video BIOS's fonts: the second half of the 8x8 font for the CGA
    // graphics modes, and the graphics font of the mode it starts in.
    use crate::video::bios::{FONT_8X8, FONT_8X8_HIGH, rom_pointer};
    ivt[0x1F] = rom_pointer(FONT_8X8_HIGH);
    ivt[0x43] = rom_pointer(FONT_8X8);
    ivt
}

/// Write the ROM code and data, and the default vector table.
pub fn install(bus: &mut Bus) {
    for (i, &vector) in HLE_VECTORS.iter().enumerate() {
        write_rom(bus, TRAP_BASE + 4 * i as u16, &[0xFE, 0x38, vector, 0xCF]);
    }
    // Timer (IRQ 0): count the tick, call the user timer hook INT 1Ch,
    // acknowledge the interrupt, as the IBM BIOS does.
    write_rom(
        bus,
        TIMER_HANDLER,
        &[
            0xFE, 0x39, SERVICE_TIMER_TICK, // count the tick
            0xCD, 0x1C, // INT 1Ch
            0x50, // PUSH AX
            0xB0, 0x20, // MOV AL, 20h
            0xE6, 0x20, // OUT 20h, AL
            0x58, // POP AX
            0xCF, // IRET
        ],
    );
    // IRQs nothing else handles: acknowledge them.
    write_rom(bus, MASTER_EOI_HANDLER, &[0x50, 0xB0, 0x20, 0xE6, 0x20, 0x58, 0xCF]);
    write_rom(
        bus,
        SLAVE_EOI_HANDLER,
        &[0x50, 0xB0, 0x20, 0xE6, 0xA0, 0xE6, 0x20, 0x58, 0xCF],
    );
    // IRQ 9 is the AT's rerouted IRQ 2: acknowledge the slave and run the
    // IRQ 2 handler (INT 0Ah), which acknowledges the master.
    write_rom(
        bus,
        IRQ9_HANDLER,
        &[0x50, 0xB0, 0x20, 0xE6, 0xA0, 0x58, 0xCD, 0x0A, 0xCF],
    );
    // The XMS entry starts with a short jump over three NOPs, so programs
    // can hook it, as the XMS spec requires; then the driver, then RETF.
    write_rom(
        bus,
        XMS_ENTRY,
        &[0xEB, 0x03, 0x90, 0x90, 0x90, 0xFE, 0x39, SERVICE_XMS, 0xCB],
    );
    // Step rate and head unload, head load, motor off delay, 512-byte
    // sectors, 18 per track, gap length, data length, format gap length,
    // format filler, head settle and motor start times.
    write_rom(
        bus,
        DISKETTE_PARAMS,
        &[0xDF, 0x02, 0x25, 0x02, 0x12, 0x1B, 0xFF, 0x6C, 0xF6, 0x0F, 0x08],
    );
    // Disk services wait here for slow disk access (`diskio::wait`).
    write_rom(bus, IO_WAIT, &[0xFE, 0x39, SERVICE_IO_WAIT]);
    write_rom(bus, IRET_HANDLER, &[0xCF]);
    write_rom(bus, RESET_VECTOR, &[0xFE, 0x39, SERVICE_POST]);
    // BIOS date, the model byte and the base memory.
    write_rom(bus, 0xFFF5, b"01/10/92");
    set_machine_id(bus);

    let ivt = default_ivt();
    for (vector, &entry) in ivt.iter().enumerate() {
        bus.write_16(vector * 4, entry as u16);
        bus.write_16(vector * 4 + 2, (entry >> 16) as u16);
    }
    // The video BIOS's VESA data.
    crate::interrupts::vbe::install_rom(bus);
}

/// Put the default vectors back, except those pointing into `keep`
/// (the memory of resident programs, whose hooks must survive).
pub fn restore_ivt(bus: &mut Bus, keep: &[std::ops::Range<usize>]) {
    for (vector, &entry) in default_ivt().iter().enumerate() {
        let offset = bus.read_16(vector * 4) as usize;
        let segment = bus.read_16(vector * 4 + 2) as usize;
        let at = (segment << 4) + offset;
        if keep.iter().any(|range| range.contains(&at)) {
            continue;
        }
        bus.write_16(vector * 4, entry as u16);
        bus.write_16(vector * 4 + 2, (entry >> 16) as u16);
    }
}

/// The reset entry point (F000:FFF0), reached after a CPU reset through the
/// keyboard controller, port 92h, or a triple fault. Like an AT BIOS, check
/// the CMOS shutdown status: codes 05h and 0Ah resume a program through the
/// far pointer at 40:67, which is how 286-era protected mode code gets back
/// to real mode. Anything else is a cold boot, which ends the program.
pub fn post(cpu: &mut Cpu) {
    let status = cpu.bus.cmos.get(crate::cmos::SHUTDOWN_STATUS);
    cpu.bus.cmos.set(crate::cmos::SHUTDOWN_STATUS, 0);
    match status {
        0x05 | 0x0A => {
            if status == 0x05 {
                // Flush the keyboard and acknowledge any interrupt.
                cpu.bus.io_write(0x20, 0x20);
                cpu.bus.io_write(0xA0, 0x20);
            }
            let offset = cpu.bus.read_16(0x0467);
            let segment = cpu.bus.read_16(0x0469);
            cpu.bus.log_string(&format!(
                "[BIOS] Reset with shutdown code {:02X}h: resuming at {:04X}:{:04X}",
                status, segment, offset
            ));
            cpu.set_cs(segment);
            cpu.set_ip(offset);
        }
        _ => {
            cpu.bus.log_string(&format!(
                "[BIOS] CPU reset (shutdown code {:02X}h): ending the program",
                status
            ));
            cpu.state = crate::cpu::CpuState::RebootShell;
        }
    }
}
