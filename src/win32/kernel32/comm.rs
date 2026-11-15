//! Win32's serial ports: the handles CreateFileA gives for COM1 to COM4,
//! the comm functions on them (SetupComm to TransmitCommChar), and
//! ReadFile and WriteFile on them, which Windows 95 has VCOMM and its
//! port driver do. As Windows 3.1's (win16/comm.rs, whose UART code this
//! shares), they drive the machine's UARTs themselves, without their
//! interrupts: what a port received moves into its receive queue in the
//! comm functions and in ReadFile and WriteFile. While that waits, the
//! UART holds what comes from the other end outside the chip once its
//! FIFO is full, as a cable with flow control would, so nothing is lost
//! between calls.
//!
//! A port's state is kept by the DOS file table entry of the file object
//! CreateFileA made for it, and goes when that object goes. A port is
//! open once at a time, for Win32 and Win16 programs together.
//!
//! The structures are Win32's: the DCB of 28 bytes (DCBlength, BaudRate,
//! a dword of flags, wReserved, XonLim, XoffLim, ByteSize, Parity,
//! StopBits, XonChar, XoffChar, ErrorChar, EofChar, EvtChar, wReserved1),
//! COMSTAT of 12 (a dword of flags, cbInQue, cbOutQue) and COMMTIMEOUTS
//! of 20 (ReadIntervalTimeout, ReadTotalTimeoutMultiplier,
//! ReadTotalTimeoutConstant, WriteTotalTimeoutMultiplier,
//! WriteTotalTimeoutConstant).

use super::{ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error};
use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::win16::comm::{self as win16, Line};
use std::collections::VecDeque;

/// The DCB's size and its fields' places.
const DCB_SIZE: usize = 28;
const DCB_LENGTH: usize = 0;
const DCB_BAUD: usize = 4;
const DCB_FLAGS: usize = 8;
const DCB_BYTESIZE: usize = 18;
const DCB_PARITY: usize = 19;
const DCB_STOPBITS: usize = 20;
const DCB_XONCHAR: usize = 21;
const DCB_XOFFCHAR: usize = 22;

/// The DCB's flags.
const F_BINARY: u32 = 0x0001;
const F_PARITY: u32 = 0x0002;
const F_OUTX_CTS_FLOW: u32 = 0x0004;
const F_OUTX_DSR_FLOW: u32 = 0x0008;
const F_OUTX: u32 = 0x0100;
const F_INX: u32 = 0x0200;
/// fDtrControl, two bits: DTR_CONTROL_DISABLE, _ENABLE, _HANDSHAKE.
const DTR_CONTROL_SHIFT: u32 = 4;
const DTR_CONTROL_ENABLE: u32 = 1;
const DTR_CONTROL_HANDSHAKE: u32 = 2;
/// fRtsControl, two bits: RTS_CONTROL_DISABLE, _ENABLE, _HANDSHAKE,
/// _TOGGLE.
const RTS_CONTROL_SHIFT: u32 = 12;
const RTS_CONTROL_ENABLE: u32 = 1;
const RTS_CONTROL_HANDSHAKE: u32 = 2;

/// The XON and XOFF characters a port starts with.
const XON: u8 = 0x11;
const XOFF: u8 = 0x13;

/// The size of the queues before SetupComm sets them.
const QUEUE: u32 = 4096;

/// ClearCommError's errors.
const CE_OVERRUN: u32 = 0x0002;
const CE_RXPARITY: u32 = 0x0004;
const CE_FRAME: u32 = 0x0008;
const CE_BREAK: u32 = 0x0010;

/// COMSTAT's flag of transmission held by an XOFF.
const COMSTAT_XOFF_HOLD: u32 = 0x08;

/// PurgeComm's flags.
const PURGE_TXABORT: u32 = 1;
const PURGE_RXABORT: u32 = 2;
const PURGE_TXCLEAR: u32 = 4;
const PURGE_RXCLEAR: u32 = 8;

