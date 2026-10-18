//! `rust-dos run`: a DOS program or command line run as a tool of the host,
//! without a window: what DOS prints goes to the host's standard output
//! (handle 2 to its standard error), the host's standard input is typed at
//! the keyboard, the folder given (the current one by default) is drive C:,
//! and the program's exit code is the process's.

use crate::config::Settings;
use crate::cpu::{Cpu, HostConsole};
use crate::exec::{self, NoHook, StopReason};
use crate::shell::ShellWait;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// What `rust-dos run` was given.
pub struct RunOptions {
    /// The command line: a program, a batch file or a built-in command and
    /// its parameters.
    pub command: Vec<String>,
    /// Drive C:'s folder.
    pub dir: PathBuf,
    /// Instructions per emulated millisecond.
    pub cycles: u32,
    /// The machine's memory in MB.
    pub memory_mb: usize,
    /// Print the text screen at the end if nothing went through DOS's
    /// console: for programs that write to the screen's memory.
    pub screen: bool,
}

/// The exit code when the program waits for input after the host's
/// standard input has ended, or the machine doesn't get to the end.
const ABORTED: i32 = 255;

/// The exit code when the command's program isn't there, as a Unix shell's.
const NOT_FOUND: i32 = 127;

/// How long a program may wait for keys after standard input ended, in
/// emulated milliseconds.
const STARVED_MS: u32 = 1000;

/// The longest a wait for keys is apart from the next one while the
/// program still waits, in emulated milliseconds.
const STARVED_GAP_MS: u32 = 200;

/// The Enter key's keystroke.
const ENTER: u16 = 0x1C0D;

/// Run `options.command` to its end; the exit code it leaves.
pub fn run(options: RunOptions) -> i32 {
    if !options.dir.is_dir() {
        eprintln!("rust-dos: {} is not a folder", options.dir.display());
        return ABORTED;
    }
    let mut cpu = Box::new(Cpu::with_memory(options.dir.clone(), options.memory_mb));
    let settings = Settings { memsize: options.memory_mb, ..Settings::default() };
    for warning in crate::hardware::configure(&mut cpu, &settings, crate::keylayout::Layout::us()) {
        cpu.bus.log_string(&format!("[RUN] {}", warning));
    }
    cpu.bus.set_cycles_per_ms(options.cycles);
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    // The command as a batch file of its own, without ECHO, ending the
    // session after it.
    let line = options.command.join(" ");
    cpu.queue_batch_lines(["@ECHO OFF", line.as_str(), "EXIT"]);

    let input = stdin_reader();
    let mut pending: VecDeque<u16> = VecDeque::new();
    let mut last_byte = b'\n';
    // The batches since the program first waited for a key after the
    // end of the input, and since it last did.
    let mut starved: Option<(u32, u32)> = None;
    let mut printed = false;
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
    let code = loop {
        // The host's keys, as many as the keyboard buffer takes.
        let console = cpu.host_console.as_mut().expect("the host's console");
        while !console.input_ended {
            match input.try_recv() {
                Ok(bytes) => {
                    last_byte = *bytes.last().unwrap_or(&last_byte);
                    pending.extend(bytes.into_iter().filter_map(keystroke));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // A last line without its line end ends with Enter.
                    if last_byte != b'\n' {
                        pending.push_back(ENTER);
                    }
                    console.input_ended = true;
                }
            }
        }
        while let Some(&key) = pending.front() {
            if !crate::keyboard::queue_keystroke(&mut cpu.bus, key) {
                break;
            }
            pending.pop_front();
        }

        let waits = cpu.bus.key_waits;
        let end = cpu.bus.clock.icount + options.cycles as u64;
        cpu.bus.start_batch(end);
        let stop = exec::run_batch(&mut cpu, &mut NoHook, false);

        let console = cpu.host_console.as_mut().expect("the host's console");
        printed |= !console.stdout.is_empty() || !console.stderr.is_empty();
        let _ = out.write_all(&std::mem::take(&mut console.stdout));
        let _ = err.write_all(&std::mem::take(&mut console.stderr));
        let _ = out.flush();
        if stop == StopReason::Exit {
            break if console.not_found { NOT_FOUND } else { cpu.errorlevel as i32 };
        }

        // Waiting for a key that won't come: a wait in the keyboard
        // services is tried again with each interrupt, not all the time.
        // A built-in command's wait (PAUSE, CHOICE, MORE) is the shell's.
        let shell_waits = matches!(
            cpu.shell_wait,
            Some(ShellWait::Pause | ShellWait::Choice(_) | ShellWait::Line(_) | ShellWait::MakeImg(_))
                | Some(ShellWait::More(_) | ShellWait::Edit(_) | ShellWait::Rc(_))
        );
        let waited = console.input_ended
            && pending.is_empty()
            && (cpu.bus.key_waits != waits || shell_waits && crate::keyboard::peek_keystroke(&mut cpu.bus).is_none());
        starved = match starved {
            _ if waited => Some(starved.map_or((0, 0), |(since, _)| (since + 1, 0))),
            Some((since, last)) if last < STARVED_GAP_MS => Some((since + 1, last + 1)),
            _ => None,
        };
        if starved.is_some_and(|(since, _)| since > STARVED_MS) {
            let _ = writeln!(err, "rust-dos: the program waits for input after the end of standard input");
            break ABORTED;
        }
    };
    if options.screen && !printed {
        let _ = out.write_all(screen_text(&cpu).as_bytes());
    }
    let _ = out.flush();
    code
}

/// A byte of the host's input as the keystroke that types it: Enter for a
/// line's end (LF, the CR of a CR LF typing nothing).
fn keystroke(byte: u8) -> Option<u16> {
    match byte {
        b'\r' => None,
        b'\n' => Some(ENTER),
        _ => {
            let scan = crate::keyboard::char_to_key(byte as char).map_or(0, |(key, _)| key.scan);
            Some(crate::keyboard::bios_keystroke(scan, byte, 0))
        }
    }
}

/// The host's standard input, read on a thread of its own as it comes.
fn stdin_reader() -> Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// The text screen's lines, without the empty ones at the bottom.
fn screen_text(cpu: &Cpu) -> String {
    let lines = crate::video::text::screen_lines(&cpu.bus).unwrap_or_default();
    let used = lines.iter().rposition(|line| !line.is_empty()).map_or(0, |i| i + 1);
    lines[..used].iter().map(|line| format!("{}\n", line)).collect()
}
