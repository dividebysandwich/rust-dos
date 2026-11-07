//! Windows 3.1's serial ports: USER's comm functions (OpenComm to
//! FlushComm, ordinals 200 to 215), which Windows 3.1 has COMM.DRV do and
//! Windows 95 VCOMM, through COMM.DRV. Here they drive the UARTs of the
//! machine's COM ports themselves, as COMM.DRV does, but without their
//! interrupts: a port's received characters move into its receive queue,
//! and what happened into its event word, in the comm functions and
//! whenever a Win16 program calls any system function (`service`). While
//! that waits, a UART holds what comes from the other end outside the
//! chip once its FIFO is full, as a cable with flow control would, so
//! nothing is lost between calls.
//!
//! A port's id (cid) is its number from 0, COM1's 0. The structures are
//! Windows 3.1's, byte-packed: the DCB of 25 bytes (Id, BaudRate,
//! ByteSize, Parity, StopBits, three timeouts, a word of flags, XonChar,
//! XoffChar, XonLim, XoffLim, PeChar, EofChar, EvtChar, TxDelay) and the
//! COMSTAT of 5 (a status byte, cbInQue, cbOutQue).

use super::global;
use crate::cpu::Cpu;
use std::collections::VecDeque;

/// OpenComm's and the others' errors.
const IE_BADID: i16 = -1;
const IE_OPEN: i16 = -2;
const IE_NOPEN: i16 = -3;
const IE_MEMORY: i16 = -4;
const IE_DEFAULT: i16 = -5;
const IE_HARDWARE: i16 = -10;
const IE_BYTESIZE: i16 = -11;
const IE_BAUDRATE: i16 = -12;

/// GetCommError's errors.
const CE_RXOVER: u16 = 0x0001;
const CE_OVERRUN: u16 = 0x0002;
const CE_RXPARITY: u16 = 0x0004;
const CE_FRAME: u16 = 0x0008;
const CE_BREAK: u16 = 0x0010;
const CE_TXFULL: u16 = 0x0100;

/// The events of the event word.
const EV_RXCHAR: u16 = 0x0001;
const EV_RXFLAG: u16 = 0x0002;
const EV_TXEMPTY: u16 = 0x0004;
const EV_CTS: u16 = 0x0008;
const EV_DSR: u16 = 0x0010;
const EV_RLSD: u16 = 0x0020;
const EV_BREAK: u16 = 0x0040;
const EV_ERR: u16 = 0x0080;
const EV_RING: u16 = 0x0100;
/// Windows 3.1's: the state of CTS, DSR and RLSD (set while the line is
/// on), and the trailing edge of a ring.
const EV_CTSS: u16 = 0x0400;
const EV_DSRS: u16 = 0x0800;
const EV_RLSDS: u16 = 0x1000;
const EV_RINGTE: u16 = 0x2000;

/// COMSTAT's status: transmission held as by an XOFF.
const CSTF_XOFFHOLD: u8 = 0x08;

/// EscapeCommFunction's functions.
const SETXOFF: u16 = 1;
const SETXON: u16 = 2;
const SETRTS: u16 = 3;
const CLRRTS: u16 = 4;
const SETDTR: u16 = 5;
const CLRDTR: u16 = 6;
const GETMAXLPT: u16 = 8;
const GETMAXCOM: u16 = 9;
const GETBASEIRQ: u16 = 10;

/// The baud rates' CBR_ codes, which BaudRate may hold in place of the
/// rate, and the rates they stand for.
const CBR: &[(u16, u32)] = &[
    (0xFF10, 110),
    (0xFF11, 300),
    (0xFF12, 600),
    (0xFF13, 1200),
    (0xFF14, 2400),
    (0xFF15, 4800),
    (0xFF16, 9600),
    (0xFF17, 14400),
    (0xFF18, 19200),
    (0xFF1B, 38400),
    (0xFF1F, 56000),
    (0xFF20, 57600),
    (0xFF21, 115200),
    (0xFF23, 128000),
    (0xFF27, 256000),
];

/// The DCB's size and its fields' places.
pub const DCB_SIZE: usize = 25;
const DCB_BAUD: usize = 1;
const DCB_BYTESIZE: usize = 3;
const DCB_PARITY: usize = 4;
const DCB_STOPBITS: usize = 5;
const DCB_FLAGS: usize = 12;
const DCB_EVTCHAR: usize = 22;

