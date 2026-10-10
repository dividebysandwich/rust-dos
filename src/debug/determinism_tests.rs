//! Deterministic mode across the whole machine: a program that uses every
//! device the emulator has (the clocks, the PIT and the PIC, the sound
//! cards with their DMA and interrupts, MIDI, a CD playing audio, the
//! display and the 3D cards, the mouse, the joystick, the keyboard, the
//! serial and parallel ports, the network card and IPX, EMS, XMS, upper
//! memory and a DPMI client), with input from the debug server at
//! emulated times, run twice through `DebugHub::run_deterministic` in host
//! frames of other sizes with seeded jitter, more than a second of the
//! host's time apart. The machine's whole save state must be the same at
//! each stop, and a difference is reported by device (`SOUN.sb`,
//! `CORE.pic`). Two more tests put a leak in on purpose and check that
//! the comparison finds it.
//!
//! The coverage test fails when a section of the save state isn't in the
//! run's machine, when a part of it doesn't change during the run, or when
//! a field of the bus isn't a named part of it, unless the section or part
//! is listed with the reason. A device added to the machine has to be
//! added to the run, or listed.

use super::{Cmd, Coords, DebugHub, InputEvent, MouseAction, Reply, Request};
use crate::cpu::Cpu;
use iced_x86::code_asm::*;
use rust_dos::deterministic::Deterministic;
use std::collections::BTreeSet;
use std::ops::Range;
use std::path::Path;
use tokio::sync::oneshot;

#[path = "../../tests/cdimage/mod.rs"]
mod cdimage;

// ----- the program --------------------------------------------------------

/// Where the program keeps things, in its segment.
const SB_HANDLER: u16 = 0x5000;
const GUS_HANDLER: u16 = 0x5080;
const SBIRQ: u16 = 0x5F00;
const GUSIRQ: u16 = 0x5F02;
const LOOPS: u16 = 0x5F04;
const NKEYS: u16 = 0x5F06;
const SERIDX: u16 = 0x5F08;
const EMSFRAME: u16 = 0x5F0A;
const XMSE: u16 = 0x5F10;
const EMSH: u16 = 0x5F14;
const XMSH: u16 = 0x5F16;
const DPMIE: u16 = 0x5F18;
const RMSEG: u16 = 0x5F1C;
const PMSEL: u16 = 0x5F1E;
const IPXE: u16 = 0x5F20;
const VBAR: u16 = 0x5F24;
const VSEL: u16 = 0x5F28;
const MOVE: u16 = 0x5F40;
const IPXADDR: u16 = 0x5F60;
const NEPROM: u16 = 0x5F70;
const FOLDER_FILE: u16 = 0x5F80;
const FOLDER_GLOB: u16 = 0x5F90;
const PBAR: u16 = 0x5F2C;
const PSEL: u16 = 0x5F30;
const TEXT: u16 = 0x5FA0;
const RMCALL: u16 = 0x5D00;
const HDR: u16 = 0x5E00;
const CTRL: u16 = 0x5E40;
/// Results, a word each (`slot`).
const R: u16 = 0x6000;
const KEYS: u16 = 0x6400;
const SERIAL: u16 = 0x6600;
const SECTOR_BUF: u16 = 0x6800;
const FOLDER_BUF: u16 = 0x7000;
const END: u16 = 0x7800;

/// Result slots the test reads.
mod slot {
    pub const DSP_RESET: u16 = 26;
    pub const MPU_ACK: u16 = 29;
    pub const IPX: u16 = 31;
    pub const MOUSE_RESET: u16 = 32;
    pub const MSCDEX: u16 = 38;
    pub const CD_PLAY: u16 = 41;
    pub const VBE: u16 = 43;
    pub const PM_CS: u16 = 45;
    pub const DPMI_VERSION: u16 = 46;
    pub const VOODOO: u16 = 63;
    pub const CD_Q: u16 = 64;
    pub const CD_LOCK: u16 = 75;
}

type Asm = Result<(), IcedError>;

fn store(a: &mut CodeAssembler, slot: u16, reg: AsmRegister16) -> Asm {
    a.mov(word_ptr((R + 2 * slot) as u32), reg)
}

fn outb(a: &mut CodeAssembler, port: u16, value: u8) -> Asm {
    a.mov(dx, port as u32)?;
    a.mov(al, value as u32)?;
    a.out(dx, al)
}

/// IN AL from `port`, AH cleared.
fn inb(a: &mut CodeAssembler, port: u16) -> Asm {
    a.mov(dx, port as u32)?;
    a.in_(al, dx)?;
    a.xor(ah, ah)
}

/// Wait, a bounded time, for the bits `mask` of `port` to be `want`.
fn wait_port(a: &mut CodeAssembler, port: u16, mask: u8, want: u8) -> Asm {
    let mut again = a.create_label();
    let mut done = a.create_label();
    a.mov(dx, port as u32)?;
    a.mov(cx, 0xFFFFu32)?;
    a.set_label(&mut again)?;
    a.in_(al, dx)?;
    a.and(al, mask as u32)?;
    a.cmp(al, want as u32)?;
    a.je(done)?;
    a.dec(cx)?;
    a.jnz(again)?;
    a.set_label(&mut done)?;
    a.nop()
}

/// CALL FAR [at].
fn call_far(a: &mut CodeAssembler, at: u16) -> Asm {
    a.db(&[0xFF, 0x1E])?;
    a.dw(&[at])
}

fn dsp_write(a: &mut CodeAssembler, value: u8) -> Asm {
    wait_port(a, 0x22C, 0x80, 0)?;
    outb(a, 0x22C, value)
}

fn opl(a: &mut CodeAssembler, reg: u8, value: u8) -> Asm {
    outb(a, 0x388, reg)?;
    for _ in 0..6 {
        a.in_(al, dx)?;
    }
    outb(a, 0x389, value)?;
    a.mov(dx, 0x388u32)?;
    for _ in 0..35 {
        a.in_(al, dx)?;
    }
    Ok(())
}

fn gus8(a: &mut CodeAssembler, reg: u8, value: u8) -> Asm {
    outb(a, 0x343, reg)?;
    outb(a, 0x345, value)
}

fn gus16(a: &mut CodeAssembler, reg: u8, value: u16) -> Asm {
    outb(a, 0x343, reg)?;
    outb(a, 0x344, value as u8)?;
    outb(a, 0x345, (value >> 8) as u8)
}

fn mpu_out(a: &mut CodeAssembler, value: u8) -> Asm {
    wait_port(a, 0x331, 0x40, 0)?;
    outb(a, 0x330, value)
}

/// Program a UART at `base` for `divisor` and the line format `lcr`.
fn uart(a: &mut CodeAssembler, base: u16, divisor: u16, lcr: u8) -> Asm {
    outb(a, base + 3, 0x80)?;
    outb(a, base, divisor as u8)?;
    outb(a, base + 1, (divisor >> 8) as u8)?;
    outb(a, base + 3, lcr)
}

/// A byte the UART at `base` received, if any, onto the serial log.
fn uart_take(a: &mut CodeAssembler, base: u16) -> Asm {
    let mut none = a.create_label();
    a.mov(dx, (base + 5) as u32)?;
    a.in_(al, dx)?;
    a.test(al, 1)?;
    a.jz(none)?;
    a.mov(dx, base as u32)?;
    a.in_(al, dx)?;
    a.mov(bx, word_ptr(SERIDX as u32))?;
    a.and(bx, 0xFFu32)?;
    a.mov(byte_ptr(bx + SERIAL as i32), al)?;
    a.inc(word_ptr(SERIDX as u32))?;
    a.set_label(&mut none)?;
    a.nop()
}

/// Clear the CD request header (ES = DS) and give it a command.
fn cd_header(a: &mut CodeAssembler, command: u8, params: &[u8]) -> Asm {
    a.mov(di, HDR as u32)?;
    a.mov(cx, 0x10u32)?;
    a.xor(ax, ax)?;
    a.rep().stosw()?;
    a.mov(byte_ptr(HDR as u32), 0x1Bu32)?;
    a.mov(byte_ptr((HDR + 2) as u32), command as u32)?;
    for (i, &b) in params.iter().enumerate() {
        a.mov(byte_ptr((HDR + 0x0D + i as u16) as u32), b as u32)?;
    }
    Ok(())
}

