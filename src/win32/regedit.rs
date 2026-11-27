//! The Registry Editor: C:\WINDOWS\REGEDIT.EXE, which setups run to put a
//! registration file into the registry. Video for Windows 1.1's setup
//! runs `regedit.exe /s C:\WINDOWS\mplayer.reg` to register Media
//! Player.
//!
//! Windows 95 and 98 have REGEDIT.EXE in the Windows directory, and its
//! command line does what Windows 3.1's did:
//!
//! - `regedit file.reg` imports the file, in Windows 95's format
//!   (`REGEDIT4`, then `[key]` and `"name"="value"` lines) or in Windows
//!   3.1's (`REGEDIT`, then `HKEY_CLASSES_ROOT\key = value` lines), and
//!   says in a message box that it did, or why it couldn't. `/s` imports
//!   it without the message box.
//! - `regedit /e file.reg [key]` writes the registry, or the branch of
//!   `key`, to the file in Windows 95's format.
//! - `/l:` and `/r:`, which name the registry's files, are taken and
//!   left out. `/c` (replace the whole registry with a file) and `/d`
//!   (delete a key) do nothing here, and nor does REGEDIT started with
//!   no file: the editor's window isn't there.
//!
//! The file is a PE image the emulator builds (`file`), shown in
//! C:\WINDOWS with Notepad (dll_files.rs), so WinExec, CreateProcess
//! and the prompt find it and start it as a process of its own. Its code
//! calls RegeditMain, a function of ADVAPI32 that ADVAPI32 doesn't
//! export, which does the work, and shows the message RegeditMain leaves
//! with MessageBoxA, until RegeditMain leaves none; then it ends.

use super::kernel32;
use super::registry;
use crate::cpu::Cpu;
use rust_dos_pe::{self as pe, Builder, Imp};

/// Where the program is loaded.
const BASE: u32 = 0x40_0000;

/// What RegeditMain leaves in the program's data, by offset: the message
/// box's type, its caption and its text, and the place on the command
/// line of the next file to import (0 before the first call).
const D_TYPE: u32 = 0;
const D_CAPTION: u32 = 4;
const CAPTION_SIZE: u32 = 64;
const D_TEXT: u32 = D_CAPTION + CAPTION_SIZE;
const TEXT_SIZE: u32 = 1024;
const D_NEXT: u32 = D_TEXT + TEXT_SIZE;
const DATA_SIZE: u32 = D_NEXT + 4;

const MB_ICONERROR: u32 = 0x10;
const MB_ICONINFORMATION: u32 = 0x40;

/// The caption of Windows 95's REGEDIT's message boxes.
const CAPTION: &str = "Registry Editor";

/// The file of REGEDIT.EXE: a program for the GUI subsystem that calls
/// RegeditMain(data) and shows the message box it asks for, again until
/// it asks for none, and ends.
pub fn file(bus: &crate::bus::Bus) -> Vec<u8> {
    let mut b = Builder::exe(BASE);
    b.subsystem(pe::SUBSYSTEM_GUI);
    let kernel32 = [Imp::Name("ExitProcess")];
    let user32 = [Imp::Name("MessageBoxA")];
    // An import of ADVAPI32 puts it in the process, RegeditMain with it.
    let advapi32 = [Imp::Name("RegCloseKey")];
    let iat = b.imports(&[
        ("KERNEL32.DLL", &kernel32),
        ("USER32.DLL", &user32),
        ("ADVAPI32.DLL", &advapi32),
    ]);
    let (exit, message_box) = (iat[0][0], iat[1][0]);
    let flags = pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE;
    let data = BASE + b.section_sized(".data", &[], DATA_SIZE, flags);
    // Where RegeditMain is in the ADVAPI32 image the process will have.
    let main = super::advapi32::address_for_new(bus, "RegeditMain");
    let mut code = Vec::new();
    let mut op = |bytes: &[u8], value: Option<u32>| {
        code.extend_from_slice(bytes);
        if let Some(v) = value {
            code.extend_from_slice(&v.to_le_bytes());
        }
    };
    // loop:
    op(&[0x68], Some(data)); // push data
    op(&[0xB8], Some(main)); // mov eax, RegeditMain
    op(&[0xFF, 0xD0], None); // call eax
    op(&[0x85, 0xC0], None); // test eax, eax
    op(&[0x74, 26], None); // jz done
    op(&[0xFF, 0x35], Some(data + D_TYPE)); // push dword [type]
    op(&[0x68], Some(data + D_CAPTION)); // push caption
    op(&[0x68], Some(data + D_TEXT)); // push text
    op(&[0x6A, 0], None); // push 0: no owner
    op(&[0xFF, 0x15], Some(message_box)); // call [MessageBoxA]
    op(&[0xEB, (-42i8) as u8], None); // jmp loop
    // done:
    op(&[0x6A, 0], None); // push 0
    op(&[0xFF, 0x15], Some(exit)); // call [ExitProcess]
    let text = b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.entry(text);
    b.build()
}

/// What the command line asks for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Command {
    /// `/s`: no message boxes.
    silent: bool,
    /// `/e`: export instead of import.
    export: bool,
    /// `/c` or `/d`, which aren't done here.
    unsupported: bool,
    /// The words that aren't switches: the files to import, or the file
    /// to export to and the key to export.
    names: Vec<String>,
}

/// The words of the command line `line`, a word in quotes taken whole
/// without them.
pub(super) fn words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(&first) = chars.peek() else {
            break;
        };
        let mut word = String::new();
        if first == '"' {
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                word.push(c);
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                word.push(c);
            }
        }
        words.push(word);
    }
    words
}

