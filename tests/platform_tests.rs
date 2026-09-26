//! The AT platform: memory above 1 MB and the A20 gate, the cascaded PICs,
//! the 8042 keyboard controller, CMOS, CPU reset, and INT 15h.

use iced_x86::Register;
use rust_dos::bus::Bus;
use rust_dos::cpu::{Cpu, CpuFlags, CpuState};
use rust_dos::interrupts::int15;
use std::path::PathBuf;

mod testrunners;
use testrunners::run_cpu_code;

fn cpu() -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.set_ss(0);
    cpu.set_sp(0x8000);
    cpu
}

#[test]
fn a20_gate_decides_whether_addresses_wrap_at_1mb() {
    let mut cpu = cpu();
    cpu.bus.write_8(0x0000_0010, 0x11);
    cpu.bus.write_8(0x0010_0010, 0x22);
    cpu.set_es(0xFFFF);
    cpu.set_bx(0x0020);
    // 26 8A 07 -> MOV AL, ES:[BX] (FFFF:0020 = 10_0010h)
    let code = [0x26, 0x8A, 0x07];

    cpu.bus.set_a20(false);
    run_cpu_code(&mut cpu, &code);
    assert_eq!(cpu.get_al(), 0x11, "A20 off: wraps to 0000:0010");

    cpu.set_ip(0x100);
    cpu.bus.set_a20(true);
    run_cpu_code(&mut cpu, &code);
    assert_eq!(cpu.get_al(), 0x22, "A20 on: the HMA");
}

#[test]
fn a20_through_port_92h_and_the_keyboard_controller() {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.io_write(0x92, 0x02);
    assert!(bus.a20());
    assert_eq!(bus.io_read(0x92) & 0x02, 0x02);
    bus.io_write(0x92, 0x00);
    assert!(!bus.a20());

    // 8042: D1h writes the output port; bit 1 is A20, bit 0 must stay set.
    bus.io_write(0x64, 0xD1);
    bus.io_write(0x60, 0xDF);
    assert!(bus.a20());
    assert!(!bus.reset_requested);
    // D0h reads it back.
    bus.io_write(0x64, 0xD0);
    assert_eq!(bus.io_read(0x60), 0xDF);
    bus.io_write(0x64, 0xDD);
    assert!(!bus.a20());
}

#[test]
fn memory_above_1mb_and_past_the_end_of_ram() {
    let mut bus = Bus::with_memory(PathBuf::from("."), 4);
    bus.write_32(0x0030_0000, 0xDEAD_BEEF);
    assert_eq!(bus.read_32(0x0030_0000), 0xDEAD_BEEF);
    // Nothing past the end of RAM: reads float high, writes vanish.
    bus.write_8(0x0040_0000, 0x12);
    assert_eq!(bus.read_8(0x0040_0000), 0xFF);
    // The top of the address space mirrors the BIOS ROM.
    assert_eq!(bus.read_8(0xFFFF_FFFE), 0xFC, "model byte");
}

#[test]
fn pic_initialization_sets_the_vector_base() {
    let mut bus = Bus::new(PathBuf::from("."));
    // Remap the master to 20h, as some protected-mode programs do.
    bus.io_write(0x20, 0x11); // ICW1: ICW4 follows, cascade
    bus.io_write(0x21, 0x20); // ICW2: vector base
    bus.io_write(0x21, 0x04); // ICW3: slave on IRQ 2
    bus.io_write(0x21, 0x01); // ICW4: 8086 mode
    assert_eq!(bus.io_read(0x21), 0x00, "initialization clears the mask");
    bus.pic.raise(0);
    assert_eq!(bus.pic_pending_irq(), Some(0));
    assert_eq!(bus.pic_acknowledge(0), 0x20);
}

#[test]
fn slave_pic_delivers_through_the_cascade() {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.io_write(0xA1, 0x00); // unmask the slave
    bus.pic.raise(10);
    assert_eq!(bus.pic_pending_irq(), Some(10));
    assert_eq!(bus.pic_acknowledge(10), 0x72);
    // In service on both chips until both get an EOI.
    bus.io_write(0x20, 0x0B);
    bus.io_write(0xA0, 0x0B);
    assert_eq!(bus.io_read(0x20), 0x04);
    assert_eq!(bus.io_read(0xA0), 0x04);
    bus.pic.raise(10);
    assert_eq!(bus.pic_pending_irq(), None);
    bus.io_write(0xA0, 0x20);
    bus.io_write(0x20, 0x20);
    assert_eq!(bus.pic_pending_irq(), Some(10));
}

