//! Processes: starting a program as a process of its own, finding one by
//! its id, and what a process learns of how it was started.

use super::threads::{process_of, thread_of};
use super::{ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error, string_at};
use crate::cpu::Cpu;
use crate::win32::memory::{PAGE, PRIVATE, SYSTEM};
use crate::win32::object;
use crate::win32::{create_process, process};

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_BAD_EXE_FORMAT: u32 = 193;
const ERROR_NOACCESS: u32 = 998;

const CREATE_SUSPENDED: u32 = 4;
const STARTF_USESTDHANDLES: u32 = 0x100;

/// STARTUPINFOA's size and fields.
pub(crate) const STARTUPINFO_SIZE: u32 = 68;
const SI_RESERVED: u32 = 4;
const SI_DESKTOP: u32 = 8;
const SI_TITLE: u32 = 12;
const SI_FLAGS: u32 = 44;
const SI_CB_RESERVED2: u32 = 50;
const SI_RESERVED2: u32 = 52;
const SI_STD_INPUT: u32 = 56;

/// The program a command line names first, as Windows finds it with no
/// application name (`program_in_line`).
pub struct ProgramInLine<'a> {
    /// Its DOS path, with ".EXE" added if the line gave no extension.
    pub path: String,
    /// The text of the line that names it, without quotes.
    pub name: &'a str,
    /// The rest of the line: after the closing quote of a quoted name
    /// (empty if it has none), else right after the name.
    pub args: &'a str,
    quoted: bool,
}

impl ProgramInLine<'_> {
    /// The command line the program gets for `line`, the line it was
    /// found in: with its name put in quotes if it was found past a space
    /// or tab, as Windows 98 SE does. WinExec of
    /// C:\Program Files\XENO\xeno3dfx.exe -ar3 gives the game
    /// "C:\Program Files\XENO\xeno3dfx.exe" -ar3 for GetCommandLineA.
    /// The C runtime skips an unquoted program name up to its first blank
    /// to find WinMain's command line, and would take the rest of the
    /// path for arguments. That line is built anew, so blanks before the
    /// name go; any other line is given as it is.
    pub fn command_line(&self, line: &str) -> String {
        if self.quoted || !self.name.contains([' ', '\t']) {
            return line.to_string();
        }
        format!("\"{}\"{}", self.name, self.args)
    }
}

/// The program a command line names first, as Windows finds it with no
/// application name. That is the quoted name, or for an unquoted one,
/// the line up to each space or tab in turn, the shortest first, until
/// one is a program (`find_program`). "C:\Program Files\Game\Game.exe -x"
/// tries C:\Program, C:\Program Files\Game\Game.exe and stops there.
pub(super) fn program_in_line<'a>(cpu: &mut Cpu, line: &'a str) -> Option<ProgramInLine<'a>> {
    let line = line.trim_start();
    if let Some(rest) = line.strip_prefix('"') {
        let (name, args) = rest.split_once('"').unwrap_or((rest, ""));
        let path = find_program(cpu, name)?;
        return Some(ProgramInLine {
            path,
            name,
            args,
            quoted: true,
        });
    }
    let (path, end) = line
        .match_indices([' ', '\t'])
        .map(|(i, _)| i)
        .chain([line.len()])
        .filter(|&i| i > 0 && !line[..i].ends_with([' ', '\t']))
        .find_map(|end| Some((find_program(cpu, &line[..end])?, end)))?;
    let (name, args) = line.split_at(end);
    Some(ProgramInLine {
        path,
        name,
        args,
        quoted: false,
    })
}

/// The DOS path of the program `name`, with ".EXE" added if it has no
/// extension: there if it has a directory in it, else where SearchPathA
/// looks (the running program's directory, the current one, the system
/// and Windows directories, then PATH). A directory isn't a program, so
/// a name that is one isn't found.
fn find_program(cpu: &mut Cpu, name: &str) -> Option<String> {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let name = if file.contains('.') {
        name.to_string()
    } else {
        format!("{}.EXE", name)
    };
    let found = super::path::search(cpu, None, &name)?;
    Some(cpu.program_path(&super::path::dos_path(&found)))
}