/// The DCB's flags.
const F_BINARY: u16 = 0x0001;
const F_RTS_DISABLE: u16 = 0x0002;
const F_PARITY: u16 = 0x0004;
const F_OUTX_CTS_FLOW: u16 = 0x0008;
const F_OUTX_DSR_FLOW: u16 = 0x0010;
const F_DTR_DISABLE: u16 = 0x0080;
const F_OUTX: u16 = 0x0100;
const F_INX: u16 = 0x0200;
const F_DTRFLOW: u16 = 0x2000;
const F_RTSFLOW: u16 = 0x4000;

/// The ports the machine can have: COM1 to COM4.
const PORTS: usize = 4;

/// The event word's block: the word, and the copy of the modem status
/// register at offset 35 that programs read there (Microsoft's KB
/// Q101417).
const EVENT_BLOCK: u32 = 40;
const MSR_OFFSET: u32 = 35;

/// The UART's registers and bits the driver uses.
const UART_LCR: u16 = 3;
const UART_MCR: u16 = 4;
const UART_LSR: u16 = 5;
const UART_MSR: u16 = 6;
const LCR_BREAK: u8 = 0x40;
const LCR_DLAB: u8 = 0x80;
const MCR_DTR: u8 = 0x01;
const MCR_RTS: u8 = 0x02;

/// An open port.
#[derive(Clone, Debug, Default)]
pub struct Port {
    /// Its id: 0 for COM1.
    pub id: u8,
    /// The task that opened it.
    pub task: u16,
    /// The sizes of the receive and transmit queues OpenComm was given.
    pub in_size: u16,
    pub out_size: u16,
    /// What came in and wasn't read yet, and UngetCommChar's character,
    /// which ReadComm gives first.
    pub input: VecDeque<u8>,
    pub unget: Option<u8>,
    /// What waits to go out while transmission is held (SETXOFF,
    /// SetCommBreak), and whether it is.
    pub held: VecDeque<u8>,
    pub xoff: bool,
    pub brk: bool,
    /// The DCB the port was last set up with.
    pub dcb: Vec<u8>,
    /// The global block of the event word, and the events it collects.
    pub block: u16,
    pub mask: u16,
    /// The CE_ errors since GetCommError last gave them.
    pub errors: u16,
    /// The modem status lines (MSR's top four bits) last seen.
    pub lines: u8,
    /// Whether it had characters to send when last seen, for EV_TXEMPTY.
    pub sending: bool,
}

crate::state_fields!(Port {
    id,
    task,
    in_size,
    out_size,
    input,
    unget,
    held,
    xoff,
    brk,
    dcb,
    block,
    mask,
    errors,
    lines,
    sending
});

/// A 16-bit function's INT16 result.
fn ret(value: i16) -> Option<u32> {
    Some(value as u16 as u32)
}

/// The place in the open ports of the port `cid`.
fn find(cpu: &Cpu, cid: u32) -> Option<usize> {
    let cid = cid as u16;
    if cid as usize >= PORTS {
        return None;
    }
    cpu.bus
        .win32
        .win16
        .comm
        .iter()
        .position(|p| p.id as u16 == cid)
}

/// The COM port a device name names ("COM1", "com2:"): its number from
/// 0, None for another name.
fn com_number(name: &[u8]) -> Option<usize> {
    let name = name.strip_suffix(b":").unwrap_or(name);
    match name {
        [c, o, m, digit] if [c, o, m].map(u8::to_ascii_uppercase) == *b"COM" => (b'1'..=b'9')
            .contains(digit)
            .then(|| (digit - b'1') as usize),
        _ => None,
    }
}