/// The program: it sets every device going, ends as a DPMI client, and
/// then reads them over and over, keeping what it reads at `R`.
fn main_code(a: &mut CodeAssembler, audio_start: u32) -> Asm {
    a.mov(word_ptr(RMSEG as u32), cs)?;
    // The Sound Blaster's IRQ 7 and the Ultrasound's IRQ 5.
    a.mov(ax, 0x250Fu32)?;
    a.mov(dx, SB_HANDLER as u32)?;
    a.int(0x21)?;
    a.mov(ax, 0x250Du32)?;
    a.mov(dx, GUS_HANDLER as u32)?;
    a.int(0x21)?;
    a.in_(al, 0x21)?;
    a.and(al, 0x5Fu32)?;
    a.out(0x21, al)?;

    // The clock: the BIOS's, DOS's, the CMOS's, and a wait.
    a.mov(ah, 2u32)?;
    a.int(0x1A)?;
    store(a, 0, cx)?;
    store(a, 1, dx)?;
    a.mov(ah, 4u32)?;
    a.int(0x1A)?;
    store(a, 2, cx)?;
    store(a, 3, dx)?;
    a.mov(ah, 0u32)?;
    a.int(0x1A)?;
    store(a, 4, cx)?;
    store(a, 5, dx)?;
    a.mov(ah, 0x2Au32)?;
    a.int(0x21)?;
    store(a, 6, cx)?;
    store(a, 7, dx)?;
    a.mov(ah, 0x2Cu32)?;
    a.int(0x21)?;
    store(a, 8, cx)?;
    store(a, 9, dx)?;
    outb(a, 0x70, 0x0A)?;
    inb(a, 0x71)?;
    store(a, 10, ax)?;
    outb(a, 0x70, 0x00)?;
    inb(a, 0x71)?;
    store(a, 11, ax)?;
    a.mov(ah, 0x86u32)?;
    a.xor(cx, cx)?;
    a.mov(dx, 10_000u32)?;
    a.int(0x15)?;
    a.pushf()?;
    a.pop(ax)?;
    store(a, 12, ax)?;
    a.mov(ah, 0x2Cu32)?;
    a.int(0x21)?;
    store(a, 13, dx)?;

    // The PC speaker at about 1 kHz.
    outb(a, 0x43, 0xB6)?;
    outb(a, 0x42, 0x9C)?;
    outb(a, 0x42, 0x04)?;
    inb(a, 0x61)?;
    a.or(al, 3u32)?;
    a.out(dx, al)?;

    // EMS: status, the page frame, 4 pages mapped and written.
    a.mov(ah, 0x40u32)?;
    a.int(0x67)?;
    store(a, 14, ax)?;
    a.mov(ah, 0x41u32)?;
    a.int(0x67)?;
    store(a, 15, bx)?;
    a.mov(word_ptr(EMSFRAME as u32), bx)?;
    a.mov(ah, 0x43u32)?;
    a.mov(bx, 4u32)?;
    a.int(0x67)?;
    store(a, 16, ax)?;
    a.mov(word_ptr(EMSH as u32), dx)?;
    a.mov(ax, 0x4400u32)?;
    a.xor(bx, bx)?;
    a.mov(dx, word_ptr(EMSH as u32))?;
    a.int(0x67)?;
    store(a, 17, ax)?;
    a.mov(es, word_ptr(EMSFRAME as u32))?;
    a.mov(word_ptr(0).es(), 0x1234u32)?;
    a.mov(ax, word_ptr(0).es())?;
    store(a, 18, ax)?;
    a.push(ds)?;
    a.pop(es)?;

    // XMS: the driver, its free memory, a block and a move into it, A20.
    a.mov(ax, 0x4300u32)?;
    a.int(0x2F)?;
    store(a, 19, ax)?;
    a.mov(ax, 0x4310u32)?;
    a.int(0x2F)?;
    a.mov(word_ptr(XMSE as u32), bx)?;
    a.mov(word_ptr((XMSE + 2) as u32), es)?;
    a.push(ds)?;
    a.pop(es)?;
    a.mov(ah, 8u32)?;
    call_far(a, XMSE)?;
    store(a, 20, ax)?;
    store(a, 21, dx)?;
    a.mov(ah, 9u32)?;
    a.mov(dx, 64u32)?;
    call_far(a, XMSE)?;
    store(a, 22, ax)?;
    a.mov(word_ptr(XMSH as u32), dx)?;
    a.mov(word_ptr(MOVE as u32), 512u32)?;
    a.mov(word_ptr((MOVE + 2) as u32), 0u32)?;
    a.mov(word_ptr((MOVE + 4) as u32), 0u32)?;
    a.mov(word_ptr((MOVE + 6) as u32), R as u32)?;
    a.mov(word_ptr((MOVE + 8) as u32), cs)?;
    a.mov(ax, word_ptr(XMSH as u32))?;
    a.mov(word_ptr((MOVE + 0x0A) as u32), ax)?;
    a.mov(word_ptr((MOVE + 0x0C) as u32), 0u32)?;
    a.mov(word_ptr((MOVE + 0x0E) as u32), 0u32)?;
    a.mov(ah, 0x0Bu32)?;
    a.mov(si, MOVE as u32)?;
    call_far(a, XMSE)?;
    store(a, 23, ax)?;
    a.mov(ah, 7u32)?;
    call_far(a, XMSE)?;
    store(a, 24, ax)?;
    // An upper memory block.
    a.mov(ah, 0x10u32)?;
    a.mov(dx, 0x100u32)?;
    call_far(a, XMSE)?;
    store(a, 72, ax)?;
    store(a, 73, bx)?;

    // PCI: the 3dfx card (vendor 121Ah) and the PowerVR (1033h), their
    // memory enabled and their BAR0s kept.
    for (vendor, bar) in [(0x121Au32, VBAR), (0x1033, PBAR)] {
        let mut next = a.create_label();
        let mut found = a.create_label();
        let mut pci_done = a.create_label();
        a.xor(bx, bx)?;
        a.set_label(&mut next)?;
        a.movzx(ecx, bx)?;
        a.mov(eax, 0x8000_0000u32)?;
        a.or(eax, ecx)?;
        a.mov(dx, 0xCF8u32)?;
        a.out(dx, eax)?;
        a.mov(dx, 0xCFCu32)?;
        a.in_(eax, dx)?;
        a.cmp(ax, vendor)?;
        a.je(found)?;
        a.add(bx, 0x800u32)?;
        a.jnz(next)?;
        a.jmp(pci_done)?;
        a.set_label(&mut found)?;
        a.mov(eax, 0x8000_0004u32)?;
        a.or(eax, ecx)?;
        a.mov(dx, 0xCF8u32)?;
        a.out(dx, eax)?;
        a.mov(dx, 0xCFCu32)?;
        a.mov(ax, 2u32)?;
        a.out(dx, ax)?;
        a.mov(eax, 0x8000_0010u32)?;
        a.or(eax, ecx)?;
        a.mov(dx, 0xCF8u32)?;
        a.out(dx, eax)?;
        a.mov(dx, 0xCFCu32)?;
        a.in_(eax, dx)?;
        a.and(eax, 0xFFFF_FFF0u32)?;
        a.mov(dword_ptr(bar as u32), eax)?;
        a.set_label(&mut pci_done)?;
    }

    // POST code, CMOS RAM, the PIC's mask, and the PIT's channel 0 at
    // twice the BIOS's rate, latched and read.
    outb(a, 0x80, 0x42)?;
    outb(a, 0x70, 0x34)?;
    outb(a, 0x71, 0x5A)?;
    a.in_(al, 0x21)?;
    a.or(al, 0x08u32)?;
    a.out(0x21, al)?;
    outb(a, 0x43, 0x24)?;
    outb(a, 0x40, 0x80)?;
    outb(a, 0x43, 0x00)?;
    a.in_(al, 0x40)?;
    store(a, 70, ax)?;
    // Channel 2 again, an octave up, as the last control word.
    outb(a, 0x43, 0xB6)?;
    outb(a, 0x42, 0x4E)?;
    outb(a, 0x42, 0x02)?;

    // DOS's upper memory link, turned the other way.
    a.mov(ax, 0x5802u32)?;
    a.int(0x21)?;
    a.xor(bh, bh)?;
    a.xor(bl, 1u32)?;
    a.mov(ax, 0x5803u32)?;
    a.int(0x21)?;
    a.pushf()?;
    a.pop(ax)?;
    store(a, 76, ax)?;

    // DOS: a search on E:, whose state DOS keeps.
    a.mov(ah, 0x4Eu32)?;
    a.xor(cx, cx)?;
    a.mov(dx, FOLDER_GLOB as u32)?;
    a.int(0x21)?;
    a.pushf()?;
    a.pop(ax)?;
    store(a, 71, ax)?;

    // The Sound Blaster: reset, version, mixer, then 8-bit auto-init DMA
    // on channel 1 at 10 kHz from 7000:0000, interrupting every 1 KB.
    outb(a, 0x226, 1)?;
    for _ in 0..8 {
        a.in_(al, dx)?;
    }
    outb(a, 0x226, 0)?;
    wait_port(a, 0x22E, 0x80, 0x80)?;
    inb(a, 0x22A)?;
    store(a, slot::DSP_RESET, ax)?;
    dsp_write(a, 0xE1)?;
    wait_port(a, 0x22E, 0x80, 0x80)?;
    inb(a, 0x22A)?;
    a.mov(bl, al)?;
    wait_port(a, 0x22E, 0x80, 0x80)?;
    inb(a, 0x22A)?;
    a.mov(bh, al)?;
    store(a, 27, bx)?;
    outb(a, 0x224, 0x22)?;
    outb(a, 0x225, 0xCC)?;
    let mut fill = a.create_label();
    a.mov(ax, 0x7000u32)?;
    a.mov(es, ax)?;
    a.xor(di, di)?;
    a.mov(cx, 2048u32)?;
    a.xor(al, al)?;
    a.set_label(&mut fill)?;
    a.stosb()?;
    a.add(al, 3u32)?;
    a.dec(cx)?;
    a.jnz(fill)?;
    a.push(ds)?;
    a.pop(es)?;
    for (port, value) in [(0x0A, 5), (0x0C, 0), (0x0B, 0x59), (0x02, 0), (0x02, 0), (0x83, 7), (0x03, 0xFF), (0x03, 0x07), (0x0A, 1)] {
        outb(a, port, value)?;
    }
    for byte in [0xD1, 0x40, 156, 0x48, 0xFF, 0x03, 0x1C] {
        dsp_write(a, byte)?;
    }

    // The OPL: a note on channel 0, and timer 1 running.
    for (reg, value) in [
        (0x20, 0x01),
        (0x40, 0x10),
        (0x60, 0xF0),
        (0x80, 0x77),
        (0xA0, 0x98),
        (0x23, 0x01),
        (0x43, 0x00),
        (0x63, 0xF0),
        (0x83, 0x77),
        (0xB0, 0x31),
        (0x02, 0x80),
        (0x04, 0x01),
    ] {
        opl(a, reg, value)?;
    }

    // The AWE32's sample counter.
    a.mov(dx, 0xE22u32)?;
    a.mov(ax, 0x3Bu32)?;
    a.out(dx, ax)?;
    a.mov(dx, 0xA22u32)?;
    a.in_(ax, dx)?;
    store(a, 28, ax)?;

    // The Ultrasound: reset and run with its DAC and IRQs, 256 bytes
    // uploaded by DMA on channel 3 from 7200:0000 to DRAM 1000h, and voice
    // 0 looping over them with its wave IRQ.
    gus8(a, 0x4C, 0)?;
    gus8(a, 0x4C, 1)?;
    gus8(a, 0x4C, 7)?;
    let mut fill = a.create_label();
    a.mov(ax, 0x7200u32)?;
    a.mov(es, ax)?;
    a.xor(di, di)?;
    a.mov(cx, 256u32)?;
    a.mov(al, 0x80u32)?;
    a.set_label(&mut fill)?;
    a.stosb()?;
    a.add(al, 7u32)?;
    a.dec(cx)?;
    a.jnz(fill)?;
    a.push(ds)?;
    a.pop(es)?;
    for (port, value) in [(0x0A, 7), (0x0C, 0), (0x0B, 0x4B), (0x06, 0x00), (0x06, 0x20), (0x82, 7), (0x07, 0xFF), (0x07, 0), (0x0A, 3)] {
        outb(a, port, value)?;
    }
    gus16(a, 0x42, 0x100)?;
    gus8(a, 0x41, 0x21)?;
    outb(a, 0x342, 0)?;
    for (reg, value) in [(0x02, 0x0020), (0x03, 0), (0x04, 0x0022), (0x05, 0), (0x0A, 0x0020), (0x0B, 0), (0x01, 0x400), (0x09, 0xFFF0)] {
        gus16(a, reg, value)?;
    }
    gus8(a, 0x0C, 7)?;
    gus8(a, 0x0D, 3)?;
    gus8(a, 0x00, 0x28)?;

    // The MPU-401 in UART mode: a program change and a note.
    wait_port(a, 0x331, 0x40, 0)?;
    outb(a, 0x331, 0x3F)?;
    wait_port(a, 0x331, 0x80, 0)?;
    inb(a, 0x330)?;
    store(a, slot::MPU_ACK, ax)?;
    for byte in [0xC0, 0x00, 0x90, 0x3C, 0x7F] {
        mpu_out(a, byte)?;
    }

    // The Disney Sound Source on LPT1: 16 bytes into its FIFO.
    for i in 0..16u8 {
        outb(a, 0x378, i.wrapping_mul(16))?;
        outb(a, 0x37A, 0x04)?;
        outb(a, 0x37A, 0x0C)?;
    }
    inb(a, 0x379)?;
    store(a, 30, ax)?;

    // The serial ports: ATI3 to the modem on COM2, and the mouse on COM1
    // woken as a mouse driver does.
    uart(a, 0x2F8, 12, 0x03)?;
    outb(a, 0x2FC, 0x03)?;
    for &byte in b"ATI3\r" {
        wait_port(a, 0x2FD, 0x20, 0x20)?;
        outb(a, 0x2F8, byte)?;
    }
    uart(a, 0x3F8, 96, 0x02)?;
    outb(a, 0x3FC, 0x00)?;
    outb(a, 0x3FC, 0x0B)?;

    // The NE2000: its address from the PROM, by remote DMA.
    for (port, value) in [(0x300, 0x21), (0x30E, 0x48), (0x30A, 12), (0x30B, 0), (0x308, 0), (0x309, 0), (0x300, 0x0A)] {
        outb(a, port, value)?;
    }
    let mut prom = a.create_label();
    a.mov(di, NEPROM as u32)?;
    a.mov(cx, 12u32)?;
    a.mov(dx, 0x310u32)?;
    a.set_label(&mut prom)?;
    a.in_(al, dx)?;
    a.mov(byte_ptr(di), al)?;
    a.inc(di)?;
    a.dec(cx)?;
    a.jnz(prom)?;

    // IPX: the driver and its address.
    let mut no_ipx = a.create_label();
    a.mov(ax, 0x7A00u32)?;
    a.int(0x2F)?;
    a.xor(ah, ah)?;
    store(a, slot::IPX, ax)?;
    a.mov(word_ptr(IPXE as u32), di)?;
    a.mov(word_ptr((IPXE + 2) as u32), es)?;
    a.push(ds)?;
    a.pop(es)?;
    a.cmp(al, 0xFFu32)?;
    a.jne(no_ipx)?;
    a.mov(bx, 9u32)?;
    a.mov(si, IPXADDR as u32)?;
    call_far(a, IPXE)?;
    a.push(ds)?;
    a.pop(es)?;
    // A socket opened.
    a.xor(bx, bx)?;
    a.xor(al, al)?;
    a.mov(dx, 0x4545u32)?;
    call_far(a, IPXE)?;
    store(a, 74, ax)?;
    a.push(ds)?;
    a.pop(es)?;
    a.set_label(&mut no_ipx)?;

    // The mouse driver, and its range.
    a.xor(ax, ax)?;
    a.int(0x33)?;
    store(a, slot::MOUSE_RESET, ax)?;
    store(a, 33, bx)?;
    a.mov(ax, 7u32)?;
    a.xor(cx, cx)?;
    a.mov(dx, 639u32)?;
    a.int(0x33)?;
    a.mov(ax, 8u32)?;
    a.xor(cx, cx)?;
    a.mov(dx, 199u32)?;
    a.int(0x33)?;

    // The joystick: the BIOS's buttons and axes, and the port's one-shots.
    a.mov(ah, 0x84u32)?;
    a.xor(dx, dx)?;
    a.int(0x15)?;
    store(a, 34, ax)?;
    a.mov(ah, 0x84u32)?;
    a.mov(dx, 1u32)?;
    a.int(0x15)?;
    store(a, 35, ax)?;
    store(a, 36, bx)?;
    let mut joy = a.create_label();
    let mut joy_done = a.create_label();
    a.mov(dx, 0x201u32)?;
    a.out(dx, al)?;
    a.xor(cx, cx)?;
    a.set_label(&mut joy)?;
    a.in_(al, dx)?;
    a.test(al, 3)?;
    a.jz(joy_done)?;
    a.inc(cx)?;
    a.cmp(cx, 0x4000u32)?;
    a.jb(joy)?;
    a.set_label(&mut joy_done)?;
    store(a, 37, cx)?;

    // The CD: MSCDEX, sector 16 of the image's data track and of the
    // folder's disc, and the image's audio track playing.
    a.mov(ax, 0x1500u32)?;
    a.xor(bx, bx)?;
    a.int(0x2F)?;
    store(a, slot::MSCDEX, bx)?;
    store(a, 39, cx)?;
    a.mov(ax, 0x1508u32)?;
    a.mov(cx, 3u32)?;
    a.mov(dx, 1u32)?;
    a.xor(si, si)?;
    a.mov(di, 16u32)?;
    a.mov(bx, SECTOR_BUF as u32)?;
    a.int(0x2F)?;
    a.push(ds)?;
    a.pop(es)?;
    let mut no_file = a.create_label();
    a.mov(ax, 0x3D00u32)?;
    a.mov(dx, FOLDER_FILE as u32)?;
    a.int(0x21)?;
    a.jc(no_file)?;
    a.mov(bx, ax)?;
    a.mov(ah, 0x3Fu32)?;
    a.mov(cx, 32u32)?;
    a.mov(dx, FOLDER_BUF as u32)?;
    a.int(0x21)?;
    a.mov(ah, 0x3Eu32)?;
    a.int(0x21)?;
    a.set_label(&mut no_file)?;
    let mut play = vec![0u8];
    play.extend(audio_start.to_le_bytes());
    play.extend(150u32.to_le_bytes());
    cd_header(a, 0x84, &play)?;
    a.mov(ax, 0x1510u32)?;
    a.mov(cx, 3u32)?;
    a.mov(bx, HDR as u32)?;
    a.int(0x2F)?;
    a.mov(ax, word_ptr((HDR + 3) as u32))?;
    store(a, slot::CD_PLAY, ax)?;
    // The door locked: IOCTL output 01h.
    cd_header(a, 0x0C, &[0, (CTRL & 0xFF) as u8, (CTRL >> 8) as u8, 0, 0, 2, 0])?;
    a.mov(word_ptr((HDR + 0x10) as u32), cs)?;
    a.mov(word_ptr(CTRL as u32), 0x0101u32)?;
    a.mov(ax, 0x1510u32)?;
    a.mov(cx, 3u32)?;
    a.mov(bx, HDR as u32)?;
    a.int(0x2F)?;
    a.mov(ax, word_ptr((HDR + 3) as u32))?;
    store(a, slot::CD_LOCK, ax)?;

    // The display: mode 13h with pixels and a palette entry, a retrace
    // waited for, a VESA mode, mode 12h, text, and 13h again.
    let retrace = |a: &mut CodeAssembler, slot: u16| -> Asm {
        let mut out_of = a.create_label();
        let mut into = a.create_label();
        let mut done = a.create_label();
        a.mov(dx, 0x3DAu32)?;
        a.xor(cx, cx)?;
        a.set_label(&mut out_of)?;
        a.in_(al, dx)?;
        a.test(al, 8)?;
        a.jz(into)?;
        a.inc(cx)?;
        a.jnz(out_of)?;
        a.set_label(&mut into)?;
        a.in_(al, dx)?;
        a.test(al, 8)?;
        a.jnz(done)?;
        a.inc(cx)?;
        a.jnz(into)?;
        a.set_label(&mut done)?;
        store(a, slot, cx)
    };
    let fill_vram = |a: &mut CodeAssembler, count: u32, colour: u32| -> Asm {
        a.mov(ax, 0xA000u32)?;
        a.mov(es, ax)?;
        a.xor(di, di)?;
        a.mov(cx, count)?;
        a.mov(al, colour)?;
        a.rep().stosb()?;
        a.push(ds)?;
        a.pop(es)
    };
    a.mov(ax, 0x13u32)?;
    a.int(0x10)?;
    fill_vram(a, 3200, 5)?;
    outb(a, 0x3C8, 5)?;
    outb(a, 0x3C9, 63)?;
    outb(a, 0x3C9, 0)?;
    outb(a, 0x3C9, 0)?;
    retrace(a, 42)?;
    a.mov(ax, 0x4F02u32)?;
    a.mov(bx, 0x101u32)?;
    a.int(0x10)?;
    store(a, slot::VBE, ax)?;
    fill_vram(a, 0x1000, 9)?;
    // The S3's registers unlocked: the Trio's engine through its ports,
    // and the ViRGE's BitBLT registers through its MMIO at A0000h.
    for (port, value) in [(0x3D4, 0x38), (0x3D5, 0x48), (0x3D4, 0x39), (0x3D5, 0xA5), (0x3D4, 0x40)] {
        outb(a, port, value)?;
    }
    inb(a, 0x3D5)?;
    a.or(al, 1u32)?;
    a.out(dx, al)?;
    for (port, value) in [(0xA6E8u32, 0x55u32), (0x86E8, 10), (0x82E8, 20)] {
        a.mov(dx, port)?;
        a.mov(ax, value)?;
        a.out(dx, ax)?;
    }
    outb(a, 0x3D4, 0x53)?;
    inb(a, 0x3D5)?;
    a.or(al, 0x10u32)?;
    a.out(dx, al)?;
    a.mov(ax, 0xA000u32)?;
    a.mov(es, ax)?;
    a.mov(dword_ptr(0xA4D8).es(), 0x1000u32)?;
    a.push(ds)?;
    a.pop(es)?;
    outb(a, 0x3D4, 0x53)?;
    inb(a, 0x3D5)?;
    a.and(al, 0xEFu32)?;
    a.out(dx, al)?;
    a.mov(ax, 0x12u32)?;
    a.int(0x10)?;
    a.mov(ax, 0x03u32)?;
    a.int(0x10)?;
    a.mov(ax, 0x13u32)?;
    a.int(0x10)?;
    fill_vram(a, 6400, 12)?;
    // A page flip: the CRTC's start address, then a retrace.
    outb(a, 0x3D4, 0x0C)?;
    outb(a, 0x3D5, 0x00)?;
    outb(a, 0x3D4, 0x0D)?;
    outb(a, 0x3D5, 0x50)?;
    retrace(a, 44)?;
    // Text through DOS, which moves the cursor.
    a.mov(ah, 9u32)?;
    a.mov(dx, TEXT as u32)?;
    a.int(0x21)?;

    // DPMI: all but 64 KB back to DOS, then a 16-bit client.
    let mut no_dpmi = a.create_label();
    let mut no_voodoo = a.create_label();
    let mut main_loop = a.create_label();
    a.mov(ah, 0x4Au32)?;
    a.mov(bx, 0x1000u32)?;
    a.int(0x21)?;
    a.mov(ax, 0x1687u32)?;
    a.int(0x2F)?;
    a.test(ax, ax)?;
    a.jnz(no_dpmi)?;
    a.mov(word_ptr(DPMIE as u32), di)?;
    a.mov(word_ptr((DPMIE + 2) as u32), es)?;
    a.mov(bx, si)?;
    a.mov(ah, 0x48u32)?;
    a.int(0x21)?;
    a.jc(no_dpmi)?;
    a.mov(es, ax)?;
    a.xor(ax, ax)?;
    call_far(a, DPMIE)?;
    a.jc(no_dpmi)?;
    // Protected mode.
    a.push(ds)?;
    a.pop(es)?;
    a.mov(word_ptr(PMSEL as u32), cs)?;
    a.mov(ax, cs)?;
    store(a, slot::PM_CS, ax)?;
    a.mov(ax, 0x400u32)?;
    a.int(0x31)?;
    store(a, slot::DPMI_VERSION, ax)?;
    a.mov(ax, 0x501u32)?;
    a.mov(bx, 1u32)?;
    a.xor(cx, cx)?;
    a.int(0x31)?;
    a.pushf()?;
    a.pop(ax)?;
    store(a, 47, ax)?;
    // The 3dfx card's and the PowerVR's registers, mapped: a clip
    // rectangle and a buffer swap on the 3dfx card, the PowerVR's
    // interrupt mask.
    for (bar, sel) in [(VBAR, VSEL), (PBAR, PSEL)] {
        let mut not_there = a.create_label();
        a.mov(eax, dword_ptr(bar as u32))?;
        a.test(eax, eax)?;
        a.jz(not_there)?;
        a.mov(cx, ax)?;
        a.shr(eax, 16)?;
        a.mov(bx, ax)?;
        a.xor(si, si)?;
        a.mov(di, 0x1000u32)?;
        a.mov(ax, 0x800u32)?;
        a.int(0x31)?;
        a.jc(not_there)?;
        a.push(bx)?;
        a.push(cx)?;
        a.xor(ax, ax)?;
        a.mov(cx, 1u32)?;
        a.int(0x31)?;
        a.mov(word_ptr(sel as u32), ax)?;
        a.mov(bx, ax)?;
        a.pop(dx)?;
        a.pop(cx)?;
        a.mov(ax, 7u32)?;
        a.int(0x31)?;
        a.mov(bx, word_ptr(sel as u32))?;
        a.xor(cx, cx)?;
        a.mov(dx, 0xFFFu32)?;
        a.mov(ax, 8u32)?;
        a.int(0x31)?;
        a.set_label(&mut not_there)?;
    }
    a.mov(ax, word_ptr(VSEL as u32))?;
    a.test(ax, ax)?;
    a.jz(no_voodoo)?;
    a.mov(es, ax)?;
    a.mov(dword_ptr(0x118).es(), 0x0000_0140u32)?;
    a.mov(dword_ptr(0x128).es(), 1u32)?;
    a.push(ds)?;
    a.pop(es)?;
    a.set_label(&mut no_voodoo)?;
    let mut no_powervr = a.create_label();
    a.mov(ax, word_ptr(PSEL as u32))?;
    a.test(ax, ax)?;
    a.jz(no_powervr)?;
    a.mov(es, ax)?;
    a.mov(dword_ptr(0x10).es(), 1u32)?;
    a.push(ds)?;
    a.pop(es)?;
    a.set_label(&mut no_powervr)?;
    a.jmp(main_loop)?;
    a.set_label(&mut no_dpmi)?;
    a.mov(word_ptr((R + 2 * 48) as u32), 0xDEADu32)?;
    a.push(cs)?;
    a.pop(ds)?;
    a.push(cs)?;
    a.pop(es)?;

    // Read everything, over and over.
    a.set_label(&mut main_loop)?;
    a.inc(word_ptr(LOOPS as u32))?;
    // Five keys, and no more: the rest stay in the BIOS's buffer.
    let mut no_key = a.create_label();
    a.cmp(word_ptr(NKEYS as u32), 5u32)?;
    a.jae(no_key)?;
    a.mov(ah, 1u32)?;
    a.int(0x16)?;
    a.jz(no_key)?;
    a.mov(ah, 0u32)?;
    a.int(0x16)?;
    a.mov(bx, word_ptr(NKEYS as u32))?;
    a.and(bx, 0x7Fu32)?;
    a.shl(bx, 1)?;
    a.mov(word_ptr(bx + KEYS as i32), ax)?;
    a.inc(word_ptr(NKEYS as u32))?;
    a.set_label(&mut no_key)?;
    a.mov(ax, 3u32)?;
    a.int(0x33)?;
    store(a, 50, bx)?;
    store(a, 51, cx)?;
    store(a, 52, dx)?;
    a.mov(dx, 0x201u32)?;
    a.out(dx, al)?;
    a.in_(al, dx)?;
    store(a, 53, ax)?;
    a.mov(ah, 0x84u32)?;
    a.mov(dx, 1u32)?;
    a.int(0x15)?;
    store(a, 54, ax)?;
    store(a, 55, bx)?;
    inb(a, 0x3DA)?;
    store(a, 56, ax)?;
    a.mov(ah, 2u32)?;
    a.int(0x1A)?;
    store(a, 57, cx)?;
    store(a, 58, dx)?;
    outb(a, 0x70, 0)?;
    inb(a, 0x71)?;
    store(a, 59, ax)?;
    inb(a, 0x388)?;
    store(a, 60, ax)?;
    a.mov(dx, 0xE22u32)?;
    a.mov(ax, 0x3Bu32)?;
    a.out(dx, ax)?;
    a.mov(dx, 0xA22u32)?;
    a.in_(ax, dx)?;
    store(a, 61, ax)?;
    inb(a, 0x246)?;
    store(a, 62, ax)?;
    uart_take(a, 0x2F8)?;
    uart_take(a, 0x3F8)?;
    let mut no_vsel = a.create_label();
    a.mov(ax, word_ptr(VSEL as u32))?;
    a.test(ax, ax)?;
    a.jz(no_vsel)?;
    a.mov(es, ax)?;
    a.mov(eax, dword_ptr(0).es())?;
    a.push(ds)?;
    a.pop(es)?;
    store(a, slot::VOODOO, ax)?;
    a.set_label(&mut no_vsel)?;

    // Every 64th time: active sensing to the MPU-401, and the CD's
    // position through a real-mode call.
    let mut not_now = a.create_label();
    a.test(word_ptr(LOOPS as u32), 63)?;
    a.jnz(not_now)?;
    mpu_out(a, 0xFE)?;
    a.cmp(word_ptr(PMSEL as u32), 0u32)?;
    a.je(not_now)?;
    cd_header(a, 0x03, &[0, (CTRL & 0xFF) as u8, (CTRL >> 8) as u8, 0, 0, 16, 0])?;
    a.mov(ax, word_ptr(RMSEG as u32))?;
    a.mov(word_ptr((HDR + 0x10) as u32), ax)?;
    a.mov(byte_ptr(CTRL as u32), 0x0Cu32)?;
    a.mov(di, RMCALL as u32)?;
    a.mov(cx, 0x19u32)?;
    a.xor(ax, ax)?;
    a.rep().stosw()?;
    a.mov(word_ptr((RMCALL + 0x1C) as u32), 0x1510u32)?;
    a.mov(word_ptr((RMCALL + 0x18) as u32), 3u32)?;
    a.mov(word_ptr((RMCALL + 0x10) as u32), HDR as u32)?;
    a.mov(ax, word_ptr(RMSEG as u32))?;
    a.mov(word_ptr((RMCALL + 0x22) as u32), ax)?;
    a.mov(di, RMCALL as u32)?;
    a.mov(ax, 0x300u32)?;
    a.mov(bx, 0x2Fu32)?;
    a.xor(cx, cx)?;
    a.int(0x31)?;
    a.mov(ax, word_ptr((HDR + 3) as u32))?;
    store(a, slot::CD_Q, ax)?;
    a.mov(ax, word_ptr((CTRL + 1) as u32))?;
    store(a, 65, ax)?;
    a.mov(ax, word_ptr((CTRL + 3) as u32))?;
    store(a, 66, ax)?;
    a.mov(ax, word_ptr((CTRL + 8) as u32))?;
    store(a, 67, ax)?;
    a.set_label(&mut not_now)?;
    a.jmp(main_loop)
}

