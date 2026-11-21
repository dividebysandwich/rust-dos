//! DOS programs a Windows program starts: WinExec, LoadModule,
//! CreateProcessA, ShellExecute and the C runtime's spawn functions of
//! a DOS program (an MZ file that is neither a PE file nor a Windows 3.1
//! program, a COM file, or a batch file, which COMMAND.COM /C runs), or
//! of a PIF, which names the DOS program it starts (pif.rs).
//!
//! Windows 98 runs such a program in a virtual DOS machine of its own:
//! its VMM gives the machine a copy of the first megabyte as DOS was
//! when Windows started, with its own vector table, DOS data and
//! conventional memory, and switches the processor between it and the
//! system's machine, in which the Windows programs run. WinExec returns
//! at once (33 or more), and CreateProcessA gives handles of a process
//! and a thread that are signaled when the DOS program ends, whose exit
//! code GetExitCodeProcess gives. The program runs full screen or in a
//! window, as its PIF (or _DEFAULT.PIF) says, and when it ends its
//! machine goes, with whatever it left in memory.
//!
//! The built-in Windows runs in one DOS process of the built-in DOS (the
//! one DOS started the first Windows program in), whose real mode it
//! still has: the real-mode code it calls for its programs (realmode.rs)
//! runs there. A DOS program starts there too, as that process's child:
//! when the service that started it returns, the system calls DOS's
//! EXEC in real mode, with the caller's context kept in a real-mode frame
//! (`Kind::Exec`), and the Windows programs wait while the DOS program
//! runs, full screen. When it ends, DOS goes back to the code that called
//! EXEC, which returns to the system (`ended`): the process and thread
//! objects get its exit code, the screen gets the desktop back, and the
//! caller goes on after its call, having been told that the program
//! started. So a program that waits for the one it started
//! (WaitForSingleObject, GetExitCodeProcess, GetModuleUsage, a loop
//! looking for a file the program writes) finds it ended.
//!
//! That is what a full-screen DOS box does on Windows 98, but for the
//! Windows programs, which go on running there behind it. It shares
//! DOS's drives and current directories with the Windows programs, so
//! what the DOS program writes is there for them.
//!
//! The rest of a virtual machine waits for later: the Windows programs
//! running beside the DOS program, a window for it, and a first megabyte
//! of its own. They need a second copy of the first megabyte and of
//! DOS's state, switched with the processor's context, as the VMM does.
//! The parts the DOS program can leave behind are put back here, as
//! they were before it started (on Windows 98 the DOS machine has its
//! own): the vector table, DOS's current drive and directories, its
//! memory strategy and upper memory link, the PICs' masks, the timer's
//! rate, the mouse driver's event handler, and the FPU and SSE
//! registers of the thread that started it (`Machine`). Its memory and
//! files are freed by DOS when it ends, and the screen is the desktop
//! again, in its mode, with the mouse's range over it.

use super::boot::Directories;
use super::realmode::{self, Frame, Kind};
use super::{Context, EAX, EBX, EDX, ESP, ROM_SEG, object};
use crate::cpu::{Cpu, CpuFlags};
use crate::instructions::fpu::control;

/// The block of conventional memory the start needs, in the DOS
/// process's memory: EXEC's parameter block, its two FCBs, the command
/// tail, the program's path and the environment, at these offsets.
const PARAMS: u16 = 0x0000;
const FCB1: u16 = 0x0010;
const FCB2: u16 = 0x0020;
const TAIL: u16 = 0x0030;
const PATH: u16 = 0x00B0;
const ENVIRONMENT: u16 = 0x0130;

/// The longest command tail DOS takes, without its length and CR.
const MAX_TAIL: usize = 126;
/// The longest path EXEC takes, with its NUL.
const MAX_PATH: usize = (ENVIRONMENT - PATH) as usize;

/// The command interpreter that runs a batch file.
const COMMAND_COM: &str = "Z:\\COMMAND.COM";

/// The real-mode vector table's size, which the program gets back as it
/// was.
const VECTORS: usize = 0x400;

/// The W32 section's version that added the DOS program a Windows
/// program started.
pub const DOSVM_SAVED: u16 = 85;
/// The W32 section's version that added the folder it starts in.
pub const DOSVM_DIRECTORY: u16 = 86;