/// EscapeCommFunction's functions.
const SETXOFF: u32 = 1;
const SETXON: u32 = 2;
const SETRTS: u32 = 3;
const CLRRTS: u32 = 4;
const SETDTR: u32 = 5;
const CLRDTR: u32 = 6;
const RESETDEV: u32 = 7;
const SETBREAK: u32 = 8;
const CLRBREAK: u32 = 9;

/// The rates the UART's divisor makes from its 1.8432 MHz clock: 115200
/// bauds over the divisor.
const MAX_BAUD: u32 = 115_200;

/// MAXDWORD in a timeout.
const MAXDWORD: u32 = u32::MAX;

/// How often a thread waiting in ReadFile looks for characters, in
/// microseconds. 115200 bauds bring 11 characters in that time, fewer
/// than the 16 the FIFO holds.
const READ_POLL: u64 = 1_000;

/// An open port.
#[derive(Clone, Debug, Default)]
pub struct Port {
    /// The DOS file table entry of the handle's file object.
    pub sft: u16,
    /// Its number: 0 for COM1.
    pub id: u8,
    /// The sizes of the receive and transmit queues (SetupComm).
    pub in_size: u32,
    pub out_size: u32,
    /// What came in and wasn't read yet.
    pub input: VecDeque<u8>,
    /// What waits to go out while transmission is held (SETXOFF,
    /// SetCommBreak), and whether it is.
    pub held: VecDeque<u8>,
    pub xoff: bool,
    pub brk: bool,
    /// The DCB the port was last set up with.
    pub dcb: Vec<u8>,
    /// SetCommMask's events.
    pub mask: u32,
    /// The CE_ errors since ClearCommError last gave them.
    pub errors: u32,
    /// The COMMTIMEOUTS, its five dwords.
    pub timeouts: Vec<u32>,
    /// When the last character came in, by the clock's microseconds.
    pub received_at: u64,
    /// When the ReadFile waiting for characters began, if one does.
    pub read_since: Option<u64>,
}

crate::state_fields!(Port {
    sft,
    id,
    in_size,
    out_size,
    input,
    held,
    xoff,
    brk,
    dcb,
    mask,
    errors,
    timeouts,
    received_at,
    read_since
});

/// Whether a Win32 program has port `n` open.
pub(crate) fn is_open(bus: &Bus, n: usize) -> bool {
    bus.win32.comm.iter().any(|p| p.id as usize == n)
}

/// Whether CreateFileA may open port `n`: the error it fails with, for a
/// port the machine doesn't have (ERROR_FILE_NOT_FOUND) or one that is
/// open already, by a Win32 or a Win16 program (ERROR_ACCESS_DENIED).
pub(super) fn may_open(cpu: &Cpu, n: usize) -> Result<(), u32> {
    if cpu.bus.serial.ports.get(n).is_none_or(Option::is_none) {
        return Err(super::file::ERROR_FILE_NOT_FOUND);
    }
    if is_open(&cpu.bus, n) || win16::is_open(cpu, n) {
        return Err(super::ERROR_ACCESS_DENIED);
    }
    Ok(())
}