/// IRQ 7: the Sound Blaster's 8-bit interrupt, acknowledged.
fn sb_handler(a: &mut CodeAssembler) -> Asm {
    a.push(ax)?;
    a.push(dx)?;
    a.mov(dx, 0x22Eu32)?;
    a.in_(al, dx)?;
    a.inc(word_ptr(SBIRQ as u32).cs())?;
    a.mov(al, 0x20u32)?;
    a.out(0x20, al)?;
    a.pop(dx)?;
    a.pop(ax)?;
    a.iret()
}

/// IRQ 5: the Ultrasound's voice and DMA interrupts, acknowledged.
fn gus_handler(a: &mut CodeAssembler) -> Asm {
    a.push(ax)?;
    a.push(dx)?;
    a.mov(dx, 0x246u32)?;
    a.in_(al, dx)?;
    a.mov(dx, 0x343u32)?;
    a.mov(al, 0x8Fu32)?;
    a.out(dx, al)?;
    a.mov(dx, 0x345u32)?;
    a.in_(al, dx)?;
    a.mov(dx, 0x343u32)?;
    a.mov(al, 0x41u32)?;
    a.out(dx, al)?;
    a.mov(dx, 0x345u32)?;
    a.in_(al, dx)?;
    a.inc(word_ptr(GUSIRQ as u32).cs())?;
    a.mov(al, 0x20u32)?;
    a.out(0x20, al)?;
    a.pop(dx)?;
    a.pop(ax)?;
    a.iret()
}