/// The DCB a device definition makes, as MODE writes one and WIN.INI's
/// [ports] has them: "COM1:9600,n,8,1" and an optional fifth field, x for
/// XON/XOFF or p for hardware flow control. The rate may be the first
/// two digits of one (96 for 9600). The rest of the DCB is zero, but for
/// fBinary, with no flow control unless the definition asks for it. None
/// for a definition it can't read.
fn build_dcb(def: &[u8]) -> Option<[u8; DCB_SIZE]> {
    let text = std::str::from_utf8(def).ok()?;
    let (device, settings) = text.split_once(':').unwrap_or((text, ""));
    let id = com_number(device.trim_end().as_bytes())?;
    if settings.contains('=') {
        // The Win32 form ("baud=9600 parity=n"), which Windows 3.1 doesn't
        // read.
        return None;
    }
    let fields: Vec<&str> = settings.split(',').map(str::trim).collect();
    if !(4..=5).contains(&fields.len()) {
        return None;
    }
    let baud: u32 = match fields[0].parse().ok()? {
        11 => 110,
        30 => 300,
        60 => 600,
        12 => 1200,
        24 => 2400,
        48 => 4800,
        96 => 9600,
        19 => 19200,
        rate => rate,
    };
    // 115200 doesn't fit the word: SetCommState takes a rate above 57600
    // for 115200, as Wine writes it.
    let baud = match baud {
        115200 => 57601,
        rate => u16::try_from(rate).ok()?,
    };
    let parity = match fields[1].to_ascii_uppercase().as_str() {
        "N" => 0,
        "O" => 1,
        "E" => 2,
        "M" => 3,
        "S" => 4,
        _ => return None,
    };
    let size: u8 = fields[2].parse().ok()?;
    if !(5..=8).contains(&size) {
        return None;
    }
    let stop = match fields[3] {
        "1" => 0,
        "1.5" => 1,
        "2" => 2,
        _ => return None,
    };
    let mut flags = F_BINARY;
    if parity != 0 {
        flags |= F_PARITY;
    }
    match fields.get(4).map(|f| f.to_ascii_uppercase()) {
        None => {}
        Some(f) if f == "X" => flags |= F_OUTX | F_INX,
        Some(f) if f == "P" => {
            flags |= F_OUTX_CTS_FLOW | F_OUTX_DSR_FLOW | F_DTRFLOW | F_RTSFLOW;
        }
        Some(_) => return None,
    }
    let mut dcb = [0u8; DCB_SIZE];
    dcb[0] = id as u8;
    dcb[DCB_BAUD..DCB_BAUD + 2].copy_from_slice(&baud.to_le_bytes());
    dcb[DCB_BYTESIZE] = size;
    dcb[DCB_PARITY] = parity;
    dcb[DCB_STOPBITS] = stop;
    dcb[DCB_FLAGS..DCB_FLAGS + 2].copy_from_slice(&flags.to_le_bytes());
    Some(dcb)
}

fn dcb_word(dcb: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([dcb[at], dcb[at + 1]])
}

/// The UART's divisor for the DCB's BaudRate (a rate, or a CBR_ code; a
/// rate above 57600 is 115200), or its error.
fn divisor(baud: u16) -> Result<u16, i16> {
    let rate = match CBR.iter().find(|&&(code, _)| code == baud) {
        Some(&(_, rate)) => rate,
        None if baud > 57600 => 115200,
        None => baud as u32,
    };
    if rate == 0 || rate > 115200 {
        return Err(IE_BAUDRATE);
    }
    Ok((115200 / rate) as u16)
}

/// Set port `n`'s UART up as the DCB says: the rate, the line format,
/// DTR and RTS, and a break while one is held.
fn apply_dcb(cpu: &mut Cpu, n: usize, dcb: &[u8], brk: bool) -> Result<(), i16> {
    let divisor = divisor(dcb_word(dcb, DCB_BAUD))?;
    let size = dcb[DCB_BYTESIZE];
    if !(5..=8).contains(&size) {
        return Err(IE_BYTESIZE);
    }
    // Parity: none, odd, even, mark, space (the stick parity bits).
    let parity = match dcb[DCB_PARITY] {
        1 => 0x08,
        2 => 0x18,
        3 => 0x28,
        4 => 0x38,
        _ => 0,
    };
    let stop = if dcb[DCB_STOPBITS] != 0 { 0x04 } else { 0 };
    let lcr = (size - 5) | stop | parity | if brk { LCR_BREAK } else { 0 };
    let bus = &mut cpu.bus;
    bus.serial_set(n, UART_LCR, LCR_DLAB);
    bus.serial_set(n, 0, divisor as u8);
    bus.serial_set(n, 1, (divisor >> 8) as u8);
    bus.serial_set(n, UART_LCR, lcr);
    let flags = dcb_word(dcb, DCB_FLAGS);
    let mut mcr = bus.serial_get(n, UART_MCR).unwrap_or(0) & !(MCR_DTR | MCR_RTS);
    if flags & F_DTR_DISABLE == 0 {
        mcr |= MCR_DTR;
    }
    if flags & F_RTS_DISABLE == 0 {
        mcr |= MCR_RTS;
    }
    bus.serial_set(n, UART_MCR, mcr);
    Ok(())
}

