//! FormatMessageA: messages from a string or from the system's table,
//! with inserts (%1 to %99) formatted as wsprintf formats them.

use super::set_error;
use super::text::{read_string, read_wide_string, wide_to_ansi};
use super::{ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY};
use crate::cpu::Cpu;
use crate::win32::with_process;

const FORMAT_MESSAGE_ALLOCATE_BUFFER: u32 = 0x100;
const FORMAT_MESSAGE_IGNORE_INSERTS: u32 = 0x200;
const FORMAT_MESSAGE_FROM_STRING: u32 = 0x400;
const FORMAT_MESSAGE_FROM_HMODULE: u32 = 0x800;
const FORMAT_MESSAGE_FROM_SYSTEM: u32 = 0x1000;
const FORMAT_MESSAGE_ARGUMENT_ARRAY: u32 = 0x2000;
const FORMAT_MESSAGE_MAX_WIDTH_MASK: u32 = 0xFF;

const ERROR_MR_MID_NOT_FOUND: u32 = 317;

/// The system's messages: the common error codes' texts.
const MESSAGES: &[(u32, &str)] = &[
    (0, "The operation completed successfully."),
    (1, "Incorrect function."),
    (2, "The system cannot find the file specified."),
    (3, "The system cannot find the path specified."),
    (4, "The system cannot open the file."),
    (5, "Access is denied."),
    (6, "The handle is invalid."),
    (7, "The storage control blocks were destroyed."),
    (
        8,
        "Not enough storage is available to process this command.",
    ),
    (9, "The storage control block address is invalid."),
    (10, "The environment is incorrect."),
    (
        11,
        "An attempt was made to load a program with an incorrect format.",
    ),
    (12, "The access code is invalid."),
    (13, "The data is invalid."),
    (
        14,
        "Not enough storage is available to complete this operation.",
    ),
    (15, "The system cannot find the drive specified."),
    (16, "The directory cannot be removed."),
    (
        17,
        "The system cannot move the file to a different disk drive.",
    ),
    (18, "There are no more files."),
    (19, "The media is write protected."),
    (21, "The device is not ready."),
    (
        32,
        "The process cannot access the file because it is being used by another process.",
    ),
    (
        33,
        "The process cannot access the file because another process has locked a portion of the file.",
    ),
    (38, "Reached end of file."),
    (39, "The disk is full."),
    (50, "The network request is not supported."),
    (80, "The file exists."),
    (82, "The directory or file cannot be created."),
    (87, "The parameter is incorrect."),
    (109, "The pipe has been ended."),
    (110, "The system cannot open the device or file specified."),
    (111, "The file name is too long."),
    (112, "There is not enough space on the disk."),
    (120, "This function is only valid in Win32 mode."),
    (122, "The data area passed to a system call is too small."),
    (
        123,
        "The filename, directory name, or volume label syntax is incorrect.",
    ),
    (126, "The specified module could not be found."),
    (127, "The specified procedure could not be found."),
    (183, "Cannot create a file when that file already exists."),
    (193, "%1 is not a valid Win32 application."),
    (
        203,
        "The system could not find the environment option that was entered.",
    ),
    (206, "The filename or extension is too long."),
    (234, "More data is available."),
    (259, "No more data is available."),
    (
        317,
        "The system cannot find message for message number 0x%1 in message file for %2.",
    ),
    (487, "Attempt to access invalid address."),
    (998, "Invalid access to memory location."),
    (1004, "Invalid flags."),
    (1400, "Invalid window handle."),
];

/// Where an insert's arguments are: a va_list or an array of them.
struct Arguments {
    at: u32,
    array: bool,
}

impl Arguments {
    /// The `n`th argument (from 1). None if its page isn't there.
    fn get(&self, cpu: &mut Cpu, n: u32) -> Option<u32> {
        let base = match self.array {
            true => self.at,
            false => cpu.bus.guest_read_32(self.at),
        };
        let value = cpu.bus.guest_read_32(base.wrapping_add(4 * (n - 1)));
        (!cpu.bus.guest_faulted()).then_some(value)
    }
}

