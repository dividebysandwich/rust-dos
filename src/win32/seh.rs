//! Structured exceptions: processor faults and RaiseException become an
//! EXCEPTION_RECORD and a CONTEXT on the thread's stack, and the handlers
//! on the `fs:[0]` chain are called one after the other as Windows'
//! dispatcher calls them, until one continues the program at a context
//! or none is left. Then the filter SetUnhandledExceptionFilter set gets
//! its say, and the process ends with a message. RtlUnwind calls the
//! handlers again, unwinding, on the way to a frame. The handlers are the
//! program's code (MSVC's `_except_handler3`, C++'s), called through
//! `call` (see call.rs).
//!
//! A page fault in the part of a thread's stack that is reserved but not
//! committed yet commits it, and the program goes on: stacks grow as on
//! Windows. A write to a module's resources, which are read-only, makes
//! their page writable and goes through, as on Windows 98 (VirtualQuery
//! says PAGE_READONLY and IsBadWritePtr TRUE, and the write still works).
//! InstallShield's engine changes the bitmaps it loads in place.

use super::call::{self, Call, SEH_FILTER, SEH_HANDLER, SEH_UNWIND};
use super::memory::{PAGE, PAGE_READWRITE, USER, WRITABLE};
use super::{Context, ESP, process};
use crate::cpu::{Cpu, Seg};
use crate::instructions::fpu::control;

/// Exception codes.
pub const ACCESS_VIOLATION: u32 = 0xC000_0005;
const NONCONTINUABLE_EXCEPTION: u32 = 0xC000_0025;
const UNWIND: u32 = 0xC000_0027;
const BREAKPOINT: u32 = 0x8000_0003;
const SINGLE_STEP: u32 = 0x8000_0004;
const ARRAY_BOUNDS_EXCEEDED: u32 = 0xC000_008C;
const FLT_DENORMAL_OPERAND: u32 = 0xC000_008D;
const FLT_DIVIDE_BY_ZERO: u32 = 0xC000_008E;
const FLT_INEXACT_RESULT: u32 = 0xC000_008F;
const FLT_INVALID_OPERATION: u32 = 0xC000_0090;
const FLT_OVERFLOW: u32 = 0xC000_0091;
const FLT_STACK_CHECK: u32 = 0xC000_0092;
const FLT_UNDERFLOW: u32 = 0xC000_0093;
const INT_DIVIDE_BY_ZERO: u32 = 0xC000_0094;
const INT_OVERFLOW: u32 = 0xC000_0095;
const PRIV_INSTRUCTION: u32 = 0xC000_0096;
const ILLEGAL_INSTRUCTION: u32 = 0xC000_001D;
const STACK_OVERFLOW: u32 = 0xC000_00FD;

/// EXCEPTION_RECORD's flags.
const NONCONTINUABLE: u32 = 1;
const UNWINDING: u32 = 2;
const EXIT_UNWIND: u32 = 4;

/// What handlers return.
const CONTINUE_EXECUTION: u32 = 0;
/// What filters return.
const FILTER_EXECUTE_HANDLER: u32 = 1;
const FILTER_CONTINUE_EXECUTION: u32 = 0xFFFF_FFFF;

/// CONTEXT's size, its flags (CONTEXT_i386 with the control, integer,
/// segment and floating-point parts) and its fields' offsets.
pub const CONTEXT_SIZE: u32 = 0x2CC;
pub const CONTEXT_FULL: u32 = 0x1_000F;
const CONTEXT_CONTROL: u32 = 0x1;
const CONTEXT_INTEGER: u32 = 0x2;
const CONTEXT_SEGMENTS: u32 = 0x4;
const CONTEXT_FLOATING_POINT: u32 = 0x8;
const CTX_FLOAT: u32 = 0x1C;
const CTX_GS: u32 = 0x8C;
const CTX_EDI: u32 = 0x9C;
const CTX_EBP: u32 = 0xB4;
/// EXCEPTION_RECORD's size: room for 15 parameters.
const RECORD_SIZE: u32 = 0x50;
const MAX_PARAMETERS: u32 = 15;
/// Below the record on the stack: EXCEPTION_POINTERS and the dispatcher
/// context handlers get.
const BELOW_RECORD: u32 = 16;

