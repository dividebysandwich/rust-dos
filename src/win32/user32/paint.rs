//! Painting: the DCs USER gives windows, update regions and WM_PAINT,
//! the desktop on the VBE screen, scrolling, and the drawing functions
//! of USER (FillRect, DrawEdge, DrawFrameControl, DrawIconEx).
//!
//! A window's update region is kept in window coordinates, so that it
//! covers the frame as well as the client area: what lies outside the
//! client area needs WM_NCPAINT. Visible regions are worked out when a
//! DC draws, from the tree as it is then, so moving a window changes
//! nothing here but what it uncovers and what it shows.
//!
//! The desktop is on the screen once a window shows or a program draws
//! there: the display goes to the VBE mode of the screen's size at 32
//! bits a pixel, and back to text when the last window goes.

use super::message::{WM_ERASEBKGND, WM_NCPAINT, WM_PAINT};
use super::sysparams::{self, COLOR_BACKGROUND, COLOR_BTNFACE, COLOR_WINDOW};
use super::window::{
    SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOREDRAW, SWP_NOSIZE, SWP_NOZORDER, WS_CLIPCHILDREN,
    WS_CLIPSIBLINGS, WS_MINIMIZE, WS_VISIBLE, Window, send,
};
use super::{
    DESKTOP, ERROR_INVALID_WINDOW_HANDLE, FALSE, State, TRUE, address_of, set_error, user,
};
use crate::cpu::Cpu;
use crate::win32::gdi32::dc::{
    DCX_CACHE, DCX_CLIPCHILDREN, DCX_CLIPSIBLINGS, DCX_EXCLUDERGN, DCX_INTERSECTRGN,
    DCX_PARENTCLIP, DCX_USESTYLE, DCX_WINDOW, WINDOW,
};
use crate::win32::gdi32::{self, Rect, Region, draw};
use crate::win32::task::{self, Next, Task};
use crate::win32::thread;

/// RedrawWindow's flags.
pub const RDW_INVALIDATE: u32 = 0x0001;
pub const RDW_INTERNALPAINT: u32 = 0x0002;
pub const RDW_ERASE: u32 = 0x0004;
pub const RDW_VALIDATE: u32 = 0x0008;
pub const RDW_NOINTERNALPAINT: u32 = 0x0010;
pub const RDW_NOERASE: u32 = 0x0020;
pub const RDW_NOCHILDREN: u32 = 0x0040;
pub const RDW_ALLCHILDREN: u32 = 0x0080;
pub const RDW_UPDATENOW: u32 = 0x0100;
pub const RDW_ERASENOW: u32 = 0x0200;
pub const RDW_FRAME: u32 = 0x0400;
pub const RDW_NOFRAME: u32 = 0x0800;

/// What a window's update needs besides WM_PAINT (`Window::paint`):
/// WM_ERASEBKGND, WM_NCPAINT, a WM_PAINT without an update region
/// (RDW_INTERNALPAINT), and an erase the window didn't do, which
/// BeginPaint reports as fErase.
pub const PAINT_ERASE: u32 = 0x01;
pub const PAINT_FRAME: u32 = 0x02;
pub const PAINT_INTERNAL: u32 = 0x04;
pub const PAINT_DELAYED_ERASE: u32 = 0x08;

/// ScrollWindowEx's flags.
const SW_SCROLLCHILDREN: u32 = 0x0001;
const SW_INVALIDATE: u32 = 0x0002;
const SW_ERASE: u32 = 0x0004;

/// Whether the window needs WM_PAINT.
pub fn needs_paint(w: &Window) -> bool {
    !w.update.is_empty() || w.paint & PAINT_INTERNAL != 0
}

/// The window tree changed: nothing is kept that depends on it, as
/// visible regions are worked out when a DC draws, so this only wakes
/// the threads, whose windows may need painting now.
pub fn tree_changed(cpu: &mut Cpu) {
    thread::wake(cpu);
}

// The visible regions of windows.

/// GetDCEx's flags as it fixes them for the window `hwnd`, as Windows
/// does.
fn fix_flags(state: &State, hwnd: u32, flags: u32) -> u32 {
    let Some(w) = state.window(hwnd) else {
        return flags;
    };
    let style = w.style;
    let class_style = state
        .classes
        .get(w.class as usize)
        .and_then(|c| c.as_ref())
        .map_or(0, |c| c.style);
    let mut flags = flags;
    if flags & (DCX_WINDOW | DCX_PARENTCLIP) != 0 {
        flags |= DCX_CACHE;
    }
    if flags & DCX_USESTYLE != 0 {
        flags &= !(DCX_CLIPCHILDREN | DCX_CLIPSIBLINGS | DCX_PARENTCLIP);
        if style & WS_CLIPSIBLINGS != 0 {
            flags |= DCX_CLIPSIBLINGS;
        }
        if flags & DCX_WINDOW == 0 {
            if class_style & super::class::CS_PARENTDC != 0 {
                flags |= DCX_PARENTCLIP;
            }
            if style & WS_CLIPCHILDREN != 0 && style & WS_MINIMIZE == 0 {
                flags |= DCX_CLIPCHILDREN;
            }
        }
    }
    if flags & DCX_WINDOW != 0 {
        flags &= !DCX_CLIPCHILDREN;
    }
    let parent = w.parent;
    if parent == 0 || parent == DESKTOP {
        flags = (flags & !DCX_PARENTCLIP) | DCX_CLIPSIBLINGS;
    }
    if flags & (DCX_CLIPSIBLINGS | DCX_CLIPCHILDREN) != 0 {
        flags &= !DCX_PARENTCLIP;
    }
    flags
}

/// The window's shape on the screen: its rectangle, or its region.
fn shape(state: &State, hwnd: u32) -> Region {
    let r = state.screen_rect(hwnd);
    match state.window(hwnd).and_then(|w| w.region.as_ref()) {
        Some(region) => region.offset_by(r.left, r.top).and_rect(&r),
        None => Region::from_rect(r),
    }
}

/// What of the screen the window `hwnd` may draw on, by GetDCEx's
/// `flags`: its client area (or whole window with DCX_WINDOW), less what
/// covers it.
pub fn visible_region(state: &State, hwnd: u32, flags: u32) -> Region {
    if hwnd == DESKTOP {
        let mut vis = shape(state, DESKTOP);
        if flags & DCX_CLIPCHILDREN != 0 {
            for child in state.top_levels() {
                if state
                    .window(child)
                    .is_some_and(|c| c.style & WS_VISIBLE != 0)
                {
                    vis = vis.diff(&shape(state, child));
                }
            }
        }
        return vis;
    }
    if !state.is_visible(hwnd) {
        return Region::new();
    }
    if flags & DCX_PARENTCLIP != 0 {
        let parent = state.parent(hwnd);
        let parent_flags = fix_flags(state, parent, DCX_USESTYLE) & !DCX_CLIPCHILDREN;
        return visible_region(state, parent, parent_flags);
    }
    let mut vis = if flags & DCX_WINDOW != 0 {
        shape(state, hwnd)
    } else {
        shape(state, hwnd).and_rect(&state.screen_client(hwnd))
    };
    if flags & DCX_CLIPCHILDREN != 0
        && let Some(w) = state.window(hwnd)
    {
        for &child in &w.children {
            if state
                .window(child)
                .is_some_and(|c| c.style & WS_VISIBLE != 0)
            {
                vis = vis.diff(&shape(state, child));
            }
        }
    }
    // Up the tree: each parent's client area, and the siblings above
    // that clip.
    let mut h = hwnd;
    let mut clip_siblings = flags & DCX_CLIPSIBLINGS != 0;
    while h != DESKTOP {
        let Some(w) = state.window(h) else {
            break;
        };
        let parent = w.parent;
        if clip_siblings || parent == DESKTOP {
            let siblings = state
                .window(parent)
                .map(|p| p.children.clone())
                .unwrap_or_default();
            for s in siblings.into_iter().take_while(|&s| s != h) {
                if state.window(s).is_some_and(|c| c.style & WS_VISIBLE != 0) {
                    vis = vis.diff(&shape(state, s));
                }
            }
        }
        if parent == DESKTOP || parent == 0 {
            break;
        }
        vis = vis.and_rect(&state.screen_client(parent));
        h = parent;
        clip_siblings = state
            .window(h)
            .is_some_and(|w| w.style & WS_CLIPSIBLINGS != 0);
    }
    vis
}

/// A window DC's device origin on the screen and the region it may draw
/// on there: for GDI.
pub fn dc_region(
    cpu: &mut Cpu,
    hwnd: u32,
    flags: u32,
    extra: Option<&Region>,
) -> ((i32, i32), Region) {
    let state = user(cpu);
    let hwnd = if hwnd == 0 { DESKTOP } else { hwnd };
    if !state.is_window(hwnd) {
        return ((0, 0), Region::new());
    }
    let origin = if flags & DCX_WINDOW != 0 {
        state.window_origin(hwnd)
    } else {
        state.client_origin(hwnd)
    };
    let mut vis = visible_region(state, hwnd, flags);
    if let Some(extra) = extra {
        if flags & DCX_INTERSECTRGN != 0 {
            vis = vis.and(extra);
        } else if flags & DCX_EXCLUDERGN != 0 {
            vis = vis.diff(extra);
        }
    }
    (origin, vis)
}

// The DCs of windows.

/// A DC for the window `hwnd`, with GetDCEx's `flags` and region
/// (screen coordinates): the window's own or class DC, or a new one of
/// the cache.
pub fn window_dc(cpu: &mut Cpu, hwnd: u32, flags: u32, extra: Option<Region>) -> u32 {
    window_dc_rgn(cpu, hwnd, flags, extra, 0)
}

/// Give the own or class DC `hdc` GetDCEx's `flags`. Without
/// DCX_INTERSECTRGN or DCX_EXCLUDERGN it keeps the region it has; with
/// one, `extra` replaces it, and the region handle it held is deleted
/// unless it is `handle` again. A class DC passing to another window
/// leaves its region behind, undeleted.
fn reuse_dc(cpu: &mut Cpu, hdc: u32, hwnd: u32, flags: u32, extra: Option<Region>, handle: u32) {
    let mut old = 0;
    gdi32::dc::with(cpu, hdc, |dc| {
        if dc.hwnd != hwnd {
            dc.hwnd = hwnd;
            dc.extra = None;
            dc.extra_rgn = 0;
            dc.flags &= !(DCX_INTERSECTRGN | DCX_EXCLUDERGN);
        }
        if flags & (DCX_INTERSECTRGN | DCX_EXCLUDERGN) == 0 {
            dc.flags = flags | dc.flags & (DCX_INTERSECTRGN | DCX_EXCLUDERGN);
            return;
        }
        if dc.extra_rgn != handle {
            old = dc.extra_rgn;
        }
        dc.flags = flags;
        dc.extra = extra;
        dc.extra_rgn = handle;
    });
    if old != 0 {
        gdi32::delete_object_now(cpu, old);
    }
}