/// Port `n` opens for the file of the DOS file table entry `sft`. It
/// starts with WIN.INI's settings for it ([ports], "COM1:=9600,n,8,1,x"),
/// or 9600,n,8,1, with DTR and RTS on, XON and XOFF the usual characters
/// and no timeouts. False for settings the UART can't do.
pub(super) fn open(cpu: &mut Cpu, sft: u16, n: usize) -> bool {
    let Some((line, flags)) = win16::port_settings(cpu, n) else {
        return false;
    };
    let mut dcb = vec![0u8; DCB_SIZE];
    put_32(&mut dcb, DCB_LENGTH, DCB_SIZE as u32);
    put_32(&mut dcb, DCB_BAUD, MAX_BAUD / line.divisor as u32);
    // Windows 3.1's flags, which are the low bits of Win32's but for
    // the DTR and RTS controls, here on.
    let mut win32_flags = F_BINARY
        | DTR_CONTROL_ENABLE << DTR_CONTROL_SHIFT
        | RTS_CONTROL_ENABLE << RTS_CONTROL_SHIFT;
    if flags & 0x0004 != 0 {
        win32_flags |= F_PARITY;
    }
    if flags & 0x0300 != 0 {
        win32_flags |= F_OUTX | F_INX;
    }
    if flags & 0x0018 != 0 {
        win32_flags |= F_OUTX_CTS_FLOW
            | F_OUTX_DSR_FLOW
            | DTR_CONTROL_HANDSHAKE << DTR_CONTROL_SHIFT
            | RTS_CONTROL_HANDSHAKE << RTS_CONTROL_SHIFT;
        win32_flags &= !(DTR_CONTROL_ENABLE << DTR_CONTROL_SHIFT);
        win32_flags &= !(RTS_CONTROL_ENABLE << RTS_CONTROL_SHIFT);
    }
    put_32(&mut dcb, DCB_FLAGS, win32_flags);
    dcb[DCB_BYTESIZE] = line.size;
    dcb[DCB_PARITY] = line.parity;
    dcb[DCB_STOPBITS] = line.stop;
    dcb[DCB_XONCHAR] = XON;
    dcb[DCB_XOFFCHAR] = XOFF;
    let bus = &mut cpu.bus;
    win16::set_line(bus, n, line, false);
    win16::set_modem_lines(bus, n, true, true);
    win16::take_over(bus, n);
    bus.serial_status(n);
    bus.win32.comm.push(Port {
        sft,
        id: n as u8,
        in_size: QUEUE,
        out_size: QUEUE,
        dcb,
        timeouts: vec![0; 5],
        ..Port::default()
    });
    true
}

/// The file of the DOS file table entry `sft` closed: its port, if it has
/// one, closes too. What transmission held is dropped.
pub(crate) fn file_closed(bus: &mut Bus, sft: u16) {
    if let Some(i) = bus.win32.comm.iter().position(|p| p.sft == sft) {
        let port = bus.win32.comm.remove(i);
        win16::let_go(bus, port.id as usize);
    }
}

/// The place in the open ports of the port of the file whose DOS file
/// table entry is `sft`.
pub(super) fn port_of_file(cpu: &Cpu, sft: u16) -> Option<usize> {
    if cpu.bus.win32.comm.is_empty() {
        return None;
    }
    cpu.bus.win32.comm.iter().position(|p| p.sft == sft)
}

/// The open port `handle` is a handle of, brought up to now: its place.
/// Without one, the last error is ERROR_INVALID_HANDLE.
fn port(cpu: &mut Cpu, handle: u32) -> Option<usize> {
    let i = super::file::open_of(cpu, handle).and_then(|open| port_of_file(cpu, open.sft));
    match i {
        Some(i) => {
            pump(cpu, i);
            Some(i)
        }
        None => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            None
        }
    }
}

/// Bring the open port `i` up to now, as the port driver's interrupt
/// handler would have: what was received into its queue while it has
/// room, the line errors the UART noted, and the held characters out once
/// nothing holds them.
fn pump(cpu: &mut Cpu, i: usize) {
    let port = &cpu.bus.win32.comm[i];
    let n = port.id as usize;
    let room = (port.in_size as usize).saturating_sub(port.input.len());
    let mut received = Vec::new();
    while received.len() < room {
        match cpu.bus.serial_receive(n) {
            Some(byte) => received.push(byte),
            None => break,
        }
    }
    let (lsr, _) = cpu.bus.serial_status(n).unwrap_or((0, 0));
    let mut errors = 0;
    for (bit, error) in [
        (0x02, CE_OVERRUN),
        (0x04, CE_RXPARITY),
        (0x08, CE_FRAME),
        (0x10, CE_BREAK),
    ] {
        if lsr & bit != 0 {
            errors |= error;
        }
    }
    let port = &cpu.bus.win32.comm[i];
    if !port.xoff && !port.brk && !port.held.is_empty() {
        let held = std::mem::take(&mut cpu.bus.win32.comm[i].held);
        for byte in held {
            cpu.bus.serial_send(n, byte);
        }
    }
    let now = cpu.bus.clock.now_micros();
    let port = &mut cpu.bus.win32.comm[i];
    if !received.is_empty() {
        port.received_at = now;
    }
    port.input.extend(received);
    port.errors |= errors;
}

