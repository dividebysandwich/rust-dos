//! The display as DirectDraw has it: the modes it offers and sets, the
//! object that has exclusive mode, the card's video memory that surfaces
//! take their pixels from, and each process's view of that memory.
//!
//! A mode DirectDraw sets is a VBE mode with the linear frame buffer, the
//! VBE list's where it has one and one of DirectDraw's own (320x240, the
//! 8-bit 320x200) where it hasn't. While it is set the display is
//! DirectDraw's: GDI's drawing on the screen goes nowhere, and USER32
//! doesn't put the desktop back when a window shows. RestoreDisplayMode,
//! the end of exclusive mode and the end of the process give the screen
//! back: the desktop's mode again, repainted, or text mode when there is
//! no desktop.
//!
//! Video memory is the card's 4 MB. The picture on the screen is at its
//! start (the desktop, or the mode's primary surface and the rest of its
//! flip chain); the other surfaces in video memory take blocks of the
//! rest. A mode change takes all of them back, and the surfaces in it are
//! lost until restored. A process that uses DirectDraw has the whole of
//! video memory mapped in its shared arena, where the linear frame buffer
//! is, so that Lock gives it the pixels themselves.

use super::{DD_OK, DDERR_INVALIDMODE, DDERR_OUTOFVIDEOMEMORY, current_pid, state};
use crate::cpu::Cpu;
use crate::video::vbe::{self, LFB_BASE, VRAM_SIZE, VbeMode};
use crate::win32::memory::{MEM_MAPPED, PAGE, PAGE_READWRITE, SHARED, SYSTEM, USER, WRITABLE};

/// Where a process's view of video memory goes, if that is free.
const VIEW_AT: u32 = 0x9FC0_0000;

/// What blocks of video memory are aligned to.
const ALIGN: u32 = 64;

#[derive(Clone, Debug, Default)]
pub struct Display {
    /// The mode DirectDraw set: width, height and bits a pixel; all 0
    /// while the display is the desktop's.
    pub mode: (u32, u32, u32),
    /// The DirectDraw object that set it, and the one in exclusive mode.
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
}

/// Whether DirectDraw has the display: GDI doesn't draw on the screen.
pub fn owns_display(cpu: &mut Cpu) -> bool {
    state(cpu).display.mode.0 != 0
}

/// Whether the card is an S3 ViRGE, whose "24-bit" color is 3 bytes a
/// pixel instead of 4.
fn packed24(cpu: &Cpu) -> bool {
    cpu.bus.vga.adapter.is_virge()
}

/// The sizes of the modes DirectDraw offers.
const SIZES: [(u32, u32); 6] = [
    (320, 200),
    (320, 240),
    (640, 400),
    (640, 480),
    (800, 600),
    (1024, 768),
];

/// The modes DirectDraw offers, as width, height and bits a pixel, by
/// size and then depth.
pub fn modes(cpu: &Cpu) -> Vec<(u32, u32, u32)> {
    let truecolor = if packed24(cpu) { 24 } else { 32 };
    let mut list = Vec::new();
    for (w, h) in SIZES {
        for bpp in [8, 16, truecolor] {
            list.push((w, h, bpp));
        }
    }
    list
}

/// The VBE mode of a size and depth DirectDraw offers.
fn vbe_mode(cpu: &Cpu, w: u32, h: u32, bpp: u32) -> Option<VbeMode> {
    if !modes(cpu).contains(&(w, h, bpp)) {
        return None;
    }
    let packed = packed24(cpu);
    let listed = vbe::MODES.iter().find(|m| {
        let depth = match m.bpp {
            32 if packed => 24,
            d => d as u32,
        };
        m.width as u32 == w && m.height as u32 == h && depth == bpp
    });
    if let Some(m) = listed {
        return vbe::find_mode(m.number, packed);
    }
    // One of DirectDraw's own, by a number no mode of the list has. Small
    // modes are shown doubled, at the timing of the doubled lines.
    let number = 0x1F0
        + SIZES.iter().position(|&s| s == (w, h)).unwrap_or(0) as u16 * 4
        + match bpp {
            8 => 0,
            16 => 1,
            _ => 2,
        };
    let shown = if w < 640 { h * 2 } else { h };
    Some(VbeMode {
        number,
        width: w as u16,
        height: h as u16,
        bpp: bpp as u8,
        timing: vbe::timing_for(shown as u16),
    })
}