/// window_dc, the region coming from the region handle `handle`, which
/// the DC then owns.
fn window_dc_rgn(cpu: &mut Cpu, hwnd: u32, flags: u32, extra: Option<Region>, handle: u32) -> u32 {
    let state = user(cpu);
    let flags = fix_flags(state, hwnd, flags);
    let Some(w) = state.window(hwnd) else {
        return 0;
    };
    let class = w.class as usize;
    let class_style = state
        .classes
        .get(class)
        .and_then(|c| c.as_ref())
        .map_or(0, |c| c.style);
    if flags & DCX_CACHE == 0 && hwnd != DESKTOP {
        if class_style & super::class::CS_OWNDC != 0 {
            let mut hdc = w.dc;
            if hdc == 0 {
                hdc = gdi32::dc::new_window_dc(cpu, hwnd, flags, None, true);
                if let Some(w) = user(cpu).window_mut(hwnd) {
                    w.dc = hdc;
                    w.flags |= super::window::HAS_DC;
                }
            }
            reuse_dc(cpu, hdc, hwnd, flags, extra, handle);
            return hdc;
        }
        if class_style & super::class::CS_CLASSDC != 0 {
            let mut hdc = state.classes[class].as_ref().map_or(0, |c| c.dc);
            if hdc == 0 || gdi32::gdi(cpu).dc(hdc).is_none() {
                hdc = gdi32::dc::new_window_dc(cpu, hwnd, flags, None, true);
                if let Some(c) = user(cpu).classes[class].as_mut() {
                    c.dc = hdc;
                }
            }
            reuse_dc(cpu, hdc, hwnd, flags, extra, handle);
            return hdc;
        }
    }
    let hdc = gdi32::dc::new_window_dc(cpu, hwnd, flags, extra, false);
    gdi32::dc::with(cpu, hdc, |dc| dc.extra_rgn = handle);
    hdc
}

/// GetDC(hwnd): 0 is the desktop's, which draws over every window.
pub fn get_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        super::init(cpu);
        return Some(window_dc(cpu, DESKTOP, 0, None));
    }
    get_dc_ex(cpu, &[args[0], 0, DCX_USESTYLE])
}

/// GetDCEx(hwnd, region, flags).
pub fn get_dc_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, rgn, flags) = (args[0], args[1], args[2]);
    super::init(cpu);
    let hwnd = if hwnd == 0 { DESKTOP } else { hwnd };
    if !user(cpu).is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    }
    // The DC owns the region from now on. Without one, an empty region
    // stands in.
    let (extra, handle) = if flags & (DCX_INTERSECTRGN | DCX_EXCLUDERGN) == 0 {
        (None, 0)
    } else if rgn > 1
        && let Some(r) = gdi32::region_of(cpu, rgn)
    {
        (Some(r), rgn)
    } else {
        (Some(Region::new()), 0)
    };
    Some(window_dc_rgn(cpu, hwnd, flags, extra, handle))
}

/// GetWindowDC(hwnd): a DC for the whole window.
pub fn get_window_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_dc_ex(cpu, &[args[0], 0, DCX_USESTYLE | DCX_WINDOW])
}

/// ReleaseDC(hwnd, hdc): a DC of the cache goes; a window's own or class
/// DC stays, without the region BeginPaint or GetDCEx gave it.
pub fn release_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(release(cpu, args[1], false))
}

/// Release the window DC `hdc`: a DC of the cache goes with its region;
/// an own or class DC keeps its region unless `end_paint`.
fn release(cpu: &mut Cpu, hdc: u32, end_paint: bool) -> u32 {
    let Some(dc) = gdi32::gdi(cpu).dc(hdc) else {
        return FALSE;
    };
    if dc.kind != WINDOW {
        return FALSE;
    }
    let (own, handle) = (dc.own, dc.extra_rgn);
    if own && !end_paint {
        return TRUE;
    }
    if own {
        gdi32::dc::with(cpu, hdc, |dc| {
            dc.extra = None;
            dc.extra_rgn = 0;
            dc.flags &= !(DCX_INTERSECTRGN | DCX_EXCLUDERGN);
        });
    } else {
        gdi32::dc::free_window_dc(cpu, hdc);
    }
    if handle != 0 {
        gdi32::delete_object_now(cpu, handle);
    }
    TRUE
}

/// WindowFromDC(hdc).
pub fn window_from_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match gdi32::gdi(cpu).dc(args[0]) {
        Some(dc) if dc.kind == WINDOW => {
            if dc.hwnd == 0 {
                DESKTOP
            } else {
                dc.hwnd
            }
        }
        _ => 0,
    })
}

// Update regions.

/// Change the update regions of `hwnd` and its children by RedrawWindow's
/// `flags`, over `area` (screen coordinates).
fn redraw_tree(state: &mut State, hwnd: u32, area: &Region, flags: u32) {
    redraw_in(state, hwnd, area, flags, false);
}

/// redraw_tree for `hwnd`; `from_parent` when its parent's area spreads
/// to it. A CS_PARENTDC window then takes all of it, as it draws on its
/// parent's client area.
fn redraw_in(state: &mut State, hwnd: u32, area: &Region, flags: u32, from_parent: bool) {
    let Some(w) = state.window(hwnd) else {
        return;
    };
    let style = w.style;
    let children = w.children.clone();
    let wr = state.screen_rect(hwnd);
    let client = state.screen_client(hwnd);
    let own = area.and(&shape(state, hwnd));
    let mut part = own.clone();
    if flags & RDW_FRAME == 0 || hwnd == DESKTOP {
        part = part.and_rect(&client);
    }
    let frame = !part.diff_rect(&client).is_empty();
    if from_parent && flags & RDW_INVALIDATE != 0 && parent_dc(state, hwnd) && !own.is_empty() {
        part = area.clone();
    }
    let local = part.offset_by(-wr.left, -wr.top);
    if let Some(w) = state.window_mut(hwnd) {
        if flags & RDW_INVALIDATE != 0 {
            if !local.is_empty() {
                w.update = w.update.or(&local);
                if flags & RDW_ERASE != 0 {
                    w.paint |= PAINT_ERASE;
                }
                if frame {
                    w.paint |= PAINT_FRAME;
                }
            }
            if flags & RDW_INTERNALPAINT != 0 {
                w.paint |= PAINT_INTERNAL;
            }
        } else if flags & RDW_VALIDATE != 0 {
            if !local.is_empty() {
                w.update = w.update.diff(&local);
            }
            if flags & RDW_NOFRAME != 0 || w.update.is_empty() {
                w.paint &= !PAINT_FRAME;
            }
            if flags & RDW_NOERASE != 0 || w.update.is_empty() {
                w.paint &= !(PAINT_ERASE | PAINT_DELAYED_ERASE);
            }
            if flags & RDW_NOINTERNALPAINT != 0 {
                w.paint &= !PAINT_INTERNAL;
            }
        }
    }
    let to_children = flags & RDW_NOCHILDREN == 0
        && (flags & RDW_ALLCHILDREN != 0 || style & WS_CLIPCHILDREN == 0);
    if !to_children {
        return;
    }
    let inside = area.and_rect(&client);
    for child in children {
        if state
            .window(child)
            .is_some_and(|c| c.style & WS_VISIBLE != 0)
        {
            redraw_in(state, child, &inside, flags & !RDW_NOCHILDREN, true);
        }
    }
}

/// Whether the child window `hwnd`'s class has CS_PARENTDC.
fn parent_dc(state: &State, hwnd: u32) -> bool {
    let Some(w) = state.window(hwnd) else {
        return false;
    };
    let class_style = state
        .classes
        .get(w.class as usize)
        .and_then(|c| c.as_ref())
        .map_or(0, |c| c.style);
    w.style & super::window::WS_CHILD != 0 && class_style & super::class::CS_PARENTDC != 0
}

/// Invalidate the part `area` of the screen in `hwnd` and its children,
/// with its frame and to be erased: for what a change uncovered.
fn invalidate_screen(state: &mut State, hwnd: u32, area: &Region) {
    redraw_tree(
        state,
        hwnd,
        area,
        RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN,
    );
}

/// What a window that is hidden needs painted goes: a window shows
/// whole.
fn clear_updates(state: &mut State, hwnd: u32) {
    for h in super::window::pre_order(state, hwnd) {
        if let Some(w) = state.window_mut(h) {
            w.update = Region::new();
            w.paint = 0;
        }
    }
}

/// `hwnd` took a new size, frame or shape: its update region loses what
/// now lies outside it, or that part would ask for WM_PAINT that nothing
/// can validate. A CS_PARENTDC child keeps what it has over its parent's
/// client area. What is now in the frame needs WM_NCPAINT, which crops it
/// (`erase_step`), as ValidateRect only covers the client area.
pub fn crop_update(state: &mut State, hwnd: u32) {
    let Some(w) = state.window(hwnd) else {
        return;
    };
    if w.update.is_empty() {
        return;
    }
    let wr = state.screen_rect(hwnd);
    let update = w.update.offset_by(wr.left, wr.top);
    let own = update.and(&shape(state, hwnd));
    let frame = !own.diff_rect(&state.screen_client(hwnd)).is_empty();
    let kept = if parent_dc(state, hwnd) {
        update.and_rect(&state.screen_client(w.parent))
    } else {
        own
    };
    let Some(w) = state.window_mut(hwnd) else {
        return;
    };
    w.update = kept.offset_by(-wr.left, -wr.top);
    if w.update.is_empty() {
        w.paint &= !(PAINT_ERASE | PAINT_FRAME | PAINT_DELAYED_ERASE);
    } else if frame {
        w.paint |= PAINT_FRAME;
    }
}

/// Paint the desktop's background over `area` of the screen, where no
/// window covers it.
fn paint_background(cpu: &mut Cpu, area: &Region) {
    if !user(cpu).desktop || area.is_empty() {
        return;
    }
    let hdc = gdi32::dc::new_window_dc(
        cpu,
        DESKTOP,
        DCX_CLIPCHILDREN | DCX_INTERSECTRGN,
        Some(area.clone()),
        false,
    );
    if hdc == 0 {
        return;
    }
    let brush = sysparams::brush(cpu, COLOR_BACKGROUND);
    let (w, h) = sysparams::screen_size(cpu);
    draw::fill_device_rect(cpu, hdc, Rect::new(0, 0, w, h), brush);
    gdi32::dc::free_window_dc(cpu, hdc);
}

/// The part `area` of the screen shows what is under it now: the
/// desktop paints its part at once, and the windows there need
/// painting.
pub fn expose(cpu: &mut Cpu, area: &Region) {
    if area.is_empty() {
        return;
    }
    let state = user(cpu);
    for top in state.top_levels() {
        if state.window(top).is_some_and(|w| w.style & WS_VISIBLE != 0) {
            invalidate_screen(state, top, area);
        }
    }
    paint_background(cpu, area);
    thread::wake(cpu);
}