/// The TIB's first field: the exception handler chain.
const TIB_EXCEPTION_LIST: u32 = 0x00;
const TIB_STACK_LIMIT: u32 = 0x08;
const END_OF_CHAIN: u32 = 0xFFFF_FFFF;

/// Write the context `ctx` and the FPU's state `fpu` as a CONTEXT at `at`,
/// with the flags `flags`.
pub fn write_context(cpu: &mut Cpu, at: u32, ctx: &Context, fpu: &[u8; 108], flags: u32) {
    cpu.bus.guest_fill(at, CONTEXT_SIZE as usize, 0);
    cpu.bus.guest_write_32(at, flags);
    cpu.bus.guest_write_bytes(at + CTX_FLOAT, fpu);
    // GS, FS, ES, DS; EDI, ESI, EBX, EDX, ECX, EAX; EBP, EIP, CS, EFLAGS,
    // ESP, SS.
    let s = |seg: Seg| ctx.seg[seg as usize] as u32;
    let g = |i: usize| ctx.gpr[i];
    let words = [
        s(Seg::GS),
        s(Seg::FS),
        s(Seg::ES),
        s(Seg::DS),
        g(7),
        g(6),
        g(3),
        g(2),
        g(1),
        g(0),
        g(5),
        ctx.eip,
        s(Seg::CS),
        ctx.eflags,
        g(ESP),
        s(Seg::SS),
    ];
    for (i, word) in words.into_iter().enumerate() {
        cpu.bus.guest_write_32(at + CTX_GS + 4 * i as u32, word);
    }
}

/// Change `ctx` and `fpu` to the parts the CONTEXT at `at` has by its
/// flags.
pub fn read_context(cpu: &mut Cpu, at: u32, ctx: &mut Context, fpu: &mut [u8; 108]) {
    let flags = cpu.bus.guest_read_32(at);
    let word = |cpu: &mut Cpu, offset: u32| cpu.bus.guest_read_32(at + offset);
    if flags & CONTEXT_SEGMENTS != 0 {
        for (i, seg) in [Seg::GS, Seg::FS, Seg::ES, Seg::DS].into_iter().enumerate() {
            ctx.seg[seg as usize] = word(cpu, CTX_GS + 4 * i as u32) as u16;
        }
    }
    if flags & CONTEXT_INTEGER != 0 {
        for (i, reg) in [7, 6, 3, 2, 1, 0].into_iter().enumerate() {
            ctx.gpr[reg] = word(cpu, CTX_EDI + 4 * i as u32);
        }
    }
    if flags & CONTEXT_CONTROL != 0 {
        ctx.gpr[5] = word(cpu, CTX_EBP);
        ctx.eip = word(cpu, CTX_EBP + 4);
        ctx.seg[Seg::CS as usize] = word(cpu, CTX_EBP + 8) as u16;
        ctx.eflags = word(cpu, CTX_EBP + 12);
        ctx.gpr[ESP] = word(cpu, CTX_EBP + 16);
        ctx.seg[Seg::SS as usize] = word(cpu, CTX_EBP + 20) as u16;
    }
    if flags & CONTEXT_FLOATING_POINT != 0 {
        cpu.bus.guest_read_bytes(at + CTX_FLOAT, fpu);
    }
}

/// Whether the `len` bytes at `lin` have pages behind them, looked up
/// without recording a page fault.
fn present(cpu: &Cpu, lin: u32, len: u32) -> bool {
    let process = super::running(cpu).expect("a Win32 process");
    let last = lin.wrapping_add(len.saturating_sub(1));
    last >= lin
        && (lin & !(PAGE - 1)..=last)
            .step_by(PAGE as usize)
            .all(|page| process.memory.translate(&cpu.bus, page).is_some())
}

/// A page fault at `address` in the running thread's stack, below what is
/// committed of it and above its floor: commit down to there. Whether it
/// was one.
fn grow_stack(cpu: &mut Cpu, address: u32) -> bool {
    let Some(thread) = process(cpu).current_thread() else {
        return false;
    };
    let (bottom, tib) = (thread.stack_bottom(), thread.tib);
    let Some(limit) = super::peek_32(cpu, tib + TIB_STACK_LIMIT) else {
        return false;
    };
    if !(bottom..limit).contains(&address) {
        return false;
    }
    let from = address & !(PAGE - 1);
    let grown = super::with_process(cpu, |p, bus| {
        p.memory.commit(bus, from, limit - from, USER | WRITABLE)?;
        p.memory.write_32(bus, tib + TIB_STACK_LIMIT, from)
    });
    grown.is_some()
}

