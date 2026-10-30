//! The switches between a task's 16-bit code and 32-bit code, on its
//! thread. As with the calls into the program (call.rs), no Rust frame
//! lives across one: each is a `Frame` in the task's list, which says
//! whose context goes on when the code called returns and what is done
//! with what it returns.
//!
//! 16-bit code calls 32-bit code (`call32`) when a function of a system
//! module goes on to a built-in DLL's: the 16-bit context is kept, and
//! the 32-bit function runs on the task's 32-bit stack, with the flat
//! selectors, returning to KERNEL32's `Win16Return`, which gives what it
//! returned to the 16-bit code in DX:AX. 32-bit code calls 16-bit code
//! (`call16`) through a procedure thunk: a slot of 32-bit code standing
//! for a 16-bit procedure, as a window procedure a 16-bit program
//! registers is to USER32. The 16-bit procedure runs on the task's 16-bit
//! stack, below where its 16-bit code went into the 32-bit code, and
//! returns to KERNEL's `CallReturn16`. A slot of 16-bit code stands for a
//! 32-bit procedure the other way, as USER32's own window procedures are
//! to 16-bit programs that subclass a window.

use super::{Frame, Thunk, ldt};
use crate::cpu::{Cpu, Seg};
use crate::dpmi::Context;
use crate::win32::memory::{GRANULARITY, PAGE, SHARED, SYSTEM, USER};
use crate::win32::system::{CODE3, DATA3};

/// The indexes of the registers in `Context::gpr`.
pub const EAX: usize = 0;
pub const ECX: usize = 1;
pub const EDX: usize = 2;
pub const EBX: usize = 3;
pub const ESP: usize = 4;
pub const EBP: usize = 5;
pub const ESI: usize = 6;
pub const EDI: usize = 7;

/// What a procedure thunk's procedure is, which says what its arguments
/// are and how they and what it returns are translated.
pub const KIND_WNDPROC: u8 = 0;
/// A dialog procedure: as a window procedure, its BOOL widened.
pub const KIND_DLGPROC: u8 = 1;
/// The others take and give 32-bit values the 16-bit ones are the low
/// words of, `extra` the number of arguments and its high byte which of
/// them are long (bit n for argument n).
pub const KIND_PLAIN: u8 = 2;
/// A font enumeration procedure (EnumFonts, EnumFontFamilies): its
/// LOGFONT and TEXTMETRIC given as 16-bit copies.
pub const KIND_FONTENUM: u8 = 3;
/// A hook procedure (hook.rs): its lParam given as a 16-bit copy, as
/// `extra` (the hook's kind and data segment) says.
pub const KIND_HOOK: u8 = 4;

/// Bytes of a slot.
const SLOT: u32 = 8;
/// How many slots there are room for each way.
const MAX_SLOTS: u32 = 0x1_0000 / SLOT;

/// The 32-bit stack's bytes each task's thread has for its calls of
/// 32-bit code.
pub const STACK32: u32 = 0x4_0000;

/// What is done when 32-bit code 16-bit code called returns `value`: the
/// value for DX:AX, or None when the function has seen to the processor
/// itself. `ctx` is the 16-bit context that goes on.
pub type Post = fn(&mut Cpu, u32, &[u32], &mut Context) -> Option<u32>;

/// What is done when 16-bit code 32-bit code called returns DX:AX: the
/// value for EAX, or None when the function has seen to the processor
/// itself. `ctx` is the context that goes on.
pub type Then = fn(&mut Cpu, u32, &[u32], &mut Context) -> Option<u32>;

/// The posts, by their place. Append only: saved states name them by
/// their place.
pub static POSTS: &[Post] = &[
    post_plain,
    super::builtin::post_auto,
    super::user::post,
    super::message::post,
    super::hook::post,
    super::hook::post_msg_filter,
];

