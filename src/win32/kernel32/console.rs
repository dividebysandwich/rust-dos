//! The console: DOS's screen and keyboard, as the console functions that
//! C runtimes (MSVC's, Watcom's, Borland's) call see them. Its handles
//! are the standard handles and CON opened by name; the screen is the
//! text mode's, its size the BIOS's.

use super::file::{self, GENERIC_WRITE, Open};
use super::{ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error};
use crate::cpu::Cpu;
use crate::disk::CharDevice;

/// The input modes: Ctrl+C handled, a line at a time, echoed.
const DEFAULT_INPUT_MODE: u32 = 0x07;
const ENABLE_LINE_INPUT: u32 = 0x02;
/// The output modes: control characters handled, wrapping at the end of
/// a line.
const DEFAULT_OUTPUT_MODE: u32 = 0x03;

/// The code page of the console: DOS's.
const CODE_PAGE: u32 = 437;

/// INPUT_RECORD's size, and KEY_EVENT.
const INPUT_RECORD: u32 = 20;
const KEY_EVENT: u16 = 1;

/// The attribute of the screen's characters.
const ATTRIBUTE: u16 = 0x07;

/// The console's open file `handle` is a handle of, if it is the
/// console's.
fn console_of(cpu: &mut Cpu, handle: u32) -> Option<Open> {
    file::open_of(cpu, handle)
        .filter(|open| cpu.bus.disk.handle_device(open.sft) == Some(CharDevice::Con))
}

/// Whether a console handle is the input's: standard input, or CON
/// opened for reading alone.
fn is_input(open: &Open) -> bool {
    open.dos == 0 || (open.dos == 0xFFFF && open.access & GENERIC_WRITE == 0)
}

fn modes(cpu: &Cpu) -> (u32, u32) {
    cpu.bus
        .win32
        .console_mode
        .unwrap_or((DEFAULT_INPUT_MODE, DEFAULT_OUTPUT_MODE))
}

/// GetConsoleMode(handle, mode): fails for a handle that isn't the
/// console's, which C runtimes ask to tell the console from files.
pub(super) fn get_console_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(open) = console_of(cpu, args[0]) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    let (input, output) = modes(cpu);
    let mode = if is_input(&open) { input } else { output };
    cpu.bus.guest_write_32(args[1], mode);
    Some(TRUE)
}

/// SetConsoleMode(handle, mode).
pub(super) fn set_console_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(open) = console_of(cpu, args[0]) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    let (mut input, mut output) = modes(cpu);
    if is_input(&open) {
        input = args[1];
    } else {
        output = args[1];
    }
    cpu.bus.win32.console_mode = Some((input, output));
    Some(TRUE)
}

/// ReadConsoleA(handle, buffer, n, read, reserved): with line input a
/// line typed at the keyboard, as ReadFile reads it; without, the
/// characters typed so far, at least one, unechoed.
pub(super) fn read_console(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, buffer, n, read, _] = args else {
        unreachable!()
    };
    if console_of(cpu, handle).is_none_or(|open| !is_input(&open)) {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    if !cpu.bus.guest_probe(buffer, n as usize, true)
        || (read != 0 && !cpu.bus.guest_probe(read, 4, true))
    {
        return None;
    }
    let bytes = if modes(cpu).0 & ENABLE_LINE_INPUT != 0 {
        file::read_console_line(cpu, n)?
    } else {
        let mut bytes = Vec::new();
        while (bytes.len() as u32) < n {
            let Some(key) = take_key(cpu) else {
                break;
            };
            if key as u8 != 0 && key as u8 != 0xE0 {
                bytes.push(key as u8);
            }
        }
        if bytes.is_empty() && n > 0 {
            cpu.key_wait();
            file::wait_for_keys(cpu);
            return None;
        }
        bytes
    };
    cpu.bus.guest_write_bytes(buffer, &bytes);
    if read != 0 {
        cpu.bus.guest_write_32(read, bytes.len() as u32);
    }
    Some(TRUE)
}

