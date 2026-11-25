//! MODE, which game batch files run to put the screen back when the game
//! ends (`mode co80`, `mode con lines=25`) or to set the keyboard's rate:
//! it sets the mode through the BIOS and prints nothing, or MS-DOS's
//! MODE's messages.

use rust_dos::command::{CommandDispatcher, split_command};
use rust_dos::cpu::Cpu;
use std::fs;
use std::path::PathBuf;

fn machine(name: &str) -> Box<Cpu> {
    let base = PathBuf::from("target/test_mode").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    let mut cpu = Cpu::new(base);
    set_mode(&mut cpu, 0x03);
    cpu
}

fn set_mode(cpu: &mut Cpu, mode: u8) {
    cpu.set_ax(mode as u16);
    rust_dos::interrupts::int10::handle(cpu);
}

/// Run a MODE command line and return what it printed.
fn mode(cpu: &mut Cpu, line: &str) -> String {
    for b in cpu.bus.vga.vram_text.iter_mut() {
        *b = 0;
    }
    cpu.bus.set_console_cursor(0, 0);
    let (command, args) = split_command(line);
    assert!(
        CommandDispatcher::new().dispatch(cpu, command, args),
        "{} runs",
        line
    );
    cpu.bus
        .vga
        .vram_text
        .chunks(160)
        .take(25)
        .map(|row| {
            row.iter()
                .step_by(2)
                .map(|&b| if b == 0 { ' ' } else { b as char })
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn bios_mode(cpu: &mut Cpu) -> u8 {
    cpu.bus.guest_read_8(0x0449)
}

fn rows(cpu: &mut Cpu) -> u8 {
    cpu.bus.guest_read_8(0x0484) + 1
}

#[test]
fn co80_puts_a_graphics_screen_back_in_80x25_and_prints_nothing() {
    let mut cpu = machine("co80");
    set_mode(&mut cpu, 0x13);
    assert_eq!(mode(&mut cpu, "mode co80"), "");
    assert_eq!(bios_mode(&mut cpu), 0x03);
    assert_eq!(cpu.bus.guest_read_16(0x044A), 80);
    assert_eq!(rows(&mut cpu), 25);
    // The equipment word says 80x25 on a colour card, as MODE sets it.
    assert_eq!(cpu.bus.guest_read_16(0x0410) & 0x30, 0x20);
    // MODE.COM's name works too, in any case.
    set_mode(&mut cpu, 0x13);
    assert_eq!(mode(&mut cpu, "MODE.COM CO80"), "");
    assert_eq!(bios_mode(&mut cpu), 0x03);
}

#[test]
fn the_display_types_and_widths_set_their_modes() {
    let mut cpu = machine("types");
    for (line, want) in [
        ("mode bw40", 0x00),
        ("mode co40", 0x01),
        ("mode bw80", 0x02),
        ("mode co80", 0x03),
    ] {
        set_mode(&mut cpu, 0x13);
        assert_eq!(mode(&mut cpu, line), "", "{}", line);
        assert_eq!(bios_mode(&mut cpu), want, "{}", line);
    }
    // A width keeps the colours of the mode there is (HANDLE_40_OR_80).
    for (from, line, want) in [
        (0x03, "mode 40", 0x01),
        (0x01, "mode 80", 0x03),
        (0x00, "mode 80", 0x02),
        (0x13, "mode 80", 0x03),
    ] {
        set_mode(&mut cpu, from);
        assert_eq!(mode(&mut cpu, line), "", "{}", line);
        assert_eq!(bios_mode(&mut cpu), want, "{} from {:02X}h", line, from);
    }
}

#[test]
fn mono_needs_a_monochrome_display() {
    let mut cpu = machine("mono");
    set_mode(&mut cpu, 0x13);
    assert_eq!(
        mode(&mut cpu, "mode mono"),
        "Function not supported on this computer - MONO"
    );
    assert_eq!(bios_mode(&mut cpu), 0x13, "the mode stays");
}

#[test]
fn con_lines_and_cols_set_the_text_screen() {
    let mut cpu = machine("lines");
    assert_eq!(mode(&mut cpu, "mode con lines=50"), "");
    assert_eq!((bios_mode(&mut cpu), rows(&mut cpu)), (0x03, 50));
    assert_eq!(mode(&mut cpu, "mode con: cols=80 lines=25"), "");
    assert_eq!((bios_mode(&mut cpu), rows(&mut cpu)), (0x03, 25));
    // After a graphics mode, as a game leaves it.
    set_mode(&mut cpu, 0x13);
    assert_eq!(mode(&mut cpu, "mode con cols=80 lines=25"), "");
    assert_eq!((bios_mode(&mut cpu), rows(&mut cpu)), (0x03, 25));
    // The rows as the display form's second parameter.
    assert_eq!(mode(&mut cpu, "mode co80,50"), "");
    assert_eq!((bios_mode(&mut cpu), rows(&mut cpu)), (0x03, 50));
    // A width keeps the rows, as ANSI.SYS does for MODE.
    assert_eq!(mode(&mut cpu, "mode con cols=40"), "");
    assert_eq!((bios_mode(&mut cpu), rows(&mut cpu)), (0x01, 50));
}

#[test]
fn lines_take_ansi_sys() {
    let mut cpu = machine("no_ansi");
    cpu.bus.ansi_sys = false;
    assert_eq!(
        mode(&mut cpu, "mode con lines=50"),
        "ANSI.SYS must be installed to perform requested function"
    );
    assert_eq!(rows(&mut cpu), 25);
    // The width alone doesn't (HANDLE_40_OR_80).
    assert_eq!(mode(&mut cpu, "mode con cols=40"), "");
    assert_eq!(bios_mode(&mut cpu), 0x01);
}

#[test]
fn rate_and_delay_set_the_typematic_rate() {
    let mut cpu = machine("rate");
    assert_eq!(mode(&mut cpu, "mode con rate=32 delay=1"), "");
    assert_eq!(cpu.bus.kbc.typematic, 0x00, "30 a second after 250 ms");
    assert_eq!(mode(&mut cpu, "mode con rate=1 delay=4"), "");
    assert_eq!(cpu.bus.kbc.typematic, 0x7F, "2 a second after 1 s");
    assert_eq!(
        mode(&mut cpu, "mode con rate=20"),
        "RATE and DELAY must be specified together"
    );
    assert_eq!(cpu.bus.kbc.typematic, 0x7F);
}

#[test]
fn wrong_parameters_get_ms_dos_messages() {
    let mut cpu = machine("wrong");
    set_mode(&mut cpu, 0x13);
    assert_eq!(mode(&mut cpu, "mode co81"), "Invalid parameter - CO81");
    assert_eq!(
        mode(&mut cpu, "mode con lines=30"),
        "Invalid parameter - LINES=30"
    );
    assert_eq!(
        mode(&mut cpu, "mode con rate=33 delay=1"),
        "Invalid parameter - RATE=33"
    );
    assert_eq!(mode(&mut cpu, "mode co80,51"), "Invalid parameter - 51");
    assert_eq!(bios_mode(&mut cpu), 0x13, "nothing changes");
    assert_eq!(
        mode(&mut cpu, "mode com1:9600,n,8,1"),
        "Illegal device name"
    );
    assert_eq!(mode(&mut cpu, "mode lpt1:=com1"), "Illegal device name");
}

#[test]
fn con_status_tells_the_columns_and_lines() {
    let mut cpu = machine("status");
    let text = mode(&mut cpu, "mode con");
    assert_eq!(
        text,
        "Status for device CON:\n----------------------\nCOLUMNS=80\nLINES=25\n\n\
         Code page operation not supported on this device"
    );
}