pub const POST_PLAIN: u16 = 0;
pub const POST_AUTO: u16 = 1;
pub const POST_USER: u16 = 2;
pub const POST_MESSAGE: u16 = 3;
pub const POST_HOOK: u16 = 4;
pub const POST_MSG_FILTER: u16 = 5;

/// The thens, by their place. Append only.
pub static THENS: &[Then] = &[
    then_keep,
    then_proc,
    super::task::then,
    super::message::then,
    super::kernel::then_libs,
    super::task::then_dll_entry,
    super::flat::then_call,
    super::flat::then_inits,
    super::hook::then,
    super::dpmi::then_callback,
];

// Place 0 is then_keep, which no call names now.
pub const THEN_PROC: u16 = 1;
pub const THEN_TASK: u16 = 2;
pub const THEN_MESSAGE: u16 = 3;
pub const THEN_LIBS: u16 = 4;
pub const THEN_DLL_ENTRY: u16 = 5;
pub const THEN_FLAT_CALL: u16 = 6;
pub const THEN_FLAT_INITS: u16 = 7;
pub const THEN_HOOK: u16 = 8;
pub const THEN_CALLBACK: u16 = 9;

fn post_plain(_: &mut Cpu, value: u32, _: &[u32], _: &mut Context) -> Option<u32> {
    Some(value)
}

/// The context goes on as it was.
fn then_keep(_: &mut Cpu, _: u32, _: &[u32], ctx: &mut Context) -> Option<u32> {
    Some(ctx.gpr[EAX])
}

/// A procedure thunk's 16-bit procedure returned (`call_slot`): what it
/// returned, as a 32-bit value. `data` is the thunk's kind and extra.
fn then_proc(_: &mut Cpu, value: u32, data: &[u32], _: &mut Context) -> Option<u32> {
    let extra = data.get(1).copied().unwrap_or(0);
    // A procedure that returns a WORD leaves DX as it was.
    if extra & 0x1_0000 != 0 {
        Some(value as u16 as i16 as i32 as u32)
    } else {
        Some(value)
    }
}

/// Set AX of `ctx`.
pub fn set_ax(ctx: &mut Context, value: u16) {
    ctx.gpr[EAX] = ctx.gpr[EAX] & 0xFFFF_0000 | value as u32;
}

/// Set SP of `ctx`.
pub fn set_sp(ctx: &mut Context, value: u16) {
    ctx.gpr[ESP] = ctx.gpr[ESP] & 0xFFFF_0000 | value as u32;
}

/// The running task's frames.
fn frames(cpu: &mut Cpu) -> &mut Vec<Frame> {
    &mut super::task(cpu).frames
}

/// The top of the 32-bit stack the running task's next call of 32-bit
/// code uses: below the 32-bit code that is waiting for 16-bit code it
/// called, if there is such, or else the stack's top.
fn stack32(cpu: &mut Cpu) -> u32 {
    let task = super::task(cpu);
    let waiting = task.frames.iter().rev().find_map(|f| match f {
        Frame::To16 { ctx, .. } if is_flat(ctx) => Some(ctx.gpr[ESP]),
        _ => None,
    });
    waiting.map_or(task.stack32, |esp| esp & !3)
}

/// Whether `ctx` is 32-bit code's, with the flat selectors.
fn is_flat(ctx: &Context) -> bool {
    ctx.seg[Seg::CS as usize] == CODE3
}

/// The 16-bit stack (SS, SP) the running task's next call of 16-bit code
/// uses, from the context `from` that calls it: its own stack if it is
/// 16-bit code's, else below the 16-bit code that is waiting for 32-bit
/// code it called, else the stack the task started with.
fn stack16(cpu: &mut Cpu, from: &Context) -> (u16, u16) {
    if !is_flat(from) {
        return (from.seg[Seg::SS as usize], from.gpr[ESP] as u16);
    }
    let task = super::task(cpu);
    let waiting = task.frames.iter().rev().find_map(|f| match f {
        Frame::To32 { ctx, .. } => Some((ctx.seg[Seg::SS as usize], ctx.gpr[ESP] as u16)),
        _ => None,
    });
    waiting.unwrap_or(task.stack16)
}