/// Change bits of port `n`'s UART register `reg`: those of `bits` on or
/// off.
fn set_bits(cpu: &mut Cpu, n: usize, reg: u16, bits: u8, on: bool) {
    let value = cpu.bus.serial_get(n, reg).unwrap_or(0);
    let value = if on { value | bits } else { value & !bits };
    cpu.bus.serial_set(n, reg, value);
}

/// The linear address of the port's event word.
fn event_word(cpu: &Cpu, port: &Port) -> u32 {
    super::lin(cpu, global::handle_to_sel(port.block), 0)
}

/// Bring the open port `i` up to now, as COMM.DRV's interrupt handler
/// would have: what was received into its queue while it has room, the
/// held characters out once nothing holds them, and the events into its
/// event word. Whether anything changed.
fn pump(cpu: &mut Cpu, i: usize) -> bool {
    let port = &cpu.bus.win32.win16.comm[i];
    let n = port.id as usize;
    let room = (port.in_size as usize).saturating_sub(port.input.len());
    let evt_char = port.dcb.get(DCB_EVTCHAR).copied().unwrap_or(0);
    let mut received = Vec::new();
    while received.len() < room {
        match cpu.bus.serial_receive(n) {
            Some(byte) => received.push(byte),
            None => break,
        }
    }
    let mut events = 0;
    if !received.is_empty() {
        events |= EV_RXCHAR;
        if received.contains(&evt_char) {
            events |= EV_RXFLAG;
        }
    }
    let lsr = cpu.bus.serial_get(n, UART_LSR).unwrap_or(0);
    let msr = cpu.bus.serial_get(n, UART_MSR).unwrap_or(0);
    let mut errors = 0;
    for (bit, error) in [(0x02, CE_OVERRUN), (0x04, CE_RXPARITY), (0x08, CE_FRAME)] {
        if lsr & bit != 0 {
            errors |= error;
            events |= EV_ERR;
        }
    }
    if lsr & 0x10 != 0 {
        errors |= CE_BREAK;
        events |= EV_BREAK;
    }
    // The lines that changed since the last look, or in between (MSR's
    // delta bits).
    let port = &cpu.bus.win32.win16.comm[i];
    let lines = msr & 0xF0;
    let changed = (lines ^ port.lines) | (msr & 0x0F) << 4;
    for (bit, event) in [(0x10, EV_CTS), (0x20, EV_DSR), (0x80, EV_RLSD)] {
        if changed & bit != 0 {
            events |= event;
        }
    }
    if lines & !port.lines & 0x40 != 0 {
        events |= EV_RING;
    }
    if msr & 0x04 != 0 {
        events |= EV_RINGTE;
    }
    let state = [(0x10, EV_CTSS), (0x20, EV_DSRS), (0x80, EV_RLSDS)]
        .iter()
        .filter(|&&(bit, _)| lines & bit != 0)
        .fold(0, |s, &(_, event)| s | event);
    let release = !port.xoff && !port.brk && !port.held.is_empty();
    let mut held = VecDeque::new();
    if release {
        held = std::mem::take(&mut cpu.bus.win32.win16.comm[i].held);
        for &byte in &held {
            cpu.bus.serial_send(n, byte);
        }
    }
    let in_uart = cpu.bus.serial_sending(n).unwrap_or(0) > 0;
    let port = &cpu.bus.win32.win16.comm[i];
    let sending = in_uart || !port.held.is_empty();
    if port.sending && !sending {
        events |= EV_TXEMPTY;
    }
    let (mask, old_lines, at) = (port.mask, port.lines, event_word(cpu, port));
    let port = &mut cpu.bus.win32.win16.comm[i];
    port.input.extend(&received);
    port.errors |= errors;
    port.lines = lines;
    port.sending = sending;
    let mut wrote = false;
    if at != 0 {
        let word = super::read_16(cpu, at);
        let states = EV_CTSS | EV_DSRS | EV_RLSDS;
        let new = (word & !(states & mask)) | (events | state) & mask;
        if new != word {
            super::write_16(cpu, at, new);
            wrote = true;
        }
        let msr_at = at + MSR_OFFSET;
        let old = super::peek(cpu, msr_at, 1).map_or(0, |b| b[0]);
        let copy = (old & 0x0F) | lines;
        if copy != old {
            super::poke(cpu, msr_at, &[copy]);
            wrote = true;
        }
    }
    wrote || !received.is_empty() || errors != 0 || !held.is_empty() || lines != old_lines
}