/// The environment block at `at`: "NAME=value" strings, each ended by a
/// zero, the last by two.
fn environment_at(cpu: &mut Cpu, mut at: u32) -> Option<Vec<String>> {
    let mut strings = Vec::new();
    loop {
        let s = string_at(cpu, at)?;
        if s.is_empty() {
            return Some(strings);
        }
        at += crate::dosstr::to_bytes(&s).len() as u32 + 1;
        strings.push(s);
    }
}

/// CreateProcessA(application, command line, process attributes, thread
/// attributes, inherit, flags, environment, directory, startup info,
/// process information): the program as a process of its own, which runs
/// beside its parent. The process information gets the handles of the
/// process and its main thread, and their ids.
pub(super) fn create_process_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[
        app_at,
        line_at,
        _,
        _,
        inherit,
        flags,
        env_at,
        _,
        startup_at,
        info_at,
    ] = args
    else {
        unreachable!()
    };
    let app = match app_at {
        0 => None,
        at => Some(string_at(cpu, at)?),
    };
    let line = match line_at {
        0 => None,
        at => Some(string_at(cpu, at)?),
    };
    let mut startup = vec![0u8; STARTUPINFO_SIZE as usize];
    if startup_at != 0 {
        cpu.bus.guest_read_bytes(startup_at, &mut startup);
    }
    // The C runtime's block (lpReserved2) goes to the child after it.
    let field = |i: u32| u32::from_le_bytes(startup[i as usize..][..4].try_into().unwrap());
    let size = u16::from_le_bytes([startup[50], startup[51]]) as usize;
    if field(SI_RESERVED2) != 0 && size != 0 {
        let mut block = vec![0u8; size];
        cpu.bus.guest_read_bytes(field(SI_RESERVED2), &mut block);
        startup.extend_from_slice(&block);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    let environment = match env_at {
        0 => None,
        at => Some(environment_at(cpu, at)?),
    };
    if !cpu.bus.guest_probe(info_at, 16, true) {
        return None;
    }
    let (app, line) = (app.as_deref(), line.as_deref());
    match spawn(cpu, app, line, environment, inherit != 0, flags, startup) {
        Ok(info) => {
            for (i, value) in info.into_iter().enumerate() {
                cpu.bus.guest_write_32(info_at + 4 * i as u32, value);
            }
            set_error(cpu, 0);
            Some(TRUE)
        }
        Err(code) => {
            set_error(cpu, code);
            Some(FALSE)
        }
    }
}