/// A DOS program a Windows program started.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// Its path, as the log names it.
    path: String,
    /// The objects of its process and thread, which the session holds.
    process: u32,
    thread: u32,
    /// The segment of the block of conventional memory with what EXEC
    /// is given.
    block: u16,
    /// The real-mode vector table before it started.
    vectors: Vec<u8>,
    /// Whether the desktop was on the screen.
    desktop: bool,
    /// DOS's current PSP before it started.
    psp: u16,
    /// DOS's current drive and directories before it started.
    directories: Directories,
    /// The rest of the machine before it started.
    machine: Machine,
    /// The folder it starts in, a full path: its PIF's working folder,
    /// CreateProcessA's lpCurrentDirectory, or the current folder of
    /// its caller. ShellExecute has put back its own by the time the
    /// program starts (`start_pending`).
    directory: crate::savestate::Since<DOSVM_DIRECTORY, String>,
    /// The bytes of the Windows programs' memory pool lent to the XMS
    /// pool while it runs (`memory::Pool::lend`).
    lent: crate::savestate::Since<DOSVM_DIRECTORY, u32>,
}

/// What of the machine a DOS program can change that Windows 98 keeps
/// apart for each virtual machine, which its VMM and virtual devices
/// switch with it: the Windows programs find it as it was when the
/// program ends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Machine {
    /// The FPU's registers (FSAVE's image), and the XMM registers and
    /// MXCSR on a processor with SSE: the calling thread's, which the
    /// DOS program shares the processor with.
    fpu: Vec<u8>,
    sse: Option<([u128; 8], u32)>,
    /// The masks of the master and the slave PIC.
    pic: (u8, u8),
    /// The timer's channel 0: its mode and count, IRQ 0's rate.
    pit: (u8, u32),
    /// The mouse driver's event handler (INT 33h AX=000Ch): its mask,
    /// segment and offset. One the program leaves points into memory
    /// that is freed when it ends.
    mouse: (u16, u16, u16),
    /// DOS's memory strategy (INT 21h AX=5801h) and whether upper memory
    /// is linked (AX=5803h).
    memory: (u16, bool),
}

impl Machine {
    fn now(cpu: &Cpu) -> Machine {
        let bus = &cpu.bus;
        let mouse = &bus.mouse;
        Machine {
            fpu: control::save_image(cpu).to_vec(),
            sse: cpu.model.has_sse().then_some((cpu.sse.xmm, cpu.sse.mxcsr)),
            pic: (bus.pic.master.imr, bus.pic.slave.imr),
            pit: (bus.pit0.mode(), bus.pit0.reload()),
            mouse: (mouse.callback_mask, mouse.callback_cs, mouse.callback_ip),
            memory: (cpu.alloc_strategy, bus.umb.is_some_and(|u| u.linked)),
        }
    }

    /// Put the machine back as it was.
    fn put_back(self, cpu: &mut Cpu) {
        if let Ok(image) = <[u8; 108]>::try_from(self.fpu.as_slice()) {
            control::load_image(cpu, &image);
        }
        if let Some((xmm, mxcsr)) = self.sse {
            cpu.sse.xmm = xmm;
            cpu.sse.mxcsr = mxcsr;
        }
        let bus = &mut cpu.bus;
        (bus.pic.master.imr, bus.pic.slave.imr) = self.pic;
        bus.refresh_irq();
        // A new rate, set as the BIOS sets one: the control word, then
        // the count's low and high byte (0 for 65536).
        let (mode, count) = self.pit;
        if (bus.pit0.mode(), bus.pit0.reload()) != self.pit {
            let [low, high] = (count as u16).to_le_bytes();
            bus.io_write(0x43, 0x30 | mode << 1);
            bus.io_write(0x40, low);
            bus.io_write(0x40, high);
        }
        let mouse = &mut bus.mouse;
        (mouse.callback_mask, mouse.callback_cs, mouse.callback_ip) = self.mouse;
        mouse.pending_callback_events = 0;
        crate::mouse::clear_callback_busy(bus);
        let (strategy, linked) = self.memory;
        cpu.alloc_strategy = strategy;
        super::dos(cpu, |cpu| crate::mcb::link_upper(&mut cpu.bus, linked));
    }
}

/// The DOS program waiting for the service that started it to return,
/// and the one running.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pending: Option<Session>,
    running: Option<Session>,
}