fn get_32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn put_32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// Write `bytes` to the program's `at`: false when a page isn't there,
/// which then faults.
fn write_out(cpu: &mut Cpu, at: u32, bytes: &[u8]) -> bool {
    if !cpu.bus.guest_probe(at, bytes.len(), true) {
        return false;
    }
    cpu.bus.guest_write_bytes(at, bytes);
    !cpu.bus.guest_faulted()
}

/// `len` bytes of the program's `at`, or None when a page isn't there,
/// which then faults.
fn read_in(cpu: &mut Cpu, at: u32, len: usize) -> Option<Vec<u8>> {
    let mut bytes = vec![0u8; len];
    cpu.bus.guest_read_bytes(at, &mut bytes);
    (!cpu.bus.guest_faulted()).then_some(bytes)
}

/// FALSE with ERROR_INVALID_PARAMETER.
fn invalid(cpu: &mut Cpu) -> Option<u32> {
    set_error(cpu, ERROR_INVALID_PARAMETER);
    Some(FALSE)
}

/// SetupComm(handle, cbInQueue, cbOutQueue): the sizes of the port's
/// receive and transmit queues. What is in them stays.
pub(super) fn setup_comm(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = port(cpu, args[0]) else {
        return Some(FALSE);
    };
    let port = &mut cpu.bus.win32.comm[i];
    port.in_size = args[1];
    port.out_size = args[2];
    Some(TRUE)
}

/// PurgeComm(handle, flags): PURGE_TXCLEAR empties the transmit queue
/// and the UART's but for the character going out, PURGE_RXCLEAR the
/// receive queue and the UART's. PURGE_TXABORT and PURGE_RXABORT end
/// the reads and writes that wait, and none does: a ReadFile that waits
/// holds its thread.
pub(super) fn purge_comm(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let flags = args[1];
    let all = PURGE_TXABORT | PURGE_RXABORT | PURGE_TXCLEAR | PURGE_RXCLEAR;
    let Some(i) = port(cpu, args[0]) else {
        return Some(FALSE);
    };
    if flags & !all != 0 {
        return invalid(cpu);
    }
    let n = cpu.bus.win32.comm[i].id as usize;
    if flags & PURGE_TXCLEAR != 0 {
        cpu.bus.win32.comm[i].held.clear();
        cpu.bus.serial_clear_tx(n);
    }
    if flags & PURGE_RXCLEAR != 0 {
        while cpu.bus.serial_receive(n).is_some() {}
        cpu.bus.win32.comm[i].input.clear();
    }
    Some(TRUE)
}

/// GetCommState(handle, dcb): the DCB the port was last set up with.
pub(super) fn get_comm_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = port(cpu, args[0]) else {
        return Some(FALSE);
    };
    let dcb = cpu.bus.win32.comm[i].dcb.clone();
    write_out(cpu, args[1], &dcb).then_some(TRUE)
}