/// Bring the open ports up to now (`pump`), after a system function a
/// Win16 program called. When that changed anything, the function
/// changed more than RAM and registers, which an idle proof has to know.
pub fn service(cpu: &mut Cpu) {
    let mut changed = false;
    for i in 0..cpu.bus.win32.win16.comm.len() {
        changed |= pump(cpu, i);
    }
    if changed {
        crate::idle::before_service(cpu);
    }
}

/// The open port `cid`, brought up to now: its place.
fn open_port(cpu: &mut Cpu, cid: u32) -> Option<usize> {
    let i = find(cpu, cid)?;
    pump(cpu, i);
    Some(i)
}

/// OpenComm(name, cbInQueue, cbOutQueue): the port's id, or IE_BADID for
/// a name that isn't a COM port's, IE_HARDWARE for a port the machine
/// doesn't have, IE_OPEN for one open already. The port starts with
/// WIN.INI's settings for it ([ports], "COM1:=9600,n,8,1,x"), or
/// 9600,n,8,1, with DTR and RTS on.
pub fn open_comm(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = super::string_bytes(cpu, args[0], 16);
    // LPT ports aren't there: their ids would be 80h on.
    let Some(n) = com_number(&name) else {
        return ret(IE_BADID);
    };
    if n >= PORTS || cpu.bus.serial.ports[n].is_none() {
        return ret(IE_HARDWARE);
    }
    if find(cpu, n as u32).is_some() {
        return ret(IE_OPEN);
    }
    let key = format!("COM{}:", n + 1);
    let settings = crate::win32::kernel32::profile_value(cpu, b"WIN.INI", b"ports", key.as_bytes())
        .unwrap_or_else(|| b"9600,n,8,1".to_vec());
    let mut def = key.into_bytes();
    def.extend_from_slice(&settings);
    let Some(dcb) = build_dcb(&def) else {
        return ret(IE_DEFAULT);
    };
    let task = super::current_task(cpu).map_or(0, |t| cpu.bus.win32.win16.tasks[t].handle);
    let block = global::alloc(
        cpu,
        global::GMEM_FIXED | global::GMEM_ZEROINIT,
        EVENT_BLOCK,
        task,
        false,
    );
    if block == 0 {
        return ret(IE_MEMORY);
    }
    if apply_dcb(cpu, n, &dcb, false).is_err() {
        // WIN.INI's settings are ones the UART can't do.
        global::free(cpu, block);
        return ret(IE_DEFAULT);
    }
    // What the UART received before goes, as the driver's start reads it
    // out.
    while cpu.bus.serial_receive(n).is_some() {}
    let lines = cpu.bus.serial_get(n, UART_MSR).unwrap_or(0) & 0xF0;
    cpu.bus.win32.win16.comm.push(Port {
        id: n as u8,
        task,
        in_size: args[1] as u16,
        out_size: args[2] as u16,
        dcb: dcb.to_vec(),
        block,
        lines,
        ..Port::default()
    });
    ret(n as i16)
}

/// Close the open port `i`: DTR and RTS off, no break, its event word
/// gone. What it was sending goes on out; what transmission held is
/// dropped.
fn close(cpu: &mut Cpu, i: usize) {
    let port = cpu.bus.win32.win16.comm.remove(i);
    let n = port.id as usize;
    set_bits(cpu, n, UART_LCR, LCR_BREAK, false);
    set_bits(cpu, n, UART_MCR, MCR_DTR | MCR_RTS, false);
    global::free(cpu, port.block);
}

/// CloseComm(cid): 0, or -1 for a port that isn't open.
pub fn close_comm(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = find(cpu, args[0]) else {
        return ret(-1);
    };
    close(cpu, i);
    ret(0)
}

/// The task `task` ends: the ports it opened close, so that they aren't
/// left open with no one to close them.
pub fn task_ended(cpu: &mut Cpu, task: u16) {
    while let Some(i) = cpu.bus.win32.win16.comm.iter().position(|p| p.task == task) {
        close(cpu, i);
    }
}