/// Whether a DOS program a Windows program started is running: the
/// Windows programs wait.
pub fn running(win32: &super::Win32) -> bool {
    win32.dosvm.running.is_some()
}

/// The Windows version a DOS program it started finds (INT 2Fh AX=1600h
/// and 160Ah): the major and minor version GetVersion gives.
pub fn windows_version(cpu: &Cpu) -> (u8, u8) {
    let info = super::version::info(super::version::current(cpu));
    (info.major as u8, info.minor as u8)
}

/// Whether the file `file` at `path` is a DOS program, which a Windows
/// program starts in a DOS machine: an MZ file that is neither a PE file
/// nor a Windows 3.1 program (an NE file for OS/2, as Borland's DPMI16
/// programs are, is one), a COM file, a batch file, or a PIF, which
/// names the DOS program it starts.
pub fn is_program(path: &str, file: &[u8]) -> bool {
    if is_batch(path) || is_pif(path) {
        return true;
    }
    if rust_dos_pe::is_pe(file) || super::launcher::is_win16(file) {
        return false;
    }
    file.starts_with(b"MZ") || file.starts_with(b"ZM") || has_extension(path, "COM")
}

/// Whether `path` names a batch file.
fn is_batch(path: &str) -> bool {
    has_extension(path, "BAT")
}

/// Whether `path` names a PIF.
fn is_pif(path: &str) -> bool {
    has_extension(path, "PIF")
}

/// What a start of a DOS program runs: the program, the arguments that
/// follow its name (from the blank before them) and the folder it starts
/// in.
struct Start {
    program: String,
    arguments: String,
    directory: String,
}

/// The program the PIF at `path` starts (pif.rs), as Windows 98 starts
/// it: the program the PIF names, looked for in the folder it starts
/// in, the PIF's folder and then where SearchPathA looks, with
/// `arguments` if there are any, else the PIF's parameters, in the
/// PIF's working folder, else in `directory`. The error, as
/// CreateProcessA gives it, when the PIF can't be read or the program
/// isn't a DOS program that is there.
fn open_pif(cpu: &mut Cpu, path: &str, arguments: &str, directory: String) -> Result<Start, u32> {
    use super::kernel32::{files, path as paths};
    const ERROR_FILE_NOT_FOUND: u32 = 2;
    const ERROR_BAD_EXE_FORMAT: u32 = 193;
    let data = files::read_whole(cpu, path)?;
    let Some(pif) = super::pif::parse(&data) else {
        cpu.bus
            .log_string(&format!("[W32] {} can't start: it isn't a PIF", path));
        return Err(ERROR_BAD_EXE_FORMAT);
    };
    let mut directory = directory;
    if !pif.directory.is_empty() {
        let full = paths::full_path(cpu, &pif.directory);
        if super::kernel32::is_directory(cpu, &full) {
            directory = full;
        } else {
            cpu.bus.log_string(&format!(
                "[W32] {}'s working folder {} isn't there: the program starts in {}",
                path, pif.directory, directory
            ));
        }
    }
    let folder = path.rsplit_once('\\').map_or("", |(folder, _)| folder);
    let dirs = format!("{};{}", directory, folder);
    // A name without an extension is a program DOS finds as COMMAND.COM
    // does: a COM file, an EXE file or a batch file.
    let name = pif.program.rsplit(['\\', '/']).next().unwrap_or("");
    let names: Vec<String> = if name.contains('.') {
        vec![pif.program.clone()]
    } else {
        ["COM", "EXE", "BAT"]
            .iter()
            .map(|e| format!("{}.{}", pif.program, e))
            .collect()
    };
    let found = names.iter().find_map(|name| {
        files::search(cpu, Some(&dirs), name).or_else(|| files::search(cpu, None, name))
    });
    let Some(found) = found else {
        cpu.bus.log_string(&format!(
            "[W32] {} can't start: its program {} isn't there",
            path, pif.program
        ));
        return Err(ERROR_FILE_NOT_FOUND);
    };
    let program = cpu.program_path(&paths::dos_path(&found));
    let file = cpu.read_program_file(&program);
    let file = file.ok_or(ERROR_FILE_NOT_FOUND)?;
    if is_pif(&program) || !is_program(&program, &file) {
        cpu.bus.log_string(&format!(
            "[W32] {} can't start: its program {} isn't a DOS program",
            path, program
        ));
        return Err(ERROR_BAD_EXE_FORMAT);
    }
    let arguments = if !arguments.trim().is_empty() {
        arguments.to_string()
    } else if pif.parameters == "?" {
        // Windows asks for the parameters in a dialog box.
        cpu.bus.log_string(&format!(
            "[W32] {} asks for parameters: the program starts without any",
            path
        ));
        String::new()
    } else if pif.parameters.is_empty() {
        String::new()
    } else {
        format!(" {}", pif.parameters)
    };
    cpu.bus.log_string(&format!(
        "[W32] {} starts {}{} in {}",
        path, program, arguments, directory
    ));
    Ok(Start {
        program,
        arguments,
        directory,
    })
}