/// The line a DCB asks for, or None for one the port can't do: a rate of
/// 0 or above 115200 bauds, 5 to 8 data bits (the UART has no 4), a
/// parity or stop bits Win32 doesn't have, 5 data bits with 2 stop bits
/// or 6 to 8 with 1.5, and XonChar the same as XoffChar, which
/// SetCommState's documentation says it fails for.
fn dcb_line(dcb: &[u8]) -> Option<Line> {
    let rate = get_32(dcb, DCB_BAUD);
    if rate == 0 || rate > MAX_BAUD {
        return None;
    }
    let (size, parity, stop) = (dcb[DCB_BYTESIZE], dcb[DCB_PARITY], dcb[DCB_STOPBITS]);
    let valid = (5..=8).contains(&size)
        && parity <= 4
        && match stop {
            0 => true,
            1 => size == 5,
            2 => size != 5,
            _ => false,
        }
        && dcb[DCB_XONCHAR] != dcb[DCB_XOFFCHAR];
    // The divisor nearest the rate.
    let divisor = ((MAX_BAUD + rate / 2) / rate).max(1) as u16;
    valid.then_some(Line {
        divisor,
        size,
        parity,
        stop,
    })
}

/// SetCommState(handle, dcb): the port set up as the DCB says: the rate,
/// the line format, and DTR and RTS, on for _ENABLE and _HANDSHAKE (the
/// receive queue always has room here), off for _DISABLE; RTS_CONTROL_
/// TOGGLE leaves RTS on. FALSE with ERROR_INVALID_PARAMETER for a DCB the
/// port can't do (`dcb_line`), which leaves the port as it was.
pub(super) fn set_comm_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = port(cpu, args[0]) else {
        return Some(FALSE);
    };
    let mut dcb = read_in(cpu, args[1], DCB_SIZE)?;
    let Some(line) = dcb_line(&dcb) else {
        return invalid(cpu);
    };
    let flags = get_32(&dcb, DCB_FLAGS);
    let dtr = (flags >> DTR_CONTROL_SHIFT) & 3 != 0;
    let rts = (flags >> RTS_CONTROL_SHIFT) & 3 != 0;
    let port = &cpu.bus.win32.comm[i];
    let (n, brk) = (port.id as usize, port.brk);
    win16::set_line(&mut cpu.bus, n, line, brk);
    win16::set_modem_lines(&mut cpu.bus, n, dtr, rts);
    put_32(&mut dcb, DCB_LENGTH, DCB_SIZE as u32);
    cpu.bus.win32.comm[i].dcb = dcb;
    Some(TRUE)
}

/// ClearCommError(handle, errors, comstat): the errors since the last
/// call, which it clears, to `*errors`, and the port's state to the
/// COMSTAT, each when its pointer isn't NULL.
pub(super) fn clear_comm_error(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = port(cpu, args[0]) else {
        return Some(FALSE);
    };
    let n = cpu.bus.win32.comm[i].id as usize;
    let sending = cpu.bus.serial_sending(n).unwrap_or(0);
    let port = &cpu.bus.win32.comm[i];
    let mut stat = [0u8; 12];
    put_32(&mut stat, 0, if port.xoff { COMSTAT_XOFF_HOLD } else { 0 });
    put_32(&mut stat, 4, port.input.len() as u32);
    put_32(&mut stat, 8, (port.held.len() + sending) as u32);
    let errors = port.errors;
    if args[1] != 0 && !write_out(cpu, args[1], &errors.to_le_bytes()) {
        return None;
    }
    if args[2] != 0 && !write_out(cpu, args[2], &stat) {
        return None;
    }
    cpu.bus.win32.comm[i].errors = 0;
    Some(TRUE)
}

/// SetCommMask(handle, mask): the events the port is to watch for.
pub(super) fn set_comm_mask(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = port(cpu, args[0]) else {
        return Some(FALSE);
    };
    cpu.bus.win32.comm[i].mask = args[1];
    Some(TRUE)
}

/// GetCommMask(handle, mask): SetCommMask's events to `*mask`.
pub(super) fn get_comm_mask(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = port(cpu, args[0]) else {
        return Some(FALSE);
    };
    let mask = cpu.bus.win32.comm[i].mask;
    write_out(cpu, args[1], &mask.to_le_bytes()).then_some(TRUE)
}

