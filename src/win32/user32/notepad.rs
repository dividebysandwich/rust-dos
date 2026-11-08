//! Notepad: C:\WINDOWS\NOTEPAD.EXE, which setups and games run to show a
//! readme ("C:\WINDOWS\notepad.exe C:\WarGames\readme.txt" is what
//! WarGames' InstallShield setup gives WinExec for "View readme file
//! now").
//!
//! Windows 98 has Notepad in the Windows directory. Here the file is a
//! PE image the emulator builds (`file`), shown in C:\WINDOWS with the
//! built-in DLLs (dll_files.rs), so a program that looks for it finds it
//! and one that starts it gets a process of its own, which goes on after
//! the program that started it ends, as on Windows. Its code is the
//! message loop of a Windows program; what Notepad itself does is here,
//! in two functions of USER32 that USER32 doesn't export:
//!
//! - NotepadMain reads the file the command line names, registers the
//!   window class "Notepad", makes the window, titled "readme.txt -
//!   Notepad", with a multi-line edit control in it that has the file's
//!   text in the system's fixed font, and shows it.
//! - NotepadWndProc, the class's window procedure, keeps the edit control
//!   as big as the window and the focus in it, and ends the message loop
//!   when the window is destroyed.
//!
//! The text can be read, scrolled, selected, copied and changed, but not
//! saved: Notepad's menus (File, Edit, Search, Help) aren't there.

use super::{address_of, defwnd, string_at, user};
use crate::cpu::Cpu;
use crate::win32::gdi32;
use crate::win32::kernel32;
use crate::win32::task::{self, Next, Task};
use rust_dos_pe::{self as pe, Builder, Imp};

/// Where the program is loaded: where Windows 98's NOTEPAD.EXE is.
const BASE: u32 = 0x40_0000;

/// The window class, and the edit control's id in the window, as
/// Windows 98's Notepad has them.
const CLASS: &str = "Notepad";
const ID_EDIT: u32 = 15;

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_HSCROLL: u32 = 0x0010_0000;
const WS_EX_CLIENTEDGE: u32 = 0x0000_0200;
const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_NOHIDESEL: u32 = 0x0100;
const CW_USEDEFAULT: u32 = 0x8000_0000;
const CS_HREDRAW: u32 = 0x0002;
const CS_VREDRAW: u32 = 0x0001;
const COLOR_WINDOW: u32 = 5;
const SW_SHOWNORMAL: u32 = 1;
const STARTF_USESHOWWINDOW: u32 = 1;

const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_SETFOCUS: u32 = 0x0007;
const WM_SETFONT: u32 = 0x0030;

/// The task's kinds, its first variable.
const K_OPEN: u32 = 0;
const K_SIZE: u32 = 1;
const K_FOCUS: u32 = 2;

/// The open task's variables after its kind: the show command, the
/// address of the text, the window and the edit control.
const V_SHOW: usize = 1;
const V_TEXT: usize = 2;
const V_FRAME: usize = 3;
const V_EDIT: usize = 4;

/// The open task's scratch space: the WNDCLASSA, the class names and the
/// title.
const SCRATCH: u32 = 256;
const S_CLASS: u32 = 48;
const S_EDIT_CLASS: u32 = 64;
const S_TITLE: u32 = 80;
const TITLE_MAX: usize = (SCRATCH - S_TITLE) as usize;

/// The file of NOTEPAD.EXE: a program for the GUI subsystem whose code
/// is NotepadMain and then the message loop, which ends the process
/// with the exit code of WM_QUIT.
pub fn file() -> Vec<u8> {
    let mut b = Builder::exe(BASE);
    b.subsystem(pe::SUBSYSTEM_GUI);
    let kernel32 = [Imp::Name("ExitProcess")];
    let user32 = [
        Imp::Name("GetMessageA"),
        Imp::Name("TranslateMessage"),
        Imp::Name("DispatchMessageA"),
    ];
    let iat = b.imports(&[("KERNEL32.DLL", &kernel32), ("USER32.DLL", &user32)]);
    let (exit, get, translate, dispatch) = (iat[0][0], iat[1][0], iat[1][1], iat[1][2]);
    // The MSG the loop reads into.
    let data = pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE;
    let msg = BASE + b.section_sized(".data", &[], 28, data);
    let main = address_of("NotepadMain");
    let mut code = Vec::new();
    let mut op = |bytes: &[u8], value: Option<u32>| {
        code.extend_from_slice(bytes);
        if let Some(v) = value {
            code.extend_from_slice(&v.to_le_bytes());
        }
    };
    op(&[0xB8], Some(main)); // mov eax, NotepadMain
    op(&[0xFF, 0xD0], None); // call eax
    op(&[0x85, 0xC0], None); // test eax, eax
    op(&[0x74, 45], None); // jz done
    // loop:
    op(&[0x6A, 0, 0x6A, 0, 0x6A, 0], None); // push 0, 0, 0
    op(&[0x68], Some(msg)); // push msg
    op(&[0xFF, 0x15], Some(get)); // call [GetMessageA]
    op(&[0x85, 0xC0], None); // test eax, eax
    op(&[0x7E, 24], None); // jle done
    op(&[0x68], Some(msg)); // push msg
    op(&[0xFF, 0x15], Some(translate)); // call [TranslateMessage]
    op(&[0x68], Some(msg)); // push msg
    op(&[0xFF, 0x15], Some(dispatch)); // call [DispatchMessageA]
    op(&[0xEB, (-45i8) as u8], None); // jmp loop
    // done:
    op(&[0xFF, 0x35], Some(msg + 8)); // push dword [msg.wParam]
    op(&[0xFF, 0x15], Some(exit)); // call [ExitProcess]
    let text = b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.entry(text);
    b.build()
}