/// The display's mode as programs see it: width, height, bits a pixel
/// and bytes a line. The desktop's when DirectDraw hasn't set one.
pub fn current(cpu: &mut Cpu) -> (u32, u32, u32, u32) {
    let (w, h, bpp) = state(cpu).display.mode;
    if w != 0 {
        return (w, h, bpp, cpu.bus.vbe.pitch);
    }
    let (w, h, bpp, pitch) = crate::win32::gdi32::surface::screen(cpu);
    (w as u32, h as u32, bpp, pitch as u32)
}

/// The bytes of video memory the picture on the screen takes.
fn screen_len(cpu: &mut Cpu) -> u32 {
    let (_, h, _, pitch) = current(cpu);
    pitch * h
}

/// Set the mode `w` x `h` x `bpp` for the DirectDraw object `owner`.
/// The surfaces in video memory are lost.
pub fn set_mode(cpu: &mut Cpu, owner: u32, w: u32, h: u32, bpp: u32) -> u32 {
    let Some(mode) = vbe_mode(cpu, w, h, bpp) else {
        return DDERR_INVALIDMODE;
    };
    super::surface::video_memory_lost(cpu);
    let display = &mut state(cpu).display;
    display.blocks.clear();
    display.mode = (w, h, bpp);
    display.mode_owner = owner;
    crate::interrupts::vbe::enter_mode(&mut cpu.bus, &mode, true, false);
    // The mouse moves over the whole picture.
    let mouse = &mut cpu.bus.mouse;
    (mouse.min_x, mouse.max_x, mouse.min_y, mouse.max_y) = (0, w as i32 - 1, 0, h as i32 - 1);
    DD_OK
}

/// Give the screen back to the desktop, if DirectDraw has it.
pub fn restore(cpu: &mut Cpu) {
    if !owns_display(cpu) {
        return;
    }
    super::surface::video_memory_lost(cpu);
    let display = &mut state(cpu).display;
    display.blocks.clear();
    display.mode = (0, 0, 0);
    display.mode_owner = 0;
    restore_screen(cpu);
}

/// The screen as it is without DirectDraw: the desktop, or text mode.
fn restore_screen(cpu: &mut Cpu) {
    if cpu.bus.win32.user.desktop {
        crate::win32::user32::paint::restore_desktop(cpu);
        let (w, h) = crate::win32::user32::sysparams::screen_size();
        let mouse = &mut cpu.bus.mouse;
        (mouse.min_x, mouse.max_x, mouse.min_y, mouse.max_y) = (0, w - 1, 0, h - 1);
    } else {
        crate::win32::dos(cpu, |cpu| crate::interrupts::int10::set_mode(cpu, 3));
    }
}

/// Take a block of `len` bytes of video memory: its offset, or the
/// error.
pub fn alloc(cpu: &mut Cpu, len: u32) -> Result<u32, u32> {
    let floor = screen_len(cpu).next_multiple_of(ALIGN);
    let len = len.max(1).next_multiple_of(ALIGN);
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
    if at as u64 + len as u64 > VRAM_SIZE as u64 {
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
    let total = VRAM_SIZE as u32 - screen_len(cpu);
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
    let lfb = cpu.bus.vbe.lfb_base.unwrap_or(LFB_BASE as u32);
    let len = VRAM_SIZE as u32;
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
    (cpu.bus.vga.timing(), now)
}

/// Show the picture at `offset` of video memory, from the next frame.
pub fn show(cpu: &mut Cpu, offset: u32) {
    cpu.bus.vbe.start = offset;
    cpu.bus.note_display_start();
    cpu.bus.vga.mark_dirty_full();
}

/// `len` bytes of video memory at `offset` were written: the rows of
/// the picture they are in are drawn again.
pub fn written(cpu: &mut Cpu, offset: u32, len: u32) {
    if cpu.bus.vbe.mode.is_some()
        && let Some((first, last)) = cpu.bus.vbe.frame_rows(offset as usize, len as usize)
    {
        cpu.bus.vga.mark_dirty_rows(first, last);
    }
}

crate::state_fields!(Display {
    mode,
    mode_owner,
    exclusive,
    blocks,
    views,
    flip_at
});
