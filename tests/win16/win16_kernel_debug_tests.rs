//! Win16 KERNEL's debugging and task-switching calls that retail
//! Windows 3.1 and 95 answer quietly: _DebugOutput, which writes nothing
//! there, and DirectedYield. The programs are made byte by byte
//! (tests/common/win16).

use crate::win16;

use win16::{CODE, Code, program, run};

/// KERNEL's ordinals.
const GET_CURRENT_TASK: u16 = 36;
const DIRECTED_YIELD: u16 = 150;
const DEBUG_OUTPUT: u16 = 328;

#[test]
fn debug_output_formats_its_message_into_the_log_and_returns() {
    // _DebugOutput(DBF_ERROR, "count %d of %s", 42, "MCLIB"), cdecl:
    // the caller takes its 14 bytes of arguments off the stack. Windows'
    // retail KERNEL returns without a word; Memphis Math's MCLIB.DLL calls
    // it with its own warnings and goes on. The program ends with 1 once
    // it is back, with SP where it was.
    let mut code = Code::new();
    code.emit(&[0x89, 0xE5]); // mov bp,sp
    let name_at = code.here() + 2;
    code.push_far(0); // the %s argument
    code.push(42); // the %d argument
    let format_at = code.here() + 2;
    code.push_far(0)
        .push(0x8000) // DBF_ERROR
        .call("KERNEL", DEBUG_OUTPUT)
        .emit(&[0x83, 0xC4, 0x0C]) // add sp,12
        .emit(&[
            0x39, 0xE5, // cmp bp,sp
            0xB0, 0x01, // mov al,1
            0x74, 0x02, // je over the next
            0xB0, 0x10, // mov al,10h
        ])
        .exit_al();
    let format = code.here();
    code.emit(b"count %d of %s\0");
    let name = code.here();
    code.emit(b"MCLIB\0");
    code.patch(format_at, format);
    code.patch(name_at, name);
    let (exit, log) = run("debug_output", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
    assert!(
        log.contains("[W16] _DebugOutput (error, flags 8000): count 42 of MCLIB"),
        "{log}"
    );
}

#[test]
fn debug_output_names_each_severity() {
    // The flags' top two bits: DBF_TRACE, DBF_WARNING, DBF_ERROR and
    // DBF_FATAL. A fatal message stops the debug KERNEL only; the retail
    // one returns, and so does this one.
    let mut code = Code::new();
    let mut formats = Vec::new();
    for flags in [0x0000u16, 0x4000, 0x8000, 0xC000] {
        formats.push(code.here() + 2);
        code.push_far(0)
            .push(flags)
            .call("KERNEL", DEBUG_OUTPUT)
            .emit(&[0x83, 0xC4, 0x06]); // add sp,6
    }
    code.emit(&[0xB0, 0x01]).exit_al(); // mov al,1
    let text = code.here();
    code.emit(b"text\0");
    for at in formats {
        code.patch(at, text);
    }
    let (exit, log) = run("debug_output_levels", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
    for line in [
        "(trace, flags 0000): text",
        "(warning, flags 4000): text",
        "(error, flags 8000): text",
        "(fatal, flags C000): text",
    ] {
        assert!(log.contains(line), "{line} missing:\n{log}");
    }
}

#[test]
fn directed_yield_returns_to_the_caller_whatever_the_task() {
    // DirectedYield to the running task, to no task (0) and to a handle
    // that is no task's: each gives up the turn and comes back, as Yield
    // does, with the task going on after the call. Memphis Math's login
    // calls it with the task of the story it closes. The program ends
    // with 1, having come back from all three.
    let mut code = Code::new();
    code.call("KERNEL", GET_CURRENT_TASK)
        .emit(&[0x50]) // push ax
        .call("KERNEL", DIRECTED_YIELD)
        .push(0)
        .call("KERNEL", DIRECTED_YIELD)
        .push(0x1234)
        .call("KERNEL", DIRECTED_YIELD)
        .emit(&[0xB0, 0x01]) // mov al,1
        .exit_al();
    let (exit, log) = run("directed_yield", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
    assert!(!log.contains("doesn't have"), "{log}");
}
