//! DlgDirListA, DlgDirListComboBoxA, DlgDirSelectExA and
//! DlgDirSelectComboBoxExA, after Wine's dialog.c (DIALOG_DlgDirListW,
//! DIALOG_DlgDirSelect): a dialog's list box or combo box filled with
//! LB_DIR or CB_DIR, and the entry picked in it read back as a path.
//! They send their messages to the controls, which the program may have
//! subclassed, so each is a task. Windows 98 has no Unicode versions:
//! its DlgDirListW and the others fail with ERROR_CALL_NOT_IMPLEMENTED.

use super::controls::combobox::{
    CB_DIR, CB_GETCURSEL, CB_GETLBTEXT, CB_GETLBTEXTLEN, CB_RESETCONTENT,
};
use super::controls::listbox::{
    DDL_DIRECTORY, DDL_DRIVES, DDL_EXCLUSIVE, DDL_POSTMSGS, LB_DIR, LB_GETCURSEL, LB_GETTEXT,
    LB_GETTEXTLEN, LB_RESETCONTENT,
};
use super::message::WM_SETTEXT;
use super::text::ansi_case;
use super::{FALSE, TRUE, address_of, bytes_at, copy_out, set_error};
use crate::cpu::Cpu;
use crate::win32::kernel32::{self, files};
use crate::win32::task::{self, Next, Task};

const ERROR_NO_WILDCARD_CHARACTERS: u32 = 1417;

/// The task's functions, by its first variable.
const K_LIST: u32 = 0;
const K_SELECT: u32 = 1;

/// DlgDirList's scratch space: the file mask, "*.*" and the folder for
/// the static control, each at most MAX_PATH bytes with its 0.
const MASK_AT: u32 = 0;
const ANY_AT: u32 = 0x110;
const PATH_AT: u32 = 0x120;
const PATH_SIZE: u32 = 0x104;
const LIST_SCRATCH: u32 = PATH_AT + PATH_SIZE;

/// DlgDirSelectEx's scratch space: the entry's text.
const SELECT_SCRATCH: u32 = 0x1000;

/// The task's variables for DlgDirSelectEx.
const S_LIST: usize = 1;
const S_COMBO: usize = 2;
const S_BUFFER: usize = 3;
const S_SIZE: usize = 4;
const S_ITEM: usize = 5;

/// The control `id` of the dialog `dlg`, or 0.
fn item(cpu: &mut Cpu, dlg: u32, id: u32) -> u32 {
    if id == 0 {
        return 0;
    }
    super::dialog::get_dlg_item(cpu, &[dlg, id]).unwrap_or(0)
}

/// Where a file spec's folder ends and its mask starts (Wine's
/// DIALOG_DlgDirListW): the folder is what comes before the last '\' or
/// '/', or the drive before a ':' when there is neither. A spec of a
/// drive's root ("C:\*.*", "\*.*") keeps the separator in its folder.
fn split_spec(spec: &[u8]) -> (&[u8], &[u8]) {
    let after_drive = spec.iter().position(|&b| b == b':').map_or(0, |i| i + 1);
    match spec[after_drive..]
        .iter()
        .rposition(|&b| b == b'\\' || b == b'/')
    {
        Some(i) => {
            let sep = after_drive + i;
            let end = if sep == after_drive { sep + 1 } else { sep };
            (&spec[..end], &spec[sep + 1..])
        }
        None => spec.split_at(after_drive),
    }
}

/// DlgDirListA(dlg, spec, idListBox, idStaticPath, attrib).
pub fn dlg_dir_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    list(cpu, args, false)
}

/// DlgDirListComboBoxA(dlg, spec, idComboBox, idStaticPath, attrib).
pub fn dlg_dir_list_combo_box(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    list(cpu, args, true)
}

