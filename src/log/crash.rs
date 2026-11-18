//! A crash's reason in the log file (`LogFile::report_crashes`).
//!
//! The panic hook writes a panic's message and its place in the source,
//! then hands the panic to the hook before it, which prints it. On
//! Windows a top-level exception filter writes the exception's code and
//! the module and offset where it happened, such as an access violation
//! in a graphics driver's DLL, and lets Windows go on to end the
//! program. Both write the lines the log still buffers first, so the
//! reason comes after the last thing the emulator did.

use super::Lines;
use std::sync::{Mutex, Once, TryLockError, Weak};

/// The log that crashes are written to, while it is open: the log file
/// that owns it writes out what it buffers when it closes.
static TARGET: Mutex<Weak<Mutex<Lines>>> = Mutex::new(Weak::new());

/// Write crashes to `lines` from now on.
pub(super) fn report_to(lines: Weak<Mutex<Lines>>) {
    *TARGET.lock().unwrap_or_else(|p| p.into_inner()) = lines;
    static HOOKS: Once = Once::new();
    HOOKS.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            write(&panic_line(info));
            previous(info);
        }));
        #[cfg(windows)]
        windows::install();
    });
}

/// Add `line` to the log and write the log out. A log that is in use
/// (the crash came while a line was being written) or not set up is
/// left as it is: waiting for it could hang the crash.
fn write(line: &str) {
    let target = match TARGET.try_lock() {
        Ok(target) => target.clone(),
        Err(TryLockError::Poisoned(p)) => p.into_inner().clone(),
        Err(TryLockError::WouldBlock) => return,
    };
    let Some(lines) = target.upgrade() else {
        return;
    };
    let mut lines = match lines.try_lock() {
        Ok(lines) => lines,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        Err(TryLockError::WouldBlock) => return,
    };
    lines.write_line(line);
    lines.flush();
}

/// The log line of a panic: its message and where it happened.
fn panic_line(info: &std::panic::PanicHookInfo) -> String {
    let message = info.payload_as_str().unwrap_or("(no message)");
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("unnamed");
    match info.location() {
        Some(at) => format!(
            "[CRASH] Panic in thread '{thread}' at {}:{}: {message}",
            at.file(),
            at.line()
        ),
        None => format!("[CRASH] Panic in thread '{thread}': {message}"),
    }
}