/// Start the program `app`, or the one the command line `line` names
/// first, as CreateProcessA does, with `environment` or the running
/// process's, all its handles if `inherit`, and `startup`: a STARTUPINFOA
/// and after it the C runtime's block that lpReserved2 points to, if
/// there is one. The handles of the process and its main thread, and
/// their ids; or the error.
pub(crate) fn spawn(
    cpu: &mut Cpu,
    app: Option<&str>,
    line: Option<&str>,
    environment: Option<Vec<String>>,
    inherit: bool,
    flags: u32,
    mut startup: Vec<u8>,
) -> Result<[u32; 4], u32> {
    let (path, line) = match (app, line) {
        // Given the program, Windows passes the line on as it is.
        (Some(app), line) => {
            let path = find_program(cpu, app).ok_or(ERROR_FILE_NOT_FOUND)?;
            let line = line.map_or_else(|| path.clone(), str::to_string);
            (path, line)
        }
        (None, Some(line)) => {
            let found = program_in_line(cpu, line).ok_or(ERROR_FILE_NOT_FOUND)?;
            let line = found.command_line(line);
            (found.path, line)
        }
        (None, None) => return Err(ERROR_INVALID_PARAMETER),
    };
    let environment = environment.unwrap_or_else(|| process(cpu).environment.clone());
    let fields = startup.clone();
    let field = |i: u32| u32::from_le_bytes(fields[i as usize..][..4].try_into().unwrap());
    // The child's own copy: no strings or data of the parent's, and the
    // C runtime's block where GetStartupInfoA puts it.
    for i in [SI_RESERVED, SI_DESKTOP, SI_TITLE, SI_RESERVED2] {
        startup[i as usize..i as usize + 4].fill(0);
    }
    startup[..4].copy_from_slice(&STARTUPINFO_SIZE.to_le_bytes());
    let block = (startup.len() - STARTUPINFO_SIZE as usize) as u16;
    startup[SI_CB_RESERVED2 as usize..][..2].copy_from_slice(&block.to_le_bytes());
    let inherited: Vec<(u32, u32)> = if inherit {
        process(cpu).handles.iter().collect()
    } else {
        Vec::new()
    };
    let std = if field(SI_FLAGS) & STARTF_USESTDHANDLES != 0 {
        let mut ids = [0; 3];
        for (i, id) in ids.iter_mut().enumerate() {
            let handle = field(SI_STD_INPUT + 4 * i as u32);
            *id = object::resolve(cpu, handle).unwrap_or(0);
        }
        // A handle that isn't one is DOS's.
        let own = process(cpu).std;
        for (i, id) in ids.iter_mut().enumerate() {
            if *id == 0 {
                *id = object::resolve(cpu, own[i]).unwrap_or(0);
            }
        }
        Some(ids)
    } else {
        let own = process(cpu).std;
        let ids = own.map(|h| object::resolve(cpu, h).unwrap_or(0));
        Some(ids)
    };
    let std = std.filter(|ids| ids.iter().all(|&id| object::objects(cpu).get(id).is_some()));
    let suspended = flags & CREATE_SUSPENDED != 0;
    let created = create_process(
        cpu,
        &path,
        &line,
        environment,
        inherited,
        std,
        startup,
        suspended,
    )
    .map_err(|message| {
        cpu.bus.log_string(&format!("[W32] {}", message));
        ERROR_BAD_EXE_FORMAT
    })?;
    let (process_object, pid) = created.process;
    let (thread_object, tid) = created.thread;
    let objects = object::objects(cpu);
    objects.hold(process_object);
    objects.hold(thread_object);
    let process_handle = object::new_handle(cpu, process_object);
    let thread_handle = object::new_handle(cpu, thread_object);
    Ok([process_handle, thread_handle, pid, tid])
}

/// WinExec(command line, show): the program the command line names
/// first, started by CreateProcessA with `show` for its first window.
/// As on Windows 95, 33 once it started, or the error: 2 for a program
/// not found, 11 for one that isn't a program.
pub(super) fn win_exec(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    const STARTF_USESHOWWINDOW: u32 = 1;
    const SI_SHOW_WINDOW: usize = 48;
    const ERROR_BAD_FORMAT: u32 = 11;
    let [line_at, show] = args[..] else {
        unreachable!()
    };
    let line = string_at(cpu, line_at)?;
    let mut startup = vec![0u8; STARTUPINFO_SIZE as usize];
    startup[SI_FLAGS as usize..][..4].copy_from_slice(&STARTF_USESHOWWINDOW.to_le_bytes());
    startup[SI_SHOW_WINDOW..][..2].copy_from_slice(&(show as u16).to_le_bytes());
    match spawn(cpu, None, Some(&line), None, false, 0, startup) {
        Ok([process, thread, _, _]) => {
            object::close(cpu, process);
            object::close(cpu, thread);
            Some(33)
        }
        Err(ERROR_FILE_NOT_FOUND | ERROR_INVALID_PARAMETER) => Some(ERROR_FILE_NOT_FOUND),
        Err(_) => Some(ERROR_BAD_FORMAT),
    }
}

/// OpenProcess(access, inherit, id): a handle of the process with the id
/// `id`, while it runs.
pub(super) fn open_process(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = args[2];
    let found = cpu
        .bus
        .win32
        .processes
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.object);
    let Some(object) = found else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    object::objects(cpu).hold(object);
    Some(object::new_handle(cpu, object))
}