/// DlgDirList for a list box or a combo box. A spec that names a folder
/// makes it the current one and lists all of it ("*.*"); one with a
/// folder and a mask makes the folder current and lists what the mask
/// matches. The mask, in capitals, goes back into `spec`. The list gets
/// LB_RESETCONTENT and LB_DIR, and DDL_DRIVES alone means only the
/// drives. With DDL_DIRECTORY the folders are listed with "*.*" whatever
/// the mask, in an LB_DIR of their own. The static control shows the
/// current folder in small letters. TRUE, or FALSE with the error set
/// when the folder isn't there or a spec that isn't one has no wildcards.
fn list(cpu: &mut Cpu, args: &[u32], combo: bool) -> Option<u32> {
    let &[dlg, spec_at, id_list, id_static, attrib] = args else {
        unreachable!()
    };
    let spec = if spec_at != 0 {
        Some(bytes_at(cpu, spec_at)?)
    } else {
        None
    };
    // A spec that names a folder becomes the current one here.
    let enter = |cpu: &mut Cpu, s: &[u8]| {
        kernel32::change_directory(cpu, &crate::dosstr::from_bytes(s)).is_ok()
    };
    let mask: Vec<u8> = match &spec {
        Some(s) if !s.is_empty() && !enter(cpu, s) => {
            if !s.contains(&b'*') && !s.contains(&b'?') {
                set_error(cpu, ERROR_NO_WILDCARD_CHARACTERS);
                return Some(FALSE);
            }
            let (folder, mask) = split_spec(s);
            if !folder.is_empty() {
                let folder = crate::dosstr::from_bytes(folder);
                if let Err(code) = kernel32::change_directory(cpu, &folder) {
                    set_error(cpu, code);
                    return Some(FALSE);
                }
            }
            mask.to_vec()
        }
        _ => b"*.*".to_vec(),
    };
    let acp = crate::win32::nls::ansi(cpu);
    let upper: Vec<u8> = mask.iter().map(|&b| ansi_case(acp, b, true)).collect();
    if spec.is_some() {
        let mut bytes = upper.clone();
        bytes.push(0);
        cpu.bus.guest_write_bytes(spec_at, &bytes);
    }

    // Each call is [function, hwnd, msg, wParam, lParam, whether lParam
    // is an offset in the scratch space].
    let mut calls: Vec<[u32; 6]> = Vec::new();
    let list = item(cpu, dlg, id_list);
    if list != 0 {
        let (reset, dir) = if combo {
            (CB_RESETCONTENT, CB_DIR)
        } else {
            (LB_RESETCONTENT, LB_DIR)
        };
        let attrib = if attrib == DDL_DRIVES {
            attrib | DDL_EXCLUSIVE
        } else {
            attrib
        };
        // Posted messages are read after the function returns, so their
        // strings can't be on its stack: the mask is the program's own
        // spec, and "*.*" a block of its own, which stays.
        let post = attrib & DDL_POSTMSGS != 0;
        let (mask_at, any_at, in_scratch) = if post {
            // The block only where an LB_DIR reads "*.*".
            let any = if attrib & DDL_DIRECTORY != 0 || spec_at == 0 {
                posted_any(cpu)
            } else {
                0
            };
            (if spec_at != 0 { spec_at } else { any }, any, 0)
        } else {
            (MASK_AT, ANY_AT, 1)
        };
        let target = address_of(if post { "PostMessageA" } else { "SendMessageA" });
        calls.push([target, list, reset, 0, 0, 0]);
        if attrib & DDL_DIRECTORY != 0 {
            if attrib & DDL_EXCLUSIVE == 0 {
                let files = attrib & !(DDL_DIRECTORY | DDL_DRIVES);
                calls.push([target, list, dir, files, mask_at, in_scratch]);
            }
            let folders = attrib & (DDL_DIRECTORY | DDL_DRIVES) | DDL_EXCLUSIVE;
            calls.push([target, list, dir, folders, any_at, in_scratch]);
        } else {
            calls.push([target, list, dir, attrib, mask_at, in_scratch]);
        }
    }
    // The current folder in small letters, as CharLowerA makes them.
    let path = item(cpu, dlg, id_static);
    let mut folder = Vec::new();
    if path != 0 {
        let at = address_of("SendMessageA");
        calls.push([at, path, WM_SETTEXT, 0, PATH_AT, 1]);
        folder = crate::dosstr::to_bytes(&files::current_directory(cpu))
            .iter()
            .map(|&b| ansi_case(acp, b, false))
            .collect();
    }

    // The scratch space as it starts, which the first step writes.
    let mut image = vec![0u8; LIST_SCRATCH as usize];
    let mut place = |at: u32, size: u32, text: &[u8]| {
        let n = text.len().min(size as usize - 1);
        image[at as usize..at as usize + n].copy_from_slice(&text[..n]);
    };
    place(MASK_AT, ANY_AT - MASK_AT, &mask);
    place(ANY_AT, PATH_AT - ANY_AT, b"*.*");
    place(PATH_AT, PATH_SIZE, &folder);
    let mut vars = vec![K_LIST, calls.len() as u32];
    vars.extend(calls.iter().flatten());
    vars.extend(
        image
            .chunks(4)
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])),
    );
    task::start(cpu, task::DLG_DIR, vars, LIST_SCRATCH)
}

/// A block holding "*.*" for DlgDirList's posted LB_DIR, which the list
/// reads after the function has returned. Each call that needs one
/// makes its own, which stays: the list may read it at any time, and
/// USER32 has no place in the program's memory for one they share.
fn posted_any(cpu: &mut Cpu) -> u32 {
    let handle = kernel32::global_alloc(cpu, 0, 4);
    let at = kernel32::global_address(cpu, handle);
    if at != 0 {
        cpu.bus.guest_write_bytes(at, b"*.*\0");
    }
    at
}

/// DlgDirSelectExA(dlg, buffer, size, idListBox).
pub fn dlg_dir_select_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    select(cpu, args, false)
}

/// DlgDirSelectComboBoxExA(dlg, buffer, size, idComboBox).
pub fn dlg_dir_select_combo_box_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    select(cpu, args, true)
}