/// What of the window `hwnd` the visible siblings above it cover, on the
/// screen.
pub fn covered(state: &State, hwnd: u32) -> Region {
    let siblings = state
        .window(state.parent(hwnd))
        .map_or(&[][..], |p| &p.children[..]);
    let mut area = Region::new();
    for &h in siblings.iter().take_while(|&&h| h != hwnd) {
        if state.window(h).is_some_and(|w| w.style & WS_VISIBLE != 0) {
            area = area.or(&shape(state, h));
        }
    }
    area.and(&shape(state, hwnd))
}

/// SetWindowPos brought `hwnd` above siblings that covered `before` of
/// it: what they no longer cover needs painting.
pub fn raised(cpu: &mut Cpu, hwnd: u32, before: &Region) {
    let state = user(cpu);
    let shown = before.diff(&covered(state, hwnd));
    if !shown.is_empty() {
        invalidate_screen(state, hwnd, &shown);
        thread::wake(cpu);
    }
}

/// SetWindowPos moved, sized, showed or hid `hwnd`: what it uncovered
/// shows what is under it, and the window needs painting where it is.
pub fn moved(
    cpu: &mut Cpu,
    hwnd: u32,
    old: Rect,
    was_visible: bool,
    now_visible: bool,
    flags: u32,
) {
    if now_visible && !user(cpu).desktop {
        ensure_desktop(cpu);
    }
    let state = user(cpu);
    let new = state.screen_rect(hwnd);
    if !now_visible {
        clear_updates(state, hwnd);
    }
    if flags & SWP_NOREDRAW != 0 && flags & SWP_HIDEWINDOW == 0 {
        return;
    }
    if was_visible && (!now_visible || old != new) {
        let mut gone = Region::from_rect(old);
        if now_visible {
            gone = gone.diff_rect(&new);
        }
        // What is under it: its parent, or the desktop and the windows
        // there.
        let parent = state.parent(hwnd);
        if parent != DESKTOP && parent != 0 {
            invalidate_screen(state, parent, &gone);
        } else {
            for top in state.top_levels() {
                if top != hwnd && state.window(top).is_some_and(|w| w.style & WS_VISIBLE != 0) {
                    invalidate_screen(state, top, &gone);
                }
            }
            let mut background = gone.clone();
            for top in state.top_levels() {
                if state.window(top).is_some_and(|w| w.style & WS_VISIBLE != 0) {
                    background = background.diff(&shape(state, top));
                }
            }
            paint_background(cpu, &background);
        }
    }
    let state = user(cpu);
    if now_visible && (!was_visible || old != new || flags & super::window::SWP_FRAMECHANGED != 0) {
        let area = Region::from_rect(new);
        invalidate_screen(state, hwnd, &area);
    }
    thread::wake(cpu);
}

/// Put the desktop on the screen, if it isn't: the VBE mode, the
/// background and the windows that show.
pub fn ensure_desktop(cpu: &mut Cpu) {
    if user(cpu).desktop {
        return;
    }
    super::init(cpu);
    user(cpu).desktop = true;
    // A mode DirectDraw set stays, with the mouse's range over it, and
    // the desktop comes back with the mode it gives back
    // (`restore_desktop`).
    if !crate::win32::ddraw::owns_display(cpu) {
        enter_desktop_mode(cpu);
        // The mouse moves over the whole desktop, from its middle.
        let (w, h) = sysparams::screen_size(cpu);
        let mouse = &mut cpu.bus.mouse;
        (mouse.min_x, mouse.max_x, mouse.min_y, mouse.max_y) = (0, w - 1, 0, h - 1);
        mouse.warp(w / 2, h / 2);
    }
    super::input::desktop_started(cpu);
    redraw_screen(cpu);
}

/// Set the desktop's display mode (`screen::desktop`).
fn enter_desktop_mode(cpu: &mut Cpu) {
    crate::win32::screen::set_desktop_mode(cpu);
}

/// Put the desktop back on the screen after a DirectDraw mode: its
/// mode again, and everything painted.
pub fn restore_desktop(cpu: &mut Cpu) {
    if !user(cpu).desktop {
        return;
    }
    enter_desktop_mode(cpu);
    redraw_screen(cpu);
}

/// Take the desktop off the screen: the display goes back to text.
pub fn leave_desktop(cpu: &mut Cpu) {
    if !user(cpu).desktop {
        return;
    }
    user(cpu).desktop = false;
    crate::win32::screen::set_text_mode(cpu);
}

/// Paint the whole screen again: after the colors changed, or when the
/// desktop comes on it.
pub fn redraw_screen(cpu: &mut Cpu) {
    if !user(cpu).desktop {
        return;
    }
    let (w, h) = sysparams::screen_size(cpu);
    expose(cpu, &Region::from_rect(Rect::new(0, 0, w, h)));
}

// RedrawWindow and the rest, and their task.

const TASK_PAINT: u16 = 10;

/// The kinds of the paint task.
const K_BEGIN: u32 = 0;
const K_REDRAW: u32 = 1;
const K_UPDATE_RECT: u32 = 2;
const K_UPDATE_RGN: u32 = 3;
const K_SCROLL: u32 = 4;

/// The task's variables: its kind, the window, flags, the place in the
/// list, the DC, the structure it fills, a result, the window being
/// erased, and then the list of windows.
const V_KIND: usize = 0;
const V_HWND: usize = 1;
const V_FLAGS: usize = 2;
const V_AT: usize = 3;
const V_HDC: usize = 4;
const V_OUT: usize = 5;
const V_RESULT: usize = 6;
const V_CUR: usize = 7;
const V_LIST: usize = 8;

/// The steps of erasing a window (WM_NCPAINT, WM_ERASEBKGND), which the
/// kinds share, and the step each goes on with after.
const STEP_ERASE: u16 = 100;
const STEP_ERASE_BKGND: u16 = 101;
const STEP_ERASED: u16 = 102;

/// The region a window's DC is clipped to for its update: its update
/// region within its client area, on the screen.
fn client_update(state: &State, hwnd: u32) -> Region {
    let Some(w) = state.window(hwnd) else {
        return Region::new();
    };
    let wr = state.screen_rect(hwnd);
    // A CS_PARENTDC window's may reach over its parent's client area.
    let area = if parent_dc(state, hwnd) {
        state.screen_client(w.parent)
    } else {
        state.screen_client(hwnd)
    };
    w.update.offset_by(wr.left, wr.top).and_rect(&area)
}

/// Start erasing the window in V_CUR: its frame, then its background.
/// `keep` keeps the DC it erased with (BeginPaint's), which is in V_HDC.
fn erase_step(cpu: &mut Cpu, t: &mut Task, value: u32, keep: bool) -> Option<Next> {
    let hwnd = t.var(V_CUR);
    loop {
        match t.step {
            STEP_ERASE => {
                let state = user(cpu);
                let wr = state.screen_rect(hwnd);
                let client = match state.window(hwnd)? {
                    w if parent_dc(state, hwnd) => state.screen_client(w.parent),
                    _ => state.screen_client(hwnd),
                };
                let client = client.offset(-wr.left, -wr.top);
                let w = state.window_mut(hwnd)?;
                if w.paint & PAINT_FRAME != 0 && hwnd != DESKTOP {
                    // The frame is painted: what is left to update is in
                    // the client area (`client_update`'s), or a window
                    // that validates its client area would get WM_PAINT
                    // forever.
                    w.paint &= !PAINT_FRAME;
                    w.update = w.update.and_rect(&client);
                    return Some(send(hwnd, WM_NCPAINT, 1, 0, STEP_ERASE_BKGND));
                }
                t.step = STEP_ERASE_BKGND;
            }
            STEP_ERASE_BKGND => {
                let state = user(cpu);
                let w = state.window(hwnd)?;
                let erase = w.paint & PAINT_ERASE != 0;
                if !erase && !keep {
                    return None;
                }
                let iconic = w.style & WS_MINIMIZE != 0;
                let region = client_update(state, hwnd);
                if let Some(w) = state.window_mut(hwnd) {
                    w.paint &= !PAINT_ERASE;
                }
                let mut hdc = t.var(V_HDC);
                if hdc == 0 {
                    let flags =
                        DCX_INTERSECTRGN | DCX_USESTYLE | if iconic { DCX_WINDOW } else { 0 };
                    hdc = window_dc(cpu, hwnd, flags, Some(region.clone()));
                    t.set(V_HDC, hdc);
                }
                let (vis, _) = {
                    let dc = gdi32::gdi(cpu).dc(hdc).cloned();
                    match dc {
                        Some(dc) => gdi32::dc::visible(cpu, &dc),
                        None => (Region::new(), (0, 0)),
                    }
                };
                if erase && !vis.is_empty() {
                    t.step = STEP_ERASED;
                    return Some(send(hwnd, WM_ERASEBKGND, hdc, 0, STEP_ERASED));
                }
                // Nothing to erase on: done, as if the window had.
                t.step = STEP_ERASED;
                return erase_done(cpu, t, if erase { 1 } else { 0 }, keep, !erase);
            }
            STEP_ERASED => return erase_done(cpu, t, value, keep, false),
            _ => return None,
        }
    }
}

/// The window erased, its WM_ERASEBKGND returning `value`: an erase it
/// didn't do is still to be done. None: the erasing is over.
fn erase_done(cpu: &mut Cpu, t: &mut Task, value: u32, keep: bool, skipped: bool) -> Option<Next> {
    let hwnd = t.var(V_CUR);
    if !skipped
        && value == 0
        && let Some(w) = user(cpu).window_mut(hwnd)
    {
        w.paint |= PAINT_DELAYED_ERASE;
    }
    if !keep {
        let hdc = t.var(V_HDC);
        if hdc != 0 {
            release(cpu, hdc, true);
            t.set(V_HDC, 0);
        }
    }
    None
}

/// BeginPaint(hwnd, ps): the window's DC clipped to its update region,
/// after WM_NCPAINT and WM_ERASEBKGND; the update region is validated.
pub fn begin_paint(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, ps) = (args[0], args[1]);
    if ps == 0 {
        return Some(0);
    }
    if !user(cpu).is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    }
    super::input::hide_caret(cpu, &[hwnd]);
    // The DC clipped to the update region as it is now, which is then
    // validated, frame and all.
    let state = user(cpu);
    let region = client_update(state, hwnd);
    let iconic = state
        .window(hwnd)
        .is_some_and(|w| w.style & WS_MINIMIZE != 0);
    let flags = DCX_INTERSECTRGN | DCX_USESTYLE | if iconic { DCX_WINDOW } else { 0 };
    let hdc = window_dc(cpu, hwnd, flags, Some(region));
    if let Some(w) = user(cpu).window_mut(hwnd) {
        w.update = Region::new();
        w.paint &= !PAINT_INTERNAL;
    }
    let vars = vec![K_BEGIN, hwnd, 0, 0, hdc, ps, 0, hwnd];
    task::start(cpu, TASK_PAINT, vars, 0)
}

