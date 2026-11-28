//! WinHelp as a program: C:\WINDOWS\WINHELP.EXE and WINHLP32.EXE, which
//! a shortcut or a command line runs to open a help file. The Magic
//! School Bus Explores the Solar System's setup adds a "Bus Help" item
//! that runs `WINHELP.EXE C:\MSKIDS\MSBSOLAR\README.HLP`, and its
//! README.TXT tells players to run `winhelp` with the file.
//!
//! Windows 98 has both in the Windows directory. Here each is a PE image
//! the emulator builds, shown there with NOTEPAD.EXE (dll_files.rs). Its
//! code is Notepad's (`notepad::loop_program`) around WinHelpMain, a
//! function of USER32 that USER32 doesn't export: it opens the file the
//! command line names in the viewer WinHelpA opens, at its contents
//! topic, and the program ends when the viewer is closed. With the
//! viewer already open, the file goes into it, as Windows' WinHelp hands
//! it to the running one, and the program ends at once; it ends at once
//! too with no file, or one the viewer can't read. The command line's
//! options (`-n`, `-k` and the others) aren't read: a line that has one
//! names no file the viewer can open.

use super::super::{message, notepad, string_at, user};
use super::{HELP_CONTENTS, win_help};
use crate::cpu::Cpu;
use crate::win32::kernel32;
use crate::win32::loader::WINDOWS_DIR;

/// The names of the program's files in the Windows directory.
pub const FILES: [&str; 2] = ["WINHELP.EXE", "WINHLP32.EXE"];

/// The file of WINHELP.EXE and WINHLP32.EXE.
pub fn file(bus: &crate::bus::Bus) -> Vec<u8> {
    notepad::loop_program(bus, "WinHelpMain")
}

/// WinHelpMain(): the viewer, open on the file the command line names.
/// Nonzero while the program waits for the viewer to be closed; 0 if it
/// ends now.
pub fn main(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let line = match crate::win32::process(cpu).command_line {
        0 => String::new(),
        at => string_at(cpu, at)?,
    };
    let Some(name) = notepad::file_named(&line).map(str::to_string) else {
        return Some(0);
    };
    let mut bytes = crate::dosstr::to_bytes(&name);
    bytes.push(0);
    let Some(at) = kernel32::alloc_pages(cpu, bytes.len() as u32) else {
        return Some(0);
    };
    cpu.bus.guest_write_bytes(at, &bytes);
    let open = user(cpu).help.viewer.as_ref().is_some_and(|v| v.frame != 0);
    if open {
        // The open viewer shows the file; this program has nothing to
        // wait for, and its loop ends at the WM_QUIT.
        message::post_quit_message(cpu, &[0]);
    }
    // WinHelpA reads the name before it returns or starts its task.
    let result = win_help(cpu, &[0, at, HELP_CONTENTS, 0]);
    kernel32::free_pages(cpu, at);
    result
}

/// The viewer's window is being destroyed, on the thread that made it,
/// as DestroyWindow refuses any other. If that is this program's, the
/// program ends. It goes by the process's path, which save states keep,
/// and not by what the viewer keeps, which they don't: a viewer open in
/// a state that is loaded ends the program as well when it is closed.
pub fn closed(cpu: &mut Cpu) {
    let path = crate::win32::running(cpu).map_or("", |p| p.path.as_str());
    let ours = path.rsplit_once('\\').is_some_and(|(dir, file)| {
        dir.eq_ignore_ascii_case(WINDOWS_DIR) && FILES.iter().any(|f| f.eq_ignore_ascii_case(file))
    });
    if ours {
        message::post_quit_message(cpu, &[0]);
    }
}
