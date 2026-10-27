//! The display as DirectDraw has it: the modes it offers and sets, the
//! object that has exclusive mode, the card's video memory that surfaces
//! take their pixels from, and each process's view of that memory.
//!
//! A mode DirectDraw sets is one the display adapter has (`screen`):
//! one of the VESA BIOS's, or of DirectDraw's own sizes, or on a Banshee
//! or Voodoo3 the card's own. While it is set the display is
//! DirectDraw's: GDI draws on its picture in that mode
//! (`screen::gdi_mode`), and USER32 doesn't put the desktop back when a
//! window shows. RestoreDisplayMode, the end of exclusive mode and the
//! end of the process give the screen back: the desktop's mode again,
//! repainted, or text mode when there is no desktop. Glide on a Banshee
//! or Voodoo3 takes the display the same way (`take`, `give_back`), and
//! GDI's drawing on the screen goes nowhere then.
//!
//! Video memory is the adapter's: the VESA BIOS's 4 MB, or the card's
//! 16 MB, which its 3D engine draws into. The picture on the screen is
//! at its start (the desktop, or the mode's primary surface and the rest
//! of its flip chain); the other surfaces in video memory take blocks of
//! the rest. A mode change takes all of them back, and the surfaces in it
//! are lost until restored. A process that uses DirectDraw has the whole
//! of video memory mapped in its shared arena, where the linear frame
//! buffer (or the card's BAR1) is, so that Lock gives it the pixels
//! themselves.

use super::{DD_OK, DDERR_INVALIDMODE, DDERR_OUTOFVIDEOMEMORY, current_pid, state};
use crate::cpu::Cpu;
use crate::win32::memory::{MEM_MAPPED, PAGE, PAGE_READWRITE, SHARED, SYSTEM, USER, WRITABLE};
use crate::win32::screen::{self, Mode};
use crate::win32::user32::sysparams::{display_changed, display_mode};

/// Where a process's view of video memory goes, if that is free.
const VIEW_AT: u32 = 0x9FC0_0000;

/// What blocks of video memory are aligned to: more than the 16 bytes
/// the card's 3D engine needs of a buffer's address.
const ALIGN: u32 = 64;

/// The owner of a mode Glide set (`take`), which no DirectDraw object
/// is.
pub const GLIDE: u32 = u32::MAX;

#[derive(Clone, Debug, Default)]
pub struct Display {
    /// The mode DirectDraw set: width, height and bits a pixel; all 0
    /// while the display is the desktop's.
    pub mode: (u32, u32, u32),
    /// The DirectDraw object that set it (`GLIDE` for Glide), and the
    /// one in exclusive mode.
    pub mode_owner: u32,
    pub exclusive: u32,
    /// The blocks of video memory surfaces have: offset and length, in
    /// order.
    pub blocks: Vec<(u32, u32)>,
    /// The processes' views of video memory: process id and address.
    pub views: Vec<(u32, u32)>,
    /// When the last flip of the primary surface shows, by the clock's
    /// nanoseconds.
    pub flip_at: u64,
    /// The polls of that flip that found it still to show.
    pub flip_polls: Polls,
}

/// Failed polls in a row of something the display does at a frame, by
/// one thread.
#[derive(Clone, Debug, Default)]
pub struct Polls {
    /// The thread that polled, and when it last did: `Cpu::executed` and
    /// `Win32::calls`.
    thread: u32,
    executed: u64,
    calls: u64,
    /// The polls in a row, each the next call of a built-in function by
    /// any thread after the one before, and no more than `POLL_GAP`
    /// instructions after it.
    count: u32,
}

/// Polls in a row after which a thread is taken to be doing nothing but
/// polling.
const POLLS: u32 = 16;
/// Instructions between two polls of one loop, at most.
const POLL_GAP: u64 = 2_000;

impl Polls {
    /// The thread `thread` polled and found the display not done yet,
    /// with `Cpu::executed` at `executed` and `Win32::calls` at `calls`:
    /// whether it has done next to nothing else for `POLLS` polls, with
    /// no other built-in function called in between by any thread. A
    /// loop like that does the same until the frame the display waits
    /// for, so the caller can make the thread wait for that frame instead
    /// of running each poll (a game calling Flip without DDFLIP_WAIT until
    /// it succeeds makes hundreds of calls a millisecond). A loop that
    /// does more while it waits, such as filling its sound buffer or
    /// reading its messages, keeps every poll.
    pub fn failed(&mut self, thread: u32, executed: u64, calls: u64) -> bool {
        let close = self.thread == thread
            && calls == self.calls.wrapping_add(1)
            && executed.saturating_sub(self.executed) <= POLL_GAP;
        self.count = if close {
            self.count.saturating_add(1)
        } else {
            1
        };
        self.thread = thread;
        self.executed = executed;
        self.calls = calls;
        self.count >= POLLS
    }
}