/// GetCommModemStatus(handle, status): the modem status lines to
/// `*status`: MS_CTS_ON, MS_DSR_ON, MS_RING_ON and MS_RLSD_ON, the
/// modem status register's top four bits.
pub(super) fn get_comm_modem_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = port(cpu, args[0]) else {
        return Some(FALSE);
    };
    let n = cpu.bus.win32.comm[i].id as usize;
    let msr = cpu.bus.serial_status(n).map_or(0, |(_, msr)| msr);
    let status = (msr & 0xF0) as u32;
    write_out(cpu, args[1], &status.to_le_bytes()).then_some(TRUE)
}

/// EscapeCommFunction(handle, function): SETXOFF and SETXON hold
/// transmission and let it go on, as an XOFF and an XON from the other
/// end do; SETRTS, CLRRTS, SETDTR and CLRDTR set the lines; SETBREAK and
/// CLRBREAK are SetCommBreak and ClearCommBreak. RESETDEV does nothing
/// to a serial port. FALSE with ERROR_INVALID_PARAMETER for another
/// function.
pub(super) fn escape_comm_function(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = port(cpu, args[0]) else {
        return Some(FALSE);
    };
    let n = cpu.bus.win32.comm[i].id as usize;
    match args[1] {
        SETXOFF => cpu.bus.win32.comm[i].xoff = true,
        SETXON => cpu.bus.win32.comm[i].xoff = false,
        SETRTS => win16::set_modem_line(&mut cpu.bus, n, false, true),
        CLRRTS => win16::set_modem_line(&mut cpu.bus, n, false, false),
        SETDTR => win16::set_modem_line(&mut cpu.bus, n, true, true),
        CLRDTR => win16::set_modem_line(&mut cpu.bus, n, true, false),
        RESETDEV => {}
        SETBREAK => comm_break(cpu, i, true),
        CLRBREAK => comm_break(cpu, i, false),
        _ => return invalid(cpu),
    }
    pump(cpu, i);
    Some(TRUE)
}

/// The open port `i`'s line in a break, with transmission held, or back.
fn comm_break(cpu: &mut Cpu, i: usize, on: bool) {
    let port = &mut cpu.bus.win32.comm[i];
    port.brk = on;
    let n = port.id as usize;
    win16::set_break(&mut cpu.bus, n, on);
}

/// SetCommBreak(handle) and ClearCommBreak(handle): the line in a break,
/// with transmission held, and back.
pub(super) fn set_comm_break(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    escape_comm_function(cpu, &[args[0], SETBREAK])
}

pub(super) fn clear_comm_break(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    escape_comm_function(cpu, &[args[0], CLRBREAK])
}

/// TransmitCommChar(handle, char): the character out at once, ahead of
/// what transmission holds. FALSE with ERROR_INVALID_PARAMETER in a
/// break.
pub(super) fn transmit_comm_char(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = port(cpu, args[0]) else {
        return Some(FALSE);
    };
    let port = &cpu.bus.win32.comm[i];
    if port.brk {
        return invalid(cpu);
    }
    let n = port.id as usize;
    cpu.bus.serial_send(n, args[1] as u8);
    Some(TRUE)
}

/// GetCommTimeouts(handle, timeouts): the COMMTIMEOUTS ReadFile and
/// WriteFile go by.
pub(super) fn get_comm_timeouts(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = port(cpu, args[0]) else {
        return Some(FALSE);
    };
    let bytes: Vec<u8> = cpu.bus.win32.comm[i]
        .timeouts
        .iter()
        .flat_map(|t| t.to_le_bytes())
        .collect();
    write_out(cpu, args[1], &bytes).then_some(TRUE)
}