#[test]
fn keyboard_controller_queues_every_scan_code_byte() {
    let mut bus = Bus::new(PathBuf::from("."));
    // Right arrow pressed and released: E0 4D, E0 CD.
    rust_dos::keyboard::deliver_key_down(&mut bus, 0x4D00, true);
    rust_dos::keyboard::deliver_key_up(&mut bus, 0x4D, true);

    let mut bytes = Vec::new();
    while bus.io_read(0x64) & 0x01 != 0 {
        // One IRQ 1 per byte.
        assert_eq!(bus.pic_pending_irq(), Some(1));
        bus.pic_acknowledge(1);
        bytes.push(bus.io_read(0x60));
        bus.io_write(0x20, 0x20);
    }
    assert_eq!(bytes, [0xE0, 0x4D, 0xE0, 0xCD]);
    assert_eq!(bus.pic_pending_irq(), None);
}

#[test]
fn keys_the_bios_never_handled_go_when_the_shell_takes_over() {
    use rust_dos::keyboard::{bios_saw_keys, drop_unseen_keys, key_event};
    let mut bus = Bus::new(PathBuf::from("."));
    // A typed while the BIOS's INT 09h runs: typed ahead for the prompt.
    key_event(&mut bus, 0x1E, false, true, None);
    key_event(&mut bus, 0x1E, false, false, None);
    bios_saw_keys(&mut bus);
    // S typed into a program that reads the keyboard itself.
    key_event(&mut bus, 0x1F, false, true, None);
    key_event(&mut bus, 0x1F, false, false, None);
    assert_eq!(bus.keyboard_buffer.len(), 2);
    drop_unseen_keys(&mut bus);
    assert_eq!(bus.keyboard_buffer.iter().copied().collect::<Vec<_>>(), [0x1E61]);
}

#[test]
fn keyboard_acknowledges_commands() {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.io_write(0x60, 0xED); // set LEDs
    assert_eq!(bus.io_read(0x60), 0xFA);
    bus.io_write(0x60, 0x02);
    assert_eq!(bus.io_read(0x60), 0xFA);
    bus.io_write(0x64, 0xAA); // controller self test
    assert_eq!(bus.io_read(0x60), 0x55);
}

#[test]
fn cmos_reports_memory_and_keeps_the_shutdown_byte() {
    let mut bus = Bus::with_memory(PathBuf::from("."), 8);
    let read = |bus: &mut Bus, index: u8| {
        bus.io_write(0x70, index);
        bus.io_read(0x71)
    };
    let ext_kb = read(&mut bus, 0x17) as u16 | (read(&mut bus, 0x18) as u16) << 8;
    assert_eq!(ext_kb, 7 * 1024);
    assert_eq!(read(&mut bus, 0x15), 0x80, "640 KB base memory");
    bus.io_write(0x70, 0x0F);
    bus.io_write(0x71, 0x0A);
    assert_eq!(read(&mut bus, 0x0F), 0x0A);
    // The clock ticks in BCD.
    assert!(read(&mut bus, 0x00) & 0x0F <= 9);
}

#[test]
fn reset_with_shutdown_code_0ah_resumes_through_40_67() {
    let mut cpu = cpu();
    cpu.bus.io_write(0x70, 0x0F);
    cpu.bus.io_write(0x71, 0x0A);
    cpu.bus.write_16(0x0467, 0x0200);
    cpu.bus.write_16(0x0469, 0x3000);
    // B0 FE -> MOV AL, FEh ; E6 64 -> OUT 64h, AL (pulse the reset line)
    run_cpu_code(&mut cpu, &[0xB0, 0xFE, 0xE6, 0x64]);
    assert_eq!((cpu.cs(), cpu.ip()), (0xF000, 0xFFF0), "reset vector");
    cpu.step(); // the BIOS reset entry
    assert_eq!((cpu.cs(), cpu.ip()), (0x3000, 0x0200));
    assert_eq!(cpu.state, CpuState::Running);
}

#[test]
fn reset_without_shutdown_code_ends_the_program() {
    let mut cpu = cpu();
    // B0 01 -> MOV AL, 1 ; E6 92 -> OUT 92h, AL (fast reset)
    run_cpu_code(&mut cpu, &[0xB0, 0x01, 0xE6, 0x92]);
    cpu.step();
    assert_eq!(cpu.state, CpuState::RebootShell);
}

