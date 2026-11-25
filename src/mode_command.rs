//! MODE, the program MS-DOS has in C:\DOS, for the screen and keyboard
//! forms game batch files run when the game ends:
//!
//! ```text
//! MODE CO80                       MODE 80
//! MODE CON COLS=80 LINES=25       MODE CON RATE=32 DELAY=1
//! ```
//!
//! It does what MS-DOS 4.0's MODE does (MODEVID.ASM, MODESCRN.ASM,
//! TYPAMAT.ASM and INVOKE.ASM in Microsoft's published source), through
//! the BIOS, and prints its messages (MODE.SKL). The serial and printer
//! forms (`MODE COM1:9600,N,8,1`, `MODE LPT1:=COM1`) aren't here: they say
//! the device name is illegal. Code pages, which MODE leaves to
//! DISPLAY.SYS, aren't either.
//!
//! DOSBox has MODE as a program on Z:; MS-DOS has it in C:\DOS, on the
//! PATH. A game's own MODE.COM in the current directory or on the PATH
//! runs instead (`command::z_program`).

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::shell::video_call;
use crate::video::print_string;

pub struct ModeCommand;

impl ShellCommand for ModeCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let line = args.trim_matches([' ', '\t']);
        if line == "/?" {
            return print_string(cpu, HELP);
        }
        let upper = line.to_ascii_uppercase();
        let words: Vec<&str> = upper.split([' ', '\t']).filter(|w| !w.is_empty()).collect();
        let Some(&first) = words.first() else {
            return con_status(cpu);
        };
        let device = first.split([':', '=', ',']).next().unwrap_or(first);
        if device == "CON" {
            if words.len() == 1 {
                return con_status(cpu);
            }
            return con_features(cpu, &words[1..]);
        }
        if is_port(device) {
            return print_string(cpu, "\r\nIllegal device name\r\n");
        }
        // The display forms: a type or a width, then the rows or the
        // direction to shift the screen, all one parameter apart (by
        // commas or spaces).
        let parts: Vec<&str> = upper
            .split([' ', '\t', ','])
            .filter(|w| !w.is_empty())
            .collect();
        display(cpu, &parts);
    }
}

/// MODE /?, after DOSBox's MODE (MS-DOS 6.22 has its HELP for it).
const HELP: &str = "Configures system devices.\r\n\r\n\
MODE display-type        display-type codes are CO80, BW80, CO40, BW40, MONO,\r\n\
\x20                        80 or 40\r\n\
MODE display-type,n      n is the lines on the screen: 25, 43 or 50\r\n\
MODE CON COLS=c LINES=n  c is 80 or 40, n 25, 43 or 50\r\n\
MODE CON RATE=r DELAY=d  typematic rates, r=1-32 (32=fastest), d=1-4 (1=lowest)\r\n";

/// The display types, with the equipment word's bits 4-5 MODE sets for
/// them (0410h), the BIOS mode and whether it is the monochrome adapter's.
const DISPLAY_TYPES: [(&str, u16, u8, bool); 5] = [
    ("BW40", 0x10, 0x00, false),
    ("CO40", 0x10, 0x01, false),
    ("BW80", 0x20, 0x02, false),
    ("CO80", 0x20, 0x03, false),
    ("MONO", 0x30, 0x07, true),
];

fn is_port(device: &str) -> bool {
    let numbered = |prefix: &str| {
        device
            .strip_prefix(prefix)
            .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit())
    };
    numbered("COM") || numbered("LPT") || device == "PRN" || device == "AUX"
}

/// "Invalid parameter - X", the DOS parser's message (PARSE10).
fn invalid(cpu: &mut Cpu, parameter: &str) {
    print_string(cpu, &format!("Invalid parameter - {}\r\n", parameter));
}

fn not_supported(cpu: &mut Cpu, parameter: &str) {
    print_string(
        cpu,
        &format!(
            "\r\nFunction not supported on this computer - {}\r\n",
            parameter
        ),
    );
}

const ANSI_NOT_LOADED: &str = "\r\nANSI.SYS must be installed to perform requested function\r\n";