/// From 16-bit code, as a function of a system module: call the 32-bit
/// code at `target` with `args` (the first at the lowest address, as
/// stdcall and cdecl have them) and `scratch` bytes of space for what the
/// function passes it, below them. `post` runs when it returns, with
/// `data`; `fill` writes the scratch space first, given its address.
/// The 16-bit context the processor has goes on afterwards: its EIP is
/// where it goes on (the stub's RETF).
pub fn call32(cpu: &mut Cpu, target: u32, args: &[u32], post: u16, data: Vec<u32>) {
    let ctx = Context::of(cpu);
    enter32(cpu, ctx, target, args, post, data);
}

/// `call32` with the 16-bit context `ctx` that goes on afterwards.
pub fn enter32(cpu: &mut Cpu, ctx: Context, target: u32, args: &[u32], post: u16, data: Vec<u32>) {
    let top = stack32(cpu);
    enter32_at(cpu, ctx, top, target, args, post, data);
}

/// Space of `len` bytes on the running task's 32-bit stack for a call of
/// 32-bit code to come (scratch for the structures it is passed): its
/// address. The call made next with `enter32_at` and the address as its
/// top doesn't overwrite it.
pub fn scratch32(cpu: &mut Cpu, len: u32) -> u32 {
    (stack32(cpu) - len.next_multiple_of(4)) & !0xF
}

/// `enter32` on the 32-bit stack below `top`.
pub fn enter32_at(
    cpu: &mut Cpu,
    ctx: Context,
    top: u32,
    target: u32,
    args: &[u32],
    post: u16,
    data: Vec<u32>,
) {
    let esp = top - 4 * (args.len() as u32 + 1);
    let mut words = vec![crate::win32::kernel32::win16_return_address()];
    words.extend_from_slice(args);
    for (i, &word) in words.iter().enumerate() {
        cpu.bus.guest_write_32(esp + 4 * i as u32, word);
    }
    frames(cpu).push(Frame::To32 { ctx, post, data });
    let fs = crate::win32::process(cpu)
        .current_thread()
        .map_or(0, |t| t.selector);
    let mut flat = Context {
        eip: target,
        eflags: 0x0202,
        seg: [DATA3, CODE3, DATA3, DATA3, fs, 0],
        ..Context::default()
    };
    flat.gpr[ESP] = esp;
    flat.gpr[EBP] = ctx.gpr[EBP];
    crate::win32::system::load_context(cpu, &flat);
}

/// KERNEL32's `Win16Return`: 32-bit code 16-bit code called returned.
pub fn win16_return(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let value = cpu.eax();
    // A Win32 program can call it by name from a thread that isn't a
    // task's.
    if super::current_task(cpu).is_none() {
        crate::win32::fatal(cpu, "a return to 16-bit code from a Win32 thread", None);
        return None;
    }
    let Some(Frame::To32 {
        mut ctx,
        post,
        data,
    }) = frames(cpu).pop()
    else {
        crate::win32::fatal(cpu, "a return to 16-bit code that didn't call", None);
        return None;
    };
    // A plain value, or one of the automatic thunks that converts no
    // handle and copies no structure, changes nothing but registers.
    let plain = post == POST_PLAIN
        || post == POST_AUTO && matches!(data[..], [ret] if !b"imna".contains(&(ret as u8)));
    let result = match POSTS.get(post as usize) {
        Some(f) => f(cpu, value, &data, &mut ctx),
        None => Some(value),
    };
    if plain && cpu.bus.observe.on {
        crate::idle::pure_call(cpu, None);
    }
    if let Some(value) = result {
        resume16(cpu, ctx, value);
    }
    None
}