/// Fill the PAINTSTRUCT at `ps`: the DC, whether the window still needs
/// erasing, and the box of what it paints.
fn write_paint_struct(cpu: &mut Cpu, hwnd: u32, hdc: u32, ps: u32) {
    let erase = user(cpu)
        .window_mut(hwnd)
        .map(|w| {
            let e = w.paint & PAINT_DELAYED_ERASE != 0;
            w.paint &= !PAINT_DELAYED_ERASE;
            e
        })
        .unwrap_or(false);
    let mut bytes = vec![0u8; 64];
    bytes[0..4].copy_from_slice(&hdc.to_le_bytes());
    bytes[4..8].copy_from_slice(&(erase as u32).to_le_bytes());
    cpu.bus.guest_write_bytes(ps, &bytes);
    gdi32::dc::get_clip_box(cpu, &[hdc, ps + 8]);
    // What to paint is within the client area, whatever the DC reaches.
    let client = user(cpu).screen_client(hwnd);
    let size = Rect::new(0, 0, client.width(), client.height());
    let paint = Rect::read(cpu, ps + 8).intersect(&size);
    let paint = if paint.is_empty() {
        Rect::default()
    } else {
        paint
    };
    paint.write(cpu, ps + 8);
}

/// EndPaint(hwnd, ps).
pub fn end_paint(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, ps) = (args[0], args[1]);
    if ps == 0 {
        return Some(FALSE);
    }
    let hdc = cpu.bus.guest_read_32(ps);
    release(cpu, hdc, true);
    super::input::show_caret(cpu, &[hwnd]);
    Some(TRUE)
}

/// RedrawWindow(hwnd, rect, region, flags): the rectangle and region are
/// in client coordinates.
pub fn redraw_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, rect, rgn, flags) = (args[0], args[1], args[2], args[3]);
    super::init(cpu);
    let hwnd = if hwnd == 0 { DESKTOP } else { hwnd };
    if !user(cpu).is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    let state = user(cpu);
    let client = state.screen_client(hwnd);
    let area = if rgn != 0 {
        match gdi32::region_of(cpu, rgn) {
            Some(r) => r.offset_by(client.left, client.top),
            None => return Some(FALSE),
        }
    } else if rect != 0 {
        let r = Rect::read(cpu, rect);
        if r.is_empty() {
            Region::new()
        } else {
            Region::from_rect(r.offset(client.left, client.top))
        }
    } else if flags & RDW_FRAME != 0 {
        Region::from_rect(user(cpu).screen_rect(hwnd))
    } else {
        Region::from_rect(client)
    };
    redraw(cpu, hwnd, &area, flags)
}

/// RedrawWindow's work, over `area` of the screen.
fn redraw(cpu: &mut Cpu, hwnd: u32, area: &Region, flags: u32) -> Option<u32> {
    let state = user(cpu);
    if !state.is_visible(hwnd) {
        return Some(TRUE);
    }
    if flags & (RDW_INVALIDATE | RDW_VALIDATE | RDW_INTERNALPAINT | RDW_NOINTERNALPAINT) != 0 {
        if hwnd == DESKTOP && flags & RDW_INVALIDATE != 0 {
            // The desktop paints itself at once.
            let mut background = area.clone();
            for top in state.top_levels() {
                if state.window(top).is_some_and(|w| w.style & WS_VISIBLE != 0) {
                    background = background.diff(&shape(state, top));
                }
            }
            if flags & RDW_ALLCHILDREN != 0 {
                for top in state.top_levels() {
                    if state.window(top).is_some_and(|w| w.style & WS_VISIBLE != 0) {
                        redraw_tree(state, top, area, flags);
                    }
                }
            }
            paint_background(cpu, &background);
        } else {
            redraw_tree(state, hwnd, area, flags);
        }
        thread::wake(cpu);
    }
    if flags & (RDW_UPDATENOW | RDW_ERASENOW) == 0 {
        return Some(TRUE);
    }
    let state = user(cpu);
    let list = if flags & RDW_NOCHILDREN != 0 {
        vec![hwnd]
    } else {
        super::window::pre_order(state, hwnd)
    };
    let mut vars = vec![K_REDRAW, hwnd, flags, 0, 0, 0, 0, 0];
    vars.extend(list);
    task::start(cpu, TASK_PAINT, vars, 0)
}

/// UpdateWindow(hwnd): WM_PAINT now, to it and its children that need
/// it.
pub fn update_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 || !user(cpu).is_window(args[0]) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    redraw_window(cpu, &[args[0], 0, 0, RDW_UPDATENOW | RDW_ALLCHILDREN])
}

/// InvalidateRect(hwnd, rect, erase): 0 for the window repaints all.
pub fn invalidate_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, rect, erase) = (args[0], args[1], args[2]);
    if hwnd == 0 {
        let flags = RDW_ALLCHILDREN | RDW_INVALIDATE | RDW_FRAME | RDW_ERASE | RDW_ERASENOW;
        return redraw_window(cpu, &[0, 0, 0, flags]);
    }
    let flags = RDW_INVALIDATE | if erase != 0 { RDW_ERASE } else { 0 };
    redraw_window(cpu, &[hwnd, rect, 0, flags])
}

/// InvalidateRgn(hwnd, region, erase).
pub fn invalidate_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, rgn, erase) = (args[0], args[1], args[2]);
    if hwnd == 0 {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    let flags = RDW_INVALIDATE | if erase != 0 { RDW_ERASE } else { 0 };
    redraw_window(cpu, &[hwnd, 0, rgn, flags])
}

/// ValidateRect(hwnd, rect): 0 for the window repaints all, as
/// InvalidateRect does.
pub fn validate_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return invalidate_rect(cpu, &[0, 0, 1]);
    }
    redraw_window(cpu, &[args[0], args[1], 0, RDW_VALIDATE])
}

/// ValidateRgn(hwnd, region).
pub fn validate_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    redraw_window(cpu, &[args[0], 0, args[1], RDW_VALIDATE])
}

/// GetUpdateRect(hwnd, rect, erase): whether the window needs painting,
/// with the box of its update region in client coordinates.
pub fn get_update_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, rect, erase) = (args[0], args[1], args[2]);
    if !user(cpu).is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    if erase != 0 {
        let vars = vec![K_UPDATE_RECT, hwnd, 0, 0, 0, rect, 0, hwnd];
        return task::start(cpu, TASK_PAINT, vars, 0);
    }
    Some(update_rect_result(cpu, hwnd, rect))
}

fn update_rect_result(cpu: &mut Cpu, hwnd: u32, rect: u32) -> u32 {
    let state = user(cpu);
    let region = client_update(state, hwnd);
    let client = state.screen_client(hwnd);
    let any = state.window(hwnd).is_some_and(|w| !w.update.is_empty());
    if rect != 0 {
        let b = if region.is_empty() {
            Rect::default()
        } else {
            region.bounds().offset(-client.left, -client.top)
        };
        b.write(cpu, rect);
    }
    any as u32
}

/// GetUpdateRgn(hwnd, region, erase): the update region in client
/// coordinates, and its kind.
pub fn get_update_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, rgn, erase) = (args[0], args[1], args[2]);
    if !user(cpu).is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    }
    if erase != 0 {
        let vars = vec![K_UPDATE_RGN, hwnd, 0, 0, 0, rgn, 0, hwnd];
        return task::start(cpu, TASK_PAINT, vars, 0);
    }
    Some(update_rgn_result(cpu, hwnd, rgn))
}

fn update_rgn_result(cpu: &mut Cpu, hwnd: u32, rgn: u32) -> u32 {
    let state = user(cpu);
    let client = state.screen_client(hwnd);
    let region = client_update(state, hwnd).offset_by(-client.left, -client.top);
    let kind = region.kind();
    if !gdi32::set_region(cpu, rgn, region) {
        return 0;
    }
    kind
}

/// ExcludeUpdateRgn(hdc, hwnd): the DC no longer draws where the window
/// needs painting.
pub fn exclude_update_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, hwnd) = (args[0], args[1]);
    let Some(dc) = gdi32::gdi(cpu).dc(hdc).cloned() else {
        return Some(0);
    };
    if !user(cpu).is_window(hwnd) {
        return Some(0);
    }
    let update = client_update(user(cpu), hwnd);
    let (vis, origin) = gdi32::dc::visible(cpu, &dc);
    let update = update.offset_by(-origin.0, -origin.1);
    let (w, h) = gdi32::dc::device_size(cpu, &dc);
    let clip = dc
        .attrs
        .clip
        .clone()
        .unwrap_or_else(|| Region::from_rect(Rect::new(0, 0, w, h)))
        .diff(&update);
    let kind = clip.and(&vis).kind();
    gdi32::dc::with(cpu, hdc, |d| d.attrs.clip = Some(clip));
    Some(kind)
}

/// The paint task's steps.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.var(V_KIND) {
        K_BEGIN => begin_step(cpu, t, value),
        K_REDRAW => redraw_step(cpu, t, value),
        K_UPDATE_RECT | K_UPDATE_RGN => {
            if t.step == 0 {
                t.step = STEP_ERASE;
            }
            if let Some(next) = erase_step(cpu, t, value, false) {
                return next;
            }
            let (hwnd, out) = (t.var(V_HWND), t.var(V_OUT));
            Next::Done(if t.var(V_KIND) == K_UPDATE_RECT {
                update_rect_result(cpu, hwnd, out)
            } else {
                update_rgn_result(cpu, hwnd, out)
            })
        }
        K_SCROLL => scroll_step(cpu, t, value),
        _ => Next::Done(0),
    }
}

fn begin_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == 0 {
        t.step = STEP_ERASE;
    }
    if let Some(next) = erase_step(cpu, t, value, true) {
        return next;
    }
    let (hwnd, hdc, ps) = (t.var(V_HWND), t.var(V_HDC), t.var(V_OUT));
    write_paint_struct(cpu, hwnd, hdc, ps);
    Next::Done(hdc)
}