/// SetCommState(dcb): set the port the DCB's Id names up as it says. 0,
/// IE_NOPEN for a port that isn't open, IE_BAUDRATE or IE_BYTESIZE for
/// a rate or size the UART can't do.
pub fn set_comm_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dcb) = super::peek(cpu, args[0], DCB_SIZE) else {
        return ret(IE_BADID);
    };
    let Some(i) = find(cpu, dcb[0] as u32) else {
        return ret(IE_NOPEN);
    };
    let port = &cpu.bus.win32.win16.comm[i];
    let (n, brk) = (port.id as usize, port.brk);
    if let Err(e) = apply_dcb(cpu, n, &dcb, brk) {
        return ret(e);
    }
    cpu.bus.win32.win16.comm[i].dcb = dcb;
    ret(0)
}

/// GetCommState(cid, dcb): the DCB the port was last set up with. 0, or
/// -1 for a port that isn't open.
pub fn get_comm_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = find(cpu, args[0]) else {
        return ret(-1);
    };
    let dcb = cpu.bus.win32.win16.comm[i].dcb.clone();
    super::poke(cpu, args[1], &dcb);
    ret(0)
}

/// BuildCommDCB(def, dcb): the DCB of the definition (`build_dcb`). 0, or
/// -1 for one it can't read, which leaves the DCB as it was.
pub fn build_comm_dcb(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let def = super::string_bytes(cpu, args[0], 256);
    match build_dcb(&def) {
        Some(dcb) => {
            super::poke(cpu, args[1], &dcb);
            ret(0)
        }
        None => ret(-1),
    }
}

/// GetCommError(cid, comstat): the errors since the last call, which it
/// clears, and the port's state in the COMSTAT if there is one. -1 for
/// a port that isn't open.
pub fn get_comm_error(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = open_port(cpu, args[0]) else {
        return ret(-1);
    };
    let n = cpu.bus.win32.win16.comm[i].id as usize;
    let sending = cpu.bus.serial_sending(n).unwrap_or(0);
    let port = &mut cpu.bus.win32.win16.comm[i];
    let errors = std::mem::take(&mut port.errors);
    let status = if port.xoff { CSTF_XOFFHOLD } else { 0 };
    let in_que = port.input.len() + port.unget.is_some() as usize;
    let out_que = port.held.len() + sending;
    if args[1] != 0 {
        let mut stat = vec![status];
        stat.extend_from_slice(&(in_que.min(0xFFFF) as u16).to_le_bytes());
        stat.extend_from_slice(&(out_que.min(0xFFFF) as u16).to_le_bytes());
        super::poke(cpu, args[1], &stat);
    }
    Some(errors as u32)
}

/// ReadComm(cid, buffer, count): up to `count` characters of the receive
/// queue, the one UngetCommChar put back first. How many, negated while
/// GetCommError has errors to give; -1 for a port that isn't open.
pub fn read_comm(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = open_port(cpu, args[0]) else {
        return ret(-1);
    };
    let want = (args[2] as u16 as i16).max(0) as usize;
    let port = &mut cpu.bus.win32.win16.comm[i];
    let mut bytes = Vec::new();
    if want > 0
        && let Some(byte) = port.unget.take()
    {
        bytes.push(byte);
    }
    let rest = (want - bytes.len()).min(port.input.len());
    bytes.extend(port.input.drain(..rest));
    let failed = port.errors != 0;
    super::poke(cpu, args[1], &bytes);
    let count = bytes.len() as i16;
    ret(if failed { -count } else { count })
}

/// WriteComm(cid, buffer, count): the characters into the transmit queue,
/// as many as it has room for. How many; negated, with CE_TXFULL, when
/// that was fewer than `count`. -1 for a port that isn't open.
pub fn write_comm(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = open_port(cpu, args[0]) else {
        return ret(-1);
    };
    let want = (args[2] as u16 as i16).max(0) as usize;
    let port = &cpu.bus.win32.win16.comm[i];
    let n = port.id as usize;
    let (out_size, queued) = (port.out_size as usize, port.held.len());
    let queued = queued + cpu.bus.serial_sending(n).unwrap_or(0);
    let count = want.min(out_size.saturating_sub(queued));
    let bytes = super::peek(cpu, args[1], count).unwrap_or_default();
    let port = &mut cpu.bus.win32.win16.comm[i];
    if port.xoff || port.brk {
        port.held.extend(&bytes);
    } else {
        for &byte in &bytes {
            cpu.bus.serial_send(n, byte);
        }
    }
    let port = &mut cpu.bus.win32.win16.comm[i];
    port.sending |= !bytes.is_empty();
    let count = bytes.len() as i16;
    if (count as usize) < want {
        port.errors |= CE_TXFULL;
        return ret(-count);
    }
    ret(count)
}