fn asm16(origin: u16, f: impl FnOnce(&mut CodeAssembler) -> Asm) -> Vec<u8> {
    let mut a = CodeAssembler::new(16).unwrap();
    f(&mut a).unwrap();
    a.assemble(origin as u64).unwrap()
}

/// The program as a COM file.
fn program(audio_start: u32) -> Vec<u8> {
    let mut image = vec![0u8; (END - 0x100) as usize];
    for (at, code) in [
        (0x100, asm16(0x100, |a| main_code(a, audio_start))),
        (SB_HANDLER, asm16(SB_HANDLER, sb_handler)),
        (GUS_HANDLER, asm16(GUS_HANDLER, gus_handler)),
        (FOLDER_FILE, b"E:\\README.TXT\0".to_vec()),
        (FOLDER_GLOB, b"E:\\*.*\0".to_vec()),
        (TEXT, b"W\r\nW\r\nW\r\n$".to_vec()),
    ] {
        let at = (at - 0x100) as usize;
        image[at..at + code.len()].copy_from_slice(&code);
    }
    assert!(0x100 + asm16(0x100, |a| main_code(a, audio_start)).len() < SB_HANDLER as usize, "the program fits");
    image
}

// ----- the machine and the run ----------------------------------------------

/// The speed: what the mode runs `auto` at.
const CYCLES: u32 = rust_dos::deterministic::DEFAULT_CYCLES;

