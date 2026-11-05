//! The IPX driver of the built-in DOS (net/ipx.rs), as programs use it:
//! found with INT 2Fh AX=7A00h and called at its entry point, completing
//! ECBs through its IRQ and their ESRs. One machine sends packets to
//! itself and schedules events; two machines exchange a packet through a
//! relay on this machine, as two PCs on one Ethernet would.

use iced_x86::code_asm::*;
use rust_dos::cpu::Cpu;
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos::net::tunnel::relay::{RelayConfig, RelayServer};
use rust_dos::net::{IpxMode, NetSettings};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Where the test programs keep things, in their segment.
const ENTRY: u16 = 0x1000;
const START: u16 = 0x1004;
const NODE: u16 = 0x1010;
/// ECBs, with the fragment list for one fragment.
const LISTEN: u16 = 0x1100;
const LISTEN2: u16 = 0x1140;
const SEND: u16 = 0x1180;
const EVENT: u16 = 0x11C0;
/// The packet sent, and the buffers of the ECBs listening.
const PACKET: u16 = 0x1200;
const RECEIVED: u16 = 0x1300;
const RECEIVED2: u16 = 0x1500;
/// What the ESRs saw: how often they were called, AL, ES and SI.
const ESR_COUNT: u16 = 0x1700;
const ESR_AL: u16 = 0x1702;
const ESR_ES: u16 = 0x1704;
const ESR_SI: u16 = 0x1706;
const EVENT_COUNT: u16 = 0x1710;
const EVENT_AL: u16 = 0x1712;
const STEP: u16 = 0x1720;
const END: u16 = 0x1800;
/// Code at fixed offsets: the ESRs.
const ESR: u16 = 0x0C00;
const EVENT_ESR: u16 = 0x0D00;

const SOCKET: u16 = 0x5000;
const DATA: &[u8] = b"HELLO, IPX";