/// The file the command line `line` names after the program's name, its
/// quotes taken off, or None if it names none.
fn file_named(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let rest = match line.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"').map_or("", |(_, rest)| rest),
        None => line.find([' ', '\t']).map_or("", |at| &line[at..]),
    };
    let name = rest.trim();
    let name = name
        .strip_prefix('"')
        .map_or(name, |n| n.strip_suffix('"').unwrap_or(n));
    (!name.is_empty()).then_some(name)
}

/// The window's title for the file `name`: its name without the
/// directories, as Notepad shows it, or "Untitled".
fn title(name: Option<&str>) -> String {
    let file = name.map_or("Untitled", |n| {
        n.rsplit(['\\', '/', ':']).next().unwrap_or(n)
    });
    format!("{} - Notepad", file)
}

/// The text of the file `name`, up to a zero byte as the edit control
/// takes it; a name without an extension that isn't there is tried with
/// ".txt", as Notepad does. Empty for a file that can't be read.
fn text_of(cpu: &mut Cpu, name: &str) -> Vec<u8> {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let mut read = kernel32::read_path(cpu, name);
    if read.is_err() && !file.contains('.') {
        read = kernel32::read_path(cpu, &format!("{}.txt", name));
    }
    let mut bytes = read.map_or_else(|_| Vec::new(), |(bytes, _, _)| bytes);
    if let Some(end) = bytes.iter().position(|&b| b == 0) {
        bytes.truncate(end);
    }
    bytes
}

/// How the window is shown first: as the STARTUPINFO its parent gave
/// says, or normally, as the C runtime gives WinMain its nCmdShow.
fn show_command(cpu: &mut Cpu) -> u32 {
    let startup = &crate::win32::process(cpu).startup;
    let field = |at: usize, len: usize| {
        let mut b = [0u8; 4];
        if let Some(s) = startup.get(at..at + len) {
            b[..len].copy_from_slice(s);
        }
        u32::from_le_bytes(b)
    };
    if field(44, 4) & STARTF_USESHOWWINDOW != 0 {
        field(48, 2)
    } else {
        SW_SHOWNORMAL
    }
}

fn write_str(cpu: &mut Cpu, at: u32, s: &str, max: usize) {
    let mut b: Vec<u8> = crate::dosstr::to_bytes(s);
    b.truncate(max - 1);
    b.push(0);
    cpu.bus.guest_write_bytes(at, &b);
}

/// NotepadMain(): the window, with the text of the file the command line
/// names. TRUE once it is shown; FALSE if it couldn't be made.
pub fn main(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let line = match crate::win32::process(cpu).command_line {
        0 => String::new(),
        at => string_at(cpu, at)?,
    };
    let name = file_named(&line).map(str::to_string);
    let text = match &name {
        Some(name) => text_of(cpu, name),
        None => Vec::new(),
    };
    let Some(at) = kernel32::alloc_pages(cpu, text.len() as u32 + 1) else {
        return Some(0);
    };
    cpu.bus.guest_write_bytes(at, &text);
    cpu.bus.guest_write_8(at + text.len() as u32, 0);
    let show = show_command(cpu);
    let title = title(name.as_deref());
    // The title rides in the variables until the task has its scratch
    // space.
    let mut vars = vec![K_OPEN, show, at, 0, 0];
    let mut bytes = crate::dosstr::to_bytes(&title);
    bytes.truncate(TITLE_MAX - 1);
    bytes.push(0);
    vars.extend(bytes.chunks(4).map(|c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        u32::from_le_bytes(w)
    }));
    task::start(cpu, task::NOTEPAD, vars, SCRATCH)
}

/// The edit control of Notepad's window `hwnd`, which its first four
/// extra bytes keep, or 0.
fn edit_of(cpu: &mut Cpu, hwnd: u32) -> u32 {
    user(cpu)
        .window(hwnd)
        .and_then(|w| w.extra.get(..4))
        .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// NotepadWndProc(hwnd, message, wParam, lParam): Notepad's window.
pub fn wnd_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, _, lp] = args else {
        return Some(0);
    };
    let edit = edit_of(cpu, hwnd);
    match msg {
        WM_SIZE if edit != 0 => {
            let vars = vec![K_SIZE, edit, lp & 0xFFFF, lp >> 16];
            task::start(cpu, task::NOTEPAD, vars, 0)
        }
        WM_SETFOCUS if edit != 0 => task::start(cpu, task::NOTEPAD, vec![K_FOCUS, edit], 0),
        WM_DESTROY => super::message::post_quit_message(cpu, &[0]),
        _ => defwnd::def_window_proc(cpu, args),
    }
}