/// Go on with the 16-bit context `ctx`, DX:AX `value`.
pub fn resume16(cpu: &mut Cpu, mut ctx: Context, value: u32) {
    ctx.gpr[EAX] = ctx.gpr[EAX] & 0xFFFF_0000 | value & 0xFFFF;
    ctx.gpr[EDX] = ctx.gpr[EDX] & 0xFFFF_0000 | value >> 16;
    crate::win32::system::load_context(cpu, &ctx);
}

/// Call the 16-bit procedure at the 16:16 address `target` with the words
/// `args`, in the order the caller pushes them (Pascal's: the first
/// argument first, a long's high word before its low one), with DS and
/// AX `ds` (the stack's selector if it is 0). `then` runs when it returns,
/// with `data`, and the context `resume` goes on afterwards. `below`
/// bytes of the 16-bit stack are kept below where it was for the
/// structures the procedure is passed (`scratch16`).
pub fn call16(
    cpu: &mut Cpu,
    target: u32,
    args: &[u16],
    ds: u16,
    resume: Context,
    then: u16,
    data: Vec<u32>,
) {
    let (ss, sp) = stack16(cpu, &resume);
    call16_at(cpu, target, args, ds, resume, then, data, (ss, sp));
}

/// The 16-bit stack (SS, SP) a call of 16-bit code from the context
/// `from` would use, for scratch space below it: `call16_at` with the
/// stack moved down past the space keeps it.
pub fn stack16_for(cpu: &mut Cpu, from: &Context) -> (u16, u16) {
    stack16(cpu, from)
}

/// `call16` on the 16-bit stack `ss`:`sp`.
#[allow(clippy::too_many_arguments)]
pub fn call16_at(
    cpu: &mut Cpu,
    target: u32,
    args: &[u16],
    ds: u16,
    resume: Context,
    then: u16,
    data: Vec<u32>,
    stack: (u16, u16),
) {
    let mut regs = Context::default();
    regs.seg[Seg::DS as usize] = ds;
    regs.seg[Seg::ES as usize] = ds;
    regs.gpr[EAX] = ds as u32;
    call16_regs(cpu, target, args, regs, resume, then, data, stack);
}

/// `call16_at` with the registers `regs` has, DS and ES the stack's
/// selector where they are 0 (and AX then too).
#[allow(clippy::too_many_arguments)]
pub fn call16_regs(
    cpu: &mut Cpu,
    target: u32,
    args: &[u16],
    regs: Context,
    resume: Context,
    then: u16,
    data: Vec<u32>,
    (ss, sp): (u16, u16),
) {
    let ret = super::builtin::call_return16(cpu);
    let mut words: Vec<u16> = args.to_vec();
    words.push((ret >> 16) as u16);
    words.push(ret as u16);
    let base = super::selector_base(cpu, ss).map_or(0, |(b, _, _)| b);
    let mut sp = sp;
    for &word in &words {
        sp = sp.wrapping_sub(2);
        cpu.bus.guest_write_16(base.wrapping_add(sp as u32), word);
    }
    frames(cpu).push(Frame::To16 {
        ctx: resume,
        then,
        data,
    });
    let mut ctx16 = regs;
    for seg in [Seg::DS, Seg::ES] {
        if ctx16.seg[seg as usize] == 0 {
            ctx16.seg[seg as usize] = ss;
            if seg == Seg::DS && ctx16.gpr[EAX] == 0 {
                ctx16.gpr[EAX] = ss as u32;
            }
        }
    }
    ctx16.eip = target & 0xFFFF;
    ctx16.eflags = 0x0202;
    ctx16.seg[Seg::CS as usize] = (target >> 16) as u16;
    ctx16.seg[Seg::SS as usize] = ss;
    ctx16.seg[Seg::FS as usize] = 0;
    ctx16.seg[Seg::GS as usize] = 0;
    ctx16.gpr[ESP] = sp as u32;
    if ctx16.gpr[EBP] == 0 {
        ctx16.gpr[EBP] = sp as u32;
    }
    crate::win32::system::load_context(cpu, &ctx16);
}