/// A write fault at `address` in the resources of one of the running
/// process's modules: make the page writable. Whether it was one.
fn write_resource(cpu: &mut Cpu, address: u32) -> bool {
    let Some(process) = super::running(cpu) else {
        return false;
    };
    let in_resources = process
        .modules
        .iter()
        .filter(|m| (m.base..m.base.saturating_add(m.size)).contains(&address))
        .any(|m| {
            let peek = |at: u32| super::loader::peek_32(&cpu.bus, &process.memory, at);
            // The optional header's resource directory (the third).
            let directory = peek(m.base + 0x3C).and_then(|pe| {
                let at = m.base.wrapping_add(pe).wrapping_add(24 + 96 + 2 * 8);
                Some((peek(at)?, peek(at + 4)?))
            });
            directory.is_some_and(|(rva, size)| {
                let start = m.base.wrapping_add(rva);
                rva != 0 && (start..start.saturating_add(size)).contains(&address)
            })
        });
    let page = address & !(PAGE - 1);
    let made = in_resources
        && super::with_process(cpu, |p, bus| {
            p.memory.set_protection(bus, page, PAGE_READWRITE)
        });
    if made {
        cpu.tlb.flush_page(page);
    }
    made
}

/// The code and parameters of the exception a fault (`vector`, with the
/// error code `error`) is, and what it says in a message.
fn fault_exception(cpu: &Cpu, vector: u8, error: Option<u32>) -> (u32, Vec<u32>, String) {
    let name = match vector {
        0 => "divide error",
        1 => "debug exception",
        3 => "breakpoint",
        4 => "overflow",
        5 => "bound range exceeded",
        6 => "invalid opcode",
        7 => "coprocessor not available",
        8 => "double fault",
        12 => "stack fault",
        13 => "general protection fault",
        14 => "page fault",
        16 => "floating-point error",
        17 => "alignment check",
        19 => "SIMD floating-point exception",
        _ => "exception",
    };
    let mut what = format!("{} (exception {:02X}h", name, vector);
    if let Some(error) = error {
        what += &format!(", error {:X}", error);
    }
    if vector == 14 {
        what += &format!(", address {:08X}", cpu.cr2);
    }
    what += ")";
    let (code, params) = match vector {
        0 => (INT_DIVIDE_BY_ZERO, vec![]),
        1 => (SINGLE_STEP, vec![]),
        // One parameter, as Windows 98 and NT give it.
        3 => (BREAKPOINT, vec![0]),
        4 => (INT_OVERFLOW, vec![]),
        5 => (ARRAY_BOUNDS_EXCEEDED, vec![]),
        6 => (ILLEGAL_INSTRUCTION, vec![]),
        // Writing, as the error code's bit 1 says, and where.
        14 => (ACCESS_VIOLATION, vec![error.unwrap_or(0) >> 1 & 1, cpu.cr2]),
        13 if error.unwrap_or(0) == 0 && privileged(cpu) => (PRIV_INSTRUCTION, vec![]),
        16 => (fpu_exception(cpu), vec![]),
        _ => (ACCESS_VIOLATION, vec![0, 0xFFFF_FFFF]),
    };
    (code, params, what)
}

/// Whether the instruction that faulted is one only the system may run:
/// HLT, CLI, STI, IN or OUT, or a move to or from a control register.
fn privileged(cpu: &Cpu) -> bool {
    let process = super::running(cpu).expect("a Win32 process");
    let Some(phys) = process.memory.translate(&cpu.bus, cpu.eip()) else {
        return false;
    };
    let byte = cpu.bus.read_8(phys as usize);
    matches!(byte, 0xF4 | 0xFA | 0xFB | 0xE4..=0xE7 | 0xEC..=0xEF | 0x6C..=0x6F)
}