fn has_extension(path: &str, extension: &str) -> bool {
    let name = path.rsplit(['\\', '/', ':']).next().unwrap_or(path);
    name.rsplit_once('.')
        .is_some_and(|(_, e)| e.eq_ignore_ascii_case(extension))
}

/// What `spawn` made: the process's and the thread's objects and ids.
pub struct Spawned {
    pub process: (u32, u32),
    pub thread: (u32, u32),
}

/// Start the DOS program at `path` with the command line's `arguments`
/// (what follows the program's name, from the blank before them) and
/// `environment`, in the folder `directory` or else the current one: it
/// runs when the service that started it returns (`start_pending`). A
/// PIF starts the program it names, with its settings (`open_pif`). Its
/// process's and thread's objects, which the session holds; or the
/// error: ERROR_NOT_ENOUGH_MEMORY when there is no conventional memory
/// for what EXEC is given.
pub fn spawn(
    cpu: &mut Cpu,
    path: &str,
    arguments: &str,
    environment: &[String],
    directory: Option<&str>,
) -> Result<Spawned, u32> {
    const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
    const ERROR_FILENAME_EXCED_RANGE: u32 = 206;
    if cpu.bus.win32.dosvm.pending.is_some() {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    // The folder is taken now: ShellExecute changes to a link's working
    // folder around its CreateProcessA and back before the program
    // starts.
    let directory = match directory {
        Some(directory) => super::kernel32::path::full_path(cpu, directory),
        None => super::kernel32::files::current_directory(cpu),
    };
    let start = if is_pif(path) {
        open_pif(cpu, path, arguments, directory)?
    } else {
        Start {
            program: path.to_string(),
            arguments: arguments.to_string(),
            directory,
        }
    };
    let (path, arguments) = (start.program.as_str(), start.arguments.as_str());
    // The DOS machine gets the program's 8.3 path, as on Windows 98, so
    // that a folder with blanks in its long name splits no command line.
    let short = super::kernel32::path::short_path(cpu, path);
    // A batch file runs in COMMAND.COM, as Windows 98 runs one.
    let (program, tail) = if is_batch(path) {
        (
            COMMAND_COM.to_string(),
            format!(" /C {}{}", short, arguments),
        )
    } else {
        (short, arguments.to_string())
    };
    let mut name = crate::dosstr::to_bytes(&program);
    name.push(0);
    if name.len() > MAX_PATH {
        return Err(ERROR_FILENAME_EXCED_RANGE);
    }
    let mut tail = crate::dosstr::to_bytes(&tail);
    if tail.len() > MAX_TAIL {
        cpu.bus.log_string(&format!(
            "[W32] {}'s command line is longer than DOS takes: {} bytes of it go",
            path,
            tail.len() - MAX_TAIL
        ));
        tail.truncate(MAX_TAIL);
    }
    let mut env = Vec::new();
    for s in environment {
        env.extend(crate::dosstr::to_bytes(s));
        env.push(0);
    }
    if env.is_empty() {
        env.push(0);
    }
    env.push(0);
    let paras = (ENVIRONMENT as usize + env.len()).div_ceil(16) as u16;
    let block = super::dos(cpu, |cpu| {
        shrink_dos_process(cpu);
        let psp = cpu.bus.win32.psp;
        crate::mcb::alloc(&mut cpu.bus, psp, paras).ok()
    });
    let Some(block) = block else {
        cpu.bus.log_string(&format!(
            "[W32] {} can't start: there isn't enough conventional memory",
            path
        ));
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    };
    let base = (block as u32) << 4;
    let mut params = [0u8; 14];
    let words = [
        block + (ENVIRONMENT >> 4),
        TAIL,
        block,
        FCB1,
        block,
        FCB2,
        block,
    ];
    for (i, word) in words.iter().enumerate() {
        params[2 * i..2 * i + 2].copy_from_slice(&word.to_le_bytes());
    }
    // FCBs with no drive and a blank name, as for a command tail with
    // no file names.
    let mut fcb = [0u8; 16];
    fcb[1..12].fill(b' ');
    let mut tail_bytes = vec![tail.len() as u8];
    tail_bytes.extend(&tail);
    tail_bytes.push(0x0D);
    super::dos(cpu, |cpu| {
        let bus = &mut cpu.bus;
        bus.guest_write_bytes(base + PARAMS as u32, &params);
        bus.guest_write_bytes(base + FCB1 as u32, &fcb);
        bus.guest_write_bytes(base + FCB2 as u32, &fcb);
        bus.guest_write_bytes(base + TAIL as u32, &tail_bytes);
        bus.guest_write_bytes(base + PATH as u32, &name);
        bus.guest_write_bytes(base + ENVIRONMENT as u32, &env);
    });
    let win32 = &mut cpu.bus.win32;
    let (pid, tid) = (win32.new_id(), win32.new_id());
    let objects = &mut win32.objects;
    let process = objects.add(
        object::Kind::Process {
            id: pid,
            exit_code: None,
        },
        None,
    );
    let thread = objects.add(
        object::Kind::Thread {
            id: tid,
            exit_code: None,
        },
        None,
    );
    win32.dosvm.pending = Some(Session {
        path: path.to_string(),
        process,
        thread,
        block,
        directory: crate::savestate::Since(start.directory.clone()),
        ..Session::default()
    });
    cpu.bus.log_string(&format!(
        "[W32] {} is a DOS program: it runs full screen, and the Windows programs wait for it",
        path
    ));
    Ok(Spawned {
        process: (process, pid),
        thread: (thread, tid),
    })
}

/// Give the DOS process's memory block all of conventional memory but
/// what it uses: the PSP, the DOS stub and the stack its stub had, which
/// the real-mode code the system calls runs on. The program DOS loaded
/// has nearly all of it otherwise.
pub(crate) fn shrink_dos_process(cpu: &mut Cpu) {
    let psp = cpu.bus.win32.psp;
    let (ss, sp) = cpu.bus.win32.rm_stack;
    let keep = (ss as u32 + (sp as u32 >> 4) + 1).saturating_sub(psp as u32) + 0x10;
    let keep = keep.max(0x20) as u16;
    let _ = crate::mcb::resize(&mut cpu.bus, psp, keep);
}

/// The service that started a DOS program returned (or the program's
/// code it called is about to go on): the program starts, and the
/// caller goes on after it ends. Called once the service is done, when
/// nothing of it can be undone for a page fault.
pub fn start_pending(cpu: &mut Cpu) {
    if cpu.bus.win32.dosvm.pending.is_none() {
        return;
    }
    // Real-mode code the system called for the program runs or waits:
    // the program starts when the next service returns.
    if !cpu.pm() || realmode::waiting(cpu) {
        return;
    }
    let Some(mut session) = cpu.bus.win32.dosvm.pending.take() else {
        return;
    };
    let ctx = Context::of(cpu);
    let mut vectors = vec![0u8; VECTORS];
    super::dos(cpu, |cpu| cpu.bus.guest_read_bytes(0, &mut vectors));
    session.vectors = vectors;
    // The memory the Windows programs can do without goes to the DOS
    // program's DPMI and XMS memory while it runs. Pages of theirs moved
    // for it, so the translations the TLB has of them are stale.
    *session.lent = super::memory::Pool::lend(&mut cpu.bus);
    if std::mem::take(&mut cpu.bus.win32.tlb_stale) {
        cpu.tlb.flush();
    }
    session.desktop = super::user32::user(cpu).desktop;
    if session.desktop {
        super::user32::paint::leave_desktop(cpu);
    }
    session.psp = cpu.current_psp;
    session.directories = Directories::now(cpu);
    // The program starts in its folder, and the one before is put back
    // when it ends (`ended`), as on Windows 98 the DOS machine has
    // current folders of its own.
    if !session.directory.is_empty()
        && super::kernel32::files::change_directory(cpu, &session.directory).is_err()
    {
        cpu.bus.log_string(&format!(
            "[W32] {} can't start in {}: it isn't there",
            session.path, *session.directory
        ));
    }
    session.machine = Machine::now(cpu);
    cpu.current_psp = cpu.bus.win32.psp;
    let (ss, sp) = realmode::host_stack(cpu);
    let block = session.block;
    let mut rm = Context {
        eip: super::DOS_EXEC as u32,
        eflags: 0x0202,
        seg: [block, ROM_SEG, ss, block, 0, 0],
        ..Context::default()
    };
    rm.gpr[EAX] = 0x4B00;
    rm.gpr[EBX] = PARAMS as u32;
    rm.gpr[EDX] = PATH as u32;
    rm.gpr[ESP] = sp as u32;
    let end = cpu.bus.ram().len() as u32;
    let (_, free) = cpu.bus.xms.free_kb(end);
    cpu.bus.log_string(&format!(
        "[W32] {} starts, with {free} KB of extended memory free ({} KB lent by the \
         Windows programs)",
        session.path,
        *session.lent / 1024
    ));
    cpu.bus.win32.dosvm.running = Some(session);
    realmode::call(cpu, Frame::new(Kind::Exec, ctx), &rm, &[]);
}

/// The service that started a DOS program found a page missing and runs
/// again once it is there: the program it started doesn't, and the
/// service starts another.
pub fn discard_pending(cpu: &mut Cpu) {
    let Some(session) = cpu.bus.win32.dosvm.pending.take() else {
        return;
    };
    let _ = super::dos(cpu, |cpu| crate::mcb::free(&mut cpu.bus, session.block));
    let objects = &mut cpu.bus.win32.objects;
    for id in [session.process, session.thread] {
        objects.release(id);
    }
}

/// DOS's EXEC of the DOS program returned, with the real-mode context
/// `rm`: the program ended, or couldn't start. Its objects get its exit
/// code, what it may have left is put back, and the program that started
/// it goes on, in the context `frame` kept.
pub fn ended(cpu: &mut Cpu, frame: Frame, rm: &Context) {
    let Some(session) = cpu.bus.win32.dosvm.running.take() else {
        return;
    };
    let code = if rm.eflags & CpuFlags::CF.bits() != 0 {
        cpu.bus.log_string(&format!(
            "[W32] {} can't start: DOS error {:X}",
            session.path,
            rm.gpr[EAX] & 0xFFFF
        ));
        1
    } else {
        // AH=4Dh's AL: the exit code it gave.
        let code = (cpu.last_child_exit & 0xFF) as u32;
        cpu.bus.log_string(&format!(
            "[W32] {} ended with exit code {:X}",
            session.path, code
        ));
        code
    };
    super::dos(cpu, |cpu| {
        cpu.bus.guest_write_bytes(0, &session.vectors);
        let _ = crate::mcb::free(&mut cpu.bus, session.block);
    });
    cpu.current_psp = session.psp;
    super::boot::put_back(cpu, session.directories);
    session.machine.put_back(cpu);
    if !super::memory::Pool::take_back(&mut cpu.bus, *session.lent) {
        cpu.bus.log_string(&format!(
            "[W32] {} left extended memory taken: the Windows programs have {} KB less",
            session.path,
            *session.lent / 1024
        ));
    }
    cpu.bus.set_a20(true);
    let objects = &mut cpu.bus.win32.objects;
    for id in [session.process, session.thread] {
        if let Some(object) = objects.get_mut(id)
            && let object::Kind::Process { exit_code, .. } | object::Kind::Thread { exit_code, .. } =
                &mut object.kind
        {
            *exit_code = Some(code);
        }
        objects.release(id);
    }
    super::system::enter_user(cpu, &frame.ctx);
    if session.desktop {
        // The desktop's painting reaches the programs' memory through
        // their page tables.
        cpu.bus.guest_paging = Some(cpu.guest_paging(true));
        super::user32::paint::ensure_desktop(cpu);
        cpu.bus.guest_paging = None;
    }
}

crate::state_fields!(Session {
    path,
    process,
    thread,
    block,
    vectors,
    desktop,
    psp,
    directories,
    machine,
    directory,
    lent,
});
crate::state_fields!(Machine {
    fpu,
    sse,
    pic,
    pit,
    mouse,
    memory,
});
crate::state_fields!(State { pending, running });