/// KERNEL's `CallReturn16`: 16-bit code 32-bit code (or the system)
/// called returned.
pub fn call_return16(cpu: &mut Cpu) {
    let value = (cpu.dx() as u32) << 16 | cpu.ax() as u32;
    let Some(Frame::To16 {
        mut ctx,
        then,
        data,
    }) = frames(cpu).pop()
    else {
        crate::win32::fatal(cpu, "a return to 32-bit code that didn't call", None);
        return;
    };
    let result = match THENS.get(then as usize) {
        Some(f) => f(cpu, value, &data, &mut ctx),
        None => Some(value),
    };
    if let Some(eax) = result {
        ctx.gpr[EAX] = eax;
        crate::win32::system::load_context(cpu, &ctx);
    }
}

/// The 32-bit slot for the 16-bit procedure at `target` of the kind
/// `kind`: its address, which 32-bit code calls as it would a procedure
/// of its own. 0 for a null procedure, and when there is no room.
pub fn proc32(cpu: &mut Cpu, target: u32, kind: u8, extra: u32) -> u32 {
    if target == 0 {
        return 0;
    }
    // A 16-bit slot for a 32-bit procedure stands for that procedure.
    if let Some(t) = thunk16_of(cpu, target)
        && t.kind == kind
    {
        return t.target;
    }
    let win16 = &cpu.bus.win32.win16;
    if let Some(i) = win16
        .thunks32
        .iter()
        .position(|t| t.target == target && t.kind == kind && t.extra == extra)
    {
        return win16.thunk32_base + SLOT * i as u32;
    }
    let i = win16.thunks32.len() as u32;
    if i >= MAX_SLOTS || !commit_slot(cpu, win16_thunk32_base(cpu), i) {
        return 0;
    }
    let at = cpu.bus.win32.win16.thunk32_base + SLOT * i;
    let id = crate::win32::kernel32::win16_thunk_id();
    let [lo, hi] = id.to_le_bytes();
    super::poke(
        cpu,
        at,
        &[0xFE, crate::win32::TRAP, lo, hi, 0x90, 0x90, 0x90, 0x90],
    );
    cpu.bus.win32.win16.thunks32.push(Thunk {
        target,
        kind,
        extra,
    });
    at
}

fn win16_thunk32_base(cpu: &Cpu) -> u32 {
    cpu.bus.win32.win16.thunk32_base
}

/// Commit the page of slot `i` of the slots at `base` if it isn't yet,
/// its physical page one of the process's stubs'. False when there is
/// no memory.
fn commit_slot(cpu: &mut Cpu, base: u32, i: u32) -> bool {
    let at = base + SLOT * i;
    if !(SLOT * i).is_multiple_of(PAGE) {
        return true;
    }
    let pid = cpu.bus.win32.win16.pid;
    super::with_win16(cpu, pid, |p, bus| {
        p.memory.commit(bus, at, PAGE, USER)?;
        let phys = p.memory.translate(bus, at)?;
        p.stub_pages.push(phys >> 12);
        Some(())
    })
    .flatten()
    .is_some()
}

/// The 16-bit procedure a 32-bit slot stands for, if `address` is one.
pub fn thunk32_of(cpu: &Cpu, address: u32) -> Option<Thunk> {
    let win16 = &cpu.bus.win32.win16;
    let offset = address.checked_sub(win16.thunk32_base)?;
    if win16.thunk32_base == 0 || offset % SLOT != 0 {
        return None;
    }
    win16.thunks32.get((offset / SLOT) as usize).cloned()
}

