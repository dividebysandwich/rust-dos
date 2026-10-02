//! The BIOS ROM at F000: the emulator service traps, the default interrupt
//! handlers, the machine identification bytes and the reset entry point,
//! and the interrupt vector table programs start with.

use crate::bus::Bus;
use crate::video::adapter::Adapter;
use crate::cpu::Cpu;

/// Vectors handled by emulator services (`FE 38 vv` traps). Their traps sit
/// four bytes apart from F000:1000 in this order; new vectors go at the end
/// because programs may remember the addresses of the older ones.
pub const HLE_VECTORS: [u8; 24] = [
    0x08, 0x09, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x1A, 0x20, 0x21, 0x2F, 0x33, 0x00,
    0x06, 0x25, 0x26, 0x67, 0x18, 0x19, 0x27, 0x7A,
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
/// The shell's prompt, printed by `shell::prompt`.
pub const SERVICE_SHELL_PROMPT: u8 = 0x1A;
/// The key PAUSE or CHOICE waited for, handed to `shell::key_ready`.
pub const SERVICE_SHELL_KEY_READY: u8 = 0x1B;
/// A tick while PAUSE or CHOICE waits, for CHOICE's timeout (`shell::tick`).
pub const SERVICE_SHELL_TICK: u8 = 0x1C;
/// A secondary COMMAND.COM asking what to do next (`command_com::service`).
pub const SERVICE_COMMAND: u8 = 0x1D;
/// The PS/2 mouse's byte the IRQ 12 handler read, and the report once it
/// has them all (`mouse::ps2_report`).
pub const SERVICE_PS2_REPORT: u8 = 0x1E;
/// The next port access a service left to `PORT_ACCESSES` (`next_port_access`).
pub const SERVICE_PORT_ACCESS: u8 = 0x1F;
/// The INT 33h event handler the mouse's stub called has returned
/// (`mouse::clear_callback_busy`).
pub const SERVICE_MOUSE_CALLBACK_DONE: u8 = 0x20;
/// The scan code the keyboard interrupt of a booted system read, for the
/// BIOS to keep in its buffer (`keyboard::bios_scan`).
pub const SERVICE_KBD_SCAN: u8 = 0x21;
/// Whether Pause still holds the machine (`keyboard::bios_paused`).
pub const SERVICE_KBD_PAUSED: u8 = 0x22;
/// A call of the IPX driver at its entry point (`net::ipx::api`).
pub const SERVICE_IPX: u8 = 0x23;
/// The IPX IRQ handler's next completed ECB (`net::ipx::esr`).
pub const SERVICE_IPX_ESR: u8 = 0x24;
pub const SERVICE_POST: u8 = 0xF0;

/// Far-call services (`FE 3A nn`, then RETF) at the ROM's entry points for
/// real and protected mode: the Plug and Play BIOS's and APM's.
pub const FAR_PNP: u8 = 0x01;
pub const FAR_APM: u8 = 0x02;

/// Offsets in the F000 segment.
const TIMER_HANDLER: u16 = 0x1100;
const MASTER_EOI_HANDLER: u16 = 0x1110;
const SLAVE_EOI_HANDLER: u16 = 0x1120;
const IRQ9_HANDLER: u16 = 0x1130;
/// The diskette parameter table INT 1Eh points to, for 1.44 MB drives.
pub const DISKETTE_PARAMS: u16 = 0x1150;
/// Where disk services wait for slow disk access to end, after the CD-ROM
/// driver's entries.
pub const IO_WAIT: u16 = 0x1190;
/// The PS/2 mouse's IRQ 12 handler (INT 74h), and where it finds the far
/// address of the program's handler (INT 15h AX=C207h).
const PS2_HANDLER: u16 = 0x11A0;
pub const PS2_HANDLER_ADDRESS: u16 = 0x11E0;
/// Where services in virtual-8086 mode return through when they changed
/// hardware a V86 monitor follows through the ports it traps: it makes the
/// port accesses that make the changes (`Bus::port_accesses`), then IRETs.
pub const PORT_ACCESSES: u16 = 0x11F0;
/// The rest of the port accesses' kinds (`port_access_more`), which don't
/// fit the loop.
const PORT_ACCESSES_MORE: u16 = 0x1500;
/// The keyboard interrupt (IRQ 1) of a booted system, which keeps the
/// keystrokes in the BIOS data area as a PC's BIOS does.
const KBD_HANDLER: u16 = 0x1230;
/// The fixed disk parameter tables of the first two hard disks, which
/// INT 41h and 46h point to.
const FIXED_DISK_PARAMS: u16 = 0x12C0;
/// Where a DOS service in virtual-8086 mode that waits for a key goes
/// between its tries (`exec::service_trap`): the keyboard busy loop (INT
/// 2Ah AX=8400h) and the idle interrupt (INT 28h), as DOS's own loop calls
/// them, for a V86 monitor to see the machine idle, then the INT 21h trap
/// again.
pub const DOS_IDLE: u16 = 0x1310;
/// The IPX driver's entry point, which INT 2Fh AX=7A00h hands out, and
/// its IRQ handler, which calls the ESRs of completed ECBs.
pub const IPX_ENTRY: u16 = 0x1400;
pub const IPX_IRQ: u16 = 0x1410;
/// Where the IBM PC BIOS keeps its dummy interrupt handler (an IRET).
pub const IRET_HANDLER: u16 = 0xFF53;
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
    bus.write_rom(ROM + offset as usize, code);
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
    ivt[0x74] = far(PS2_HANDLER);
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

/// The vector table the BIOS leaves an operating system it boots
/// (`boot::power_on`): the default one without the built-in DOS's and its
/// drivers' services, with a keyboard interrupt that keeps the keystrokes
/// in the BIOS data area, and the fixed disk parameter tables.
pub fn boot_ivt() -> [u32; 256] {
    let mut ivt = default_ivt();
    for vector in [0x00, 0x20, 0x21, 0x25, 0x26, 0x27, 0x2F, 0x33, 0x67, 0x7A] {
        ivt[vector] = far(IRET_HANDLER);
    }
    ivt[0x09] = far(KBD_HANDLER);
    ivt[0x41] = far(FIXED_DISK_PARAMS);
    ivt[0x46] = far(FIXED_DISK_PARAMS + 16);
    ivt
}

fn write_ivt(bus: &mut Bus, ivt: &[u32; 256]) {
    for (vector, &entry) in ivt.iter().enumerate() {
        bus.write_16(vector * 4, entry as u16);
        bus.write_16(vector * 4 + 2, (entry >> 16) as u16);
    }
    // The IPX driver's IRQ is its own while it is installed.
    bus.arm_ipx_irq();
}

/// Set up the vector table and the BIOS data area as a PC's BIOS leaves
/// them for the operating system it boots, in memory cleared to zeros,
/// with `hard_disks` the geometries of the hard disks as INT 13h has them.
pub fn install_for_boot(bus: &mut Bus, hard_disks: &[crate::diskimage::Chs]) {
    write_ivt(bus, &boot_ivt());
    // The fixed disk parameter tables: cylinders, heads, no reduced write
    // current, no write precompensation, the control byte (8: more than
    // eight heads), the landing zone and the sectors per track.
    for i in 0..2 {
        let mut table = [0u8; 16];
        if let Some(chs) = hard_disks.get(i) {
            let cylinders = chs.cylinders.clamp(1, 1024) as u16;
            table[0..2].copy_from_slice(&cylinders.to_le_bytes());
            table[2] = chs.heads as u8;
            table[5..7].copy_from_slice(&0xFFFFu16.to_le_bytes());
            table[8] = if chs.heads > 8 { 0x08 } else { 0 };
            table[12..14].copy_from_slice(&cylinders.to_le_bytes());
            table[14] = chs.sectors as u8;
        }
        write_rom(bus, FIXED_DISK_PARAMS + 16 * i as u16, &table);
    }

    // The serial ports (the parallel port after the equipment word).
    let serial = bus.serial.bios_ports();
    for (n, base) in serial.iter().enumerate() {
        bus.write_16(0x0400 + 2 * n, *base);
    }
    // The equipment word: two floppy drives (bits 0 and 6-7), the
    // coprocessor (bit 1), a PS/2 mouse (bit 2), the video adapter's
    // initial mode (bits 4-5, from `video::bios::install`), the game port
    // (bit 12), the serial ports (bits 9-11) and the parallel port (bits
    // 14-15).
    let mut equipment = 0x0047 | (serial.len() as u16) << 9;
    equipment |= if bus.vga.setup().mono() { 0x0030 } else { 0x0020 };
    if bus.joystick.present() {
        equipment |= 0x1000;
    }
    bus.write_16(0x0410, equipment);
    bus.write_lpt_bda();
    set_machine_id(bus);
    // The keyboard: Num Lock on, an enhanced keyboard, and the buffer
    // from 40:1E to 40:3E, empty.
    bus.write_8(0x0417, 0x20);
    bus.write_8(0x0496, crate::keyboard::ENHANCED_KEYBOARD);
    bus.write_8(0x0497, 0x02);
    bus.write_16(0x041A, 0x001E);
    bus.write_16(0x041C, 0x001E);
    bus.write_16(0x0480, 0x001E);
    bus.write_16(0x0482, 0x003E);
    // The hard disks.
    bus.write_8(0x0475, hard_disks.len().min(0xFF) as u8);
    // The ticks since midnight, from the real-time clock.
    let now = bus.cmos.now();
    let seconds = chrono::Timelike::num_seconds_from_midnight(&now) as u64;
    let ticks = seconds * crate::timer::PIT_HZ / 65536;
    bus.write_16(0x046C, ticks as u16);
    bus.write_16(0x046E, (ticks >> 16) as u16);
}

/// The offset in F000 of the trap of the HLE vector `vector`.
fn hle_trap(vector: u8) -> u16 {
    let i = HLE_VECTORS.iter().position(|&v| v == vector).expect("an HLE vector");
    TRAP_BASE + 4 * i as u16
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
    let [trap_lo, trap_hi] = hle_trap(0x21).to_le_bytes();
    write_rom(
        bus,
        DOS_IDLE,
        &[
            0xFB, // STI
            0x50, // PUSH AX
            0xB8, 0x00, 0x84, // MOV AX, 8400h
            0xCD, 0x2A, // INT 2Ah
            0x58, // POP AX
            0xCD, 0x28, // INT 28h
            0xEA, trap_lo, trap_hi, 0x00, 0xF0, // JMP FAR F000:trap
        ],
    );
    // The PS/2 mouse (IRQ 12), as an IBM PS/2 BIOS runs it: save the
    // registers, read the mouse's byte, and with a report's last and a
    // handler installed push the status, X, Y and a 0 word and CALL FAR
    // it; then acknowledge both PICs.
    let [handler_lo, handler_hi] = PS2_HANDLER_ADDRESS.to_le_bytes();
    write_rom(
        bus,
        PS2_HANDLER,
        &[
            0x1E, 0x50, 0x53, 0x51, 0x52, 0x56, 0x57, 0x55, 0x06, // PUSH DS, AX, BX, CX, DX, SI, DI, BP, ES
            0xE4, 0x60, // IN AL, 60h
            0xFE, 0x39, SERVICE_PS2_REPORT, // AX, BX, CX: the report; DX: call the handler
            0x85, 0xD2, // TEST DX, DX
            0x74, 0x0E, // JZ done
            0x50, 0x53, 0x51, // PUSH AX, BX, CX
            0x31, 0xC0, 0x50, // XOR AX, AX; PUSH AX
            0x2E, 0xFF, 0x1E, handler_lo, handler_hi, // CALL FAR CS:[handler]
            0x83, 0xC4, 0x08, // ADD SP, 8
            0xB0, 0x20, // done: MOV AL, 20h
            0xE6, 0xA0, // OUT A0h, AL
            0xE6, 0x20, // OUT 20h, AL
            0x07, 0x5D, 0x5F, 0x5E, 0x5A, 0x59, 0x5B, 0x58, 0x1F, // POP ES, BP, DI, SI, DX, CX, BX, AX, DS
            0xCF, // IRET
        ],
    );
    // The IPX driver: its entry point, and its IRQ, which hands each
    // completed ECB with an event service routine to it.
    write_rom(bus, IPX_ENTRY, &[0xFE, 0x39, SERVICE_IPX, 0xCB]);
    write_rom(bus, IPX_IRQ, &ipx_irq_handler());
    // Services in V86 mode: make each port access the service left, then
    // return.
    write_rom(bus, PORT_ACCESSES, &port_access_loop());
    write_rom(bus, PORT_ACCESSES_MORE, &port_access_more());
    write_rom(bus, KBD_HANDLER, &keyboard_handler());
    // The Plug and Play BIOS's and APM's entry points: the service, then a
    // far return (32-bit in APM's 32-bit code segment).
    write_rom(bus, crate::pnpbios::ENTRY, &[0xFE, 0x3A, FAR_PNP, 0xCB]);
    write_rom(bus, crate::apm::ENTRY, &[0xFE, 0x3A, FAR_APM, 0xCB]);
    crate::pnpbios::install(bus);
    crate::dpmi::install_rom(bus);
    write_rom(bus, IRET_HANDLER, &[0xCF]);
    write_rom(bus, RESET_VECTOR, &[0xFE, 0x39, SERVICE_POST]);
    // BIOS date, the model byte and the base memory.
    write_rom(bus, 0xFFF5, b"01/10/92");
    set_machine_id(bus);

    write_ivt(bus, &default_ivt());
    // The video BIOS's VESA data.
    crate::interrupts::vbe::install_rom(bus);
}

/// The way out of services in V86 mode (`PORT_ACCESSES`): each port access
/// or memory fill the service left (`next_port_access`), made by the
/// processor as a BIOS makes it, then IRET. A fill is a REP STOSW, which
/// goes on after a page fault from where it stopped: a V86 monitor maps a
/// machine's video memory a page at a time.
fn port_access_loop() -> Vec<u8> {
    let mut a = crate::asm16::Asm::new(PORT_ACCESSES);
    a.op(&[0x50, 0x53, 0x51, 0x52, 0x57, 0x06]); // PUSH AX, BX, CX, DX, DI, ES
    a.label("next");
    a.op(&[0xFE, 0x39, SERVICE_PORT_ACCESS]); // BL: what to do
    a.op(&[0x80, 0xFB, 0x01]); // CMP BL, 1
    a.jump(0x72, "write");
    a.jump(0x74, "read");
    a.op(&[0x80, 0xFB, 0x03]); // CMP BL, 3
    a.jump(0x72, "update");
    a.jump(0x74, "fill");
    a.op(&[0x80, 0xFB, 0xFF]); // CMP BL, FFh
    a.jump(0x75, "more");
    a.op(&[0x07, 0x5F, 0x5A, 0x59, 0x5B, 0x58, 0xCF]); // POP ES, DI, DX, CX, BX, AX; IRET
    a.label("write");
    a.op(&[0xEE]); // OUT DX, AL
    a.jump(0xEB, "next");
    a.label("read");
    a.op(&[0xEC]); // IN AL, DX
    a.jump(0xEB, "next");
    a.label("update");
    a.op(&[0xEC, 0x22, 0xC1, 0x0A, 0xC5, 0xEE]); // IN AL, DX; AND AL, CL; OR AL, CH; OUT DX, AL
    a.jump(0xEB, "next");
    a.label("fill");
    a.op(&[0xFC, 0xF3, 0xAB]); // CLD; REP STOSW
    a.jump(0xEB, "next");
    a.label("more");
    let [lo, hi] = PORT_ACCESSES_MORE.to_le_bytes();
    a.op(&[0xEA, lo, hi, 0x00, 0xF0]); // JMP FAR F000:PORT_ACCESSES_MORE
    let code = a.finish();
    assert!(PORT_ACCESSES as usize + code.len() <= KBD_HANDLER as usize);
    code
}

/// The port accesses a BIOS makes driving an IDE disk (`ide::int13`),
/// from the loop: 04h CLI, 05h a status read at DX until the bits of BH
/// clear (at most FFFFh times), 06h CX word reads of DX; then back to the
/// loop for the next.
fn port_access_more() -> Vec<u8> {
    let mut a = crate::asm16::Asm::new(PORT_ACCESSES_MORE);
    a.op(&[0x80, 0xFB, 0x04]); // CMP BL, 4
    a.jump(0x75, "wait");
    a.op(&[0xFA]); // CLI
    a.jump(0xEB, "back");
    a.label("wait");
    a.op(&[0x80, 0xFB, 0x05]); // CMP BL, 5
    a.jump(0x75, "words");
    a.op(&[0xB9, 0xFF, 0xFF]); // MOV CX, FFFFh
    a.label("busy");
    a.op(&[0xEC, 0x84, 0xF8]); // IN AL, DX; TEST AL, BH
    a.jump(0xE0, "busy"); // LOOPNZ
    a.jump(0xEB, "back");
    a.label("words");
    a.op(&[0xED]); // IN AX, DX
    a.jump(0xE2, "words"); // LOOP
    a.label("back");
    let [lo, hi] = (PORT_ACCESSES + 6).to_le_bytes();
    a.op(&[0xEA, lo, hi, 0x00, 0xF0]); // JMP FAR F000:next
    let code = a.finish();
    assert!(PORT_ACCESSES_MORE as usize + code.len() <= 0x1600);
    code
}

/// The IPX driver's IRQ: save the registers, and while the driver hands
/// out a completed ECB (ES:SI, AL FFh or 00h), CALL FAR its event service
/// routine; then acknowledge both PICs.
fn ipx_irq_handler() -> Vec<u8> {
    let mut a = crate::asm16::Asm::new(IPX_IRQ);
    a.op(&[0x1E, 0x06, 0x50, 0x53, 0x51, 0x52, 0x56, 0x57, 0x55]); // PUSH DS, ES, AX, BX, CX, DX, SI, DI, BP
    a.label("next");
    a.op(&[0xFE, 0x39, SERVICE_IPX_ESR]); // CF: none left
    a.jump(0x72, "done");
    a.op(&[0x26, 0xFF, 0x5C, 0x04]); // CALL FAR ES:[SI+4]
    a.jump(0xEB, "next");
    a.label("done");
    a.op(&[0xB0, 0x20, 0xE6, 0xA0, 0xE6, 0x20]); // MOV AL, 20h; OUT A0h, AL; OUT 20h, AL
    a.op(&[0x5D, 0x5F, 0x5E, 0x5A, 0x59, 0x5B, 0x58, 0x07, 0x1F]); // POP BP, DI, SI, DX, CX, BX, AX, ES, DS
    a.op(&[0xCF]); // IRET
    let code = a.finish();
    assert!(IPX_IRQ as usize + code.len() <= 0x1500);
    code
}

/// Whether the vector table entry `entry` (segment:offset) is one of the
/// BIOS's own IRQ handlers that only acknowledge the interrupt, or the
/// IPX driver's, which a device may take the IRQ over from.
pub fn is_default_irq_handler(entry: u32) -> bool {
    [MASTER_EOI_HANDLER, SLAVE_EOI_HANDLER, IRQ9_HANDLER, IRET_HANDLER, IPX_IRQ].into_iter().any(|h| far(h) == entry)
}

/// The keyboard interrupt of a booted system, as a PC's BIOS runs it:
/// read the scan code, let INT 15h AH=4Fh take it or change it, keep the
/// keystroke (`keyboard::bios_scan`), and acknowledge the interrupt.
/// Ctrl+Break calls INT 1Bh, Print Screen INT 05h, and Pause holds the
/// machine here until another key.
fn keyboard_handler() -> Vec<u8> {
    let mut a = crate::asm16::Asm::new(KBD_HANDLER);
    a.op(&[0x50]); // PUSH AX
    a.op(&[0xE4, 0x60]); // IN AL, 60h
    a.op(&[0xB4, 0x4F, 0xF9, 0xCD, 0x15]); // MOV AH, 4Fh; STC; INT 15h
    a.jump(0x73, "eoi"); // JNC eoi: the hook took the key
    a.op(&[0xFE, 0x39, SERVICE_KBD_SCAN]); // AH: what else to do
    a.op(&[0x80, 0xFC, 0x01]); // CMP AH, 1
    a.jump(0x75, "not_break");
    a.op(&[0xCD, 0x1B]); // INT 1Bh
    a.jump(0xEB, "eoi");
    a.label("not_break");
    a.op(&[0x80, 0xFC, 0x02]); // CMP AH, 2
    a.jump(0x75, "not_print");
    a.op(&[0xCD, 0x05]); // INT 05h
    a.jump(0xEB, "eoi");
    a.label("not_print");
    a.op(&[0x80, 0xFC, 0x03]); // CMP AH, 3
    a.jump(0x75, "eoi");
    // Pause: acknowledge, then wait with interrupts on for the key that
    // ends it.
    a.op(&[0xB0, 0x20, 0xE6, 0x20]); // MOV AL, 20h; OUT 20h, AL
    a.label("pause");
    a.op(&[0xFB, 0xF4]); // STI; HLT
    a.op(&[0xFE, 0x39, SERVICE_KBD_PAUSED]); // AH=3 while paused
    a.op(&[0x80, 0xFC, 0x03]); // CMP AH, 3
    a.jump(0x74, "pause");
    a.op(&[0xFA, 0x58, 0xCF]); // CLI; POP AX; IRET
    a.label("eoi");
    a.op(&[0xFA, 0xB0, 0x20, 0xE6, 0x20]); // CLI; MOV AL, 20h; OUT 20h, AL
    a.op(&[0x58, 0xCF]); // POP AX; IRET
    a.finish()
}

/// Windows' 386 enhanced mode starts (`on`) or ends: while it runs, its
/// keyboard driver hands each virtual machine its keys through the
/// keyboard controller, and the machine's BIOS makes the keystrokes from
/// the scan codes in its own buffer (`keyboard::bios_keystrokes`), its
/// keyboard interrupt's trap going on to the one a booted system has (the
/// vector table, and a TSR's hook, still lead there), the buffer empty.
/// Keys typed ahead for the program that started Windows are dropped.
pub fn windows_keyboard(bus: &mut Bus, on: bool) {
    if bus.kbd.windows == on {
        return;
    }
    bus.kbd.windows = on;
    crate::keyboard::reset_keystrokes(bus);
    let trap = hle_trap(0x09);
    let code = if on {
        let [lo, hi] = KBD_HANDLER.wrapping_sub(trap + 3).to_le_bytes();
        [0xE9, lo, hi, 0xCF] // JMP near KBD_HANDLER
    } else {
        [0xFE, 0x38, 0x09, 0xCF]
    };
    write_rom(bus, trap, &code);
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
/// to real mode. Anything else boots again: a system booted from a disk
/// starts over from it, the built-in DOS reboots (`boot::reboot_dos`).
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
        // A booted system starts over from its disk.
        _ if cpu.bus.boot.is_some() => crate::boot::restart(cpu),
        _ => {
            cpu.bus.log_string(&format!(
                "[BIOS] CPU reset (shutdown code {:02X}h): rebooting",
                status
            ));
            cpu.state = crate::cpu::CpuState::RebootShell;
            cpu.reboot = true;
        }
    }
}