/// The running thread polled for the pending flip of the primary
/// surface and found it still to show: whether it only polls
/// (`Polls::failed`).
pub fn flip_poll_failed(cpu: &mut Cpu) -> bool {
    let thread = crate::win32::user32::current_tid(cpu);
    let (executed, calls) = (cpu.executed, cpu.bus.win32.calls);
    state(cpu)
        .display
        .flip_polls
        .failed(thread, executed, calls)
}

/// Whether DirectDraw (or Glide) has the display: GDI draws on the
/// picture in its mode, or nowhere for Glide (`screen::gdi_mode`).
pub fn owns_display(cpu: &mut Cpu) -> bool {
    state(cpu).display.mode.0 != 0
}

/// The mode DirectDraw set, which is the screen's (USER32's
/// `sysparams::display_mode`): width, height and bits a pixel. None
/// while the display is the desktop's, or Glide's.
pub fn screen_mode(cpu: &Cpu) -> Option<(u32, u32, u32)> {
    let display = &cpu.bus.win32.ddraw.display;
    (display.mode.0 != 0 && display.mode_owner != GLIDE).then_some(display.mode)
}

/// The modes DirectDraw offers, as width, height and bits a pixel, by
/// size and then depth.
pub fn modes(cpu: &Cpu) -> Vec<(u32, u32, u32)> {
    // An S3 ViRGE's "24-bit" color is 3 bytes a pixel instead of 4.
    let truecolor = if cpu.bus.vga.adapter.is_virge() {
        24
    } else {
        32
    };
    let mut list = Vec::new();
    for (w, h) in screen::SIZES {
        for bpp in [8, 16, truecolor] {
            list.push((w, h, bpp));
        }
    }
    list
}

/// The display's mode as programs see it: width, height, bits a pixel
/// and bytes a line. The desktop's when DirectDraw hasn't set one.
pub fn current(cpu: &mut Cpu) -> (u32, u32, u32, u32) {
    let (w, h, bpp) = state(cpu).display.mode;
    if w != 0 {
        let pitch = screen::mode(cpu).map_or(w * bpp / 8, |m| m.pitch);
        return (w, h, bpp, pitch);
    }
    let d = screen::desktop(cpu);
    (d.width, d.height, d.bpp, d.pitch)
}

/// The bytes of video memory the picture on the screen takes.
fn screen_len(cpu: &mut Cpu) -> u32 {
    let (_, h, _, pitch) = current(cpu);
    pitch * h
}

/// Set the mode `w` x `h` x `bpp` for the DirectDraw object `owner`.
/// The surfaces in video memory are lost.
pub fn set_mode(cpu: &mut Cpu, owner: u32, w: u32, h: u32, bpp: u32) -> u32 {
    if !modes(cpu).contains(&(w, h, bpp)) || set(cpu, owner, w, h, bpp).is_none() {
        return DDERR_INVALIDMODE;
    }
    DD_OK
}

/// Set the mode for `owner`, any the adapter has: the surfaces in video
/// memory are lost, and the mouse moves over the picture. Unless it is
/// Glide's, the screen takes the mode's size (`display_changed`).
fn set(cpu: &mut Cpu, owner: u32, w: u32, h: u32, bpp: u32) -> Option<Mode> {
    let old = display_mode(cpu);
    let mode = screen::set_mode(cpu, w, h, bpp)?;
    super::surface::video_memory_lost(cpu);
    let display = &mut state(cpu).display;
    display.blocks.clear();
    display.mode = (w, h, bpp);
    display.mode_owner = owner;
    display_changed(cpu, old);
    // After `display_changed`, which keeps the mouse on the screen as
    // Windows sees it: a mode Glide set can be larger.
    let mouse = &mut cpu.bus.mouse;
    (mouse.min_x, mouse.max_x, mouse.min_y, mouse.max_y) = (0, w as i32 - 1, 0, h as i32 - 1);
    Some(mode)
}

/// The desktop's mode changed (ChangeDisplaySettings): unless
/// DirectDraw has the display, the surfaces in video memory are lost,
/// as the desktop's picture may now cover them.
pub fn desktop_changed(cpu: &mut Cpu) {
    if owns_display(cpu) {
        return;
    }
    super::surface::video_memory_lost(cpu);
    state(cpu).display.blocks.clear();
}

/// Take the display for Glide on a Banshee or Voodoo3: the mode `w` x
/// `h` x `bpp` (any the adapter has) is set and its picture is at the
/// start of video memory, which is Glide's until `give_back`. A mode
/// DirectDraw set ends, and its surfaces in video memory are lost; GDI
/// draws nowhere. None if the adapter has no such mode, and nothing
/// changes then.
pub fn take(cpu: &mut Cpu, w: u32, h: u32, bpp: u32) -> Option<Mode> {
    let mode = set(cpu, GLIDE, w, h, bpp)?;
    state(cpu).display.exclusive = 0;
    Some(mode)
}

/// Give back the display Glide took: the desktop's mode again, repainted,
/// or text mode when there is no desktop.
pub fn give_back(cpu: &mut Cpu) {
    if state(cpu).display.mode_owner == GLIDE {
        restore(cpu);
    }
}