/// The 16-bit slot for the 32-bit procedure at `target` of the kind
/// `kind`, which takes `arg_bytes` bytes of arguments: its 16:16 address.
/// A 32-bit slot stands for its 16-bit procedure.
pub fn proc16(cpu: &mut Cpu, target: u32, kind: u8, extra: u32, arg_bytes: u16) -> u32 {
    if target == 0 {
        return 0;
    }
    if let Some(t) = thunk32_of(cpu, target) {
        return t.target;
    }
    let win16 = &cpu.bus.win32.win16;
    if let Some(i) = win16
        .thunks16
        .iter()
        .position(|t| t.target == target && t.kind == kind)
    {
        return ((win16.thunk16_sel as u32) << 16) | (SLOT * i as u32);
    }
    let i = win16.thunks16.len() as u32;
    let base = win16.thunk16_base;
    if i >= MAX_SLOTS || !commit_slot(cpu, base, i) {
        return 0;
    }
    let id = super::builtin::thunk16_id();
    let [lo, hi] = id.to_le_bytes();
    let [n_lo, n_hi] = arg_bytes.to_le_bytes();
    super::poke(
        cpu,
        base + SLOT * i,
        &[0xFE, crate::win32::TRAP, lo, hi, 0xCA, n_lo, n_hi, 0x90],
    );
    cpu.bus.win32.win16.thunks16.push(Thunk {
        target,
        kind,
        extra,
    });
    ((cpu.bus.win32.win16.thunk16_sel as u32) << 16) | (SLOT * i)
}

/// The 32-bit procedure a 16-bit slot stands for, if `segptr` is one.
pub fn thunk16_of(cpu: &Cpu, segptr: u32) -> Option<Thunk> {
    let win16 = &cpu.bus.win32.win16;
    if win16.thunk16_sel == 0 || (segptr >> 16) as u16 | 1 != win16.thunk16_sel | 1 {
        return None;
    }
    let offset = segptr & 0xFFFF;
    if !offset.is_multiple_of(SLOT) {
        return None;
    }
    win16.thunks16.get((offset / SLOT) as usize).cloned()
}

/// Make the regions of the procedure thunks' slots, when the Win16
/// process starts: 64 KB for each, its pages committed as slots are
/// given out, the 16-bit one a code segment.
pub fn init(cpu: &mut Cpu) -> Option<()> {
    let pid = cpu.bus.win32.win16.pid;
    let (base32, base16, base_inst) = super::with_win16(cpu, pid, |p, _| {
        let a = p.memory.reserve(None, GRANULARITY, SHARED, SYSTEM)?;
        let b = p.memory.reserve(None, GRANULARITY, SHARED, SYSTEM)?;
        let c = p.memory.reserve(None, GRANULARITY, SHARED, SYSTEM)?;
        Some((a, b, c))
    })??;
    let sel = cpu.bus.win32.win16.ldt.alloc(1)?;
    let inst_sel = cpu.bus.win32.win16.ldt.alloc(1)?;
    let system = super::win16(cpu).system;
    for (s, base) in [(sel, base16), (inst_sel, base_inst)] {
        ldt::write(cpu, system, s, ldt::segment(base, 0xFFFF, ldt::ACCESS_CODE));
    }
    let win16 = &mut cpu.bus.win32.win16;
    win16.thunk32_base = base32;
    win16.thunk16_base = base16;
    win16.thunk16_sel = sel;
    win16.inst_base = base_inst;
    win16.inst_sel = inst_sel;
    Some(())
}

/// KERNEL32's `Win16Thunk`: 32-bit code called a 32-bit slot. The 16-bit
/// procedure it stands for runs, with the arguments translated as its
/// kind says.
pub fn thunk32_trap(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let slot = cpu.eip().wrapping_sub(4);
    let Some(thunk) = thunk32_of(cpu, slot) else {
        crate::win32::fatal(cpu, "a call of a thunk that isn't there", None);
        return None;
    };
    match thunk.kind {
        KIND_WNDPROC | KIND_DLGPROC => super::message::call_wndproc16(cpu, &thunk),
        KIND_FONTENUM => super::gdi::call_font_enum16(cpu, &thunk),
        KIND_HOOK => super::hook::call_hook16(cpu, &thunk),
        _ => call_plain16(cpu, &thunk),
    }
    None
}