/// A port access a service leaves for a V86 monitor to see (`PORT_ACCESSES`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortAccess {
    Out(u16, u8),
    /// A read, for what it does: 3DAh's resets the attribute flip-flop.
    In(u16),
    /// Read, keep the bits of `keep`, add those of `set` and write back, as
    /// a BIOS changes a mask register.
    Update { port: u16, keep: u8, set: u8 },
    /// Fill `words` words of memory at `segment`:0 with `value`, as a mode
    /// set clears video memory.
    Fill { segment: u16, words: u16, value: u16 },
    /// CLI, as a BIOS's disk code does before it reads a sector.
    Cli,
    /// Read the status at `port` until the bits of `mask` clear.
    WaitWhile { port: u16, mask: u8 },
    /// Read `count` words at `port`: a sector's data.
    InWords { port: u16, count: u16 },
    /// The accesses after are (true) or aren't a BIOS's show for a V86
    /// monitor, which the IDE disks answer at once (`Bus::ide_faked`).
    Faked(bool),
}

/// The ROM's loop (`FE 39 SERVICE_PORT_ACCESS`): the next port access in
/// DX, with BL 00h and the value in AL for a write, 01h for a read, 02h
/// with the bits to keep in CL and to set in CH for an update, 03h for a
/// fill of CX words of AX at ES:DI, and FFh when there are no more.
pub fn next_port_access(cpu: &mut Cpu) {
    use iced_x86::Register::{AL, BH, BL, CH, CL};
    let mut next = cpu.bus.port_accesses.pop_front();
    while let Some(PortAccess::Faked(on)) = next {
        cpu.bus.ide_faked = on;
        next = cpu.bus.port_accesses.pop_front();
    }
    match next {
        Some(PortAccess::Out(port, value)) => {
            cpu.set_dx(port);
            cpu.set_reg8(AL, value);
            cpu.set_reg8(BL, 0x00);
        }
        Some(PortAccess::In(port)) => {
            cpu.set_dx(port);
            cpu.set_reg8(BL, 0x01);
        }
        Some(PortAccess::Update { port, keep, set }) => {
            cpu.set_dx(port);
            cpu.set_reg8(CL, keep);
            cpu.set_reg8(CH, set);
            cpu.set_reg8(BL, 0x02);
        }
        Some(PortAccess::Fill { segment, words, value }) => {
            cpu.set_es(segment);
            cpu.set_di(0);
            cpu.set_cx(words);
            cpu.set_ax(value);
            cpu.set_reg8(BL, 0x03);
        }
        Some(PortAccess::Cli) => cpu.set_reg8(BL, 0x04),
        Some(PortAccess::WaitWhile { port, mask }) => {
            cpu.set_dx(port);
            cpu.set_reg8(BH, mask);
            cpu.set_reg8(BL, 0x05);
        }
        Some(PortAccess::InWords { port, count }) => {
            cpu.set_dx(port);
            cpu.set_cx(count);
            cpu.set_reg8(BL, 0x06);
        }
        Some(PortAccess::Faked(_)) => unreachable!(),
        None => {
            cpu.bus.ide_faked = false;
            cpu.set_reg8(BL, 0xFF);
        }
    }
}