/// Give the screen back to the desktop, if DirectDraw has it.
pub fn restore(cpu: &mut Cpu) {
    if !owns_display(cpu) {
        return;
    }
    let old = display_mode(cpu);
    super::surface::video_memory_lost(cpu);
    let display = &mut state(cpu).display;
    display.blocks.clear();
    display.mode = (0, 0, 0);
    display.mode_owner = 0;
    // The desktop window takes the screen's size before it is painted.
    display_changed(cpu, old);
    restore_screen(cpu);
}

/// The screen as it is without DirectDraw: the desktop, or text mode.
fn restore_screen(cpu: &mut Cpu) {
    if cpu.bus.win32.user.desktop {
        crate::win32::user32::paint::restore_desktop(cpu);
        // The mouse's range may still be Glide's mode's.
        let (w, h) = crate::win32::user32::sysparams::screen_size(cpu);
        let mouse = &mut cpu.bus.mouse;
        (mouse.min_x, mouse.max_x, mouse.min_y, mouse.max_y) = (0, w - 1, 0, h - 1);
    } else {
        screen::set_text_mode(cpu);
    }
}

/// Take a block of `len` bytes of video memory: its offset, or the
/// error.
pub fn alloc(cpu: &mut Cpu, len: u32) -> Result<u32, u32> {
    let floor = screen_len(cpu).next_multiple_of(ALIGN);
    let len = len.max(1).next_multiple_of(ALIGN);
    let size = screen::memory_size(cpu);
    let display = &mut state(cpu).display;
    let mut at = floor;
    let mut index = 0;
    for (i, &(start, size)) in display.blocks.iter().enumerate() {
        if start >= at && start - at >= len {
            break;
        }
        at = at.max((start + size).next_multiple_of(ALIGN));
        index = i + 1;
    }
    if at as u64 + len as u64 > size as u64 {
        return Err(DDERR_OUTOFVIDEOMEMORY);
    }
    display.blocks.insert(index, (at, len));
    Ok(at)
}

/// Give back the block at `offset`.
pub fn free(cpu: &mut Cpu, offset: u32) {
    state(cpu)
        .display
        .blocks
        .retain(|&(start, _)| start != offset);
}

/// Video memory for surfaces, and how much of it is free.
pub fn video_memory(cpu: &mut Cpu) -> (u32, u32) {
    let total = screen::memory_size(cpu) - screen_len(cpu);
    let used: u32 = state(cpu).display.blocks.iter().map(|b| b.1).sum();
    (total, total.saturating_sub(used))
}

/// Where the running process sees video memory, mapped there if it
/// isn't yet: None when its address space has no room.
pub fn view(cpu: &mut Cpu) -> Option<u32> {
    let pid = current_pid(cpu);
    if let Some(&(_, at)) = state(cpu).display.views.iter().find(|v| v.0 == pid) {
        return Some(at);
    }
    screen::linear_memory(cpu);
    let lfb = screen::physical_base(cpu);
    let len = screen::memory_size(cpu);
    let at = crate::win32::with_process(cpu, |p, bus| {
        let reserve = |p: &mut crate::win32::Process, at| {
            p.memory
                .reserve_region(at, len, SHARED, SYSTEM, PAGE_READWRITE, MEM_MAPPED)
        };
        let at = reserve(p, Some(VIEW_AT)).or_else(|| reserve(p, None))?;
        for page in 0..len / PAGE {
            let offset = page * PAGE;
            p.memory
                .map(bus, at + offset, lfb + offset, WRITABLE | USER)?;
        }
        Some(at)
    })?;
    cpu.tlb.flush();
    state(cpu).display.views.push((pid, at));
    Some(at)
}

/// The DirectDraw object `id` goes: what it had of the display goes
/// with it.
pub fn object_gone(cpu: &mut Cpu, id: u32) {
    let display = &mut state(cpu).display;
    if display.exclusive == id {
        display.exclusive = 0;
    }
    if display.mode_owner == id {
        restore(cpu);
    }
}

/// A process ended. Its memory, and its view of video memory with it, is
/// gone already; a mode its objects set is given back.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    state(cpu).display.views.retain(|v| v.0 != pid);
    let owners: Vec<u32> = {
        let s = state(cpu);
        [s.display.mode_owner, s.display.exclusive]
            .into_iter()
            .filter(|&id| s.object(id).is_some_and(|o| o.pid == pid))
            .collect()
    };
    for id in owners {
        object_gone(cpu, id);
    }
}

/// The display's timing now: the frame's lines and the time in it.
pub fn timing(cpu: &mut Cpu) -> (crate::video::crt::CrtTiming, u64) {
    let now = cpu.bus.clock.now_ns();
    (screen::timing(cpu), now)
}

/// Show the picture at `offset` of video memory, from the next frame.
pub fn show(cpu: &mut Cpu, offset: u32) {
    screen::show(cpu, offset);
}

crate::state_fields!(Display {
    mode,
    mode_owner,
    exclusive,
    blocks,
    views,
    flip_at,
    flip_polls
});

crate::state_fields!(Polls {
    thread,
    executed,
    calls,
    count
});