#[test]
fn bios_timer_calls_int_1ch() {
    let mut cpu = cpu();
    // An INT 1Ch hook that counts in 0000:0600.
    // FF 06 00 06 -> INC WORD [0600] ; CF -> IRET
    cpu.bus.load_bytes(0x0700, &[0xFF, 0x06, 0x00, 0x06, 0xCF]);
    cpu.bus.write_16(0x1C * 4, 0x0700);
    cpu.bus.write_16(0x1C * 4 + 2, 0x0000);
    cpu.set_cpu_flag(CpuFlags::IF, true);
    cpu.bus.load_bytes(0x100, &[0xEB, 0xFE]); // JMP $
    cpu.bus.pic.raise(0);
    for _ in 0..20 {
        cpu.step();
    }
    assert_eq!(cpu.bus.read_16(0x0600), 1);
    assert_eq!(cpu.bus.read_16(0x046C), 1, "BIOS tick count");
    assert_eq!(cpu.bus.pic.master.isr, 0, "EOI sent");
    assert_eq!(cpu.ip(), 0x100, "back in the program");
}

#[test]
fn int_15h_a20_and_memory_functions() {
    let mut cpu = cpu();
    cpu.set_ax(0x2401);
    int15::handle(&mut cpu);
    assert!(cpu.bus.a20() && !cpu.get_cpu_flag(CpuFlags::CF));
    cpu.set_ax(0x2402);
    int15::handle(&mut cpu);
    assert_eq!(cpu.get_al(), 1);

    // The XMS driver owns extended memory: none left for INT 15h users.
    cpu.set_reg8(Register::AH, 0x88);
    int15::handle(&mut cpu);
    assert_eq!(cpu.ax(), 0);

    cpu.set_ax(0xE801);
    int15::handle(&mut cpu);
    assert_eq!((cpu.ax(), cpu.bx()), (15 * 1024, 0));

    // Unknown functions fail the way a BIOS does.
    cpu.set_ax(0xBFDE);
    int15::handle(&mut cpu);
    assert!(cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cpu.get_ah(), 0x86);
}

#[test]
fn int_15h_block_move_copies_to_extended_memory() {
    let mut cpu = cpu();
    cpu.bus.load_bytes(0x5000, b"hello, extended memory!!");
    // GDT at 0000:4000: source descriptor at +10h, destination at +18h.
    let gdt = 0x4000;
    let desc = |base: u32| {
        [0xFF, 0xFF, base as u8, (base >> 8) as u8, (base >> 16) as u8, 0x93, 0x00, (base >> 24) as u8]
    };
    cpu.bus.load_bytes(gdt + 0x10, &desc(0x5000));
    cpu.bus.load_bytes(gdt + 0x18, &desc(0x20_0000));
    cpu.set_es(0);
    cpu.set_si(gdt as u16);
    cpu.set_cx(12);
    cpu.set_reg8(Register::AH, 0x87);
    int15::handle(&mut cpu);
    let copied: Vec<u8> = (0..24).map(|i| cpu.bus.read_8(0x20_0000 + i)).collect();
    assert_eq!(&copied, b"hello, extended memory!!");
}

#[test]
fn bios_keyboard_buffer_holds_15_keys() {
    let mut bus = Bus::new(PathBuf::from("."));
    // A game with its own keyboard handler never reads the BIOS buffer.
    for _ in 0..100 {
        rust_dos::keyboard::deliver_key_down(&mut bus, 0x1E61, false);
    }
    assert_eq!(bus.keyboard_buffer.len(), 15);
}

#[test]
fn bios_translates_shift_ctrl_and_alt_combinations() {
    use rust_dos::keyboard::{bios_keystroke, deliver_key_down};
    const SHIFT: u8 = 0x02;
    const CTRL: u8 = 0x04;
    const ALT: u8 = 0x08;
    for (scan, ascii, flags, keystroke) in [
        (0x2D, b'x', 0, 0x2D78),
        (0x2D, b'x', ALT, 0x2D00),
        (0x2E, b'c', CTRL, 0x2E03),
        (0x3F, 0, ALT, 0x6C00),  // Alt+F5
        (0x43, 0, CTRL, 0x6600), // Ctrl+F9
        (0x3B, 0, SHIFT, 0x5400),
        (0x58, 0, CTRL, 0x8A00), // Ctrl+F12
        (0x57, 0, 0, 0x8500),    // F11
        (0x58, 0, SHIFT, 0x8800),
        (0x57, 0, ALT, 0x8B00),
        (0x02, b'1', ALT, 0x7800),
        (0x4B, 0, CTRL, 0x7300),
        (0x0F, 0x09, SHIFT, 0x0F00),
        (0x1C, 0x0D, CTRL, 0x1C0A),
    ] {
        assert_eq!(bios_keystroke(scan, ascii, flags), keystroke, "{scan:02X} {flags:02X}");
    }

    // Delivered keys follow the shift state at 40:17h.
    let mut bus = Bus::new(PathBuf::from("."));
    bus.write_8(0x0417, ALT);
    deliver_key_down(&mut bus, 0x3F00, false);
    assert_eq!(bus.keyboard_buffer.pop_front(), Some(0x6C00));
    assert_eq!(bus.kbc.read_data(), 0x3F, "the controller gets the plain make code");
}