/// Ctrl+C and Ctrl+Break don't stop programs here: there is nothing to
/// call the handlers for.
pub(super) fn set_console_ctrl_handler(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TRUE)
}

/// The screen's columns and rows, as the BIOS has them.
fn screen_size(cpu: &Cpu) -> (u16, u16) {
    let columns = cpu.bus.read_16(0x44A);
    let rows = cpu.bus.read_8(0x484) as u16 + 1;
    (
        if columns == 0 { 80 } else { columns },
        if rows <= 1 { 25 } else { rows },
    )
}

/// GetConsoleScreenBufferInfo(handle, info): the screen is the buffer
/// and its window, with the cursor where the BIOS has it.
pub(super) fn get_console_screen_buffer_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if console_of(cpu, args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    let (columns, rows) = screen_size(cpu);
    let (x, y) = (cpu.bus.read_8(0x450) as u16, cpu.bus.read_8(0x451) as u16);
    let fields = [
        columns,
        rows,
        x,
        y,
        ATTRIBUTE,
        0,
        0,
        columns - 1,
        rows - 1,
        columns,
        rows,
    ];
    let at = args[1];
    if !cpu.bus.guest_probe(at, 22, true) {
        return None;
    }
    for (i, value) in fields.into_iter().enumerate() {
        cpu.bus.guest_write_16(at + 2 * i as u32, value);
    }
    Some(TRUE)
}

/// SetConsoleCursorPosition(handle, position): COORD's X and Y in one
/// argument.
pub(super) fn set_console_cursor_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if console_of(cpu, args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    let (x, y) = (args[1] as u16, (args[1] >> 16) as u16);
    let (columns, rows) = screen_size(cpu);
    if x >= columns || y >= rows {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    cpu.bus.cursor_x = x as usize;
    cpu.bus.cursor_y = y as usize;
    cpu.bus.write_8(0x450, x as u8);
    cpu.bus.write_8(0x451, y as u8);
    Some(TRUE)
}

/// The cells from (x, y) on, at most `n` of them to the screen's end, as
/// offsets in its text memory.
fn cells(cpu: &Cpu, position: u32, n: u32) -> Vec<usize> {
    let (columns, rows) = screen_size(cpu);
    let (x, y) = ((position & 0xFFFF) as usize, (position >> 16) as usize);
    let start = y * columns as usize + x;
    let end = (columns as usize * rows as usize).min(start + n as usize);
    (start..end).map(|cell| cell * 2).collect()
}

/// Write `value` to the byte `which` (0 the character, 1 the attribute)
/// of the cells `cells`.
fn fill(cpu: &mut Cpu, cells: &[usize], which: usize, value: impl Fn(usize) -> u8) {
    let len = cpu.bus.text_mem_mut().len();
    for (i, &offset) in cells.iter().enumerate() {
        if offset + which < len {
            cpu.bus.text_mem_mut()[offset + which] = value(i);
            cpu.bus.mark_text_dirty(offset, offset + 2);
        }
    }
}

/// FillConsoleOutputCharacterA(handle, character, n, position, written).
pub(super) fn fill_console_output_character(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    fill_output(cpu, args, 0)
}

/// FillConsoleOutputAttribute(handle, attribute, n, position, written).
pub(super) fn fill_console_output_attribute(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    fill_output(cpu, args, 1)
}

fn fill_output(cpu: &mut Cpu, args: &[u32], which: usize) -> Option<u32> {
    let &[handle, value, n, position, written] = args else {
        unreachable!()
    };
    if console_of(cpu, handle).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    if written != 0 && !cpu.bus.guest_probe(written, 4, true) {
        return None;
    }
    let cells = cells(cpu, position, n);
    fill(cpu, &cells, which, |_| value as u8);
    if written != 0 {
        cpu.bus.guest_write_32(written, cells.len() as u32);
    }
    Some(TRUE)
}

/// WriteConsoleOutputA(handle, cells, size, from, region): a rectangle
/// of CHAR_INFO (character and attribute) from the buffer, `size` wide
/// and high, starting at `from` in it, to the screen's `region`
/// (SMALL_RECT), which comes back as far as it was written.
pub(super) fn write_console_output(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, buffer, size, from, region] = args else {
        unreachable!()
    };
    if console_of(cpu, handle).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    let rect: Vec<i16> = (0..4)
        .map(|i| cpu.bus.guest_read_16(region + 2 * i) as i16)
        .collect();
    if cpu.bus.guest_faulted() || !cpu.bus.guest_probe(region, 8, true) {
        return None;
    }
    let (width, height) = ((size & 0xFFFF) as i32, (size >> 16) as i32);
    let (from_x, from_y) = ((from & 0xFFFF) as i32, (from >> 16) as i32);
    let (columns, rows) = screen_size(cpu);
    let left = rect[0].max(0) as i32;
    let top = rect[1].max(0) as i32;
    let right = (rect[2] as i32)
        .min(columns as i32 - 1)
        .min(left + width - from_x - 1);
    let bottom = (rect[3] as i32)
        .min(rows as i32 - 1)
        .min(top + height - from_y - 1);
    let mut cells = Vec::new();
    for y in top..=bottom {
        for x in left..=right {
            let source = (from_y + y - top) * width + from_x + x - left;
            let info = cpu.bus.guest_read_32(buffer + 4 * source as u32);
            cells.push(((y * columns as i32 + x) as usize * 2, info));
        }
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    let offsets: Vec<usize> = cells.iter().map(|&(offset, _)| offset).collect();
    fill(cpu, &offsets, 0, |i| cells[i].1 as u8);
    fill(cpu, &offsets, 1, |i| (cells[i].1 >> 16) as u8);
    for (i, value) in [left, top, right, bottom].into_iter().enumerate() {
        cpu.bus.guest_write_16(region + 2 * i as u32, value as u16);
    }
    Some(TRUE)
}

/// SetConsoleTextAttribute(handle, attribute): what is written goes on
/// in DOS's light gray, which this doesn't change.
pub(super) fn set_console_text_attribute(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match console_of(cpu, args[0]) {
        Some(_) => Some(TRUE),
        None => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            Some(FALSE)
        }
    }
}