/// The exception code of an FPU error, from the status word's flags.
fn fpu_exception(cpu: &Cpu) -> u32 {
    let sw = cpu.get_fpu_flags().bits();
    let unmasked = sw & !cpu.fpu_control & 0x3F;
    match unmasked {
        _ if unmasked & 0x01 != 0 && sw & 0x40 != 0 => FLT_STACK_CHECK,
        _ if unmasked & 0x01 != 0 => FLT_INVALID_OPERATION,
        _ if unmasked & 0x04 != 0 => FLT_DIVIDE_BY_ZERO,
        _ if unmasked & 0x02 != 0 => FLT_DENORMAL_OPERAND,
        _ if unmasked & 0x08 != 0 => FLT_OVERFLOW,
        _ if unmasked & 0x10 != 0 => FLT_UNDERFLOW,
        _ => FLT_INEXACT_RESULT,
    }
}

/// A fault in the program, at `ctx`: an exception for its handlers.
pub fn fault(cpu: &mut Cpu, vector: u8, error: Option<u32>, ctx: Context) {
    if vector == 14 && process(cpu).current.is_some() && grow_stack(cpu, cpu.cr2) {
        super::system::enter_user(cpu, &ctx);
        return;
    }
    // A write (error code bit 1) to a page that is there (bit 0).
    if vector == 14 && error.is_some_and(|e| e & 3 == 3) && write_resource(cpu, cpu.cr2) {
        super::system::enter_user(cpu, &ctx);
        return;
    }
    let (code, params, what) = fault_exception(cpu, vector, error);
    let code = if vector == 14 && in_stack_reserve(cpu, cpu.cr2) {
        STACK_OVERFLOW
    } else {
        code
    };
    let Some(thread) = process(cpu).current_thread_mut() else {
        super::fatal(cpu, &what, Some(&ctx));
        return;
    };
    thread.fault = what;
    let fpu = control::save_image(cpu);
    raise(cpu, ctx, &fpu, code, 0, ctx.eip, &params);
}

/// Whether `address` is in the running thread's stack reserve, its floor
/// included: a stack that runs past what it may grow to faults there
/// first, and the page below the reservation is another region's.
fn in_stack_reserve(cpu: &mut Cpu, address: u32) -> bool {
    process(cpu)
        .current_thread()
        .is_some_and(|t| (t.stack.0..t.stack.1).contains(&address))
}

/// Raise the exception `code` in the running thread, at the context
/// `ctx`: its record and context on the thread's stack below ctx's ESP,
/// and its handlers called.
pub fn raise(
    cpu: &mut Cpu,
    ctx: Context,
    fpu: &[u8; 108],
    code: u32,
    flags: u32,
    address: u32,
    params: &[u32],
) {
    let ctx_at = (ctx.gpr[ESP].wrapping_sub(CONTEXT_SIZE)) & !3;
    let record = ctx_at.wrapping_sub(RECORD_SIZE);
    let esp = record.wrapping_sub(BELOW_RECORD);
    if !present(cpu, esp, ctx.gpr[ESP].wrapping_sub(esp)) {
        let what = std::mem::take(&mut process(cpu).current_thread_mut().unwrap().fault);
        super::fatal(
            cpu,
            &format!("{}, and its stack is gone", describe(code, &what)),
            Some(&ctx),
        );
        return;
    }
    let mut here = ctx;
    here.gpr[ESP] = esp;
    super::system::enter_user(cpu, &here);
    write_context(cpu, ctx_at, &ctx, fpu, CONTEXT_FULL);
    write_record(cpu, record, code, flags, address, params);
    let frame = handlers(cpu);
    dispatch(cpu, record, ctx_at, frame);
}

/// Write an EXCEPTION_RECORD at `at`.
fn write_record(cpu: &mut Cpu, at: u32, code: u32, flags: u32, address: u32, params: &[u32]) {
    cpu.bus.guest_fill(at, RECORD_SIZE as usize, 0);
    let n = (params.len() as u32).min(MAX_PARAMETERS);
    for (i, word) in [code, flags, 0, address, n].into_iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, word);
    }
    for (i, &param) in params.iter().take(n as usize).enumerate() {
        cpu.bus.guest_write_32(at + 20 + 4 * i as u32, param);
    }
}