/// DlgDirSelectEx for a list box or a combo box: the selected entry as a
/// path in `buffer`, cut to `size` bytes. A drive "[-c-]" comes back as
/// "c:", a folder "[name]" as "name\", and a file as its name, with a
/// '.' after it when it has no extension. TRUE for a drive or a folder,
/// FALSE for a file or when nothing is selected, without an error.
fn select(cpu: &mut Cpu, args: &[u32], combo: bool) -> Option<u32> {
    let &[dlg, buffer, size, id] = args else {
        unreachable!()
    };
    let list = item(cpu, dlg, id);
    if list == 0 {
        return Some(FALSE);
    }
    let vars = vec![K_SELECT, list, combo as u32, buffer, size, 0];
    task::start(cpu, task::DLG_DIR, vars, SELECT_SCRATCH)
}

/// The path DlgDirSelectEx makes of the entry `text`, and whether it is
/// a drive or a folder.
fn selected_path(text: &[u8]) -> (Vec<u8>, bool) {
    match text {
        [b'[', b'-', drive, ..] => (vec![*drive, b':'], true),
        [b'[', name @ ..] => {
            let name = name.strip_suffix(b"]").unwrap_or(name);
            ([name, b"\\"].concat(), true)
        }
        _ if !text.contains(&b'.') => ([text, b"."].concat(), false),
        _ => (text.to_vec(), false),
    }
}

/// The task's steps.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.var(0) {
        K_LIST => list_step(cpu, t),
        K_SELECT => select_step(cpu, t, value),
        _ => Next::Done(FALSE),
    }
}

/// DlgDirList's calls, one after another, then TRUE. The scratch
/// space's words follow the calls, and go there first.
fn list_step(cpu: &mut Cpu, t: &mut Task) -> Next {
    let count = t.var(1) as usize;
    let end = 2 + count * 6;
    if t.step == 0 {
        let bytes: Vec<u8> = t.vars[end..].iter().flat_map(|w| w.to_le_bytes()).collect();
        cpu.bus.guest_write_bytes(t.scratch, &bytes);
        t.vars.truncate(end);
    }
    if t.step as usize >= count {
        return Next::Done(TRUE);
    }
    let c = &t.vars[2 + t.step as usize * 6..][..6];
    let lp = if c[5] != 0 { t.scratch + c[4] } else { c[4] };
    Next::call(c[0], &[c[1], c[2], c[3], lp], t.step + 1)
}

fn select_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let combo = t.var(S_COMBO) != 0;
    let list = t.var(S_LIST);
    let send = |msg: u32, wp: u32, lp: u32, step: u16| {
        Next::call(address_of("SendMessageA"), &[list, msg, wp, lp], step)
    };
    match t.step {
        0 => send(if combo { CB_GETCURSEL } else { LB_GETCURSEL }, 0, 0, 1),
        1 => {
            if value as i32 == -1 {
                return Next::Done(FALSE);
            }
            t.set(S_ITEM, value);
            let msg = if combo {
                CB_GETLBTEXTLEN
            } else {
                LB_GETTEXTLEN
            };
            send(msg, value, 0, 2)
        }
        2 => {
            if value as i32 == -1 || value >= SELECT_SCRATCH {
                return Next::Done(FALSE);
            }
            let msg = if combo { CB_GETLBTEXT } else { LB_GETTEXT };
            send(msg, t.var(S_ITEM), t.scratch, 3)
        }
        _ => {
            let text = bytes_at(cpu, t.scratch).unwrap_or_default();
            let (path, special) = selected_path(&text);
            copy_out(cpu, t.var(S_BUFFER), t.var(S_SIZE), &path);
            Next::Done(special as u32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_splits_into_folder_and_mask() {
        let split = |s: &'static [u8]| split_spec(s);
        assert_eq!(split(b"*.txt"), (&b""[..], &b"*.txt"[..]));
        assert_eq!(split(b"c:*.txt"), (&b"c:"[..], &b"*.txt"[..]));
        assert_eq!(split(b"c:\\*.txt"), (&b"c:\\"[..], &b"*.txt"[..]));
        assert_eq!(split(b"\\*.txt"), (&b"\\"[..], &b"*.txt"[..]));
        assert_eq!(
            split(b"c:\\games\\x\\*.*"),
            (&b"c:\\games\\x"[..], &b"*.*"[..])
        );
        assert_eq!(split(b"games/w*.c"), (&b"games"[..], &b"w*.c"[..]));
    }

    #[test]
    fn a_selected_entry_becomes_a_path() {
        assert_eq!(selected_path(b"[-d-]"), (b"d:".to_vec(), true));
        assert_eq!(selected_path(b"[..]"), (b"..\\".to_vec(), true));
        assert_eq!(selected_path(b"[games]"), (b"games\\".to_vec(), true));
        assert_eq!(
            selected_path(b"readme.txt"),
            (b"readme.txt".to_vec(), false)
        );
        assert_eq!(selected_path(b"makefile"), (b"makefile.".to_vec(), false));
    }
}