/// The console is there from the start: AllocConsole fails as Windows 95's
/// does for a process that has one, and FreeConsole keeps it.
pub(super) fn alloc_console(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_error(cpu, super::ERROR_ACCESS_DENIED);
    Some(FALSE)
}

pub(super) fn free_console(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TRUE)
}

/// GetConsoleCP and GetConsoleOutputCP: DOS's code page.
pub(super) fn get_console_cp(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(CODE_PAGE)
}

/// SetStdHandle(which, handle): STD_INPUT_HANDLE, STD_OUTPUT_HANDLE or
/// STD_ERROR_HANDLE (-10, -11, -12) is `handle` from now on.
pub(super) fn set_std_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let process = crate::win32::process(cpu);
    match args[0] as i32 {
        n @ -12..=-10 => {
            process.std[(-10 - n) as usize] = args[1];
            Some(TRUE)
        }
        _ => {
            process.last_error = ERROR_INVALID_HANDLE;
            Some(FALSE)
        }
    }
}

/// The keystrokes waiting in the BIOS's buffer, as console input events.
fn keys(cpu: &mut Cpu) -> Vec<u16> {
    crate::win32::dos(cpu, |cpu| {
        // Each is taken to see the next, and all put back after.
        let mut keys = Vec::new();
        while keys.len() < 64
            && let Some(key) = crate::keyboard::take_keystroke(&mut cpu.bus)
        {
            keys.push(key);
        }
        for &key in keys.iter().rev() {
            crate::keyboard::unget_keystroke(&mut cpu.bus, key);
        }
        keys
    })
}