#[test]
fn f11_and_f12_send_their_make_codes_and_only_the_enhanced_reads_return_them() {
    use rust_dos::keyboard::{apply_key, lookup};
    let mut cpu = cpu();
    let f12 = lookup("f12").unwrap();
    apply_key(&mut cpu.bus, f12, 0, true);
    assert_eq!(cpu.bus.kbc.read_data(), 0x58, "a game's own keyboard handler reads F12's make code");
    apply_key(&mut cpu.bus, f12, 0, false);
    assert_eq!(cpu.bus.kbc.read_data(), 0xD8);
    apply_key(&mut cpu.bus, lookup("a").unwrap(), b'a', true);

    // INT 16h AH=11h and 10h see F12 first...
    cpu.set_ax(0x1100);
    rust_dos::interrupts::int16::handle(&mut cpu);
    assert_eq!(cpu.ax(), 0x8600);
    // ...AH=01h and 00h skip it, as an AT BIOS does.
    cpu.set_ax(0x0100);
    rust_dos::interrupts::int16::handle(&mut cpu);
    assert_eq!(cpu.ax(), 0x1E61);
    cpu.set_ax(0x0000);
    rust_dos::interrupts::int16::handle(&mut cpu);
    assert_eq!(cpu.ax(), 0x1E61);
    assert!(cpu.bus.keyboard_buffer.is_empty());
}

#[test]
fn the_bios_keeps_left_and_right_ctrl_and_alt_apart() {
    use rust_dos::keyboard::key_event;
    let mut cpu = cpu();
    key_event(&mut cpu.bus, 0x1D, false, true, None); // left Ctrl
    key_event(&mut cpu.bus, 0x38, true, true, None); // right Alt
    assert_eq!(cpu.bus.read_8(0x0417) & 0x0C, 0x0C);
    cpu.set_ax(0x1200);
    rust_dos::interrupts::int16::handle(&mut cpu);
    assert_eq!(cpu.get_ah() & 0x0F, 0x09, "left Ctrl and right Alt");
    key_event(&mut cpu.bus, 0x1D, false, false, None);
    key_event(&mut cpu.bus, 0x38, true, false, None);
    assert_eq!(cpu.bus.read_8(0x0417) & 0x0C, 0);
    // Caps Lock toggles once however long it is held.
    let caps = cpu.bus.read_8(0x0417) & 0x40;
    key_event(&mut cpu.bus, 0x3A, false, true, None);
    key_event(&mut cpu.bus, 0x3A, false, true, None);
    key_event(&mut cpu.bus, 0x3A, false, false, None);
    assert_eq!(cpu.bus.read_8(0x0417) & 0x40, caps ^ 0x40);
}

#[test]
fn keys_type_in_the_keyboard_layout() {
    use rust_dos::keyboard::key_event;
    use rust_dos::keylayout::Layout;
    let mut cpu = cpu();
    cpu.bus.kbd.layout = Layout::by_code("gr").unwrap();
    let press = |cpu: &mut Cpu, scan: u8, extended: bool| {
        key_event(&mut cpu.bus, scan, extended, true, None);
        key_event(&mut cpu.bus, scan, extended, false, None);
        cpu.bus.keyboard_buffer.pop_front()
    };
    assert_eq!(press(&mut cpu, 0x15, false), Some(0x157A), "Z where the US keyboard has Y");
    // AltGr+Q is @, a plain character.
    key_event(&mut cpu.bus, 0x38, true, true, None);
    assert_eq!(press(&mut cpu, 0x10, false), Some(0x1040));
    // AltGr with a key that has nothing there is Alt.
    assert_eq!(press(&mut cpu, 0x1E, false), Some(0x1E00));
    key_event(&mut cpu.bus, 0x38, true, false, None);
    // Ctrl+Z is ^Z, on the German Z key.
    key_event(&mut cpu.bus, 0x1D, false, true, None);
    assert_eq!(press(&mut cpu, 0x15, false), Some(0x151A));
    key_event(&mut cpu.bus, 0x1D, false, false, None);
    // ´ then e is é; ´ then x is both; ´ then space the accent.
    assert_eq!(press(&mut cpu, 0x0D, false), None, "the dead key waits");
    assert_eq!(press(&mut cpu, 0x12, false), Some(0x1282));
    press(&mut cpu, 0x0D, false);
    assert_eq!(press(&mut cpu, 0x2D, false), Some(0x0027));
    assert_eq!(cpu.bus.keyboard_buffer.pop_front(), Some(0x2D78));
    // The make code is the key's, whatever it types.
    assert!(cpu.bus.kbc.read_data() != 0);
}