/// RedrawWindow's RDW_ERASENOW and RDW_UPDATENOW: the windows in the
/// list erased, or sent WM_PAINT, each once.
fn redraw_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let flags = t.var(V_FLAGS);
    loop {
        if t.step >= STEP_ERASE {
            if let Some(next) = erase_step(cpu, t, value, false) {
                return next;
            }
            t.step = 0;
            continue;
        }
        if t.step == 1 {
            // WM_PAINT returned.
            t.step = 0;
        }
        let at = t.var(V_AT) as usize;
        let Some(&hwnd) = t.vars.get(V_LIST + at) else {
            return Next::Done(TRUE);
        };
        let state = user(cpu);
        let Some(w) = state.window(hwnd) else {
            t.set(V_AT, at as u32 + 1);
            continue;
        };
        if !state.is_visible(hwnd) {
            t.set(V_AT, at as u32 + 1);
            continue;
        }
        let paint = w.paint;
        let needs = needs_paint(w);
        if flags & RDW_UPDATENOW != 0 && hwnd != DESKTOP {
            t.set(V_AT, at as u32 + 1);
            if needs {
                t.step = 1;
                return send(hwnd, WM_PAINT, 0, 0, 1);
            }
            continue;
        }
        if paint & (PAINT_ERASE | PAINT_FRAME) != 0 && t.step == 0 {
            // Erase this one, then come back to the next.
            t.set(V_AT, at as u32 + 1);
            t.set(V_CUR, hwnd);
            t.set(V_HDC, 0);
            t.step = STEP_ERASE;
            continue;
        }
        t.set(V_AT, at as u32 + 1);
    }
}

/// After DispatchMessageA of WM_PAINT: a window that painted without
/// BeginPaint still gets the WM_NCPAINT and WM_ERASEBKGND it needs. The
/// call goes on at `step`.
pub fn pending_erase(cpu: &mut Cpu, hwnd: u32, step: u16) -> Option<Next> {
    let w = user(cpu).window(hwnd)?;
    if w.paint & (PAINT_ERASE | PAINT_FRAME) == 0 {
        return None;
    }
    let flags = RDW_NOINTERNALPAINT | RDW_ERASENOW | RDW_NOCHILDREN;
    Some(Next::call(
        address_of("RedrawWindow"),
        &[hwnd, 0, 0, flags],
        step,
    ))
}

// Scrolling.

/// Copy `rect` (or the DC's clip box) of the DC by (`dx`, `dy`) within
/// `clip`: the logical rectangles it copied to and the part it uncovered.
fn scroll_bits(
    cpu: &mut Cpu,
    hdc: u32,
    dx: i32,
    dy: i32,
    rect: Option<Rect>,
    clip: Option<Rect>,
) -> (Rect, Region) {
    let Some(dc) = gdi32::gdi(cpu).dc(hdc).cloned() else {
        return (Rect::default(), Region::new());
    };
    let (vis, _) = gdi32::dc::effective_clip(cpu, &dc);
    let b = vis.bounds();
    let (l, t) = dc.to_logical(b.left, b.top);
    let (r, bt) = dc.to_logical(b.right, b.bottom);
    let whole = Rect::new(l, t, r, bt).ordered();
    let src = rect.unwrap_or(whole);
    let clip = clip.map_or(whole, |c| c.intersect(&whole));
    let src = src.intersect(&clip);
    let dst = src.offset(dx, dy).intersect(&clip);
    if !dst.is_empty() {
        let from = dst.offset(-dx, -dy);
        let args = [
            hdc,
            dst.left as u32,
            dst.top as u32,
            dst.width() as u32,
            dst.height() as u32,
            hdc,
            from.left as u32,
            from.top as u32,
            0x00CC_0020,
        ];
        draw::bit_blt(cpu, &args);
    }
    let uncovered = Region::from_rect(src).diff_rect(&dst);
    (dst, uncovered)
}

/// ScrollDC(hdc, dx, dy, scroll, clip, update region, update rect).
pub fn scroll_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hdc, dx, dy, scroll, clip, rgn, prc] = args else {
        unreachable!()
    };
    if gdi32::gdi(cpu).dc(hdc).is_none() {
        return Some(FALSE);
    }
    let rect = (scroll != 0).then(|| Rect::read(cpu, scroll));
    let clip = (clip != 0).then(|| Rect::read(cpu, clip));
    let (_, uncovered) = scroll_bits(cpu, hdc, dx as i32, dy as i32, rect, clip);
    if rgn != 0 {
        gdi32::set_region(cpu, rgn, uncovered.clone());
    }
    if prc != 0 {
        uncovered.bounds().write(cpu, prc);
    }
    Some(TRUE)
}

/// ScrollWindow(hwnd, dx, dy, rect, clip): its children move with it when
/// it scrolls all its client area.
pub fn scroll_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let flags = SW_INVALIDATE | SW_ERASE | if args[3] == 0 { SW_SCROLLCHILDREN } else { 0 };
    let a = [args[0], args[1], args[2], args[3], args[4], 0, 0, flags];
    scroll(cpu, &a, true)
}

/// ScrollWindowEx(hwnd, dx, dy, rect, clip, update region, update rect,
/// flags): the kind of the region it uncovered.
pub fn scroll_window_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    scroll(cpu, args, false)
}

fn scroll(cpu: &mut Cpu, args: &[u32], boolean: bool) -> Option<u32> {
    let &[hwnd, dx, dy, rect, clip, rgn, prc, flags] = args else {
        unreachable!()
    };
    let (dx, dy) = (dx as i32, dy as i32);
    let state = user(cpu);
    if !state.is_visible(hwnd) {
        return Some(if boolean { FALSE } else { 0 });
    }
    let client = state.screen_client(hwnd);
    let whole = Rect::new(0, 0, client.width(), client.height());
    let mut rc = whole;
    let mut cliprc = whole;
    if clip != 0 {
        cliprc = cliprc.intersect(&Rect::read(cpu, clip));
    }
    if rect != 0 {
        rc = rc.intersect(&Rect::read(cpu, rect));
    }
    let mut uncovered = Region::new();
    if !cliprc.is_empty() && (dx != 0 || dy != 0) {
        let state = user(cpu);
        let style = state.window(hwnd).map_or(0, |w| w.style);
        let mut dcx = DCX_CACHE | DCX_USESTYLE;
        if flags & SW_SCROLLCHILDREN == 0 && style & WS_CLIPCHILDREN != 0 {
            dcx |= DCX_CLIPCHILDREN;
        }
        let hdc = window_dc(cpu, hwnd, dcx, None);
        let (_, gone) = scroll_bits(cpu, hdc, dx, dy, Some(rc), Some(cliprc));
        release_dc(cpu, &[hwnd, hdc]);
        uncovered = gone;
        // What was to paint moves with what it covers.
        let state = user(cpu);
        if let Some(w) = state.window(hwnd) {
            let wr = state.screen_rect(hwnd);
            let off = (client.left - wr.left, client.top - wr.top);
            let moving = w
                .update
                .and_rect(&rc.offset(off.0, off.1))
                .offset_by(dx, dy)
                .and_rect(&cliprc.offset(off.0, off.1));
            if let Some(w) = state.window_mut(hwnd) {
                w.update = w.update.or(&moving);
            }
        }
    }
    if flags & SW_INVALIDATE != 0 && !uncovered.is_empty() {
        let mut rdw = RDW_INVALIDATE;
        if flags & SW_ERASE != 0 {
            rdw |= RDW_ERASE;
        }
        if flags & SW_SCROLLCHILDREN != 0 {
            rdw |= RDW_ALLCHILDREN;
        }
        let area = uncovered.offset_by(client.left, client.top);
        let state = user(cpu);
        redraw_tree(state, hwnd, &area, rdw);
        thread::wake(cpu);
    }
    let kind = uncovered.kind();
    if rgn != 0 {
        gdi32::set_region(cpu, rgn, uncovered.clone());
    }
    if prc != 0 {
        uncovered.bounds().write(cpu, prc);
    }
    // The children move by SetWindowPos, then what is uncovered is
    // erased if asked.
    let mut moves = Vec::new();
    if flags & SW_SCROLLCHILDREN != 0 {
        let state = user(cpu);
        let children = state
            .window(hwnd)
            .map(|w| w.children.clone())
            .unwrap_or_default();
        for child in children {
            let Some(c) = state.window(child) else {
                continue;
            };
            if rect == 0 || c.rect.intersect(&rc).width() > 0 {
                moves.extend([child, (c.rect.left + dx) as u32, (c.rect.top + dy) as u32]);
            }
        }
    }
    let result = if boolean { TRUE } else { kind };
    if moves.is_empty() && flags & SW_ERASE == 0 {
        return Some(result);
    }
    let mut vars = vec![K_SCROLL, hwnd, flags, 0, 0, 0, result, 0];
    vars.extend(moves);
    task::start(cpu, TASK_PAINT, vars, 0)
}

fn scroll_step(_cpu: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    let at = t.var(V_AT) as usize;
    if let Some(m) = t
        .vars
        .get(V_LIST + 3 * at..V_LIST + 3 * at + 3)
        .map(<[u32]>::to_vec)
    {
        t.set(V_AT, at as u32 + 1);
        let flags = SWP_NOZORDER | SWP_NOSIZE | SWP_NOACTIVATE;
        return Next::call(
            address_of("SetWindowPos"),
            &[m[0], 0, m[1], m[2], 0, 0, flags],
            1,
        );
    }
    if t.var(V_FLAGS) & SW_ERASE != 0 && t.step != 2 {
        let rdw = RDW_ERASENOW | RDW_ALLCHILDREN;
        return Next::call(address_of("RedrawWindow"), &[t.var(V_HWND), 0, 0, rdw], 2);
    }
    Next::Done(t.var(V_RESULT))
}

// Drawing.

/// A brush FillRect takes: a COLOR_ index + 1 is the color's brush, and
/// 0 the DC's own.
fn brush_of(cpu: &mut Cpu, hdc: u32, brush: u32) -> u32 {
    if brush == 0 {
        return gdi32::gdi(cpu).dc(hdc).map_or(0, |d| d.attrs.brush);
    }
    if brush <= 25 {
        return sysparams::brush(cpu, brush - 1);
    }
    brush
}

/// The device rectangle of the logical rectangle `r` on `hdc`.
fn device_rect(cpu: &mut Cpu, hdc: u32, r: Rect) -> Option<Rect> {
    let dc = gdi32::gdi(cpu).dc(hdc)?;
    Some(dc.rect_to_device(&r).ordered())
}

/// Fill the logical rectangle `r` of `hdc` with `brush`.
pub fn fill(cpu: &mut Cpu, hdc: u32, r: Rect, brush: u32) -> bool {
    let brush = brush_of(cpu, hdc, brush);
    let Some(d) = device_rect(cpu, hdc, r) else {
        return false;
    };
    if d.is_empty() {
        return true;
    }
    draw::fill_device_rect(cpu, hdc, d, brush)
}

/// Fill the logical rectangle `r` of `hdc` with a system color.
pub fn fill_color(cpu: &mut Cpu, hdc: u32, r: Rect, color: u32) {
    let brush = sysparams::brush(cpu, color);
    fill(cpu, hdc, r, brush);
}

/// FillRect(hdc, rect, brush).
pub fn fill_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = Rect::read(cpu, args[1]);
    Some(fill(cpu, args[0], r, args[2]) as u32)
}