/// An insert's text, formatted by the wsprintf format `spec` (without its
/// '!'s; "s" for none). Windows 95's wsprintf takes no '*': where its
/// type should be, the rest of the format is copied as it is.
fn format_insert(cpu: &mut Cpu, spec: &[u8], value: u32) -> Option<Vec<u8>> {
    let mut i = 0;
    let (mut left, mut zero) = (false, false);
    while let Some(&c) = spec.get(i) {
        match c {
            b'-' => left = true,
            b'0' => zero = true,
            b'#' | b' ' | b'+' => {}
            _ => break,
        }
        i += 1;
    }
    let digits = |i: &mut usize| {
        let mut n = 0usize;
        while let Some(d) = spec.get(*i).filter(|d| d.is_ascii_digit()) {
            n = n * 10 + (d - b'0') as usize;
            *i += 1;
        }
        n
    };
    let width = digits(&mut i);
    let precision = match spec.get(i) {
        Some(b'.') => {
            i += 1;
            Some(digits(&mut i))
        }
        _ => None,
    };
    let wide = match spec.get(i) {
        Some(b'l' | b'w') => {
            i += 1;
            true
        }
        Some(b'h') => {
            i += 1;
            false
        }
        _ => false,
    };
    let mut number = false;
    let mut text = match spec.get(i) {
        Some(b's') if !wide => read_string(cpu, value)?,
        Some(b's') => wide_to_ansi(&read_wide_string(cpu, value)?),
        Some(b'S') => wide_to_ansi(&read_wide_string(cpu, value)?),
        Some(b'c') if !wide => vec![value as u8],
        Some(b'c' | b'C') => wide_to_ansi(&[value as u16]),
        Some(&c @ (b'd' | b'i' | b'u' | b'x' | b'X')) => {
            number = true;
            let mut s = match c {
                b'd' | b'i' => (value as i32).unsigned_abs().to_string(),
                b'u' => value.to_string(),
                b'x' => format!("{:x}", value),
                _ => format!("{:X}", value),
            };
            if let Some(p) = precision.filter(|&p| p > s.len()) {
                s = "0".repeat(p - s.len()) + &s;
            }
            if matches!(c, b'd' | b'i') && (value as i32) < 0 {
                s.insert(0, '-');
            }
            s.into_bytes()
        }
        _ => return Some(spec[i.min(spec.len())..].to_vec()),
    };
    if !number && let Some(p) = precision {
        text.truncate(p);
    }
    if text.len() < width {
        let pad = width - text.len();
        if left {
            text.extend(std::iter::repeat_n(b' ', pad));
        } else {
            let fill = if zero && number { b'0' } else { b' ' };
            let at = if fill == b'0' && text.first() == Some(&b'-') {
                1
            } else {
                0
            };
            text.splice(at..at, std::iter::repeat_n(fill, pad));
        }
    }
    Some(text)
}

/// Why a message couldn't be formatted.
enum Failure {
    /// A guest page wasn't there.
    Fault,
    /// The message was wrong: a '%' at its end.
    Invalid,
}