/// TransmitCommChar(cid, char): the character out at once, ahead of what
/// transmission holds. 0, or -1 for a port that isn't open or in a
/// break.
pub fn transmit_comm_char(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = open_port(cpu, args[0]) else {
        return ret(-1);
    };
    let port = &mut cpu.bus.win32.win16.comm[i];
    if port.brk {
        return ret(-1);
    }
    port.sending = true;
    let n = port.id as usize;
    cpu.bus.serial_send(n, args[1] as u8);
    ret(0)
}

/// UngetCommChar(cid, char): the character back, for the next ReadComm.
/// 0, or -1 for a port that isn't open or has one back already
/// (CE_RXOVER).
pub fn unget_comm_char(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = find(cpu, args[0]) else {
        return ret(-1);
    };
    let port = &mut cpu.bus.win32.win16.comm[i];
    if port.unget.is_some() {
        port.errors |= CE_RXOVER;
        return ret(-1);
    }
    port.unget = Some(args[1] as u8);
    ret(0)
}

/// SetCommEventMask(cid, mask): the events the event word collects from
/// now on, and the far pointer to the word, or 0 for a port that isn't
/// open. The copy of the modem status register is 35 bytes on.
pub fn set_comm_event_mask(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = open_port(cpu, args[0]) else {
        return Some(0);
    };
    let port = &mut cpu.bus.win32.win16.comm[i];
    port.mask = args[1] as u16;
    let sel = global::handle_to_sel(port.block);
    // The state bits as they are now, for those the mask takes.
    pump(cpu, i);
    Some((sel as u32) << 16)
}

/// GetCommEventMask(cid, clear): the event word, whose bits `clear` has
/// it then clears. 0 for a port that isn't open.
pub fn get_comm_event_mask(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = open_port(cpu, args[0]) else {
        return Some(0);
    };
    let at = event_word(cpu, &cpu.bus.win32.win16.comm[i]);
    let word = super::read_16(cpu, at);
    let clear = args[1] as u16;
    super::write_16(cpu, at, word & !clear);
    Some((word & clear) as u32)
}

/// SetCommBreak(cid) and ClearCommBreak(cid): the line in a break, with
/// transmission held, and back. 0, or -1 for a port that isn't open.
pub fn set_comm_break(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    comm_break(cpu, args[0], true)
}

pub fn clear_comm_break(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    comm_break(cpu, args[0], false)
}

fn comm_break(cpu: &mut Cpu, cid: u32, on: bool) -> Option<u32> {
    let Some(i) = find(cpu, cid) else {
        return ret(-1);
    };
    let port = &mut cpu.bus.win32.win16.comm[i];
    port.brk = on;
    let n = port.id as usize;
    set_bits(cpu, n, UART_LCR, LCR_BREAK, on);
    pump(cpu, i);
    ret(0)
}