/// FrameRect(hdc, rect, brush): a line a unit wide inside it.
pub fn frame_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, brush) = (args[0], args[2]);
    let r = Rect::read(cpu, args[1]).ordered();
    if r.is_empty() {
        return Some(TRUE);
    }
    let brush = brush_of(cpu, hdc, brush);
    let Some(d) = device_rect(cpu, hdc, r) else {
        return Some(FALSE);
    };
    for side in [
        Rect::new(d.left, d.top, d.right, d.top + 1),
        Rect::new(d.left, d.bottom - 1, d.right, d.bottom),
        Rect::new(d.left, d.top, d.left + 1, d.bottom),
        Rect::new(d.right - 1, d.top, d.right, d.bottom),
    ] {
        draw::fill_device_rect(cpu, hdc, side, brush);
    }
    Some(TRUE)
}

/// InvertRect(hdc, rect).
pub fn invert_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = Rect::read(cpu, args[1]);
    let Some(d) = device_rect(cpu, args[0], r) else {
        return Some(FALSE);
    };
    draw::invert_device_rect(cpu, args[0], d);
    Some(TRUE)
}

/// DrawFocusRect(hdc, rect).
pub fn draw_focus_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = Rect::read(cpu, args[1]);
    let Some(d) = device_rect(cpu, args[0], r) else {
        return Some(FALSE);
    };
    draw::focus_device_rect(cpu, args[0], d);
    Some(TRUE)
}

/// Draw the logical line from `a` to `b` (without its end) in a system
/// color.
fn line(cpu: &mut Cpu, hdc: u32, a: (i32, i32), b: (i32, i32), color: u32) {
    let Some(dc) = gdi32::gdi(cpu).dc(hdc).cloned() else {
        return;
    };
    let rgb = user(cpu).color(color);
    draw::device_line(
        cpu,
        hdc,
        dc.to_device(a.0, a.1),
        dc.to_device(b.0, b.1),
        rgb,
    );
}

const BDR_RAISEDOUTER: u32 = 0x0001;
const BDR_SUNKENOUTER: u32 = 0x0002;
const BDR_RAISEDINNER: u32 = 0x0004;
const BDR_SUNKENINNER: u32 = 0x0008;
const BDR_OUTER: u32 = 0x0003;
const BDR_INNER: u32 = 0x000C;
pub const EDGE_RAISED: u32 = BDR_RAISEDOUTER | BDR_RAISEDINNER;
pub const EDGE_SUNKEN: u32 = BDR_SUNKENOUTER | BDR_SUNKENINNER;
pub const EDGE_ETCHED: u32 = BDR_SUNKENOUTER | BDR_RAISEDINNER;

pub const BF_LEFT: u32 = 0x0001;
pub const BF_TOP: u32 = 0x0002;
pub const BF_RIGHT: u32 = 0x0004;
pub const BF_BOTTOM: u32 = 0x0008;
pub const BF_RECT: u32 = 0x000F;
const BF_DIAGONAL: u32 = 0x0010;
pub const BF_MIDDLE: u32 = 0x0800;
pub const BF_SOFT: u32 = 0x1000;
pub const BF_ADJUST: u32 = 0x2000;
pub const BF_FLAT: u32 = 0x4000;
pub const BF_MONO: u32 = 0x8000;

/// DrawEdge's colors, by the edge's bits: Wine's tables (uitools.c).
const N: i8 = -1;
const LT_INNER_NORMAL: [i8; 16] = [N, N, N, N, N, 20, 20, N, N, 21, 21, N, N, N, N, N];
const LT_OUTER_NORMAL: [i8; 16] = [N, 22, 16, N, 20, 22, 16, N, 21, 22, 16, N, N, 22, 16, N];
const RB_INNER_NORMAL: [i8; 16] = [N, N, N, N, N, 16, 16, N, N, 22, 22, N, N, N, N, N];
const RB_OUTER_NORMAL: [i8; 16] = [N, 21, 20, N, 16, 21, 20, N, 22, 21, 20, N, N, 21, 20, N];
const LT_INNER_SOFT: [i8; 16] = [N, N, N, N, N, 22, 22, N, N, 16, 16, N, N, N, N, N];
const LT_OUTER_SOFT: [i8; 16] = [N, 20, 21, N, 22, 20, 21, N, 16, 20, 21, N, N, 20, 21, N];
const LTRB_OUTER_MONO: [i8; 16] = [N, 6, 6, 6, 5, 6, 6, 6, 5, 6, 6, 6, 5, 6, 6, 6];
const LTRB_INNER_MONO: [i8; 16] = [N, N, N, N, N, 5, 5, 5, N, 5, 5, 5, N, 5, 5, 5];
const LTRB_OUTER_FLAT: [i8; 16] = [
    N, 16, 16, 16, 15, 16, 16, 16, 15, 16, 16, 16, 15, 16, 16, 16,
];
const LTRB_INNER_FLAT: [i8; 16] = [N, N, N, N, N, 15, 15, 15, N, 15, 15, 15, N, 15, 15, 15];

/// Draw an edge of the logical rectangle `r`: what DrawEdge does, with
/// the rectangle as BF_ADJUST leaves it.
pub fn edge(cpu: &mut Cpu, hdc: u32, r: Rect, kind: u32, flags: u32) -> (bool, Rect) {
    let i = (kind & (BDR_INNER | BDR_OUTER)) as usize;
    let ok = !((kind & BDR_INNER == BDR_INNER || kind & BDR_OUTER == BDR_OUTER)
        && flags & (BF_FLAT | BF_MONO) == 0);
    let (lt_in, lt_out, rb_in, rb_out) = if flags & BF_MONO != 0 {
        (
            LTRB_INNER_MONO[i],
            LTRB_OUTER_MONO[i],
            LTRB_INNER_MONO[i],
            LTRB_OUTER_MONO[i],
        )
    } else if flags & BF_FLAT != 0 {
        let inner = if LTRB_INNER_FLAT[i] != N {
            COLOR_BTNFACE as i8
        } else {
            N
        };
        (inner, LTRB_OUTER_FLAT[i], inner, LTRB_OUTER_FLAT[i])
    } else if flags & BF_SOFT != 0 {
        (
            LT_INNER_SOFT[i],
            LT_OUTER_SOFT[i],
            RB_INNER_NORMAL[i],
            RB_OUTER_NORMAL[i],
        )
    } else {
        (
            LT_INNER_NORMAL[i],
            LT_OUTER_NORMAL[i],
            RB_INNER_NORMAL[i],
            RB_OUTER_NORMAL[i],
        )
    };
    let all = |f: u32| (flags & f == f) as i32;
    let lb = all(BF_LEFT | BF_BOTTOM);
    let rt = all(BF_TOP | BF_RIGHT);
    let rb = all(BF_RIGHT | BF_BOTTOM);
    let lt = all(BF_LEFT | BF_TOP);
    let draw = |cpu: &mut Cpu, color: i8, a: (i32, i32), b: (i32, i32)| {
        if color != N {
            line(cpu, hdc, a, b, color as u32);
        }
    };
    if flags & BF_TOP != 0 {
        draw(cpu, lt_out, (r.left, r.top), (r.right, r.top));
    }
    if flags & BF_LEFT != 0 {
        draw(cpu, lt_out, (r.left, r.top), (r.left, r.bottom));
    }
    if flags & BF_BOTTOM != 0 {
        draw(
            cpu,
            rb_out,
            (r.left, r.bottom - 1),
            (r.right - 1, r.bottom - 1),
        );
    }
    if flags & BF_RIGHT != 0 {
        draw(cpu, rb_out, (r.right - 1, r.top), (r.right - 1, r.bottom));
    }
    if flags & BF_TOP != 0 {
        draw(
            cpu,
            lt_in,
            (r.left + lt, r.top + 1),
            (r.right - rt, r.top + 1),
        );
    }
    if flags & BF_LEFT != 0 {
        draw(
            cpu,
            lt_in,
            (r.left + 1, r.top + lt),
            (r.left + 1, r.bottom - lb),
        );
    }
    if flags & BF_BOTTOM != 0 {
        draw(
            cpu,
            rb_in,
            (r.left + lb, r.bottom - 2),
            (r.right - 1 - rb, r.bottom - 2),
        );
    }
    if flags & BF_RIGHT != 0 {
        draw(
            cpu,
            rb_in,
            (r.right - 2, r.top + 2 - rb),
            (r.right - 2, r.bottom - 2 + rt),
        );
    }
    let mut inner = r;
    if (flags & BF_MIDDLE != 0 && ok) || flags & BF_ADJUST != 0 {
        let add = (LTRB_INNER_MONO[i] != N) as i32 + (LTRB_OUTER_MONO[i] != N) as i32;
        if flags & BF_LEFT != 0 {
            inner.left += add;
        }
        if flags & BF_RIGHT != 0 {
            inner.right -= add;
        }
        if flags & BF_TOP != 0 {
            inner.top += add;
        }
        if flags & BF_BOTTOM != 0 {
            inner.bottom -= add;
        }
        if flags & BF_MIDDLE != 0 && ok {
            let color = if flags & BF_MONO != 0 {
                COLOR_WINDOW
            } else {
                COLOR_BTNFACE
            };
            fill_color(cpu, hdc, inner, color);
        }
    }
    let out = if flags & BF_ADJUST != 0 { inner } else { r };
    (ok, out)
}

/// DrawEdge(hdc, rect, edge, flags).
pub fn draw_edge(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, at, kind, flags) = (args[0], args[1], args[2], args[3]);
    let r = Rect::read(cpu, at);
    if flags & BF_DIAGONAL != 0 {
        // The diagonal edges: a line across, in the outer color.
        let color = if kind & (BDR_SUNKENOUTER | BDR_SUNKENINNER) != 0 {
            16
        } else {
            20
        };
        line(
            cpu,
            hdc,
            (r.left, r.bottom - 1),
            (r.right - 1, r.top),
            color,
        );
        return Some(TRUE);
    }
    let (ok, out) = edge(cpu, hdc, r, kind, flags);
    if flags & BF_ADJUST != 0 {
        out.write(cpu, at);
    }
    Some(ok as u32)
}

pub const DFC_CAPTION: u32 = 1;
pub const DFC_MENU: u32 = 2;
pub const DFC_SCROLL: u32 = 3;
pub const DFC_BUTTON: u32 = 4;