/// A host file's time, the same in every run: what DOS keeps of an open
/// file comes from it.
fn file_time() -> std::time::SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(800_000_000)
}

/// Write a host file, with `file_time`.
fn write_file(path: &Path, data: &[u8]) {
    std::fs::write(path, data).unwrap();
    std::fs::File::options().write(true).open(path).unwrap().set_modified(file_time()).unwrap();
}

/// The hardware: everything that can be in one machine. The ViRGE is the
/// display adapter (the other adapters with state of their own are listed
/// in `ABSENT_SECTIONS`), the AWE32 the Sound Blaster, beside an
/// Ultrasound, a Disney Sound Source on LPT1 (where the printer would be),
/// an NE2000, a 3dfx card and a PowerVR; the mouse is the joystick (no
/// host controllers in this mode), COM1 has a serial mouse and COM2 the
/// modem. MIDI goes to no synthesizer: the MPU-401 is the device.
fn settings(dir: &Path) -> rust_dos::config::Settings {
    let mut s = rust_dos::config::Settings::default();
    s.machine = rust_dos::video::adapter::Adapter::S3Virge;
    s.sound.sb.model = rust_dos::sb::SbModel::Awe32;
    s.sound.midisynth = rust_dos::config::MidiSynth::None;
    s.sound.lpt_dac = rust_dos::lpt_dac::LptDacType::Disney;
    s.network.ne2000 = true;
    s.network.ipx = rust_dos::net::IpxMode::On;
    s.voodoo.enabled = true;
    s.powervr = Some(rust_dos::powervr::Chip::Pcx2);
    s.capture_dir = dir.join("capture");
    s
}