/// Take the next keystroke, if one was typed.
fn take_key(cpu: &mut Cpu) -> Option<u16> {
    crate::win32::dos(cpu, |cpu| crate::keyboard::take_keystroke(&mut cpu.bus))
}

/// GetNumberOfConsoleInputEvents(handle, count).
pub(super) fn get_number_of_console_input_events(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if console_of(cpu, args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    let count = keys(cpu).len() as u32;
    cpu.bus.guest_write_32(args[1], count);
    Some(TRUE)
}

/// The virtual key code of a keystroke (scan code high, character low).
fn virtual_key(key: u16) -> u16 {
    let (scan, ascii) = ((key >> 8) as u8, key as u8);
    match ascii {
        b'a'..=b'z' => (ascii - 0x20) as u16,
        b'A'..=b'Z' | b'0'..=b'9' | b' ' => ascii as u16,
        0x0D => 0x0D,
        0x1B => 0x1B,
        0x08 => 0x08,
        0x09 => 0x09,
        _ => match scan {
            0x3B..=0x44 => 0x70 + (scan - 0x3B) as u16,
            0x85 | 0x86 => 0x7A + (scan - 0x85) as u16,
            0x47 => 0x24,
            0x48 => 0x26,
            0x49 => 0x21,
            0x4B => 0x25,
            0x4D => 0x27,
            0x4F => 0x23,
            0x50 => 0x28,
            0x51 => 0x22,
            0x52 => 0x2D,
            0x53 => 0x2E,
            _ => 0,
        },
    }
}

/// Write the keystrokes `keys` as KEY_EVENT INPUT_RECORDs at `at`.
fn write_events(cpu: &mut Cpu, at: u32, keys: &[u16]) {
    for (i, &key) in keys.iter().enumerate() {
        let record = at + INPUT_RECORD * i as u32;
        let ascii = match key as u8 {
            0xE0 => 0,
            c => c,
        };
        cpu.bus.guest_write_16(record, KEY_EVENT);
        cpu.bus.guest_write_16(record + 2, 0);
        cpu.bus.guest_write_32(record + 4, TRUE);
        cpu.bus.guest_write_16(record + 8, 1);
        cpu.bus.guest_write_16(record + 10, virtual_key(key));
        cpu.bus.guest_write_16(record + 12, key >> 8);
        cpu.bus.guest_write_16(record + 14, ascii as u16);
        cpu.bus.guest_write_32(record + 16, 0);
    }
}

/// PeekConsoleInputA and ReadConsoleInputA(handle, records, n, read):
/// the keystrokes typed, as key-down events. Reading takes them, and
/// waits for one when there is none.
fn console_input(cpu: &mut Cpu, args: &[u32], take: bool) -> Option<u32> {
    let &[handle, records, n, read] = args else {
        unreachable!()
    };
    if console_of(cpu, handle).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    if !cpu
        .bus
        .guest_probe(records, (n * INPUT_RECORD) as usize, true)
        || !cpu.bus.guest_probe(read, 4, true)
    {
        return None;
    }
    let mut keys = keys(cpu);
    keys.truncate(n as usize);
    if take && keys.is_empty() && n > 0 {
        file::wait_for_keys(cpu);
        return None;
    }
    if take {
        for _ in &keys {
            take_key(cpu);
        }
    }
    write_events(cpu, records, &keys);
    cpu.bus.guest_write_32(read, keys.len() as u32);
    Some(TRUE)
}

pub(super) fn peek_console_input(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    console_input(cpu, args, false)
}

pub(super) fn read_console_input(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    console_input(cpu, args, true)
}

/// FlushConsoleInputBuffer(handle): the keystrokes typed go.
pub(super) fn flush_console_input_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if console_of(cpu, args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    crate::win32::dos(cpu, |cpu| crate::keyboard::clear_keystrokes(&mut cpu.bus));
    cpu.con_pending.clear();
    Some(TRUE)
}

/// SetConsoleTitleA(title): there is no window to have it.
pub(super) fn set_console_title(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TRUE)
}