pub const DFCS_BUTTONCHECK: u32 = 0x0000;
pub const DFCS_BUTTONRADIOIMAGE: u32 = 0x0001;
pub const DFCS_BUTTONRADIOMASK: u32 = 0x0002;
pub const DFCS_BUTTONRADIO: u32 = 0x0004;
pub const DFCS_BUTTON3STATE: u32 = 0x0008;
pub const DFCS_BUTTONPUSH: u32 = 0x0010;
pub const DFCS_CAPTIONCLOSE: u32 = 0x0000;
pub const DFCS_CAPTIONMIN: u32 = 0x0001;
pub const DFCS_CAPTIONMAX: u32 = 0x0002;
pub const DFCS_CAPTIONRESTORE: u32 = 0x0003;
pub const DFCS_SCROLLUP: u32 = 0x0000;
pub const DFCS_SCROLLDOWN: u32 = 0x0001;
pub const DFCS_SCROLLLEFT: u32 = 0x0002;
pub const DFCS_SCROLLRIGHT: u32 = 0x0003;
pub const DFCS_SCROLLCOMBOBOX: u32 = 0x0005;
pub const DFCS_SCROLLSIZEGRIP: u32 = 0x0008;
pub const DFCS_MENUARROW: u32 = 0x0000;
pub const DFCS_MENUCHECK: u32 = 0x0001;
pub const DFCS_MENUBULLET: u32 = 0x0002;
pub const DFCS_INACTIVE: u32 = 0x0100;
pub const DFCS_PUSHED: u32 = 0x0200;
pub const DFCS_CHECKED: u32 = 0x0400;
pub const DFCS_ADJUSTRECT: u32 = 0x2000;
pub const DFCS_FLAT: u32 = 0x4000;
pub const DFCS_MONO: u32 = 0x8000;

/// The largest square in `r`, centered.
fn square_in(r: Rect) -> Rect {
    let size = r.width().min(r.height());
    let x = r.left + (r.width() - size) / 2;
    let y = r.top + (r.height() - size) / 2;
    Rect::sized(x, y, size, size)
}

/// The rows of the glyph of the caption button `glyph` (DFCS_CAPTION*)
/// drawn on the button `r`, as `spans` takes them. They are Windows
/// 98's Marlett glyphs: on the usual 16 by 14 button a 9-pixel square
/// two in from the left and the top holds the maximize box, the restore
/// boxes and the minimize bar, and the close cross is 8 by 7, centered.
fn caption_glyph(r: Rect, glyph: u32) -> Vec<(i32, i32, i32)> {
    let g = (r.height() - 5).max(3);
    let (x0, y0) = (r.left + (r.width() - g) / 2 - 1, r.top + (r.height() - g) / 2);
    // A box with a top two pixels thick.
    let boxed = |rows: &mut Vec<(i32, i32, i32)>, x: i32, y: i32, size: i32| {
        rows.push((y, x, x + size));
        rows.push((y + 1, x, x + size));
        for row in y + 2..y + size - 1 {
            rows.push((row, x, x + 1));
            rows.push((row, x + size - 1, x + size));
        }
        rows.push((y + size - 1, x, x + size));
    };
    let mut rows = Vec::new();
    match glyph {
        DFCS_CAPTIONCLOSE => {
            let n = g - 2;
            for i in 0..n {
                let y = y0 + 1 + i;
                rows.push((y, x0 + 2 + i, x0 + 4 + i));
                rows.push((y, x0 + 1 + n - i, x0 + 3 + n - i));
            }
        }
        DFCS_CAPTIONMIN => {
            for y in [y0 + g - 2, y0 + g - 1] {
                rows.push((y, x0 + 2, x0 + g - 1));
            }
        }
        DFCS_CAPTIONMAX => boxed(&mut rows, x0, y0, g),
        _ => {
            // Restore: a box behind and to the right of another.
            let s = g - 3;
            boxed(&mut rows, x0 + 3, y0, s);
            let (fx, fy) = (x0 + 1, y0 + 3);
            // The front box hides what of the back one it covers.
            rows = rows
                .into_iter()
                .flat_map(|(y, a, b)| {
                    if y < fy || y >= fy + s {
                        vec![(y, a, b)]
                    } else {
                        vec![(y, a, b.min(fx)), (y, a.max(fx + s), b)]
                    }
                })
                .filter(|&(_, a, b)| b > a)
                .collect();
            boxed(&mut rows, fx, fy, s);
        }
    }
    rows
}

/// Fill the rows of a shape: for each row `y` of `rows`, the columns
/// `x0..x1` it gives, in a system color.
fn spans(cpu: &mut Cpu, hdc: u32, rows: &[(i32, i32, i32)], color: u32) {
    let brush = sysparams::brush(cpu, color);
    for &(y, x0, x1) in rows {
        if x1 > x0 {
            fill(cpu, hdc, Rect::new(x0, y, x1, y + 1), brush);
        }
    }
}

/// A triangle pointing `dir` (0 up, 1 down, 2 left, 3 right) in `r`.
pub fn arrow(cpu: &mut Cpu, hdc: u32, r: Rect, dir: u32, color: u32) {
    let s = square_in(r);
    let size = (s.width() / 3).max(2);
    let (cx, cy) = (s.left + s.width() / 2, s.top + s.height() / 2);
    let mut rows = Vec::new();
    for i in 0..size {
        let half = size - 1 - i;
        match dir {
            0 => rows.push((
                cy - size / 2 + i,
                cx - (size - half) + 1,
                cx + (size - half),
            )),
            1 => rows.push((cy - size / 2 + i, cx - half, cx + half + 1)),
            2 => {
                for y in cy - i..=cy + i {
                    rows.push((y, cx - size / 2 + i, cx - size / 2 + i + 1));
                }
            }
            _ => {
                for y in cy - half..=cy + half {
                    rows.push((y, cx - size / 2 + i, cx - size / 2 + i + 1));
                }
            }
        }
    }
    spans(cpu, hdc, &rows, color);
}

/// A check mark in `r`.
pub fn check_mark(cpu: &mut Cpu, hdc: u32, r: Rect, color: u32) {
    let s = square_in(r);
    let x = s.left + (s.width() - 7) / 2;
    let y = s.top + (s.height() - 7) / 2;
    // The mark of Windows' check box: 7 columns, 3 pixels high each.
    let tops = [2, 3, 4, 3, 2, 1, 0];
    let rows: Vec<(i32, i32, i32)> = tops
        .iter()
        .enumerate()
        .flat_map(|(i, &t)| (0..3).map(move |k| (y + t + k, x + i as i32, x + i as i32 + 1)))
        .collect();
    spans(cpu, hdc, &rows, color);
}

/// A disc of radius `rad` at (`cx`, `cy`).
pub fn disc(cpu: &mut Cpu, hdc: u32, (cx, cy): (i32, i32), rad: i32, color: u32) {
    let mut rows = Vec::new();
    for dy in -rad..rad {
        let y = dy as f64 + 0.5;
        let half = ((rad * rad) as f64 - y * y).max(0.0).sqrt().round() as i32;
        rows.push((cy + dy, cx - half, cx + half));
    }
    spans(cpu, hdc, &rows, color);
}

/// Draw a push button's frame in `r`: raised, or sunk when pushed.
fn push_frame(cpu: &mut Cpu, hdc: u32, r: Rect, state: u32) -> Rect {
    let kind = if state & (DFCS_PUSHED | DFCS_CHECKED) != 0 {
        EDGE_SUNKEN
    } else {
        EDGE_RAISED
    };
    let flags = (state & DFCS_FLAT) | BF_RECT | BF_SOFT | BF_MIDDLE | BF_ADJUST;
    let flags = if state & DFCS_MONO != 0 {
        flags | BF_MONO
    } else {
        flags
    };
    edge(cpu, hdc, r, kind, flags).1
}

/// DrawFrameControl(hdc, rect, type, state).
pub fn draw_frame_control(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, at, kind, state) = (args[0], args[1], args[2], args[3]);
    let r = Rect::read(cpu, at);
    let (ok, adjusted) = frame_control(cpu, hdc, r, kind, state);
    if let Some(inner) = adjusted {
        inner.write(cpu, at);
    }
    Some(ok as u32)
}

/// Draw a frame control in the logical rectangle `r`: what
/// DrawFrameControl does, with the rectangle DFCS_ADJUSTRECT makes, if
/// it makes one.
pub fn frame_control(
    cpu: &mut Cpu,
    hdc: u32,
    r: Rect,
    kind: u32,
    state: u32,
) -> (bool, Option<Rect>) {
    let mut adjusted = None;
    let text = if state & DFCS_INACTIVE != 0 { 16 } else { 18 };
    let ok = match kind {
        DFC_BUTTON => {
            let style = state & 0xFF;
            if style & DFCS_BUTTONPUSH != 0 {
                let inner = push_frame(cpu, hdc, r, state);
                if state & DFCS_ADJUSTRECT != 0 {
                    adjusted = Some(inner);
                }
            } else if style & (DFCS_BUTTONRADIO | DFCS_BUTTONRADIOIMAGE | DFCS_BUTTONRADIOMASK) != 0
            {
                let s = square_in(r);
                let rad = s.width() / 2;
                let c = (s.left + rad, s.top + rad);
                if style & DFCS_BUTTONRADIOMASK != 0 {
                    disc(cpu, hdc, c, rad, 8);
                } else {
                    disc(cpu, hdc, c, rad, 16);
                    disc(cpu, hdc, (c.0 + 1, c.1 + 1), rad - 1, 20);
                    disc(cpu, hdc, c, rad - 1, 21);
                    disc(cpu, hdc, (c.0 + 1, c.1 + 1), rad - 2, 22);
                    let face = if state & (DFCS_PUSHED | DFCS_INACTIVE) != 0 {
                        COLOR_BTNFACE
                    } else {
                        COLOR_WINDOW
                    };
                    disc(cpu, hdc, (c.0, c.1), rad - 2, face);
                    if state & DFCS_CHECKED != 0 {
                        disc(cpu, hdc, c, (rad / 3).max(1), text);
                    }
                }
            } else {
                let s = square_in(r);
                let inner = edge(
                    cpu,
                    hdc,
                    s,
                    EDGE_SUNKEN,
                    BF_RECT | BF_ADJUST | (state & (DFCS_FLAT | DFCS_MONO)),
                )
                .1;
                let face = if state & (DFCS_PUSHED | DFCS_INACTIVE) != 0
                    || style & DFCS_BUTTON3STATE != 0 && state & DFCS_CHECKED != 0
                {
                    COLOR_BTNFACE
                } else {
                    COLOR_WINDOW
                };
                fill_color(cpu, hdc, inner, face);
                if state & DFCS_CHECKED != 0 {
                    check_mark(cpu, hdc, inner, text);
                }
            }
            true
        }
        DFC_CAPTION => {
            push_frame(cpu, hdc, r, state);
            let shift = (state & DFCS_PUSHED != 0) as i32;
            let rows = caption_glyph(r.offset(shift, shift), state & 0xFF);
            spans(cpu, hdc, &rows, text);
            true
        }
        DFC_SCROLL => {
            match state & 0xFF {
                DFCS_SCROLLSIZEGRIP => {
                    fill_color(cpu, hdc, r, COLOR_BTNFACE);
                    for k in 0..3 {
                        let o = 4 * k + 2;
                        line(
                            cpu,
                            hdc,
                            (r.right - o, r.bottom),
                            (r.right, r.bottom - o),
                            20,
                        );
                        line(
                            cpu,
                            hdc,
                            (r.right - o - 1, r.bottom),
                            (r.right, r.bottom - o - 1),
                            16,
                        );
                    }
                }
                dir => {
                    let inner = push_frame(cpu, hdc, r, state);
                    let dir = if dir == DFCS_SCROLLCOMBOBOX {
                        1
                    } else {
                        dir & 3
                    };
                    let shift = (state & DFCS_PUSHED != 0) as i32;
                    arrow(cpu, hdc, inner.offset(shift, shift), dir, text);
                }
            }
            true
        }
        DFC_MENU => {
            match state & 0xFF {
                DFCS_MENUARROW => arrow(cpu, hdc, r, 3, 7),
                DFCS_MENUCHECK => check_mark(cpu, hdc, r, 7),
                _ => {
                    let s = square_in(r);
                    disc(
                        cpu,
                        hdc,
                        (s.left + s.width() / 2, s.top + s.height() / 2),
                        (s.width() / 6).max(1),
                        7,
                    );
                }
            }
            true
        }
        _ => false,
    };
    (ok, adjusted)
}