/// A machine at the prompt in deterministic mode, as the front end makes
/// it, paused at the start, with the program as W.COM on C:, a CD image
/// with a data track and an audio track as D:, and a CD made of a folder
/// as E:.
fn machine(name: &str) -> (Cpu, DebugHub, Deterministic) {
    let mode = Deterministic::new(rust_dos::deterministic::default_start());
    let dir = std::env::temp_dir().join(format!("rust-dos-determinism-{}", std::process::id())).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("c")).unwrap();
    let files = [cdimage::file("README.TXT", b"a disc image\r\n"), cdimage::file("DATA\\BIG.DAT", &[7; 9000])];
    let iso = cdimage::iso("DETERM", &files);
    let cue = cdimage::mixed_disc(&dir, &iso, 150);
    let audio_start = (iso.len() / cdimage::SECTOR) as u32 + 150;
    write_file(&dir.join("c").join("W.COM"), &program(audio_start));
    std::fs::create_dir_all(dir.join("cdfolder")).unwrap();
    write_file(&dir.join("cdfolder").join("README.TXT"), b"a disc made of a folder\r\n");

    let mut cpu = Cpu::new(dir.join("c"));
    let settings = settings(&dir);
    for warning in rust_dos::hardware::configure(&mut cpu, &settings, rust_dos::keylayout::Layout::us()) {
        // The AWE32's ROM isn't there: its RAM and registers are.
        assert!(warning.starts_with("awe32rom"), "{}", warning);
    }
    cpu.bus.set_cycles_per_ms(CYCLES);
    cpu.bus.mount_drive(3, &cue, rust_dos::disk::MountOptions::default(), false).unwrap();
    let cdrom = rust_dos::disk::MountOptions { kind: rust_dos::disk::DriveKind::CdRom, ..Default::default() };
    cpu.bus.mount_drive(4, &dir.join("cdfolder"), cdrom, false).unwrap();
    cpu.load_shell();
    let mut hub = DebugHub::new(Some(std::sync::mpsc::channel().1), None, 1000);
    hub.deterministic = Some(mode.start);
    hub.pause_at_start();
    (cpu, hub, mode)
}

/// Host frames: `base_ms` of emulated time each, plus up to `jitter_ms`
/// more, from a generator seeded with `seed`, as a host's frames vary.
struct Frames {
    base_ms: u64,
    jitter_ms: u64,
    state: u64,
}

impl Frames {
    fn new(base_ms: u64, jitter_ms: u64, seed: u64) -> Self {
        Self { base_ms, jitter_ms, state: seed.max(1) }
    }

    fn next_ms(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.base_ms + self.state % (self.jitter_ms + 1)
    }
}

/// A leak put in on purpose, to show that the test finds one.
#[derive(Clone, Copy, PartialEq)]
enum Leak {
    None,
    /// The machine's clock is the host's: as a device that read the host's
    /// time instead of `hosttime` would.
    HostClock,
    /// The POST code port counts the host's frames: as a device changed at
    /// host frames instead of emulated times would.
    HostFrames,
}

/// The machine at a stop: its emulated time, the instructions run, its
/// save state in parts, the program's segment once it runs, the picture
/// last drawn and the sound mixed so far.
struct Snapshot {
    ms: u64,
    executed: u64,
    state: Vec<u8>,
    parts: Vec<(String, Range<usize>)>,
    segment: Vec<u8>,
    picture: Vec<u8>,
    sound: Vec<i16>,
}

impl Snapshot {
    fn of(cpu: &Cpu, picture: &rust_dos::video::Frame, sound: &[i16]) -> Self {
        let (state, parts) = rust_dos::savestate::machine::save_parts(cpu);
        // The program's segment: it starts with MOV [RMSEG],CS, and keeps
        // its segment there.
        let ram = cpu.bus.ram();
        let segment = (0x60..0x9000usize)
            .find(|&seg| {
                let at = seg * 16;
                ram[at + 0x100..at + 0x103] == [0x8C, 0x0E, RMSEG as u8]
                    && ram[at + RMSEG as usize..at + RMSEG as usize + 2] == (seg as u16).to_le_bytes()
            })
            .map_or_else(Vec::new, |seg| ram[seg * 16..seg * 16 + 0x10000].to_vec());
        Snapshot {
            ms: cpu.bus.clock.now_ns() / 1_000_000,
            executed: cpu.executed,
            state,
            parts,
            segment,
            picture: picture.rgb.clone(),
            sound: sound.to_vec(),
        }
    }