/// EscapeCommFunction(cid, function): SETXOFF and SETXON hold
/// transmission and let it go on, as an XOFF and an XON from the other
/// end do; SETRTS, CLRRTS, SETDTR and CLRDTR set the lines. 0, or -1 for
/// a port that isn't open and another function. GETMAXCOM gives the
/// highest COM port id there can be (COM4's), GETMAXLPT the highest LPT
/// one (LPT3's), GETBASEIRQ the port's base address and IRQ (the IRQ in
/// the high word), for a port the machine has.
pub fn escape_comm_function(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let cid = args[0] as u16;
    match args[1] as u16 {
        GETMAXCOM => return Some(PORTS as u32 - 1),
        GETMAXLPT => return Some(0x82),
        GETBASEIRQ => {
            let port = cpu
                .bus
                .serial
                .ports
                .get(cid as usize)
                .and_then(|p| p.as_ref());
            return Some(port.map_or(0xFFFF_FFFF, |p| {
                p.uart.base as u32 | (p.uart.irq as u32) << 16
            }));
        }
        _ => {}
    }
    let Some(i) = find(cpu, cid as u32) else {
        return Some(0xFFFF_FFFF);
    };
    let n = cpu.bus.win32.win16.comm[i].id as usize;
    match args[1] as u16 {
        SETXOFF => cpu.bus.win32.win16.comm[i].xoff = true,
        SETXON => cpu.bus.win32.win16.comm[i].xoff = false,
        SETRTS => set_bits(cpu, n, UART_MCR, MCR_RTS, true),
        CLRRTS => set_bits(cpu, n, UART_MCR, MCR_RTS, false),
        SETDTR => set_bits(cpu, n, UART_MCR, MCR_DTR, true),
        CLRDTR => set_bits(cpu, n, UART_MCR, MCR_DTR, false),
        _ => return Some(0xFFFF_FFFF),
    }
    pump(cpu, i);
    Some(0)
}

/// FlushComm(cid, queue): empty the transmit queue (0) or the receive
/// queue (1). 0, or -1 for a port that isn't open or another queue.
pub fn flush_comm(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = find(cpu, args[0]) else {
        return ret(-1);
    };
    let n = cpu.bus.win32.win16.comm[i].id as usize;
    match args[1] as u16 {
        0 => {
            cpu.bus.win32.win16.comm[i].held.clear();
            cpu.bus.serial_clear_tx(n);
        }
        1 => {
            while cpu.bus.serial_receive(n).is_some() {}
            let port = &mut cpu.bus.win32.win16.comm[i];
            port.input.clear();
            port.unget = None;
        }
        _ => return ret(-1),
    }
    ret(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_dcb_reads_mode_definitions() {
        let dcb = build_dcb(b"COM2:9600,n,8,1").unwrap();
        assert_eq!(dcb[0], 1);
        assert_eq!(dcb_word(&dcb, DCB_BAUD), 9600);
        assert_eq!(
            (dcb[DCB_BYTESIZE], dcb[DCB_PARITY], dcb[DCB_STOPBITS]),
            (8, 0, 0)
        );
        assert_eq!(dcb_word(&dcb, DCB_FLAGS), F_BINARY);

        let dcb = build_dcb(b"com1: 96, E, 7, 2, x").unwrap();
        assert_eq!(dcb[0], 0);
        assert_eq!(dcb_word(&dcb, DCB_BAUD), 9600);
        assert_eq!(
            (dcb[DCB_BYTESIZE], dcb[DCB_PARITY], dcb[DCB_STOPBITS]),
            (7, 2, 2)
        );
        assert_eq!(
            dcb_word(&dcb, DCB_FLAGS),
            F_BINARY | F_PARITY | F_OUTX | F_INX
        );

        let dcb = build_dcb(b"COM1:115200,n,8,1.5,p").unwrap();
        assert_eq!(divisor(dcb_word(&dcb, DCB_BAUD)), Ok(1));
        assert_eq!(dcb[DCB_STOPBITS], 1);

        for bad in [
            &b"LPT1:9600,n,8,1"[..],
            b"COM1:9600,n,8",
            b"COM1:9600,q,8,1",
            b"COM1:9600,n,9,1",
            b"COM1:baud=9600 parity=n data=8 stop=1",
            b"COM1:9600,n,8,1,z",
        ] {
            assert_eq!(build_dcb(bad), None, "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn divisors_of_rates_and_cbr_codes() {
        assert_eq!(divisor(9600), Ok(12));
        assert_eq!(divisor(0xFF16), Ok(12));
        assert_eq!(divisor(0xFF10), Ok(1047));
        assert_eq!(divisor(57601), Ok(1));
        assert_eq!(divisor(0), Err(IE_BAUDRATE));
        assert_eq!(divisor(0xFF27), Err(IE_BAUDRATE));
    }

    #[test]
    fn com_numbers() {
        assert_eq!(com_number(b"COM1"), Some(0));
        assert_eq!(com_number(b"com4:"), Some(3));
        assert_eq!(com_number(b"COM9"), Some(8));
        assert_eq!(com_number(b"COM0"), None);
        assert_eq!(com_number(b"COM10"), None);
        assert_eq!(com_number(b"LPT1"), None);
    }
}