/// GetStartupInfoA(info): what the parent gave CreateProcessA, or for the
/// process DOS started, nothing but the standard handles. The C
/// runtime's block the parent passed is copied to the process heap the
/// first time.
pub(super) fn get_startup_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    let size = STARTUPINFO_SIZE as usize;
    let process = process(cpu);
    if process.startup.len() > size && process.startup[SI_RESERVED2 as usize..][..4] == [0; 4] {
        let block = process.startup[size..].to_vec();
        let placed = crate::win32::with_process(cpu, |p, bus| {
            let to = p.heap.alloc(&mut p.memory, bus, block.len() as u32)?;
            p.memory.write(bus, to, &block)?;
            Some(to)
        });
        if let Some(to) = placed {
            let field = &mut crate::win32::process(cpu).startup[SI_RESERVED2 as usize..][..4];
            field.copy_from_slice(&to.to_le_bytes());
        }
    }
    let process = crate::win32::process(cpu);
    let mut info = process.startup.clone();
    if info.is_empty() {
        info = vec![0; size];
        info[..4].copy_from_slice(&STARTUPINFO_SIZE.to_le_bytes());
    }
    info.truncate(size);
    if u32::from_le_bytes(info[SI_FLAGS as usize..][..4].try_into().unwrap()) & STARTF_USESTDHANDLES
        == 0
    {
        for (i, handle) in process.std.into_iter().enumerate() {
            let at = SI_STD_INPUT as usize + 4 * i;
            info[at..at + 4].copy_from_slice(&handle.to_le_bytes());
        }
    }
    cpu.bus.guest_write_bytes(at, &info);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(0)
}

/// GetProcessAffinityMask(process, process mask, system mask): one
/// processor.
pub(super) fn get_process_affinity_mask(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if process_of(cpu, args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    if !cpu.bus.guest_probe(args[1], 4, true) || !cpu.bus.guest_probe(args[2], 4, true) {
        return None;
    }
    cpu.bus.guest_write_32(args[1], 1);
    cpu.bus.guest_write_32(args[2], 1);
    Some(TRUE)
}

/// SetThreadAffinityMask(thread, mask): the mask before, 1, when `mask`
/// names the one processor and no other; else 0.
pub(super) fn set_thread_affinity_mask(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if thread_of(cpu, args[0]).is_none() {
        return Some(0);
    }
    if args[1] != 1 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    Some(1)
}

/// The priority classes.
const NORMAL_PRIORITY_CLASS: u32 = 0x20;
const IDLE_PRIORITY_CLASS: u32 = 0x40;
const HIGH_PRIORITY_CLASS: u32 = 0x80;
const REALTIME_PRIORITY_CLASS: u32 = 0x100;

/// GetPriorityClass(process).
pub(super) fn get_priority_class(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match process_of(cpu, args[0]) {
        Some(p) => Some(cpu.bus.win32.processes[p].priority_class),
        None => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            Some(0)
        }
    }
}

/// SetPriorityClass(process, class): kept; threads take turns whatever
/// their priority.
pub(super) fn set_priority_class(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(p) = process_of(cpu, args[0]) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    let classes = [
        NORMAL_PRIORITY_CLASS,
        IDLE_PRIORITY_CLASS,
        HIGH_PRIORITY_CLASS,
        REALTIME_PRIORITY_CLASS,
    ];
    if !classes.contains(&args[1]) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    cpu.bus.win32.processes[p].priority_class = args[1];
    Some(TRUE)
}

/// The physical address of `lin` in the process `p`'s memory, where the
/// program may reach it.
fn user_page(cpu: &Cpu, p: usize, lin: u32) -> Option<u32> {
    if !(PRIVATE..SYSTEM).contains(&lin) {
        return None;
    }
    cpu.bus.win32.processes[p].memory.translate(&cpu.bus, lin)
}