/// MODE type[,rows] or MODE type[,R|L[,T]].
fn display(cpu: &mut Cpu, parts: &[&str]) {
    let first = parts[0];
    let known = DISPLAY_TYPES.iter().find(|(name, ..)| *name == first);
    if known.is_none() && first != "80" && first != "40" {
        return invalid(cpu, first);
    }
    // Check the parameters before anything changes, as the parser does:
    // the rows, or R or L and then T (the test pattern) or nothing.
    let mut rows = None;
    let mut direction = None;
    match parts[1..] {
        [] => {}
        [second, ref rest @ ..] if second == "R" || second == "L" => {
            direction = Some(second);
            if let Some(&third) = rest.first().filter(|&&t| t != "T") {
                return invalid(cpu, third);
            }
            if let Some(extra) = rest.get(1) {
                return invalid(cpu, extra);
            }
        }
        [second, ref rest @ ..] => {
            match second.parse::<u16>() {
                Ok(n @ (25 | 43 | 50)) => rows = Some(n),
                _ => return invalid(cpu, second),
            }
            if let Some(extra) = rest.first() {
                return invalid(cpu, extra);
            }
        }
    }
    match known {
        Some(&(name, equipment, mode, mono)) => {
            // MODE asks the BIOS which displays there are (INT 10h
            // AH=1Bh): a colour type needs a colour one, MONO a
            // monochrome one.
            if mono != cpu.bus.vga.adapter.mono_only() {
                return not_supported(cpu, name);
            }
            let flags = cpu.bus.guest_read_16(0x0410);
            cpu.bus.guest_write_16(0x0410, flags & !0x30 | equipment);
            video_call(cpu, mode as u16, 0, 0, 0);
            video_call(cpu, 0x0200, 0, 0, 0);
            let cursor = if mono { 0x0B0C } else { 0x0607 };
            video_call(cpu, 0x0100, 0, cursor, 0);
        }
        None => {
            let cols = if first == "40" { 40 } else { 80 };
            if cpu.bus.ansi_sys {
                // MODE asks ANSI.SYS for the width, which keeps the rows.
                let rows = rows.take().unwrap_or_else(|| kept_rows(cpu));
                if let Err(()) = set_text(cpu, cols, rows) {
                    return not_supported(cpu, first);
                }
            } else {
                width(cpu, cols);
            }
        }
    }
    if let Some(direction) = direction {
        shift(cpu, direction);
    }
    if let Some(rows) = rows {
        if !cpu.bus.ansi_sys {
            return print_string(cpu, ANSI_NOT_LOADED);
        }
        let cols = kept_cols(cpu);
        if let Err(()) = set_text(cpu, cols, rows) {
            not_supported(cpu, &rows.to_string());
        }
    }
}

/// MODE 40 or 80 without ANSI.SYS (HANDLE_40_OR_80): the mode of that
/// width in the colours of the one there is, or the same mode again.
fn width(cpu: &mut Cpu, cols: u16) {
    let mode = cpu.bus.guest_read_8(0x0449);
    let bw = matches!(mode, 0x00 | 0x02 | 0x05 | 0x06 | 0x07 | 0x11);
    let colour = matches!(mode, 0x01 | 0x03 | 0x04 | 0x12 | 0x13);
    let new = match (cols, bw, colour) {
        (40, true, _) => 0x00,
        (40, _, true) => 0x01,
        (80, true, _) => 0x02,
        (80, _, true) => 0x03,
        _ => mode,
    };
    video_call(cpu, new as u16, 0, 0, 0);
}

/// MODE's SHIFT_SCREEN on an EGA or VGA with a colour display, and on a
/// monochrome one: the screen can't move.
fn shift(cpu: &mut Cpu, direction: &str) {
    let adapter = cpu.bus.vga.adapter;
    if adapter.ega_bios() || adapter.mono_only() {
        let side = if direction == "R" { "right" } else { "left" };
        print_string(
            cpu,
            &format!("\r\n      Unable to shift screen {}\r\n", side),
        );
    } else {
        cpu.bus
            .log_string("[MODE] Shifting a CGA's picture isn't emulated");
    }
}

/// The rows of the screen, as the BIOS keeps them (0484h on an EGA or
/// VGA).
fn text_rows(cpu: &mut Cpu) -> u16 {
    if cpu.bus.vga.adapter.ega_bios() {
        cpu.bus.guest_read_8(0x0484) as u16 + 1
    } else {
        25
    }
}

fn text_cols(cpu: &mut Cpu) -> u16 {
    cpu.bus.guest_read_16(0x044A)
}

/// The rows a change of the width keeps: those of the text mode there
/// is, or 25 after a graphics mode.
fn kept_rows(cpu: &mut Cpu) -> u16 {
    match text_rows(cpu) {
        rows @ (43 | 50) if text_mode(cpu) => rows,
        _ => 25,
    }
}

/// The width a change of the rows keeps: 40 columns in a text mode of
/// 40, else 80.
fn kept_cols(cpu: &mut Cpu) -> u16 {
    if text_mode(cpu) && text_cols(cpu) == 40 {
        40
    } else {
        80
    }
}

/// Whether the screen is in a text mode.
fn text_mode(cpu: &mut Cpu) -> bool {
    matches!(cpu.bus.guest_read_8(0x0449), 0x00..=0x03 | 0x07)
}