#[cfg(windows)]
mod windows {
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        EXCEPTION_POINTERS, LPTOP_LEVEL_EXCEPTION_FILTER, SetUnhandledExceptionFilter,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    };

    /// What the filter returns to have Windows go on as without it.
    const CONTINUE_SEARCH: i32 = 0;

    pub(super) fn install() {
        // SAFETY: `filter` is a function of the right signature that
        // lives as long as the program. The filter it replaces, if one
        // was set, is called from it.
        let previous = unsafe { SetUnhandledExceptionFilter(Some(filter)) };
        let _ = PREVIOUS.set(previous);
    }

    /// The filter set before `install`'s.
    static PREVIOUS: OnceLock<LPTOP_LEVEL_EXCEPTION_FILTER> = OnceLock::new();

    unsafe extern "system" fn filter(pointers: *const EXCEPTION_POINTERS) -> i32 {
        // SAFETY: Windows passes the exception's pointers, which hold
        // while the filter runs.
        let record = unsafe { pointers.as_ref().and_then(|p| p.ExceptionRecord.as_ref()) };
        if let Some(record) = record {
            let code = record.ExceptionCode as u32;
            let address = record.ExceptionAddress as usize;
            let mut line = format!(
                "[CRASH] Exception {code:08X}h ({}) at {}",
                exception_name(code),
                place(address)
            );
            // An access violation's second number is the address that
            // was read or written.
            if code == 0xC000_0005 && record.NumberParameters >= 2 {
                let access = match record.ExceptionInformation[0] {
                    0 => "reading",
                    1 => "writing",
                    8 => "executing",
                    _ => "using",
                };
                line += &format!(", {access} {:#x}", record.ExceptionInformation[1]);
            }
            super::write(&line);
        }
        match PREVIOUS.get().copied().flatten() {
            // SAFETY: the filter installed before, given the pointers
            // Windows passed this one.
            Some(previous) => unsafe { previous(pointers) },
            None => CONTINUE_SEARCH,
        }
    }

    /// The name of the exceptions a crash is most likely to have.
    fn exception_name(code: u32) -> &'static str {
        match code {
            0xC000_0005 => "access violation",
            0xC000_001D => "illegal instruction",
            0xC000_0094 => "integer division by zero",
            0xC000_00FD => "stack overflow",
            0xC000_0409 => "stack buffer overrun",
            0xC000_0374 => "heap corruption",
            0x8000_0003 => "breakpoint",
            _ => "unknown",
        }
    }

    /// `address` as the module it is in and the offset in it, such as
    /// `nvoglv64.dll+0x1a2b3c`, or the address alone.
    fn place(address: usize) -> String {
        let mut module: HMODULE = std::ptr::null_mut();
        let flags =
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        // SAFETY: with FROM_ADDRESS the name is read as an address,
        // which isn't dereferenced; `module` is a place for the handle.
        let found = unsafe { GetModuleHandleExW(flags, address as *const u16, &mut module) } != 0;
        if !found {
            return format!("{address:#x}");
        }
        let mut name = [0u16; 260];
        // SAFETY: `name` holds the 260 characters it is said to.
        let len = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) };
        let path = String::from_utf16_lossy(&name[..len as usize]);
        let file = path.rsplit(['\\', '/']).next().unwrap_or(&path);
        format!("{file}+{:#x}", address - module as usize)
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn an_address_in_a_module_is_named_by_the_module_and_offset() {
            // A function of the test program itself.
            let address = super::place as *const () as usize;
            let place = super::place(address);
            assert!(place.contains(".exe+0x"), "{place}");
            assert_eq!(super::place(0), "0x0");
        }

        /// The test runs itself again with this set to the log's path,
        /// and that run crashes.
        const CHILD: &str = "RUST_DOS_TEST_CRASH_LOG";

        #[test]
        fn an_access_violation_is_written_to_the_log() {
            if let Some(path) = std::env::var_os(CHILD) {
                use windows_sys::Win32::System::Diagnostics::Debug::{
                    RaiseException, SEM_NOGPFAULTERRORBOX, SetErrorMode,
                };
                let mut log = crate::log::LogFile::create(std::path::Path::new(&path)).unwrap();
                log.write_line("[TOADFORCE] Setting up the OpenGL renderer");
                log.report_crashes();
                // SAFETY: no Windows Error Reporting window for the crash.
                unsafe { SetErrorMode(SEM_NOGPFAULTERRORBOX) };
                // An access violation reading address 8, as Windows
                // raises it, without the undefined behaviour of a read.
                let arguments = [0usize, 8];
                // SAFETY: the arguments are the two numbers given.
                unsafe { RaiseException(0xC000_0005, 0, 2, arguments.as_ptr()) };
                unreachable!("the exception came back");
            }
            let path = crate::test_dir::path("log-access-violation");
            let name = "log::crash::windows::tests::an_access_violation_is_written_to_the_log";
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", name, "--test-threads=1"])
                .env(CHILD, &path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let _ = std::fs::remove_file(&path);
            assert_eq!(status.code(), Some(0xC000_0005_u32 as i32), "{text}");
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines.len(), 3, "{text}");
            assert_eq!(lines[1], "[TOADFORCE] Setting up the OpenGL renderer");
            let crash = lines[2];
            assert!(
                crash.starts_with("[CRASH] Exception C0000005h (access violation) at "),
                "{crash}"
            );
            // RaiseException's own module raises it.
            assert!(crash.to_ascii_lowercase().contains(".dll+0x"), "{crash}");
            assert!(crash.ends_with(", reading 0x8"), "{crash}");
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_panic_is_written_to_the_log_after_the_lines_before_it() {
        let path = crate::test_dir::path("log-crash");
        let mut log = super::super::LogFile::create(&path).unwrap();
        log.write_line("[TOADFORCE] before");
        log.report_crashes();
        // The panic hook is the process's: other tests' panics may write
        // to this log too while it is the target, so only the order of
        // this test's lines is checked.
        let result = std::thread::Builder::new()
            .name("crasher".into())
            .spawn(|| panic!("the renderer failed"))
            .unwrap()
            .join();
        assert!(result.is_err());
        drop(log);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let before = text.find("[TOADFORCE] before").expect("the line before");
        let crash = text
            .find("[CRASH] Panic in thread 'crasher' at ")
            .unwrap_or_else(|| panic!("no crash line: {text}"));
        assert!(before < crash, "{text}");
        assert!(text[crash..].contains("crash.rs:"), "{text}");
        assert!(text[crash..].contains(": the renderer failed\n"), "{text}");
    }
}