/// DrawIconEx's flags.
pub const DI_MASK: u32 = 0x0001;
pub const DI_IMAGE: u32 = 0x0002;
pub const DI_NORMAL: u32 = 0x0003;
pub const DI_DEFAULTSIZE: u32 = 0x0008;

const SRCCOPY: u32 = 0x00CC_0020;
const SRCAND: u32 = 0x0088_00C6;
const SRCINVERT: u32 = 0x0066_0046;

/// Draw the icon or cursor `icon` at (`x`, `y`) of `hdc`, `cx` by `cy`
/// (0: its own size): its mask by AND, then its image by XOR.
pub fn draw_icon_at(
    cpu: &mut Cpu,
    hdc: u32,
    (x, y): (i32, i32),
    icon: u32,
    (cx, cy): (i32, i32),
    flags: u32,
) -> bool {
    let Some(ic) = super::resource::icon(cpu, icon) else {
        return false;
    };
    let (w, h) = (ic.width, ic.height);
    let cx = if cx != 0 {
        cx
    } else if flags & DI_DEFAULTSIZE != 0 {
        sysparams::metric(11)
    } else {
        w
    };
    let cy = if cy != 0 {
        cy
    } else if flags & DI_DEFAULTSIZE != 0 {
        sysparams::metric(12)
    } else {
        h
    };
    let Some(mem) = gdi32::dc::create_compatible_dc(cpu, &[hdc]).filter(|&m| m != 0) else {
        return false;
    };
    let blt = |cpu: &mut Cpu, bitmap: u32, sy: i32, rop: u32| {
        let old = gdi32::dc::select_object(cpu, &[mem, bitmap]).unwrap_or(0);
        let args = [
            hdc, x as u32, y as u32, cx as u32, cy as u32, mem, 0, sy as u32, w as u32, h as u32,
            rop,
        ];
        draw::stretch_blt(cpu, &args);
        if old != 0 {
            gdi32::dc::select_object(cpu, &[mem, old]);
        }
    };
    let both = flags & DI_NORMAL == DI_NORMAL;
    if flags & DI_MASK != 0 {
        blt(cpu, ic.mask, 0, if both { SRCAND } else { SRCCOPY });
    }
    if flags & DI_IMAGE != 0 {
        let rop = if both { SRCINVERT } else { SRCCOPY };
        if ic.color != 0 {
            blt(cpu, ic.color, 0, rop);
        } else {
            blt(cpu, ic.mask, h, rop);
        }
    }
    gdi32::dc::delete_dc(cpu, &[mem]);
    true
}

/// DrawIcon(hdc, x, y, icon).
pub fn draw_icon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let flags = DI_NORMAL | DI_DEFAULTSIZE;
    let ok = draw_icon_at(
        cpu,
        args[0],
        (args[1] as i32, args[2] as i32),
        args[3],
        (0, 0),
        flags,
    );
    if !ok {
        set_error(cpu, super::ERROR_INVALID_ICON_HANDLE);
    }
    Some(ok as u32)
}

/// DrawIconEx(hdc, x, y, icon, cx, cy, step, brush, flags): with a brush,
/// the rectangle is filled with it first.
pub fn draw_icon_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hdc, x, y, icon, cx, cy, _step, brush, flags] = args else {
        unreachable!()
    };
    let (x, y, cx, cy) = (x as i32, y as i32, cx as i32, cy as i32);
    if brush != 0 && super::resource::icon(cpu, icon).is_some() {
        let ic = super::resource::icon(cpu, icon).unwrap();
        let w = if cx != 0 { cx } else { ic.width };
        let h = if cy != 0 { cy } else { ic.height };
        fill(cpu, hdc, Rect::sized(x, y, w, h), brush);
    }
    let flags = if flags & DI_NORMAL == 0 {
        flags | DI_NORMAL
    } else {
        flags
    };
    Some(draw_icon_at(cpu, hdc, (x, y), icon, (cx, cy), flags) as u32)
}

/// DrawCaption's flags.
pub const DC_ACTIVE: u32 = 0x0001;
pub const DC_SMALLCAP: u32 = 0x0002;
pub const DC_ICON: u32 = 0x0004;
pub const DC_TEXT: u32 = 0x0008;

/// Draw the caption of `hwnd` in the logical rectangle `r` of `hdc`: its
/// bar in the active or inactive color, its small icon, and its text.
pub fn caption(cpu: &mut Cpu, hwnd: u32, hdc: u32, r: Rect, flags: u32) {
    let active = flags & DC_ACTIVE != 0;
    let (bar, text_color) = if active {
        (sysparams::COLOR_ACTIVECAPTION, sysparams::COLOR_CAPTIONTEXT)
    } else {
        (
            sysparams::COLOR_INACTIVECAPTION,
            sysparams::COLOR_INACTIVECAPTIONTEXT,
        )
    };
    fill_color(cpu, hdc, r, bar);
    let mut left = r.left + 2;
    if flags & DC_ICON != 0 {
        let state = user(cpu);
        let icon = state.window(hwnd).map_or(0, |w| {
            if w.icons[1] != 0 {
                w.icons[1]
            } else if w.icons[0] != 0 {
                w.icons[0]
            } else {
                state
                    .classes
                    .get(w.class as usize)
                    .and_then(|c| c.as_ref())
                    .map_or(0, |c| {
                        if c.icon_small != 0 {
                            c.icon_small
                        } else {
                            c.icon
                        }
                    })
            }
        });
        if icon != 0 {
            let size = sysparams::metric(49);
            let y = r.top + (r.height() - size) / 2;
            draw_icon_at(cpu, hdc, (left, y), icon, (size, size), DI_NORMAL);
            left += size + 2;
        }
    }
    if flags & DC_TEXT != 0 {
        let text = user(cpu)
            .window(hwnd)
            .map(|w| w.text.clone())
            .unwrap_or_default();
        if !text.is_empty() {
            // The caption font SPI_GETNONCLIENTMETRICS gives: 8-point MS
            // Sans Serif, bold.
            let log = sysparams::sans_serif(true);
            let font = gdi32::add(cpu, gdi32::Kind::Font { font: gdi32::Font { log } });
            let rgb = user(cpu).color(text_color);
            let saved = gdi32::dc::with(cpu, hdc, |d| {
                let old = (d.attrs.font, d.attrs.text_color, d.attrs.bk_mode);
                d.attrs.font = font;
                d.attrs.text_color = rgb;
                d.attrs.bk_mode = gdi32::dc::TRANSPARENT;
                old
            });
            let mut tr = Rect::new(left, r.top, r.right - 2, r.bottom);
            let fl = super::text::DT_SINGLELINE
                | super::text::DT_VCENTER
                | super::text::DT_NOPREFIX
                | super::text::DT_END_ELLIPSIS;
            super::text::draw_text_bytes(cpu, hdc, &text, &mut tr, fl);
            if let Some(old) = saved {
                gdi32::dc::with(cpu, hdc, |d| {
                    d.attrs.font = old.0;
                    d.attrs.text_color = old.1;
                    d.attrs.bk_mode = old.2;
                });
            }
            gdi32::delete_object(cpu, &[font]);
        }
    }
}

/// DrawCaption(hwnd, hdc, rect, flags).
pub fn draw_caption(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, hdc, flags) = (args[0], args[1], args[3]);
    let r = Rect::read(cpu, args[2]);
    caption(cpu, hwnd, hdc, r, flags);
    Some(TRUE)
}

/// PaintDesktop(hdc): the desktop's background where the DC draws.
pub fn paint_desktop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hdc = args[0];
    let Some(dc) = gdi32::gdi(cpu).dc(hdc).cloned() else {
        return Some(FALSE);
    };
    let (w, h) = gdi32::dc::device_size(cpu, &dc);
    let brush = sysparams::brush(cpu, COLOR_BACKGROUND);
    draw::fill_device_rect(
        cpu,
        hdc,
        Rect::new(-0x4000, -0x4000, w + 0x4000, h + 0x4000),
        brush,
    );
    Some(TRUE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The glyph `glyph` on a 16 by 14 caption button, a line a row.
    fn grid(glyph: u32) -> Vec<String> {
        let mut g = vec![vec![b'.'; 16]; 14];
        for (y, a, b) in caption_glyph(Rect::new(0, 0, 16, 14), glyph) {
            for x in a..b {
                g[y as usize][x as usize] = b'#';
            }
        }
        g.into_iter().map(|r| String::from_utf8(r).unwrap()).collect()
    }

    // Windows 98 SE's caption buttons, from screenshots of Shadowgate.
    #[test]
    fn caption_glyphs_are_windows_98_s() {
        let close = [
            "................",
            "................",
            "................",
            "....##....##....",
            ".....##..##.....",
            "......####......",
            ".......##.......",
            "......####......",
            ".....##..##.....",
            "....##....##....",
            "................",
            "................",
            "................",
            "................",
        ];
        assert_eq!(grid(DFCS_CAPTIONCLOSE), close);
        let min = [
            "................",
            "................",
            "................",
            "................",
            "................",
            "................",
            "................",
            "................",
            "................",
            "....######......",
            "....######......",
            "................",
            "................",
            "................",
        ];
        assert_eq!(grid(DFCS_CAPTIONMIN), min);
        let max = [
            "................",
            "................",
            "..#########.....",
            "..#########.....",
            "..#.......#.....",
            "..#.......#.....",
            "..#.......#.....",
            "..#.......#.....",
            "..#.......#.....",
            "..#.......#.....",
            "..#########.....",
            "................",
            "................",
            "................",
        ];
        assert_eq!(grid(DFCS_CAPTIONMAX), max);
        let restore = [
            "................",
            "................",
            ".....######.....",
            ".....######.....",
            ".....#....#.....",
            "...######.#.....",
            "...######.#.....",
            "...#....###.....",
            "...#....#.......",
            "...#....#.......",
            "...######.......",
            "................",
            "................",
            "................",
        ];
        assert_eq!(grid(DFCS_CAPTIONRESTORE), restore);
    }
}