/// Set a text mode of `cols` and `rows` as ANSI.SYS does for MODE's
/// IOCTL (INT 21h AX=440Ch CL=5Fh): the mode, then the 8x8 font for 43 or
/// 50 rows, on an EGA's 350 scanlines or a VGA's 350 (43) or 400 (50).
/// Err if the adapter can't show them.
fn set_text(cpu: &mut Cpu, cols: u16, rows: u16) -> Result<(), ()> {
    let adapter = cpu.bus.vga.adapter;
    let vga = adapter.vga_bios();
    let possible = match rows {
        25 => true,
        43 => adapter.ega_bios(),
        _ => vga,
    };
    if !possible {
        return Err(());
    }
    let mode = cpu.bus.guest_read_8(0x0449);
    let new = if adapter.mono_only() || mode == 0x07 {
        0x07
    } else {
        let bw = matches!(mode, 0x00 | 0x02);
        match (cols, bw) {
            (40, true) => 0x00,
            (40, false) => 0x01,
            (_, true) => 0x02,
            (_, false) => 0x03,
        }
    };
    // A VGA's scanlines for the mode: 350 for 43 rows, 400 otherwise
    // (INT 10h AH=12h BL=30h AL=1 or 2).
    if vga {
        let scanlines = if rows == 43 { 0x1201 } else { 0x1202 };
        video_call(cpu, scanlines, 0x0030, 0, 0);
    }
    video_call(cpu, new as u16, 0, 0, 0);
    if rows != 25 {
        video_call(cpu, 0x1112, 0, 0, 0);
    }
    Ok(())
}

/// MODE CON with keywords: COLS= (or COLUMNS=), LINES=, RATE= and DELAY=
/// (or DEL=).
fn con_features(cpu: &mut Cpu, words: &[&str]) {
    let (mut cols, mut lines, mut rate, mut delay) = (None, None, None, None);
    for &word in words {
        let Some((key, value)) = word.split_once('=') else {
            return invalid(cpu, word);
        };
        let number = value.parse::<u16>().ok();
        let (slot, valid): (&mut Option<u16>, fn(u16) -> bool) = match key {
            "COLS" | "COLUMNS" => (&mut cols, |n| n == 40 || n == 80),
            "LINES" => (&mut lines, |n| matches!(n, 25 | 43 | 50)),
            "RATE" => (&mut rate, |n| (1..=32).contains(&n)),
            "DELAY" | "DEL" => (&mut delay, |n| (1..=4).contains(&n)),
            _ => return invalid(cpu, word),
        };
        match number.filter(|&n| valid(n)) {
            Some(n) => *slot = Some(n),
            None => return invalid(cpu, word),
        }
    }
    if rate.is_some() != delay.is_some() {
        return print_string(cpu, "\r\nRATE and DELAY must be specified together\r\n");
    }
    if lines.is_some() || cols.is_some() {
        if cpu.bus.ansi_sys {
            let cols = cols.unwrap_or_else(|| kept_cols(cpu));
            let rows = lines.unwrap_or_else(|| kept_rows(cpu));
            if let Err(()) = set_text(cpu, cols, rows) {
                let what = match lines {
                    Some(n) => format!("LINES={}", n),
                    None => format!("COLS={}", cols),
                };
                not_supported(cpu, &what);
            }
        } else if lines.is_some() {
            print_string(cpu, ANSI_NOT_LOADED);
        } else if let Some(cols) = cols {
            width(cpu, cols);
        }
    }
    if let (Some(rate), Some(delay)) = (rate, delay) {
        // INT 16h AX=0305h: BH the delay (0-3), BL the rate (0 the
        // fastest), from MODE's 1-4 and 1-32 (TYPAMAT.ASM).
        cpu.set_ax(0x0305);
        cpu.set_reg16(iced_x86::Register::BX, (delay - 1) << 8 | (32 - rate));
        crate::interrupts::int16::handle(cpu);
    }
}

/// MODE CON, and MODE without parameters, which also tells the printers'
/// and serial ports' settings in MS-DOS: the screen's columns and lines,
/// which MODE asks ANSI.SYS for, and the code pages, which take
/// DISPLAY.SYS.
fn con_status(cpu: &mut Cpu) {
    print_string(
        cpu,
        "\r\nStatus for device CON:\r\n----------------------\r\n",
    );
    if cpu.bus.ansi_sys {
        let (cols, rows) = if text_mode(cpu) {
            (text_cols(cpu).to_string(), text_rows(cpu).to_string())
        } else {
            ("NONE".to_string(), "NONE".to_string())
        };
        print_string(cpu, &format!("COLUMNS={}\r\nLINES={}\r\n", cols, rows));
    }
    print_string(
        cpu,
        "\r\nCode page operation not supported on this device\r\n",
    );
}