    fn part(&self, name: &str) -> Option<&[u8]> {
        self.parts.iter().find(|(n, _)| n == name).map(|(_, range)| &self.state[range.clone()])
    }

    /// The word at `offset` in the program's segment.
    fn word(&self, offset: u16) -> u16 {
        assert!(!self.segment.is_empty(), "the program ran");
        u16::from_le_bytes([self.segment[offset as usize], self.segment[offset as usize + 1]])
    }

    /// The program's result `slot`.
    fn result(&self, slot: u16) -> u16 {
        self.word(R + 2 * slot)
    }
}

fn reply(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd) {
    let (reply, mut rx) = oneshot::channel();
    hub.handle(cpu, Request { cmd, reply });
    match rx.try_recv() {
        Ok(Reply::Json(_)) => {}
        Ok(Reply::Error(code, e)) => panic!("{}: {}", code, e),
        Ok(Reply::ErrorJson(code, v)) => panic!("{}: {}", code, v),
        _ => panic!("no answer"),
    }
}

fn typed(text: &str) -> InputEvent {
    InputEvent::Type { text: text.into(), delay_ms: None }
}

fn mouse(action: MouseAction, x: i32, y: i32) -> InputEvent {
    InputEvent::Mouse {
        action,
        x: Some(x),
        y: Some(y),
        dx: None,
        dy: None,
        button: Some("left".into()),
        coords: Coords::Virtual,
        hold_ms: None,
    }
}

/// What the client does: the input it sends while the machine is paused,
/// and the emulated time it lets the machine run to. The first stop is at
/// the prompt, before the program runs.
fn script() -> Vec<(Vec<InputEvent>, u64)> {
    vec![
        (vec![], 100),
        (vec![typed("W\r")], 350),
        (vec![mouse(MouseAction::Move, 320, 100), mouse(MouseAction::Click, 330, 120), typed("ab")], 700),
        (vec![mouse(MouseAction::Move, 40, 20), typed("z"), InputEvent::Wait { ms: 120 }, typed("qxy\u{e9}"), shift_down()], 1100),
    ]
}

/// Shift pressed, and held past the end.
fn shift_down() -> InputEvent {
    InputEvent::Key {
        key: Some("shift".into()),
        scancode: None,
        ascii: None,
        action: super::KeyAction::Down,
        mods: Vec::new(),
        hold_ms: None,
    }
}

/// Run the script in `frames`, as the main loop runs the machine in
/// deterministic mode, and take the machine's state at each stop.
fn run(name: &str, mut frames: Frames, leak: Leak) -> Vec<Snapshot> {
    let (mut cpu, mut hub, mut mode) = machine(name);
    let mut picture = rust_dos::video::Frame::new(rust_dos::video::SCREEN_WIDTH, rust_dos::video::SCREEN_HEIGHT);
    let mut cursor_visible = true;
    let mut host_frames = 0u8;
    let mut sound = Vec::new();
    let mut snapshots = Vec::new();
    for (input, until_ms) in script() {
        if !input.is_empty() {
            reply(&mut cpu, &mut hub, Cmd::Input { events: input, wait: false });
        }
        reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: Some(until_ms) });
        for _ in 0..100_000 {
            // A host frame of the main loop in this mode.
            hub.poll(&mut cpu);
            if hub.paused {
                break;
            }
            let hot = hub.begin_batch(&cpu);
            let target = cpu.bus.clock.icount_at_ns(cpu.bus.clock.now_ns() + frames.next_ms() * 1_000_000);
            let end = mode.frame_end(&cpu, target);
            hub.run_deterministic(&mut cpu, &mut mode, hot, end, |cpu, hub, reached| {
                if reached.tick {
                    hub.feed_input(cpu);
                    cpu.bus.apply_freezes();
                }
                if leak == Leak::HostClock {
                    rust_dos::hosttime::fix(None);
                }
            });
            hub.end_batch(&cpu);
            sound.extend(rust_dos::audio::pump_audio(&mut cpu.bus, false));
            let visible = mode.blink_visible(&cpu);
            if visible != cursor_visible {
                cursor_visible = visible;
                cpu.bus.vga.set_blink(visible);
            }
            cpu.bus.sync_display();
            let (width, height) = rust_dos::video::frame_size(&cpu.bus);
            if picture.resize(width, height) {
                cpu.bus.vga.mark_dirty_full();
            }
            if cpu.bus.vga.dirty {
                rust_dos::video::render_screen(&mut picture, &cpu.bus);
                cpu.bus.vga.clear_dirty();
            }
            if leak == Leak::HostFrames {
                host_frames = host_frames.wrapping_add(1);
                cpu.bus.io_write(0x80, host_frames);
            }
        }
        assert!(hub.paused, "{}: stopped at {} ms", name, until_ms);
        let snapshot = Snapshot::of(&cpu, &picture, &sound);
        assert_eq!(snapshot.ms, until_ms, "{}: the stop is at the time asked for", name);
        snapshots.push(snapshot);
    }
    snapshots
}

/// The parts two states differ in, with the first byte that differs in
/// each, or that one has and the other hasn't.
fn differences(a: &Snapshot, b: &Snapshot) -> Vec<String> {
    let names: BTreeSet<&str> = a.parts.iter().chain(&b.parts).map(|(n, _)| n.as_str()).collect();
    let mut found = Vec::new();
    for name in names {
        match (a.part(name), b.part(name)) {
            (Some(x), Some(y)) if x == y => {}
            (Some(x), Some(y)) => {
                let at = x.iter().zip(y).position(|(p, q)| p != q).unwrap_or(x.len().min(y.len()));
                if name == "RAM" {
                    // After the section's header (tag, version, length) and
                    // the memory's length.
                    found.push(format!("RAM (from {:06X}h)", at.saturating_sub(18)));
                } else {
                    found.push(format!("{} (from byte {} of {}/{})", name, at, x.len(), y.len()));
                }
            }
            (Some(_), None) | (None, Some(_)) => found.push(format!("{} (in one state only)", name)),
            (None, None) => unreachable!(),
        }
    }
    found
}

/// Two runs: in frames of 16 ms and 37 ms of emulated time, each with up
/// to 25 ms of seeded jitter, more than a second of the host's time
/// apart, so that the host's clock is in another second. They use the same
/// folder, whose paths the state has.
fn two_runs(name: &str, leak: Leak) -> (Vec<Snapshot>, Vec<Snapshot>) {
    let a = run(name, Frames::new(16, 25, 0x5EED), leak);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let b = run(name, Frames::new(37, 25, 0xF00D), leak);
    (a, b)
}

#[test]
fn every_device_ends_alike_whatever_the_host_frames_and_time() {
    let (a, b) = two_runs("same", Leak::None);
    for (x, y) in a.iter().zip(&b) {
        let differ = differences(x, y);
        assert!(differ.is_empty(), "at {} ms the runs differ in: {}", x.ms, differ.join(", "));
        assert_eq!(x.executed, y.executed, "at {} ms", x.ms);
        assert!(x.picture == y.picture, "at {} ms the pictures differ", x.ms);
        assert!(x.sound == y.sound, "at {} ms the sound differs ({} and {} samples)", x.ms, x.sound.len(), y.sound.len());
    }
    let end = a.last().unwrap();
    assert!(end.sound.iter().any(|&s| s != 0), "sound was mixed");
    assert!(end.picture.iter().any(|&p| p != 0), "a picture was drawn");
    // The program did what it was meant to, in this run.
    assert_eq!(end.result(slot::DSP_RESET), 0xAA, "the Sound Blaster's DSP");
    assert_eq!(end.result(slot::MPU_ACK), 0xFE, "the MPU-401");
    assert_eq!(end.result(slot::MOUSE_RESET), 0xFFFF, "the mouse driver");
    assert_eq!(end.result(slot::MSCDEX), 2, "two CD drives");
    assert_eq!(end.result(slot::CD_PLAY) & 0x8000, 0, "the CD plays: status {:04X}", end.result(slot::CD_PLAY));
    assert_eq!(end.result(slot::VBE), 0x004F, "the VESA mode");
    assert_ne!(end.result(slot::PM_CS), 0, "a DPMI client");
    assert_eq!(end.result(slot::DPMI_VERSION) >> 8, 0, "DPMI 0.9");
    assert_eq!(end.result(slot::CD_Q), 0x0300, "the CD's position, busy playing");
    assert_ne!(end.word(SBIRQ), 0, "the Sound Blaster's interrupts");
    assert_ne!(end.word(GUSIRQ), 0, "the Ultrasound's interrupts");
    assert_eq!(end.word(NKEYS), 5, "the keys the program took");
    assert_ne!(end.word(SERIDX), 0, "bytes from the serial ports");
    assert_eq!(end.result(slot::IPX) & 0xFF, 0xFF, "the IPX driver");
    let at = NEPROM as usize;
    assert!(end.segment[at..at + 12].iter().any(|&b| b != 0), "the NE2000's address");
    assert_eq!(end.result(slot::CD_LOCK), 0x0300, "the CD's door locked");
    let at = SECTOR_BUF as usize + 1;
    assert_eq!(&end.segment[at..at + 5], b"CD001", "the image's volume descriptor");
    let at = FOLDER_BUF as usize;
    assert_eq!(&end.segment[at..at + 12], b"a disc made ", "the folder's file");
}