/// The running thread's first exception handler frame (`fs:[0]`).
pub fn handlers(cpu: &mut Cpu) -> u32 {
    let tib = process(cpu).tib;
    cpu.bus.guest_read_32(tib + TIB_EXCEPTION_LIST)
}

pub fn set_handlers(cpu: &mut Cpu, frame: u32) {
    let tib = process(cpu).tib;
    cpu.bus.guest_write_32(tib + TIB_EXCEPTION_LIST, frame);
}

/// Call the handler of the frame `frame` for the exception whose record
/// and context are at `record` and `ctx_at`, or with none left, the
/// unhandled exception filter.
fn dispatch(cpu: &mut Cpu, record: u32, ctx_at: u32, frame: u32) {
    if frame == END_OF_CHAIN || frame == 0 || !present(cpu, frame, 8) {
        unhandled(cpu, record, ctx_at);
        return;
    }
    let handler = cpu.bus.guest_read_32(frame + 4);
    let dispatcher = record - 12;
    let resume = call::here(cpu);
    if !call::call(
        cpu,
        handler,
        &[record, frame, ctx_at, dispatcher],
        SEH_HANDLER,
        resume,
        vec![record, ctx_at, frame],
    ) {
        super::fatal(cpu, "an exception, and its stack is gone", None);
    }
}

/// A handler returned: the program goes on at the context, or the next
/// handler is called.
pub fn handler_returned(cpu: &mut Cpu, value: u32, call: Call) {
    let &[record, ctx_at, frame] = call.data.as_slice() else {
        return;
    };
    if value == CONTINUE_EXECUTION {
        let flags = cpu.bus.guest_read_32(record + 4);
        if flags & NONCONTINUABLE != 0 {
            let address = cpu.bus.guest_read_32(record + 12);
            let mut ctx = Context::of(cpu);
            ctx.gpr[ESP] = record - BELOW_RECORD;
            let fpu = control::save_image(cpu);
            raise(
                cpu,
                ctx,
                &fpu,
                NONCONTINUABLE_EXCEPTION,
                NONCONTINUABLE,
                address,
                &[],
            );
            return;
        }
        continue_at(cpu, ctx_at);
        return;
    }
    // Go on searching (nested and collided exceptions too).
    let next = cpu.bus.guest_read_32(frame);
    dispatch(cpu, record, ctx_at, next);
}

/// Go on in the running thread at the CONTEXT at `at`.
pub fn continue_at(cpu: &mut Cpu, at: u32) {
    let mut ctx = Context::of(cpu);
    let mut fpu = control::save_image(cpu);
    read_context(cpu, at, &mut ctx, &mut fpu);
    control::load_image(cpu, &fpu);
    if let Some(thread) = process(cpu).current_thread_mut() {
        thread.fault.clear();
    }
    call::unwound_to(cpu, ctx.gpr[ESP]);
    sanitize(cpu, &mut ctx);
    super::system::enter_user(cpu, &ctx);
}

/// The selectors a context may have: the program's, and its thread's TIB
/// in FS.
fn sanitize(cpu: &mut Cpu, ctx: &mut Context) {
    use super::system::{CODE3, DATA3};
    ctx.seg[Seg::CS as usize] = CODE3;
    for seg in [Seg::SS, Seg::DS, Seg::ES] {
        if ctx.seg[seg as usize] != DATA3 {
            ctx.seg[seg as usize] = DATA3;
        }
    }
    let selector = process(cpu).current_thread().map_or(0, |t| t.selector);
    ctx.seg[Seg::FS as usize] = selector;
    if ctx.seg[Seg::GS as usize] != DATA3 {
        ctx.seg[Seg::GS as usize] = 0;
    }
}

/// No handler took the exception: the unhandled exception filter decides,
/// and without one the process ends.
fn unhandled(cpu: &mut Cpu, record: u32, ctx_at: u32) {
    let filter = process(cpu).exception_filter;
    let pointers = record - 8;
    if filter != 0 {
        cpu.bus.guest_write_32(pointers, record);
        cpu.bus.guest_write_32(pointers + 4, ctx_at);
        let resume = call::here(cpu);
        if call::call(
            cpu,
            filter,
            &[pointers],
            SEH_FILTER,
            resume,
            vec![record, ctx_at, 0],
        ) {
            return;
        }
    }
    die(cpu, record, ctx_at);
}