fn scratch(name: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let base = PathBuf::from("target/test_ipx").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

fn asm16(origin: u16, f: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> Vec<u8> {
    let mut a = CodeAssembler::new(16).unwrap();
    f(&mut a).unwrap();
    a.assemble(origin as u64).unwrap()
}

/// An ECB for socket `SOCKET` with an ESR at `esr` (0 for none) and one
/// fragment of `len` bytes at `buffer`; the segments are filled in when
/// the program runs.
fn ecb(esr: u16, buffer: u16, len: u16, immediate: [u8; 6]) -> Vec<u8> {
    let mut ecb = vec![0u8; 42];
    ecb[4..6].copy_from_slice(&esr.to_le_bytes());
    ecb[10..12].copy_from_slice(&SOCKET.to_be_bytes());
    ecb[28..34].copy_from_slice(&immediate);
    ecb[34..36].copy_from_slice(&1u16.to_le_bytes());
    ecb[36..38].copy_from_slice(&buffer.to_le_bytes());
    ecb[40..42].copy_from_slice(&len.to_le_bytes());
    ecb
}

/// The IPX header and data of the packet for `node`.
fn packet(node: [u8; 6]) -> Vec<u8> {
    let mut p = vec![0u8; 30];
    p[5] = 4; // a packet exchange packet
    p[10..16].copy_from_slice(&node);
    p[16..18].copy_from_slice(&SOCKET.to_be_bytes());
    p.extend_from_slice(DATA);
    p
}

/// A COM file with `parts` at their offsets in the segment.
fn com(parts: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let mut image = vec![0u8; (END - 0x100) as usize];
    for (at, code) in parts {
        let at = (*at - 0x100) as usize;
        image[at..at + code.len()].copy_from_slice(code);
    }
    image
}

/// The ESRs: count the calls, keep AL, ES and SI.
fn esrs() -> Vec<(u16, Vec<u8>)> {
    let esr = asm16(ESR, |a| {
        a.push(ds)?;
        a.push(cs)?;
        a.pop(ds)?;
        a.inc(word_ptr(ESR_COUNT))?;
        a.mov(byte_ptr(ESR_AL), al)?;
        a.mov(word_ptr(ESR_ES), es)?;
        a.mov(word_ptr(ESR_SI), si)?;
        a.pop(ds)?;
        a.retf()
    });
    let event = asm16(EVENT_ESR, |a| {
        a.push(ds)?;
        a.push(cs)?;
        a.pop(ds)?;
        a.inc(word_ptr(EVENT_COUNT))?;
        a.mov(byte_ptr(EVENT_AL), al)?;
        a.pop(ds)?;
        a.retf()
    });
    vec![(ESR, esr), (EVENT_ESR, event)]
}

/// Find the driver (step 1 on failure), and fill in this segment in the
/// ECBs' fragment addresses and in the ESR addresses of `with_esr`.
fn find_driver(a: &mut CodeAssembler, fail: CodeLabel, with_esr: &[u16]) -> Result<(), IcedError> {
    a.mov(word_ptr(STEP), 1u32)?;
    a.mov(ax, 0x7A00u32)?;
    a.int(0x2F)?;
    a.cmp(al, 0xFFu32)?;
    a.jne(fail)?;
    a.mov(word_ptr(ENTRY), di)?;
    a.mov(word_ptr(ENTRY + 2), es)?;
    a.push(cs)?;
    a.pop(es)?;
    for &ecb in with_esr {
        a.mov(word_ptr(ecb + 6), cs)?;
    }
    for ecb in [LISTEN, LISTEN2, SEND, EVENT] {
        a.mov(word_ptr(ecb + 38), cs)?;
    }
    Ok(())
}

/// Call the driver's function `function` with ES:SI at `ecb`.
fn call(a: &mut CodeAssembler, function: u16, ecb: u16) -> Result<(), IcedError> {
    a.mov(si, ecb as u32)?;
    a.mov(bx, function as u32)?;
    // CALL FAR [ENTRY]
    a.db(&[0xFF, 0x1E])?;
    a.dw(&[ENTRY])
}

/// Open socket `SOCKET`, or fail at `step`.
fn open(a: &mut CodeAssembler, step: u16, fail: CodeLabel) -> Result<(), IcedError> {
    a.mov(word_ptr(STEP), step as u32)?;
    a.mov(al, 0u32)?;
    a.mov(dx, SOCKET.swap_bytes() as u32)?;
    call(a, 0x00, 0)?;
    a.cmp(al, 0u32)?;
    a.jne(fail)
}

/// Keep the BIOS's tick count, for `waited`.
fn start_clock(a: &mut CodeAssembler) -> Result<(), IcedError> {
    a.mov(ah, 0u32)?;
    a.int(0x1A)?;
    a.mov(word_ptr(START), dx)
}

/// Jump to `timeout` once `ticks` BIOS ticks passed since `start_clock`.
fn waited(a: &mut CodeAssembler, ticks: u16, timeout: CodeLabel) -> Result<(), IcedError> {
    a.mov(ah, 0u32)?;
    a.int(0x1A)?;
    a.sub(dx, word_ptr(START))?;
    a.cmp(dx, ticks as u32)?;
    a.jae(timeout)
}

/// Relinquish control until the in-use flag of `ecb` clears, or fail at
/// `step` after `ticks` BIOS ticks.
fn wait_for(a: &mut CodeAssembler, ecb: u16, ticks: u16, step: u16, fail: CodeLabel) -> Result<(), IcedError> {
    a.mov(word_ptr(STEP), step as u32)?;
    start_clock(a)?;
    let mut again = a.create_label();
    let mut done = a.create_label();
    a.set_label(&mut again)?;
    call(a, 0x0A, 0)?;
    a.cmp(byte_ptr(ecb + 8), 0u32)?;
    a.je(done)?;
    waited(a, ticks, fail)?;
    a.jmp(again)?;
    a.set_label(&mut done)?;
    a.nop()
}

fn exit(a: &mut CodeAssembler, fail: &mut CodeLabel) -> Result<(), IcedError> {
    a.mov(ax, 0x4C00u32)?;
    a.int(0x21)?;
    a.set_label(fail)?;
    a.mov(al, byte_ptr(STEP))?;
    a.mov(ah, 0x4Cu32)?;
    a.int(0x21)
}

/// One machine: send a packet to its own node, with an ESR on the ECB
/// listening; schedule an event; then, with the driver's IRQ masked,
/// send again to an ECB without an ESR, which completes when the program
/// next calls the driver.
fn loopback_program(irq: u8) -> Vec<u8> {
    let main = asm16(0x100, |a| {
        let mut fail = a.create_label();
        find_driver(a, fail, &[LISTEN, EVENT])?;
        open(a, 2, fail)?;
        // The node, from Get Internetwork Address, is the packet's
        // destination and the ECB's immediate address.
        call(a, 0x09, NODE - 4)?;
        a.mov(ax, word_ptr(NODE))?;
        a.mov(word_ptr(PACKET + 10), ax)?;
        a.mov(word_ptr(SEND + 28), ax)?;
        a.mov(ax, word_ptr(NODE + 2))?;
        a.mov(word_ptr(PACKET + 12), ax)?;
        a.mov(word_ptr(SEND + 30), ax)?;
        a.mov(ax, word_ptr(NODE + 4))?;
        a.mov(word_ptr(PACKET + 14), ax)?;
        a.mov(word_ptr(SEND + 32), ax)?;
        a.mov(word_ptr(STEP), 3u32)?;
        call(a, 0x04, LISTEN)?;
        a.cmp(al, 0u32)?;
        a.jne(fail)?;
        // An event in two ticks.
        a.mov(ax, 2u32)?;
        call(a, 0x05, EVENT)?;
        call(a, 0x03, SEND)?;
        wait_for(a, LISTEN, 36, 4, fail)?;
        wait_for(a, EVENT, 36, 5, fail)?;
        // The IRQ masked: the next packet completes at a call.
        a.in_(al, 0xA1)?;
        a.or(al, 1u32 << (irq - 8))?;
        a.out(0xA1, al)?;
        call(a, 0x04, LISTEN2)?;
        call(a, 0x03, SEND)?;
        wait_for(a, LISTEN2, 36, 6, fail)?;
        exit(a, &mut fail)
    });
    let node = [0; 6];
    let mut parts = vec![
        (0x100, main),
        (LISTEN, ecb(ESR, RECEIVED, 0x200, node)),
        (LISTEN2, ecb(0, RECEIVED2, 0x200, node)),
        (SEND, ecb(0, PACKET, (30 + DATA.len()) as u16, node)),
        (EVENT, ecb(EVENT_ESR, 0, 0, node)),
        (PACKET, packet(node)),
    ];
    parts.extend(esrs());
    com(&parts)
}

/// A machine with the IPX driver installed and the program `name` in
/// `files` loaded from the shell.
fn machine(test: &str, files: &[(&str, Vec<u8>)], name: &str) -> (Cpu, u16) {
    let mut cpu = Cpu::new(scratch(test, files));
    cpu.bus.configure_network(&NetSettings { ipx: IpxMode::On, ..Default::default() });
    cpu.load_shell();
    assert!(cpu.load_executable(name, None));
    let psp = cpu.current_psp;
    (cpu, psp)
}

/// Run a batch; whether the program ended and the shell is back.
fn batch(cpu: &mut Cpu) -> bool {
    cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
    run_batch(cpu, &mut NoHook, false) == StopReason::ShellReloaded
}

fn word(cpu: &Cpu, psp: u16, offset: u16) -> u16 {
    cpu.bus.read_16(psp as usize * 16 + offset as usize)
}

fn bytes(cpu: &Cpu, psp: u16, offset: u16, len: usize) -> Vec<u8> {
    (0..len).map(|i| cpu.bus.read_8(psp as usize * 16 + offset as usize + i)).collect()
}

#[test]
fn a_machine_hears_itself_and_its_events() {
    // The IRQ is the first one no sound card has.
    let mut probe = Cpu::new(scratch("probe", &[]));
    probe.bus.configure_network(&NetSettings { ipx: IpxMode::On, ..Default::default() });
    let irq = probe.bus.net.ipx.as_ref().unwrap().irq;
    assert!(irq >= 9, "IRQ {}", irq);

    let (mut cpu, psp) = machine("loopback", &[("T.COM", loopback_program(irq))], "T.COM");
    let node = cpu.bus.net.ipx.as_ref().unwrap().node.0;
    let mut ended = false;
    for _ in 0..3000 {
        if batch(&mut cpu) {
            ended = true;
            break;
        }
    }
    let w = |offset| word(&cpu, psp, offset);
    assert!(ended, "the program didn't end (step {})", w(STEP));
    assert_eq!(cpu.errorlevel, 0, "failed at step {}", w(STEP));
    // The packet came to the ECB with the ESR, which was called with AL
    // FFh and ES:SI the ECB.
    assert_eq!(w(ESR_COUNT), 1);
    assert_eq!(w(ESR_AL) & 0xFF, 0xFF);
    assert_eq!((w(ESR_ES), w(ESR_SI)), (psp, LISTEN));
    assert_eq!(bytes(&cpu, psp, LISTEN + 9, 1), [0], "completion code");
    assert_eq!(bytes(&cpu, psp, LISTEN + 28, 6), node, "immediate address");
    let received = bytes(&cpu, psp, RECEIVED, 30 + DATA.len());
    assert_eq!(&received[30..], DATA);
    assert_eq!(&received[0..4], &[0xFF, 0xFF, 0, (30 + DATA.len()) as u8], "checksum and length");
    assert_eq!(&received[22..28], &node, "source node");
    assert_eq!(&received[28..30], &SOCKET.to_be_bytes(), "source socket");
    // The event's ESR was called with AL 00h.
    assert_eq!((w(EVENT_COUNT), w(EVENT_AL) & 0xFF), (1, 0));
    // The packet for the ECB without an ESR, while the IRQ was masked.
    assert_eq!(&bytes(&cpu, psp, RECEIVED2 + 30, DATA.len())[..], DATA);
    // The program's socket closed with it.
    assert!(cpu.bus.net.ipx.as_ref().unwrap().sockets.is_empty());
}

/// The sender: open the socket, then broadcast the packet every two
/// ticks, twenty times.
fn sender_program() -> Vec<u8> {
    let main = asm16(0x100, |a| {
        let mut fail = a.create_label();
        find_driver(a, fail, &[])?;
        open(a, 2, fail)?;
        a.mov(cx, 20u32)?;
        let mut again = a.create_label();
        a.set_label(&mut again)?;
        a.push(cx)?;
        call(a, 0x03, SEND)?;
        start_clock(a)?;
        let mut pause = a.create_label();
        let mut next = a.create_label();
        a.set_label(&mut pause)?;
        waited(a, 2, next)?;
        a.jmp(pause)?;
        a.set_label(&mut next)?;
        a.pop(cx)?;
        a.loop_(again)?;
        exit(a, &mut fail)
    });
    let broadcast = [0xFF; 6];
    com(&[(0x100, main), (SEND, ecb(0, PACKET, (30 + DATA.len()) as u16, broadcast)), (PACKET, packet(broadcast))])
}

/// The receiver: listen with an ESR, and wait up to five seconds.
fn receiver_program() -> Vec<u8> {
    let main = asm16(0x100, |a| {
        let mut fail = a.create_label();
        find_driver(a, fail, &[LISTEN])?;
        open(a, 2, fail)?;
        a.mov(word_ptr(STEP), 3u32)?;
        call(a, 0x04, LISTEN)?;
        a.cmp(al, 0u32)?;
        a.jne(fail)?;
        wait_for(a, LISTEN, 91, 4, fail)?;
        exit(a, &mut fail)
    });
    let mut parts = vec![(0x100, main), (LISTEN, ecb(ESR, RECEIVED, 0x200, [0; 6]))];
    parts.extend(esrs());
    com(&parts)
}

/// Wait up to five seconds for `f` to hold.
fn wait_until(mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn two_machines_exchange_a_packet_through_a_relay() {
    let relay = RelayServer::start("127.0.0.1:0".parse().unwrap(), RelayConfig::default(), Box::new(|_| {})).unwrap();
    let at = relay.local_addr().to_string();
    let (mut sender, _) = machine("sender", &[("S.COM", sender_program())], "S.COM");
    let (mut receiver, psp) = machine("receiver", &[("R.COM", receiver_program())], "R.COM");
    for cpu in [&mut sender, &mut receiver] {
        cpu.bus.net.join(Some(&at), "test", "").unwrap();
    }
    use rust_dos::net::hub::LanState;
    let joined = |cpu: &Cpu| matches!(cpu.bus.net.status().hub.map(|h| h.lan), Some(LanState::Joined { .. }));
    assert!(wait_until(|| joined(&sender) && joined(&receiver)), "{:?}", receiver.bus.net.status().hub);

    // Both run, a batch at a time, until the receiver has its packet.
    let sender_node = sender.bus.net.ipx.as_ref().unwrap().node.0;
    let deadline = Instant::now() + Duration::from_secs(20);
    let (mut sent, mut received) = (false, false);
    while !(sent && received) && Instant::now() < deadline {
        sent = sent || batch(&mut sender);
        received = received || batch(&mut receiver);
    }
    let w = |offset| word(&receiver, psp, offset);
    assert!(received, "the receiver didn't end (step {})", w(STEP));
    assert_eq!(receiver.errorlevel, 0, "the receiver failed at step {}", w(STEP));
    assert_eq!(w(ESR_COUNT), 1);
    assert_eq!((w(ESR_ES), w(ESR_SI)), (psp, LISTEN));
    assert_eq!(&bytes(&receiver, psp, RECEIVED + 30, DATA.len())[..], DATA);
    assert_eq!(bytes(&receiver, psp, LISTEN + 28, 6), sender_node, "immediate address: the sender's card");
    assert_eq!(&bytes(&receiver, psp, RECEIVED + 22, 6)[..], &sender_node, "the source node");
}