/// Notepad's task.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.var(0) {
        K_OPEN => open_step(cpu, t, value),
        K_SIZE => match t.step {
            0 => {
                let args = [t.var(1), 0, 0, t.var(2), t.var(3), 1];
                Next::call(address_of("MoveWindow"), &args, 1)
            }
            _ => Next::Done(0),
        },
        K_FOCUS => match t.step {
            0 => Next::call(address_of("SetFocus"), &[t.var(1)], 1),
            _ => Next::Done(0),
        },
        _ => Next::Done(0),
    }
}

/// Open Notepad's window: register the class, make the window and the
/// edit control, give it the text and the font, show the window.
fn open_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let instance = BASE;
    let create = address_of("CreateWindowExA");
    let s = t.scratch;
    match t.step {
        0 => {
            let title: Vec<u8> = t.vars[V_EDIT + 1..]
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect();
            cpu.bus.guest_write_bytes(s + S_TITLE, &title);
            write_str(cpu, s + S_CLASS, CLASS, 16);
            write_str(cpu, s + S_EDIT_CLASS, "Edit", 16);
            let class = [
                CS_HREDRAW | CS_VREDRAW,
                address_of("NotepadWndProc"),
                0,
                4,
                instance,
                0,
                0,
                COLOR_WINDOW + 1,
                0,
                s + S_CLASS,
            ];
            for (i, w) in class.iter().enumerate() {
                cpu.bus.guest_write_32(s + 4 * i as u32, *w);
            }
            Next::call(address_of("RegisterClassA"), &[s], 1)
        }
        1 => {
            let d = CW_USEDEFAULT;
            let args = [
                0,
                s + S_CLASS,
                s + S_TITLE,
                WS_OVERLAPPEDWINDOW,
                d,
                0,
                d,
                0,
                0,
                0,
                instance,
                0,
            ];
            Next::call(create, &args, 2)
        }
        2 => {
            if value == 0 {
                return done(cpu, t, 0);
            }
            t.set(V_FRAME, value);
            let (w, h) = user(cpu)
                .window(value)
                .map_or((0, 0), |w| (w.client.width(), w.client.height()));
            let style = WS_CHILD
                | WS_VISIBLE
                | WS_VSCROLL
                | WS_HSCROLL
                | ES_MULTILINE
                | ES_AUTOVSCROLL
                | ES_AUTOHSCROLL
                | ES_NOHIDESEL;
            let args = [
                WS_EX_CLIENTEDGE,
                s + S_EDIT_CLASS,
                0,
                style,
                0,
                0,
                w.max(0) as u32,
                h.max(0) as u32,
                value,
                ID_EDIT,
                instance,
                0,
            ];
            Next::call(create, &args, 3)
        }
        3 => {
            if value == 0 {
                return done(cpu, t, 0);
            }
            t.set(V_EDIT, value);
            if let Some(w) = user(cpu).window_mut(t.var(V_FRAME))
                && let Some(extra) = w.extra.get_mut(..4)
            {
                extra.copy_from_slice(&value.to_le_bytes());
            }
            let font = gdi32::get_stock_object(cpu, &[gdi32::SYSTEM_FIXED_FONT]).unwrap_or(0);
            let args = [value, WM_SETFONT, font, 0];
            Next::call(address_of("SendMessageA"), &args, 4)
        }
        4 => {
            let args = [t.var(V_EDIT), t.var(V_TEXT)];
            Next::call(address_of("SetWindowTextA"), &args, 5)
        }
        5 => {
            let args = [t.var(V_FRAME), t.var(V_SHOW)];
            Next::call(address_of("ShowWindow"), &args, 6)
        }
        6 => Next::call(address_of("SetFocus"), &[t.var(V_EDIT)], 7),
        _ => done(cpu, t, 1),
    }
}

/// The open task ends with `result`, the text given back.
fn done(cpu: &mut Cpu, t: &mut Task, result: u32) -> Next {
    let text = std::mem::take(&mut t.vars[V_TEXT]);
    if text != 0 {
        kernel32::free_pages(cpu, text);
    }
    Next::Done(result)
}

#[cfg(test)]
mod tests {
    use super::{file_named, title};

    #[test]
    fn the_file_is_what_follows_the_program() {
        let line = r"C:\WINDOWS\notepad.exe C:\WarGames\readme.txt";
        assert_eq!(file_named(line), Some(r"C:\WarGames\readme.txt"));
        let quoted = r#""C:\WINDOWS\NOTEPAD.EXE" "C:\Program Files\A\read me.txt" "#;
        assert_eq!(file_named(quoted), Some(r"C:\Program Files\A\read me.txt"));
        assert_eq!(file_named("notepad"), None);
        assert_eq!(file_named("notepad   "), None);
    }

    #[test]
    fn the_title_has_the_file_name_alone() {
        assert_eq!(
            title(Some(r"C:\WarGames\readme.txt")),
            "readme.txt - Notepad"
        );
        assert_eq!(title(None), "Untitled - Notepad");
    }
}