/// The process ends for the exception at `record`, saying so.
fn die(cpu: &mut Cpu, record: u32, ctx_at: u32) {
    let code = cpu.bus.guest_read_32(record);
    let mut ctx = Context::of(cpu);
    let mut fpu = [0; 108];
    read_context(cpu, ctx_at, &mut ctx, &mut fpu);
    let what = process(cpu)
        .current_thread_mut()
        .map(|t| std::mem::take(&mut t.fault))
        .unwrap_or_default();
    super::fatal(cpu, &describe(code, &what), Some(&ctx));
}

/// What a message says of the exception `code`: what the fault was, if
/// it was one.
fn describe(code: u32, fault: &str) -> String {
    if !fault.is_empty() {
        return fault.to_string();
    }
    let name = match code {
        ACCESS_VIOLATION => "access violation",
        INT_DIVIDE_BY_ZERO => "integer division by zero",
        STACK_OVERFLOW => "stack overflow",
        BREAKPOINT => "breakpoint",
        _ => "exception",
    };
    format!("{} (exception {:08X})", name, code)
}

/// The unhandled exception filter returned: the program goes on at the
/// context, or the process ends. Called by UnhandledExceptionFilter
/// (data[2] set), it returns the filter's answer instead.
pub fn filter_returned(cpu: &mut Cpu, value: u32, call: Call) {
    let &[record, ctx_at, from_api] = call.data.as_slice() else {
        return;
    };
    if from_api != 0 {
        if value == 0 {
            say_unhandled(cpu, record);
            call::finish(cpu, call.resume, FILTER_EXECUTE_HANDLER);
        } else {
            call::finish(cpu, call.resume, value);
        }
        return;
    }
    match value {
        FILTER_CONTINUE_EXECUTION => continue_at(cpu, ctx_at),
        FILTER_EXECUTE_HANDLER => {
            let code = cpu.bus.guest_read_32(record);
            super::exit(cpu, code);
        }
        _ => die(cpu, record, ctx_at),
    }
}

/// Say that the program has an exception nobody handles, where, for
/// UnhandledExceptionFilter, after which the program ends itself.
fn say_unhandled(cpu: &mut Cpu, record: u32) {
    let code = cpu.bus.guest_read_32(record);
    let address = cpu.bus.guest_read_32(record + 12);
    let what = process(cpu)
        .current_thread_mut()
        .map(|t| std::mem::take(&mut t.fault))
        .unwrap_or_default();
    let message = format!(
        "{} has an exception it doesn't handle: {} at {:08X}.",
        process(cpu).name,
        describe(code, &what),
        address
    );
    cpu.bus.log_string(&format!("[W32] {}", message));
    super::say(cpu, &message);
}

/// UnhandledExceptionFilter(pointers): what the filter SetUnhandled-
/// ExceptionFilter set says, or else, having said so, that the handler
/// that called it ends the program (EXCEPTION_EXECUTE_HANDLER).
pub fn unhandled_exception_filter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let pointers = args[0];
    let record = cpu.bus.guest_read_32(pointers);
    let ctx_at = cpu.bus.guest_read_32(pointers + 4);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let filter = process(cpu).exception_filter;
    if filter != 0 {
        let resume = call::here(cpu);
        if call::call(
            cpu,
            filter,
            &[pointers],
            SEH_FILTER,
            resume,
            vec![record, ctx_at, 1],
        ) {
            return None;
        }
    }
    say_unhandled(cpu, record);
    Some(FILTER_EXECUTE_HANDLER)
}

/// SetUnhandledExceptionFilter(filter): the filter before.
pub fn set_unhandled_exception_filter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(std::mem::replace(
        &mut process(cpu).exception_filter,
        args[0],
    ))
}