/// SetCommTimeouts(handle, timeouts): the COMMTIMEOUTS from now on.
pub(super) fn set_comm_timeouts(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = port(cpu, args[0]) else {
        return Some(FALSE);
    };
    let bytes = read_in(cpu, args[1], 20)?;
    cpu.bus.win32.comm[i].timeouts = (0..5).map(|k| get_32(&bytes, k * 4)).collect();
    Some(TRUE)
}

/// When a ReadFile of `n` characters that began at `since` (the clock's
/// microseconds) ends by its timeouts, the last character having come at
/// `received_at` and `queued` waiting: never (None), or by the total
/// timeout of ReadTotalTimeoutMultiplier times `n` plus
/// ReadTotalTimeoutConstant milliseconds, or ReadIntervalTimeout after a
/// character once one came. With ReadIntervalTimeout and the multiplier
/// MAXDWORD and a constant below it, the constant is the time to wait
/// for one character.
fn read_due(timeouts: &[u32], n: u32, since: u64, received_at: u64, queued: usize) -> Option<u64> {
    let &[interval, multiplier, constant, ..] = timeouts else {
        return None;
    };
    let ms = |ms: u64| ms.saturating_mul(1000);
    let total = if interval == MAXDWORD && multiplier == MAXDWORD {
        Some(constant as u64)
    } else if multiplier != 0 || constant != 0 {
        Some((multiplier as u64).saturating_mul(n as u64) + constant as u64)
    } else {
        None
    };
    let total = total.map(|t| since.saturating_add(ms(t)));
    let interval = (interval != 0 && interval != MAXDWORD && queued > 0)
        .then(|| received_at.max(since).saturating_add(ms(interval as u64)));
    match (total, interval) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Whether a ReadFile of `n` characters returns at once with the
/// `queued` the receive queue has: with all of them, or with what there
/// is when ReadIntervalTimeout is MAXDWORD and the total timeouts 0, or
/// with one or more when ReadIntervalTimeout and the multiplier are
/// MAXDWORD and the constant isn't (`read_due`).
fn read_now(timeouts: &[u32], n: u32, queued: usize) -> bool {
    let &[interval, multiplier, constant, ..] = timeouts else {
        return true;
    };
    queued >= n as usize
        || (interval == MAXDWORD && multiplier == 0 && constant == 0)
        || (interval == MAXDWORD
            && multiplier == MAXDWORD
            && constant != 0
            && constant != MAXDWORD
            && queued > 0)
}

/// ReadFile of the open port `i`: up to `n` characters of its receive
/// queue to `buffer`, as its timeouts say. Without them it waits for all
/// `n`. The count read and no error; None when a page isn't there, or
/// the thread waits for characters, and the call runs again then.
pub(super) fn read(cpu: &mut Cpu, i: usize, buffer: u32, n: u32) -> Option<(u32, Option<u32>)> {
    pump(cpu, i);
    let now = cpu.bus.clock.now_micros();
    let port = &cpu.bus.win32.comm[i];
    let queued = port.input.len();
    if !read_now(&port.timeouts, n, queued) {
        let since = port.read_since.unwrap_or(now);
        let due = read_due(&port.timeouts, n, since, port.received_at, queued);
        if due.is_none_or(|due| now < due) {
            cpu.bus.win32.comm[i].read_since = Some(since);
            let poll = now + READ_POLL;
            super::file::retry_at(cpu, Some(due.map_or(poll, |due| due.min(poll))));
            return None;
        }
    }
    let count = queued.min(n as usize);
    let bytes: Vec<u8> = cpu.bus.win32.comm[i]
        .input
        .iter()
        .take(count)
        .copied()
        .collect();
    if !write_out(cpu, buffer, &bytes) {
        return None;
    }
    let port = &mut cpu.bus.win32.comm[i];
    port.input.drain(..count);
    port.read_since = None;
    Some((count as u32, None))
}

/// WriteFile of the open port `i`: the `n` characters at `buffer` out,
/// or into what transmission holds while it is held. The UART takes all
/// of them at once, so the write timeouts never end one. The count
/// written; None when a page isn't there.
pub(super) fn write(cpu: &mut Cpu, i: usize, buffer: u32, n: u32) -> Option<(u32, Option<u32>)> {
    let bytes = read_in(cpu, buffer, n as usize)?;
    pump(cpu, i);
    let port = &mut cpu.bus.win32.comm[i];
    if port.xoff || port.brk {
        port.held.extend(&bytes);
    } else {
        let n = port.id as usize;
        for byte in bytes {
            cpu.bus.serial_send(n, byte);
        }
    }
    Some((n, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dcb(rate: u32, size: u8, parity: u8, stop: u8) -> Vec<u8> {
        let mut dcb = vec![0u8; DCB_SIZE];
        put_32(&mut dcb, DCB_BAUD, rate);
        dcb[DCB_BYTESIZE] = size;
        dcb[DCB_PARITY] = parity;
        dcb[DCB_STOPBITS] = stop;
        dcb[DCB_XONCHAR] = XON;
        dcb[DCB_XOFFCHAR] = XOFF;
        dcb
    }

    #[test]
    fn dcb_lines_the_uart_can_do() {
        let line = dcb_line(&dcb(1200, 7, 0, 0)).unwrap();
        assert_eq!(
            (line.divisor, line.size, line.parity, line.stop),
            (96, 7, 0, 0)
        );
        assert_eq!(dcb_line(&dcb(115_200, 8, 2, 2)).unwrap().divisor, 1);
        assert_eq!(dcb_line(&dcb(110, 5, 1, 1)).unwrap().divisor, 1047);
        for bad in [
            dcb(0, 8, 0, 0),
            dcb(230_400, 8, 0, 0),
            dcb(9600, 4, 0, 0),
            dcb(9600, 9, 0, 0),
            dcb(9600, 8, 5, 0),
            dcb(9600, 5, 0, 2),
            dcb(9600, 8, 0, 1),
            dcb(9600, 8, 0, 3),
        ] {
            assert_eq!(dcb_line(&bad), None, "{bad:?}");
        }
        let mut same = dcb(9600, 8, 0, 0);
        same[DCB_XOFFCHAR] = XON;
        assert_eq!(dcb_line(&same), None);
    }

    #[test]
    fn reads_end_as_the_timeouts_say() {
        // No timeouts: all of it, however long that takes.
        assert!(!read_now(&[0; 5], 4, 3));
        assert!(read_now(&[0; 5], 4, 4));
        assert_eq!(read_due(&[0; 5], 4, 1000, 0, 3), None);
        // What there is, at once.
        assert!(read_now(&[MAXDWORD, 0, 0, 0, 0], 4, 0));
        // One character or more, waiting up to the constant for one.
        let one = [MAXDWORD, MAXDWORD, 50, 0, 0];
        assert!(!read_now(&one, 4, 0));
        assert!(read_now(&one, 4, 1));
        assert_eq!(read_due(&one, 4, 1000, 0, 0), Some(51_000));
        // A total timeout of 10 ms a character plus 5 ms.
        let total = [0, 10, 5, 0, 0];
        assert_eq!(read_due(&total, 4, 1000, 0, 0), Some(46_000));
        // An interval of 20 ms, counted once a character came.
        let interval = [20, 0, 0, 0, 0];
        assert_eq!(read_due(&interval, 4, 1000, 0, 0), None);
        assert_eq!(read_due(&interval, 4, 1000, 5000, 1), Some(25_000));
        // Both: the earlier.
        let both = [20, 10, 5, 0, 0];
        assert_eq!(read_due(&both, 4, 1000, 5000, 1), Some(25_000));
        assert_eq!(read_due(&both, 1, 1000, 5000, 1), Some(16_000));
    }
}