/// The command the command line `line` gives, the program's name first.
fn parse(line: &str) -> Command {
    let mut command = Command::default();
    for word in words(line).into_iter().skip(1) {
        let Some(switch) = word.strip_prefix(['/', '-']) else {
            command.names.push(word);
            continue;
        };
        let letter = switch.chars().next().map(|c| c.to_ascii_lowercase());
        match letter {
            Some('s') => command.silent = true,
            Some('e') => command.export = true,
            Some('c' | 'd') => command.unsupported = true,
            // /l:system and /r:user name the registry's files.
            _ => {}
        }
    }
    command
}

/// RegeditMain(data): REGEDIT's work, as its command line asks. TRUE
/// when the message box whose type, caption and text it leaves in `data`
/// is to be shown, and RegeditMain called again after it; FALSE when
/// there is none and the program ends.
///
/// The files on the command line are imported in turn, a failed one
/// too, and without `/s` each has its own message box, shown before the
/// next is imported, as Windows' REGEDIT does: a call imports one file
/// and keeps the place of the next in `data`. With `/s` the first call
/// imports them all.
pub fn main(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let data = args[0];
    let next = cpu.bus.guest_read_32(data + D_NEXT) as usize;
    let line = match crate::win32::process(cpu).command_line {
        0 => String::new(),
        at => {
            let bytes = cpu.bus.guest_read_asciiz(at, 0x1_0000);
            crate::dosstr::from_bytes(&bytes)
        }
    };
    let command = parse(&line);
    if next == 0 {
        cpu.bus
            .log_string(&format!("[W32] REGEDIT: {}", line.trim()));
    }
    if command.unsupported {
        return Some(0);
    }
    // These end the program, so they come only in the first call.
    if command.export {
        export(cpu, &command.names);
        return Some(0);
    }
    if command.silent {
        for name in &command.names {
            let _ = import(cpu, name);
        }
        return Some(0);
    }
    let Some(name) = command.names.get(next) else {
        return Some(0);
    };
    cpu.bus.guest_write_32(data + D_NEXT, next as u32 + 1);
    let (kind, text) = match import(cpu, name) {
        Ok(text) => (MB_ICONINFORMATION, text),
        Err(text) => (MB_ICONERROR, text),
    };
    let mut block = vec![0u8; D_NEXT as usize];
    block[..4].copy_from_slice(&kind.to_le_bytes());
    put_string(&mut block, D_CAPTION, CAPTION_SIZE, CAPTION);
    put_string(&mut block, D_TEXT, TEXT_SIZE, &text);
    cpu.bus.guest_write_bytes(data, &block);
    Some(1)
}

/// Put `text`, ended by a zero, at `offset` of `block`, cut to fit `size`.
fn put_string(block: &mut [u8], offset: u32, size: u32, text: &str) {
    let mut bytes = crate::dosstr::to_bytes(text);
    bytes.truncate(size as usize - 1);
    let at = offset as usize;
    block[at..at + bytes.len()].copy_from_slice(&bytes);
}

/// Import the file `name`: the message Windows 95's REGEDIT shows then,
/// as Ok when it was imported and as Err when it wasn't.
fn import(cpu: &mut Cpu, name: &str) -> Result<String, String> {
    let Ok((mut bytes, _, _)) = kernel32::read_path(cpu, name) else {
        cpu.bus
            .log_string(&format!("[W32] REGEDIT: {} can't be read", name));
        return Err(format!(
            "Cannot import {}: Error opening the file. There may be a disk or file system error.",
            name
        ));
    };
    // A file of Windows 3.1's often ends with Ctrl+Z.
    if let Some(end) = bytes.iter().position(|&b| b == 0x1A) {
        bytes.truncate(end);
    }
    let text = crate::dosstr::from_bytes(&bytes);
    if !registry::import_script(cpu, &text) {
        cpu.bus
            .log_string(&format!("[W32] REGEDIT: {} is no registration file", name));
        return Err(format!(
            "Cannot import {}: The specified file is not a registry script. \
             You can only import registry files.",
            name
        ));
    }
    cpu.bus
        .log_string(&format!("[W32] REGEDIT: {} imported", name));
    Ok(format!(
        "Information in {} has been successfully entered into the registry.",
        name
    ))
}

/// `/e file [key]`: write the registry, or the key's branch, to the
/// file. Nothing is written for a key that isn't there.
fn export(cpu: &mut Cpu, names: &[String]) {
    let Some(file) = names.first() else {
        return;
    };
    let Some(text) = registry::export_script(cpu, names.get(1).map(String::as_str)) else {
        cpu.bus
            .log_string(&format!("[W32] REGEDIT: no key {}", names[1]));
        return;
    };
    let bytes = crate::dosstr::to_bytes(&text);
    if kernel32::files::write_whole(cpu, file, &bytes).is_err() {
        cpu.bus
            .log_string(&format!("[W32] REGEDIT: {} can't be written", file));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_gives_the_switches_and_the_files() {
        let command = parse("regedit.exe /s C:\\WINDOWS\\mplayer.reg");
        assert!(command.silent && !command.export);
        assert_eq!(command.names, ["C:\\WINDOWS\\mplayer.reg"]);
        let command = parse("\"C:\\WINDOWS\\REGEDIT.EXE\" /L:C:\\W /E \"C:\\A B.REG\" HKEY_USERS");
        assert!(command.export && !command.silent);
        assert_eq!(command.names, ["C:\\A B.REG", "HKEY_USERS"]);
        assert!(parse("regedit -c x.reg").unsupported);
        assert!(parse("regedit").names.is_empty());
    }
}