/// ReadProcessMemory(process, address, buffer, size, read) and
/// WriteProcessMemory: `size` bytes between another process's memory
/// and the caller's. Fails with ERROR_NOACCESS, having copied the bytes
/// before, where the other process has no page.
fn process_memory(cpu: &mut Cpu, args: &[u32], write: bool) -> Option<u32> {
    let &[handle, address, buffer, size, done_at] = args else {
        unreachable!()
    };
    let Some(p) = process_of(cpu, handle) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    if !cpu.bus.guest_probe(buffer, size as usize, !write) {
        return None;
    }
    if done_at != 0 && !cpu.bus.guest_probe(done_at, 4, true) {
        return None;
    }
    let mut done = 0;
    while done < size {
        let lin = address.wrapping_add(done);
        let Some(phys) = user_page(cpu, p, lin) else {
            break;
        };
        let len = (PAGE - (lin & (PAGE - 1))).min(size - done);
        let mut bytes = vec![0; len as usize];
        if write {
            cpu.bus.guest_read_bytes(buffer + done, &mut bytes);
            for (i, &b) in bytes.iter().enumerate() {
                cpu.bus.write_8(phys as usize + i, b);
            }
        } else {
            for (i, b) in bytes.iter_mut().enumerate() {
                *b = cpu.bus.read_8(phys as usize + i);
            }
            cpu.bus.guest_write_bytes(buffer + done, &bytes);
        }
        done += len;
    }
    if done_at != 0 {
        cpu.bus.guest_write_32(done_at, done);
    }
    if done < size {
        set_error(cpu, ERROR_NOACCESS);
        return Some(FALSE);
    }
    Some(TRUE)
}

pub(super) fn read_process_memory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    process_memory(cpu, args, false)
}

pub(super) fn write_process_memory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    process_memory(cpu, args, true)
}

/// FlushInstructionCache(process, address, size): nothing to do, as the
/// processor sees code written as it runs.
pub(super) fn flush_instruction_cache(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TRUE)
}

/// SetErrorMode(mode): the mode before. There are no dialogs for errors
/// to turn off.
pub(super) fn set_error_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let process = process(cpu);
    Some(std::mem::replace(&mut process.error_mode, args[0]))
}

/// GetProcessVersion(id): the Windows version the program was made for,
/// from its header, as major << 16 | minor. 0 for no such process.
pub(super) fn get_process_version(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = match args[0] {
        0 => process(cpu).id,
        id => id,
    };
    match cpu.bus.win32.processes.iter().find(|p| p.id == id) {
        Some(p) => Some(p.subsystem_version),
        None => {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            Some(0)
        }
    }
}

/// FatalAppExitA(flags, message): the program ends with the message on
/// standard error, as no message box can show it.
pub(super) fn fatal_app_exit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let message = string_at(cpu, args[1]).unwrap_or_default();
    crate::win32::fatal(cpu, &format!("FatalAppExit: {}", message), None);
    None
}

/// FatalExit(code): the program ends.
pub(super) fn fatal_exit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    crate::win32::fatal(cpu, &format!("FatalExit({})", args[0]), None);
    None
}

#[cfg(test)]
mod tests {
    use super::ProgramInLine;

    fn found<'a>(name: &'a str, args: &'a str, quoted: bool) -> ProgramInLine<'a> {
        let path = String::new();
        ProgramInLine {
            path,
            name,
            args,
            quoted,
        }
    }

    #[test]
    fn a_program_found_past_a_blank_gets_quotes() {
        let line = r"C:\Program Files\X.EXE -ar3";
        let program = found(r"C:\Program Files\X.EXE", " -ar3", false);
        assert_eq!(
            program.command_line(line),
            r#""C:\Program Files\X.EXE" -ar3"#
        );
        // A tab is a blank too, and blanks before the name go.
        let line = " C:\\A\tB\\X.EXE";
        let program = found("C:\\A\tB\\X.EXE", "", false);
        assert_eq!(program.command_line(line), "\"C:\\A\tB\\X.EXE\"");
    }

    #[test]
    fn other_lines_are_given_as_they_are() {
        let line = " X.EXE  -a";
        assert_eq!(found("X.EXE", "  -a", false).command_line(line), line);
        let line = r#""C:\Program Files\X.EXE" -a"#;
        let program = found(r"C:\Program Files\X.EXE", " -a", true);
        assert_eq!(program.command_line(line), line);
        // No closing quote.
        let line = r#""C:\Program Files\X.EXE"#;
        let program = found(r"C:\Program Files\X.EXE", "", true);
        assert_eq!(program.command_line(line), line);
    }
}