/// Call a plain 16-bit procedure for its 32-bit slot: the arguments'
/// low words (or the whole of the long ones), stdcall's frame taken off
/// when it returns.
fn call_plain16(cpu: &mut Cpu, thunk: &Thunk) {
    let count = thunk.extra & 0xFF;
    let longs = thunk.extra >> 8 & 0xFF;
    let esp = cpu.esp();
    let ret = cpu.bus.guest_read_32(esp);
    let mut args = Vec::new();
    for i in 0..count {
        let value = cpu.bus.guest_read_32(esp + 4 + 4 * i);
        if longs & 1 << i != 0 {
            args.push((value >> 16) as u16);
        }
        args.push(value as u16);
    }
    let mut resume = Context::of(cpu);
    resume.eip = ret;
    resume.gpr[ESP] = esp + 4 + 4 * count;
    call16(
        cpu,
        thunk.target,
        &args,
        0,
        resume,
        THEN_PROC,
        vec![thunk.kind as u32, thunk.extra],
    );
}

/// KERNEL's `Thunk16`: 16-bit code called a 16-bit slot, at IP - 4. The
/// 32-bit procedure it stands for runs, with the arguments translated as
/// its kind says.
pub fn thunk16_trap(cpu: &mut Cpu) {
    let slot = (cpu.cs() as u32) << 16 | cpu.ip().wrapping_sub(4) as u32;
    let Some(thunk) = thunk16_of(cpu, slot) else {
        crate::win32::fatal(cpu, "a call of a 16-bit thunk that isn't there", None);
        return;
    };
    match thunk.kind {
        KIND_WNDPROC | KIND_DLGPROC => super::message::call_wndproc32(cpu, &thunk),
        _ => {
            // Plain: the words, widened.
            let count = thunk.extra & 0xFF;
            let longs = thunk.extra >> 8 & 0xFF;
            let sp = cpu.sp() as u32;
            let base = cpu.seg_cache(Seg::SS).base;
            let mut args = vec![0u32; count as usize];
            let mut at = sp + 4;
            for i in (0..count).rev() {
                if longs & 1 << i != 0 {
                    args[i as usize] = cpu.bus.guest_read_32(base + at);
                    at += 4;
                } else {
                    args[i as usize] = cpu.bus.guest_read_16(base + at) as u32;
                    at += 2;
                }
            }
            call32(cpu, thunk.target, &args, POST_PLAIN, Vec::new());
        }
    }
}

/// MakeProcInstance's thunk for the 16-bit `function` and the data
/// segment `ds`: `mov ax, ds` and a far jump to the function, its 16:16
/// address. The same thunk for the same pair; 0 when there is no room.
pub fn instance_thunk(cpu: &mut Cpu, function: u32, ds: u16) -> u32 {
    let win16 = &cpu.bus.win32.win16;
    let sel = win16.inst_sel as u32;
    if let Some(i) = win16
        .inst_thunks
        .iter()
        .position(|&(f, d)| f == function && d == ds)
    {
        return (sel << 16) | (SLOT * i as u32);
    }
    let i = win16.inst_thunks.len() as u32;
    let at = win16.inst_base + SLOT * i;
    if i >= MAX_SLOTS {
        return 0;
    }
    if (SLOT * i).is_multiple_of(PAGE) {
        let pid = win16.pid;
        let made =
            super::with_win16(cpu, pid, |p, bus| p.memory.commit(bus, at, PAGE, USER)).flatten();
        if made.is_none() {
            return 0;
        }
    }
    let [d_lo, d_hi] = ds.to_le_bytes();
    let [o_lo, o_hi, s_lo, s_hi] = function.to_le_bytes();
    super::poke(cpu, at, &[0xB8, d_lo, d_hi, 0xEA, o_lo, o_hi, s_lo, s_hi]);
    cpu.bus.win32.win16.inst_thunks.push((function, ds));
    (sel << 16) | (SLOT * i)
}