/// RaiseException(code, flags, count, parameters): the exception, at the
/// caller's context after the call.
pub fn raise_exception(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[code, flags, count, at] = args else {
        unreachable!()
    };
    let count = count.min(MAX_PARAMETERS);
    let params: Vec<u32> = (0..count)
        .map(|i| cpu.bus.guest_read_32(at.wrapping_add(4 * i)))
        .collect();
    let mut ctx = Context::of(cpu);
    let esp = cpu.esp();
    ctx.eip = cpu.bus.guest_read_32(esp);
    if cpu.bus.guest_faulted() {
        return None;
    }
    ctx.gpr[ESP] = esp + 4 + 16;
    if let Some(thread) = process(cpu).current_thread_mut() {
        thread.fault.clear();
    }
    let fpu = control::save_image(cpu);
    raise(
        cpu,
        ctx,
        &fpu,
        code,
        flags & NONCONTINUABLE,
        ctx.eip,
        &params,
    );
    None
}

/// Raise the exception `code`, with no parameters, from the function the
/// running stub is of, which takes `args` arguments: at the caller's
/// context after the call, as if it had called RaiseException there.
/// (HeapAlloc's HEAP_GENERATE_EXCEPTIONS.)
pub fn raise_on_return(cpu: &mut Cpu, args: u32, code: u32) -> Option<u32> {
    let mut ctx = Context::of(cpu);
    let esp = cpu.esp();
    ctx.eip = cpu.bus.guest_read_32(esp);
    if cpu.bus.guest_faulted() {
        return None;
    }
    ctx.gpr[ESP] = esp.wrapping_add(4 + 4 * args);
    if let Some(thread) = process(cpu).current_thread_mut() {
        thread.fault.clear();
    }
    let fpu = control::save_image(cpu);
    raise(cpu, ctx, &fpu, code, 0, ctx.eip, &[]);
    None
}

/// RtlUnwind(frame, ip, record, value): call the handlers from the first
/// to (not with) `frame` again, unwinding, taking each off the chain, then
/// return `value`. With no frame, the whole chain unwinds.
pub fn rtl_unwind(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[target, _, record, value] = args else {
        unreachable!()
    };
    let resume = call::here(cpu);
    // A context and, if the caller has none, a record, below the stack.
    let ctx_at = (cpu.esp().wrapping_sub(CONTEXT_SIZE)) & !3;
    let own_record = ctx_at.wrapping_sub(RECORD_SIZE);
    let esp = own_record.wrapping_sub(BELOW_RECORD);
    if !cpu.bus.guest_probe(esp, (resume.1 - esp) as usize, true) {
        return None;
    }
    let ctx = Context::of(cpu);
    let fpu = control::save_image(cpu);
    write_context(cpu, ctx_at, &ctx, &fpu, CONTEXT_FULL);
    let record = if record == 0 {
        write_record(cpu, own_record, UNWIND, 0, resume.0, &[]);
        own_record
    } else {
        record
    };
    let flags =
        cpu.bus.guest_read_32(record + 4) | UNWINDING | if target == 0 { EXIT_UNWIND } else { 0 };
    cpu.bus.guest_write_32(record + 4, flags);
    if cpu.bus.guest_faulted() {
        return None;
    }
    cpu.set_esp(esp);
    let frame = handlers(cpu);
    unwind(cpu, [record, ctx_at, frame, target, value], resume);
    None
}

/// Unwind the frame `state[2]` on to `state[3]`, then return `state[4]`
/// to `resume`. `state` is the record, the context, the frame, the target
/// frame and the value.
fn unwind(cpu: &mut Cpu, state: [u32; 5], resume: (u32, u32)) {
    let [record, ctx_at, frame, target, value] = state;
    if frame == target || frame == END_OF_CHAIN || frame == 0 || !present(cpu, frame, 8) {
        call::finish(cpu, resume, value);
        return;
    }
    let handler = cpu.bus.guest_read_32(frame + 4);
    let dispatcher = ctx_at - RECORD_SIZE - 12;
    if !call::call(
        cpu,
        handler,
        &[record, frame, ctx_at, dispatcher],
        SEH_UNWIND,
        resume,
        state.to_vec(),
    ) {
        call::finish(cpu, resume, value);
    }
}

/// A handler returned from unwinding: its frame comes off the chain, and
/// the next one unwinds.
pub fn unwind_handler_returned(cpu: &mut Cpu, _: u32, call: Call) {
    let Ok(mut state) = <[u32; 5]>::try_from(call.data.as_slice()) else {
        return;
    };
    let next = cpu.bus.guest_read_32(state[2]);
    set_handlers(cpu, next);
    state[2] = next;
    unwind(cpu, state, call.resume);
}