/// The message `text` with its escapes and inserts done. The source's
/// line ends become "\r\n", or spaces with FORMAT_MESSAGE_MAX_WIDTH_MASK.
/// Without arguments, inserts are copied as they are, as Windows 95 does.
fn format(
    cpu: &mut Cpu,
    text: &[u8],
    flags: u32,
    args: Option<Arguments>,
) -> Result<Vec<u8>, Failure> {
    let ignore = flags & FORMAT_MESSAGE_IGNORE_INSERTS != 0;
    let one_line = flags & FORMAT_MESSAGE_MAX_WIDTH_MASK == FORMAT_MESSAGE_MAX_WIDTH_MASK;
    let line_end: &[u8] = if one_line { b" " } else { b"\r\n" };
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        i += 1;
        match c {
            b'\r' => {
                if text.get(i) == Some(&b'\n') {
                    i += 1;
                }
                out.extend(line_end);
                continue;
            }
            b'\n' => {
                out.extend(line_end);
                continue;
            }
            b'%' => {}
            _ => {
                out.push(c);
                continue;
            }
        }
        let start = i - 1;
        let Some(&e) = text.get(i) else {
            if ignore {
                out.push(b'%');
                break;
            }
            return Err(Failure::Invalid);
        };
        i += 1;
        match e {
            b'0' => break,
            b'n' => out.extend(b"\r\n"),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'1'..=b'9' if !ignore => {
                let mut n = (e - b'0') as u32;
                if let Some(d) = text.get(i).filter(|d| d.is_ascii_digit()) {
                    n = n * 10 + (d - b'0') as u32;
                    i += 1;
                }
                let mut spec: &[u8] = b"s";
                if text.get(i) == Some(&b'!')
                    && let Some(len) = text[i + 1..].iter().position(|&b| b == b'!')
                {
                    spec = &text[i + 1..i + 1 + len];
                    i += len + 2;
                }
                match &args {
                    None => out.extend(&text[start..i]),
                    Some(args) => {
                        let value = args.get(cpu, n).ok_or(Failure::Fault)?;
                        out.extend(format_insert(cpu, spec, value).ok_or(Failure::Fault)?);
                    }
                }
            }
            _ if ignore => out.extend([b'%', e]),
            _ => out.push(e),
        }
    }
    Ok(out)
}

/// The text of the system's message `id`, with its line end.
fn system_message(id: u32) -> Option<Vec<u8>> {
    let (_, text) = MESSAGES.iter().find(|&&(n, _)| n == id)?;
    Some([text.as_bytes(), b"\r\n"].concat())
}

/// FormatMessageA: the message from the string `args[1]`
/// (FORMAT_MESSAGE_FROM_STRING) or the system's message `args[2]`, with
/// the arguments `args[6]`, into `args[4]` (`args[5]` bytes), or with
/// FORMAT_MESSAGE_ALLOCATE_BUFFER into a buffer of the heap whose address
/// goes to `args[4]`. Returns the length without the zero; 0 when the
/// message is empty, without an error.
pub(super) fn format_message_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[flags, source, id, _language, to, size, arguments] = args else {
        unreachable!()
    };
    let from =
        FORMAT_MESSAGE_FROM_STRING | FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_FROM_HMODULE;
    if flags & from == 0 || to == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let text = if flags & FORMAT_MESSAGE_FROM_STRING != 0 {
        read_string(cpu, source)?
    } else {
        let system =
            flags & FORMAT_MESSAGE_FROM_SYSTEM != 0 || source == 0 || source == super::DLL.base;
        let Some(text) = system.then(|| system_message(id)).flatten() else {
            set_error(cpu, ERROR_MR_MID_NOT_FOUND);
            return Some(0);
        };
        text
    };
    let inserts =
        (arguments != 0 && flags & FORMAT_MESSAGE_IGNORE_INSERTS == 0).then_some(Arguments {
            at: arguments,
            array: flags & FORMAT_MESSAGE_ARGUMENT_ARRAY != 0,
        });
    let mut out = match format(cpu, &text, flags, inserts) {
        Ok(out) => out,
        Err(Failure::Fault) => return None,
        Err(Failure::Invalid) => {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            return Some(0);
        }
    };
    let len = out.len() as u32;
    out.push(0);
    if flags & FORMAT_MESSAGE_ALLOCATE_BUFFER != 0 {
        if !cpu.bus.guest_probe(to, 4, true) {
            return None;
        }
        if len == 0 {
            cpu.bus.guest_write_32(to, 0);
            return Some(0);
        }
        let room = (len + 1).max(size);
        let Some(at) = with_process(cpu, |p, bus| p.heap.alloc(&mut p.memory, bus, room)) else {
            set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
            return Some(0);
        };
        cpu.bus.guest_write_bytes(at, &out);
        cpu.bus.guest_write_32(to, at);
        return Some(len);
    }
    if len == 0 {
        return Some(0);
    }
    if len >= size {
        set_error(cpu, ERROR_INSUFFICIENT_BUFFER);
        return Some(0);
    }
    if !cpu.bus.guest_probe(to, out.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(to, &out);
    Some(len)
}