/// A test that can't fail proves nothing: with the machine's clock the
/// host's, the runs differ, and the parts they differ in say where.
#[test]
fn a_host_clock_leak_is_found() {
    let (a, b) = two_runs("clock", Leak::HostClock);
    let (a, b) = (a.last().unwrap(), b.last().unwrap());
    // The time the program read last from the real-time clock: the
    // host's, a second or more apart.
    assert_ne!((a.result(57), a.result(58)), (b.result(57), b.result(58)));
    let differ = differences(a, b);
    assert!(differ.iter().any(|d| d.starts_with("RAM")), "{:?}", differ);
}

#[test]
fn a_device_changed_at_host_frames_is_found() {
    let (a, b) = two_runs("frames", Leak::HostFrames);
    let differ = differences(a.last().unwrap(), b.last().unwrap());
    assert!(differ.iter().any(|d| d.starts_with("CORE.post_code")), "{:?}", differ);
}

// ----- coverage ---------------------------------------------------------------

/// Sections of the save state this test's machine can't have, and why.
const ABSENT_SECTIONS: &[(&str, &str)] = &[
    ("VRTE", "the Verite is a display adapter, as is the ViRGE the machine has"),
    ("ET4K", "the ET4000 is a display adapter, as is the ViRGE the machine has"),
    ("IDE", "the IDE channels are a booted system's"),
    ("SHRD", "shared disks are a booted system's"),
];

/// Parts of the save state that the run doesn't change, and why.
const STILL: &[(&str, &str)] = &[
    ("CORE.boot", "a booted system's: none is booted"),
    ("CORE.beep_frames", "a BEL beep's frames, none left at the stops"),
    ("CORE.pit_read_msb", "channel 2's read flip-flop, at rest after a whole read"),
    ("CORE.pit_write_msb", "channel 2's write flip-flop, at rest after a whole write"),
    ("CORE.pit0_read_msb", "channel 0's read flip-flop, at rest after a whole read"),
    ("CORE.pit0_write_msb", "channel 0's write flip-flop, at rest after a whole write"),
    ("CORE.pit0_latched_active", "channel 0's latch, read out"),
    ("VIDE.gate_array_shadow", "the PCjr's and Tandy's gate array"),
    ("VIDE.gate_array_shadow_at", "the PCjr's and Tandy's gate array"),
    ("SOUN.tandy_sound", "the PCjr's and Tandy's sound chip"),
    ("SOUN.gus_line", "the Ultrasound's interrupt line, up only until the handler reads the card"),
    ("DOS.dos_high", "the settings' DOS=HIGH"),
    ("DOS.disk_io", "the disk's speed settings, and the time charged until the next instruction takes it"),
    ("ENV", "the settings' environment variables"),
];

/// The `Bus` fields saved as a section of their own (or in one with the
/// other fields named), rather than as a part named after the field, as
/// `save_all!` and `Writer::label` name them.
const SECTION_FIELDS: &[(&str, &str)] = &[
    ("ram", "RAM"),
    ("virge", "VIRG"),
    ("verite", "VRTE"),
    ("voodoo", "3DFX"),
    ("powervr", "PVR"),
    ("powervr_line", "PVR"),
    ("ide", "IDE"),
    ("dpmi", "DPMI"),
    ("net", "NET"),
    ("serial", "SER"),
    ("awe", "AWE"),
];

/// The tags of the sections the save state's code writes.
fn sections_in_code() -> BTreeSet<String> {
    let code = [include_str!("../bus/state.rs"), include_str!("../savestate/machine.rs")].concat();
    code.match_indices("w.section(b\"").map(|(at, m)| code[at + m.len()..at + m.len() + 4].trim_end().to_string()).collect()
}

/// The `Bus` fields `save_state` saves: those its destructuring names
/// without `: _`.
fn saved_bus_fields() -> Vec<String> {
    let code = include_str!("../bus/state.rs");
    let start = code.find("fn save_state").expect("save_state");
    let body = &code[start..];
    let fields = &body[body.find("let Bus {").unwrap() + 9..body.find("} = self;").unwrap()];
    fields
        .lines()
        .map(|line| line.split("//").next().unwrap().trim())
        .filter(|line| !line.is_empty() && !line.contains(':'))
        .map(|line| line.trim_end_matches(',').to_string())
        .collect()
}

/// Every part of the machine's save state is in the run of the
/// determinism test and changes during it, or is listed with the reason
/// it isn't: a device added to the machine without being added to the
/// run fails here.
#[test]
fn the_run_reaches_every_part_of_the_machine() {
    let snapshots = run("coverage", Frames::new(16, 25, 0xC0FE), Leak::None);
    let (prompt, during) = snapshots.split_first().unwrap();
    let parts: BTreeSet<&str> = snapshots.iter().flat_map(|s| s.parts.iter().map(|(n, _)| n.as_str())).collect();
    let sections: BTreeSet<&str> = parts.iter().map(|p| p.split('.').next().unwrap()).collect();
    let in_code = sections_in_code();
    let fields = saved_bus_fields();
    assert!(in_code.contains("RAM") && in_code.contains("CPU") && in_code.contains("SOUN"), "{:?}", in_code);
    assert!(fields.iter().any(|f| f == "pic") && fields.len() > 40, "{:?}", fields);
    let mut problems = Vec::new();

    for tag in &in_code {
        let absent = ABSENT_SECTIONS.iter().any(|(t, _)| t == tag);
        match (sections.contains(tag.as_str()), absent) {
            (false, false) => problems.push(format!("section {} is never in the state: add its device to the run, or to ABSENT_SECTIONS", tag)),
            (true, true) => problems.push(format!("section {} is in the state: take it off ABSENT_SECTIONS", tag)),
            _ => {}
        }
    }
    for (tag, _) in ABSENT_SECTIONS {
        if !in_code.contains(*tag) {
            problems.push(format!("ABSENT_SECTIONS names {}, which no code writes", tag));
        }
    }
    for field in &fields {
        let part = parts.iter().any(|p| p.split_once('.').is_some_and(|(_, f)| f == field));
        let section = SECTION_FIELDS.iter().find(|(f, _)| f == field).map(|(_, tag)| *tag);
        match section {
            Some(tag) if !in_code.contains(tag) => problems.push(format!("SECTION_FIELDS puts {} in {}, which no code writes", field, tag)),
            None if !part => problems.push(format!("Bus field {} isn't a part of the state: save it with save_all!, or label it", field)),
            _ => {}
        }
    }
    for (name, _) in STILL {
        if !parts.contains(name) {
            problems.push(format!("STILL names {}, which isn't a part of the state", name));
        }
    }
    for &name in &parts {
        // A section's header, where its parts are named: its length.
        if !name.contains('.') && parts.iter().any(|p| p.strip_prefix(name).is_some_and(|rest| rest.starts_with('.'))) {
            continue;
        }
        let changed = during.iter().any(|later| later.part(name) != prompt.part(name));
        let still = STILL.iter().any(|(p, _)| *p == name);
        match (changed, still) {
            (false, false) => problems.push(format!("{} doesn't change during the run: make the program use it, or add it to STILL", name)),
            (true, true) => problems.push(format!("{} changes during the run: take it off STILL", name)),
            _ => {}
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}
