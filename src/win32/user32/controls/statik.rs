//! The Static class: text, icons, bitmaps, rectangles, frames and
//! etched lines, as Wine's static.c has them.
//!
//! Painting a static control sends its parent WM_CTLCOLORSTATIC while
//! the DC is clipped to the control, as Windows does, and a program can
//! rely on that clip region. Rectangles and frames send it too, without
//! using the brush; etched lines send it while they erase their
//! background instead of when they paint.

use super::button::{
    self, HOW_DC, HOW_PAINT, K_NOTIFY, P_HDC, P_HOW, P_HWND, P_OWN, P_RESULT, P_THEN,
    PAINT_SCRATCH, paint_vars,
};
use super::{STATIC, kind, start};
use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Rect};
use crate::win32::task::{Next, Task};
use crate::win32::user32::message::{
    WM_CREATE, WM_CTLCOLORSTATIC, WM_DRAWITEM, WM_ENABLE, WM_ERASEBKGND, WM_GETDLGCODE, WM_GETFONT,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_NCCREATE, WM_NCHITTEST, WM_NCLBUTTONDBLCLK,
    WM_NCLBUTTONDOWN, WM_PAINT, WM_PRINTCLIENT, WM_SETFONT, WM_SETTEXT, WM_SYSCOLORCHANGE,
};
use crate::win32::user32::paint::{
    self, BF_BOTTOM, BF_LEFT, BF_RECT, BF_RIGHT, BF_TOP, DI_NORMAL, EDGE_ETCHED, RDW_ALLCHILDREN,
    RDW_ERASE, RDW_INVALIDATE, RDW_UPDATENOW,
};
use crate::win32::user32::sysparams::{
    COLOR_3DDKSHADOW, COLOR_BTNHIGHLIGHT, COLOR_BTNSHADOW, COLOR_GRAYTEXT,
};
use crate::win32::user32::text::{
    self, DT_CENTER, DT_END_ELLIPSIS, DT_EXPANDTABS, DT_NOPREFIX, DT_PATH_ELLIPSIS, DT_RIGHT,
    DT_SINGLELINE, DT_VCENTER, DT_WORD_ELLIPSIS, DT_WORDBREAK,
};
use crate::win32::user32::window::{
    self, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, WS_DISABLED, WS_EX_STATICEDGE,
};
use crate::win32::user32::{address_of, defwnd, read_words, resource, user};

/// What a static control keeps: its font, and its icon, bitmap or
/// enhanced metafile.
#[derive(Clone, Debug, Default)]
pub struct Static {
    pub font: u32,
    pub image: u32,
}

const DT_LEFT: u32 = 0x0000;

const SS_LEFT: u32 = 0x00;
const SS_CENTER: u32 = 0x01;
const SS_RIGHT: u32 = 0x02;
const SS_ICON: u32 = 0x03;
const SS_BLACKRECT: u32 = 0x04;
const SS_GRAYRECT: u32 = 0x05;
const SS_WHITERECT: u32 = 0x06;
const SS_BLACKFRAME: u32 = 0x07;
const SS_GRAYFRAME: u32 = 0x08;
const SS_WHITEFRAME: u32 = 0x09;
const SS_SIMPLE: u32 = 0x0B;
const SS_LEFTNOWORDWRAP: u32 = 0x0C;
const SS_OWNERDRAW: u32 = 0x0D;
const SS_BITMAP: u32 = 0x0E;
const SS_ENHMETAFILE: u32 = 0x0F;
const SS_ETCHEDHORZ: u32 = 0x10;
const SS_ETCHEDVERT: u32 = 0x11;
const SS_ETCHEDFRAME: u32 = 0x12;
const SS_TYPEMASK: u32 = 0x1F;

const SS_REALSIZECONTROL: u32 = 0x40;
const SS_NOPREFIX: u32 = 0x80;
const SS_NOTIFY: u32 = 0x100;
const SS_CENTERIMAGE: u32 = 0x200;
const SS_SUNKEN: u32 = 0x1000;
const SS_ENDELLIPSIS: u32 = 0x4000;
const SS_PATHELLIPSIS: u32 = 0x8000;
const SS_WORDELLIPSIS: u32 = 0xC000;
const SS_ELLIPSISMASK: u32 = 0xC000;

pub const STM_SETICON: u32 = 0x0170;
pub const STM_GETICON: u32 = 0x0171;
pub const STM_SETIMAGE: u32 = 0x0172;
pub const STM_GETIMAGE: u32 = 0x0173;

const STN_CLICKED: u32 = 0;
const STN_DBLCLK: u32 = 1;
const STN_ENABLE: u32 = 2;
const STN_DISABLE: u32 = 3;

const IMAGE_BITMAP: u32 = 0;
const IMAGE_ICON: u32 = 1;
const IMAGE_CURSOR: u32 = 2;
const IMAGE_ENHMETAFILE: u32 = 3;

const ODT_STATIC: u32 = 5;
const ODA_DRAWENTIRE: u32 = 1;
const ODS_DISABLED: u32 = 0x04;

const HTCLIENT: u32 = 1;
const HTTRANSPARENT: u32 = -1i32 as u32;

/// WM_PAINT and WM_PRINTCLIENT, and painting at once (with GetDC).
const K_PAINT: u32 = 0;
/// An etched control's WM_ERASEBKGND.
const K_ERASE: u32 = 1;
/// WM_SETTEXT: DefWindowProcA, then painting.
const K_SETTEXT: u32 = 2;
/// WM_NCCREATE with an icon to size the control to, then
/// DefWindowProcA.
/// STM_SETICON and STM_SETIMAGE: SetWindowPos to the image's size,
/// then painting.
const K_SETIMAGE: u32 = 4;
/// WM_ENABLE: painting, then STN_ENABLE or STN_DISABLE.
const K_ENABLE: u32 = 5;

const STEP_PAINT: u16 = 20;

/// The variables of the static control's own tasks.
const V_A: usize = P_OWN;
const V_B: usize = P_OWN + 1;

fn type_of(style: u32) -> u32 {
    style & SS_TYPEMASK
}

/// Whether the style is one of text (hasTextStyle).
fn has_text(style: u32) -> bool {
    matches!(
        type_of(style),
        SS_SIMPLE | SS_LEFT | SS_LEFTNOWORDWRAP | SS_CENTER | SS_RIGHT | SS_OWNERDRAW
    )
}

/// Whether the type has something to paint.
fn paints(style: u32) -> bool {
    type_of(style) <= SS_ETCHEDFRAME && type_of(style) != 0x0A
}

fn etched(style: u32) -> bool {
    matches!(
        type_of(style),
        SS_ETCHEDHORZ | SS_ETCHEDVERT | SS_ETCHEDFRAME
    )
}

/// The size of the icon `icon`, if it is one.
fn icon_size(cpu: &mut Cpu, icon: u32) -> Option<(i32, i32)> {
    resource::icon(cpu, icon).map(|i| (i.width, i.height))
}

fn bitmap_size(cpu: &mut Cpu, bitmap: u32) -> Option<(i32, i32)> {
    gdi32::gdi(cpu).bitmap(bitmap).map(|b| (b.width, b.height))
}

/// Keep `image` as the control's icon (STATIC_SetIcon) or bitmap
/// (`bitmap`, STATIC_SetBitmap): the one before, and the size the
/// control takes for it.
fn set_image(cpu: &mut Cpu, hwnd: u32, image: u32, bitmap: bool) -> (u32, Option<(i32, i32)>) {
    let style = user(cpu).window(hwnd).map_or(0, |w| w.style);
    let wanted = if bitmap { SS_BITMAP } else { SS_ICON };
    if type_of(style) != wanted {
        return (0, None);
    }
    let size = if image == 0 {
        None
    } else if bitmap {
        bitmap_size(cpu, image)
    } else {
        icon_size(cpu, image)
    };
    if image != 0 && size.is_none() {
        return (0, None);
    }
    let old = match user(cpu).window_mut(hwnd) {
        Some(w) => std::mem::replace(&mut w.control.stat.image, image),
        None => 0,
    };
    let resize = if style & (SS_CENTERIMAGE | SS_REALSIZECONTROL) == 0 {
        size
    } else {
        None
    };
    (old, resize)
}

/// The icon an SS_ICON control is made with (STATIC_LoadIconA): the
/// module's icon or cursor `name`, or the system's icon.
fn load_icon(cpu: &mut Cpu, instance: u32, name: u32) -> u32 {
    let mut icon = 0;
    if instance >> 16 != 0 {
        icon = resource::load_icon(cpu, &[instance, name]).unwrap_or(0);
        if icon == 0 {
            icon = resource::load_cursor(cpu, &[instance, name]).unwrap_or(0);
        }
    }
    if icon == 0 {
        icon = resource::load_icon(cpu, &[0, name]).unwrap_or(0);
    }
    icon
}

/// Paint the control at once, if it is visible and not empty
/// (STATIC_TryPaintFcn), and return `result`.
fn try_paint(cpu: &mut Cpu, hwnd: u32, result: u32) -> Option<u32> {
    if !can_paint(cpu, hwnd) {
        return Some(result);
    }
    let mut vars = paint_vars(hwnd, 0, false, 0);
    vars[P_RESULT - 1] = result;
    start(cpu, STATIC, K_PAINT, vars, PAINT_SCRATCH)
}

fn can_paint(cpu: &mut Cpu, hwnd: u32) -> bool {
    let state = user(cpu);
    let Some(w) = state.window(hwnd) else {
        return false;
    };
    paints(w.style) && !w.client.is_empty() && state.is_visible(hwnd)
}

/// In a task: paint at once as `try_paint` does, and go on at `then`
/// (0: return `result`).
fn try_paint_then(cpu: &mut Cpu, t: &mut Task, result: u32, then: u16) -> Next {
    t.set(P_RESULT, result);
    t.set(P_THEN, then as u32);
    if !can_paint(cpu, t.var(P_HWND)) {
        return button::finish(t);
    }
    t.set(P_HDC, 0);
    t.set(P_HOW, HOW_DC);
    Next::call(0, &[], STEP_PAINT)
}

fn begin(cpu: &mut Cpu, hwnd: u32, kind: u32, own: &[u32]) -> Option<u32> {
    let mut vars = paint_vars(hwnd, 0, false, 0);
    vars.extend_from_slice(own);
    start(cpu, STATIC, kind, vars, PAINT_SCRATCH)
}

/// Whether a static control of the style `style` lets the mouse through
/// (WM_NCHITTEST's HTTRANSPARENT): all but those with SS_NOTIFY.
pub fn transparent(style: u32) -> bool {
    style & SS_NOTIFY == 0
}

/// The Static class's window procedure.
pub fn static_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    let Some(w) = user(cpu).window(hwnd) else {
        return Some(0);
    };
    let style = w.style;
    let s = w.control.stat.clone();
    match msg {
        WM_CREATE => Some(0),
        WM_ERASEBKGND => {
            if etched(style) {
                let mut vars = paint_vars(hwnd, wp, false, 0);
                vars[P_RESULT - 1] = 1;
                return start(cpu, STATIC, K_ERASE, vars, PAINT_SCRATCH);
            }
            // The painting is all in WM_PAINT.
            Some(1)
        }
        WM_PAINT | WM_PRINTCLIENT => {
            let vars = paint_vars(hwnd, wp, true, 0);
            start(cpu, STATIC, K_PAINT, vars, PAINT_SCRATCH)
        }
        WM_ENABLE => begin(cpu, hwnd, K_ENABLE, &[wp]),
        WM_SYSCOLORCHANGE => try_paint(cpu, hwnd, 0),
        WM_NCCREATE => nc_create(cpu, args),
        WM_SETTEXT => {
            if has_text(style) && lp >> 16 != 0 {
                return begin(cpu, hwnd, K_SETTEXT, &[wp, lp]);
            }
            Some(0)
        }
        WM_SETFONT => {
            if has_text(style) {
                if let Some(w) = user(cpu).window_mut(hwnd) {
                    w.control.stat.font = wp;
                }
                if lp & 0xFFFF != 0 {
                    let flags = RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW | RDW_ALLCHILDREN;
                    paint::redraw_window(cpu, &[hwnd, 0, 0, flags])?;
                }
            }
            Some(0)
        }
        WM_GETFONT => Some(s.font),
        WM_NCHITTEST => Some(if transparent(style) {
            HTTRANSPARENT
        } else {
            HTCLIENT
        }),
        WM_GETDLGCODE => Some(button::DLGC_STATIC),
        WM_LBUTTONDOWN | WM_NCLBUTTONDOWN => {
            if style & SS_NOTIFY != 0 {
                return button::notify(cpu, STATIC, hwnd, STN_CLICKED, 0);
            }
            Some(0)
        }
        WM_LBUTTONDBLCLK | WM_NCLBUTTONDBLCLK => {
            if style & SS_NOTIFY != 0 {
                return button::notify(cpu, STATIC, hwnd, STN_DBLCLK, 0);
            }
            Some(0)
        }
        STM_GETIMAGE => Some(image_of(style, wp, s.image)),
        STM_GETICON => Some(image_of(style, IMAGE_ICON, s.image)),
        STM_SETIMAGE => {
            let (old, size) = match wp {
                IMAGE_BITMAP => set_image(cpu, hwnd, lp, true),
                IMAGE_ICON | IMAGE_CURSOR => set_image(cpu, hwnd, lp, false),
                IMAGE_ENHMETAFILE if type_of(style) == SS_ENHMETAFILE => {
                    if let Some(w) = user(cpu).window_mut(hwnd) {
                        w.control.stat.image = lp;
                    }
                    (s.image, None)
                }
                _ => (0, None),
            };
            image_set(cpu, hwnd, old, size)
        }
        STM_SETICON => {
            let (old, size) = set_image(cpu, hwnd, wp, false);
            image_set(cpu, hwnd, old, size)
        }
        _ => defwnd::def_window_proc(cpu, args),
    }
}

/// STM_GETIMAGE: the image if it is of the kind `kind` the control
/// holds (STATIC_GetImage).
fn image_of(style: u32, kind: u32, image: u32) -> u32 {
    let fits = match type_of(style) {
        SS_ICON => kind == IMAGE_ICON || kind == IMAGE_CURSOR,
        SS_BITMAP => kind == IMAGE_BITMAP,
        SS_ENHMETAFILE => kind == IMAGE_ENHMETAFILE,
        _ => false,
    };
    if fits { image } else { 0 }
}

/// After STM_SETIMAGE or STM_SETICON: size the control to the image,
/// paint it and return the image before.
fn image_set(cpu: &mut Cpu, hwnd: u32, old: u32, size: Option<(i32, i32)>) -> Option<u32> {
    match size {
        Some((cx, cy)) => begin(cpu, hwnd, K_SETIMAGE, &[old, cx as u32, cy as u32]),
        None => try_paint(cpu, hwnd, old),
    }
}

/// WM_NCCREATE: SS_SUNKEN gives WS_EX_STATICEDGE, SS_ICON and
/// SS_BITMAP load their image from the window's name; then
/// DefWindowProcA.
fn nc_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, cs) = (args[0], args[3]);
    let style = user(cpu).window(hwnd).map_or(0, |w| w.style);
    if style & SS_SUNKEN != 0
        && let Some(w) = user(cpu).window_mut(hwnd)
    {
        w.ex_style |= WS_EX_STATICEDGE;
    }
    let words = read_words(cpu, cs, 12);
    let (instance, name) = (words[1], words[9]);
    let size = match type_of(style) {
        SS_ICON => {
            let icon = load_icon(cpu, instance, name);
            set_image(cpu, hwnd, icon, false).1
        }
        SS_BITMAP if instance >> 16 != 0 => {
            let bitmap = resource::load_bitmap(cpu, &[instance, name]).unwrap_or(0);
            set_image(cpu, hwnd, bitmap, true).1
        }
        _ => None,
    };
    // The window isn't on the screen yet: it takes the image's size as
    // it is made.
    if let Some((cx, cy)) = size
        && let Some(w) = user(cpu).window_mut(hwnd)
    {
        w.rect.right = w.rect.left + cx;
        w.rect.bottom = w.rect.top + cy;
        w.client = w.rect;
        w.normal = w.rect;
    }
    defwnd::def_window_proc(cpu, args)
}

/// The steps of the static control's tasks.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let k = kind(t);
    if k == K_NOTIFY {
        return button::notify_step(t);
    }
    if t.step >= STEP_PAINT {
        return paint_step(cpu, t, value);
    }
    let hwnd = t.var(P_HWND);
    match k {
        K_PAINT => Next::call(0, &[], STEP_PAINT),
        K_ERASE => erase_step(cpu, t, value),
        K_SETTEXT => match t.step {
            0 => {
                let args = [hwnd, WM_SETTEXT, t.var(V_A), t.var(V_B)];
                Next::call(address_of("DefWindowProcA"), &args, 1)
            }
            _ => try_paint_then(cpu, t, value, 0),
        },
        K_SETIMAGE => match t.step {
            0 => {
                let (cx, cy) = (t.var(V_B), t.var(V_B + 1));
                let flags = SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER;
                let args = [hwnd, 0, 0, 0, cx, cy, flags];
                Next::call(address_of("SetWindowPos"), &args, 1)
            }
            _ => try_paint_then(cpu, t, t.var(V_A), 0),
        },
        K_ENABLE => match t.step {
            0 => try_paint_then(cpu, t, 0, 1),
            1 => {
                let style = user(cpu).window(hwnd).map_or(0, |w| w.style);
                if style & SS_NOTIFY == 0 {
                    return Next::Done(0);
                }
                let code = if t.var(V_A) != 0 {
                    STN_ENABLE
                } else {
                    STN_DISABLE
                };
                let parent = button::parent_of(cpu, hwnd);
                let id = user(cpu).window(hwnd).map_or(0, |w| w.id);
                let msg = crate::win32::user32::message::WM_COMMAND;
                window::send(parent, msg, id & 0xFFFF | code << 16, hwnd, 2)
            }
            _ => Next::Done(0),
        },
        _ => Next::Done(0),
    }
}

/// An etched control's WM_ERASEBKGND: its background in the
/// WM_CTLCOLORSTATIC brush, with the DC clipped to it.
fn erase_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let hwnd = t.var(P_HWND);
    match t.step {
        0 => button::open(cpu, t, 1),
        1 => {
            let hdc = button::prepare(cpu, t, value, 0);
            if hdc == 0 {
                return button::close(cpu, t, 3);
            }
            let parent = button::parent_of(cpu, hwnd);
            window::send(parent, WM_CTLCOLORSTATIC, hdc, hwnd, 2)
        }
        2 => {
            let hdc = t.var(P_HDC);
            let brush = if value != 0 {
                value
            } else {
                let parent = button::parent_of(cpu, hwnd);
                button::default_color(cpu, parent, hdc, WM_CTLCOLORSTATIC)
            };
            let r = button::client_rect(cpu, hwnd);
            paint::fill(cpu, hdc, r, brush);
            button::close(cpu, t, 3)
        }
        _ => button::finish(t),
    }
}

/// The painting steps, `STEP_PAINT` on.
fn paint_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let hwnd = t.var(P_HWND);
    let Some(w) = user(cpu).window(hwnd) else {
        return Next::Done(t.var(P_RESULT));
    };
    let style = w.style;
    let s = w.control.stat.clone();
    match t.step - STEP_PAINT {
        0 => {
            if !paints(style) && t.var(P_HOW) != HOW_PAINT {
                return button::finish(t);
            }
            button::open(cpu, t, STEP_PAINT + 1)
        }
        1 => {
            let font = if has_text(style) { s.font } else { 0 };
            let hdc = button::prepare(cpu, t, value, font);
            if hdc == 0 || !paints(style) {
                return button::close(cpu, t, STEP_PAINT + 4);
            }
            if etched(style) {
                let r = button::client_rect(cpu, hwnd);
                let flags = match type_of(style) {
                    SS_ETCHEDHORZ => BF_TOP | BF_BOTTOM,
                    SS_ETCHEDVERT => BF_LEFT | BF_RIGHT,
                    _ => BF_RECT,
                };
                paint::edge(cpu, hdc, r, EDGE_ETCHED, flags);
                return button::close(cpu, t, STEP_PAINT + 4);
            }
            let parent = button::parent_of(cpu, hwnd);
            window::send(parent, WM_CTLCOLORSTATIC, hdc, hwnd, STEP_PAINT + 2)
        }
        2 => {
            let hdc = t.var(P_HDC);
            let brush = if value != 0 {
                value
            } else {
                let parent = button::parent_of(cpu, hwnd);
                button::default_color(cpu, parent, hdc, WM_CTLCOLORSTATIC)
            };
            let r = button::client_rect(cpu, hwnd);
            match type_of(style) {
                SS_OWNERDRAW => {
                    let dis = t.scratch + button::DRAWITEM;
                    let id = user(cpu).window(hwnd).map_or(0, |w| w.id);
                    let state = if style & WS_DISABLED != 0 {
                        ODS_DISABLED
                    } else {
                        0
                    };
                    let words = [
                        ODT_STATIC,
                        id,
                        0,
                        ODA_DRAWENTIRE,
                        state,
                        hwnd,
                        hdc,
                        r.left as u32,
                        r.top as u32,
                        r.right as u32,
                        r.bottom as u32,
                        0,
                    ];
                    crate::win32::user32::write_words(cpu, dis, &words);
                    let parent = button::parent_of(cpu, hwnd);
                    return window::send(parent, WM_DRAWITEM, id, dis, STEP_PAINT + 3);
                }
                SS_ICON => paint_icon(cpu, hdc, r, brush, style, s.image),
                SS_BITMAP => paint_bitmap(cpu, hdc, r, brush, style, s.image),
                SS_ENHMETAFILE => {
                    paint::fill(cpu, hdc, r, brush);
                }
                SS_BLACKRECT..=SS_WHITEFRAME => paint_rect(cpu, hdc, r, style),
                _ => paint_text(cpu, hwnd, hdc, r, brush, style),
            }
            button::close(cpu, t, STEP_PAINT + 4)
        }
        3 => button::close(cpu, t, STEP_PAINT + 4),
        _ => button::finish(t),
    }
}

/// The DrawText flags of a text style (STATIC_PaintTextfn).
fn text_flags(style: u32) -> u32 {
    let mut flags = match type_of(style) {
        SS_CENTER => DT_CENTER | DT_EXPANDTABS | DT_WORDBREAK,
        SS_RIGHT => DT_RIGHT | DT_EXPANDTABS | DT_WORDBREAK,
        SS_SIMPLE => DT_LEFT | DT_SINGLELINE,
        SS_LEFTNOWORDWRAP => DT_LEFT | DT_EXPANDTABS,
        _ => DT_LEFT | DT_EXPANDTABS | DT_WORDBREAK,
    };
    if style & SS_NOPREFIX != 0 {
        flags |= DT_NOPREFIX;
    }
    if type_of(style) != SS_SIMPLE {
        if style & SS_CENTERIMAGE != 0 {
            flags |= DT_SINGLELINE | DT_VCENTER;
        }
        match style & SS_ELLIPSISMASK {
            SS_ENDELLIPSIS => flags |= DT_SINGLELINE | DT_END_ELLIPSIS,
            SS_PATHELLIPSIS => flags |= DT_SINGLELINE | DT_PATH_ELLIPSIS,
            SS_WORDELLIPSIS => flags |= DT_SINGLELINE | DT_WORD_ELLIPSIS,
            _ => {}
        }
    }
    flags
}

fn paint_text(cpu: &mut Cpu, hwnd: u32, hdc: u32, r: Rect, brush: u32, style: u32) {
    // SS_SIMPLE gets WM_CTLCOLORSTATIC but doesn't use the brush.
    if type_of(style) != SS_SIMPLE {
        paint::fill(cpu, hdc, r, brush);
        if style & WS_DISABLED != 0 {
            let gray = user(cpu).color(COLOR_GRAYTEXT);
            gdi32::dc::set_text_color(cpu, &[hdc, gray]);
        }
    }
    let label = user(cpu)
        .window(hwnd)
        .map(|w| w.text.clone())
        .unwrap_or_default();
    if label.is_empty() {
        return;
    }
    let mut r = r;
    text::draw_text_bytes(cpu, hdc, &label, &mut r, text_flags(style));
}

fn paint_icon(cpu: &mut Cpu, hdc: u32, r: Rect, brush: u32, style: u32, icon: u32) {
    paint::fill(cpu, hdc, r, brush);
    let Some((cx, cy)) = icon_size(cpu, icon) else {
        return;
    };
    let at = if style & SS_CENTERIMAGE != 0 {
        (r.left + (r.width() - cx) / 2, r.top + (r.height() - cy) / 2)
    } else {
        (r.left, r.top)
    };
    let size = if style & SS_CENTERIMAGE != 0 {
        (cx, cy)
    } else {
        (r.width(), r.height())
    };
    paint::draw_icon_at(cpu, hdc, at, icon, size, DI_NORMAL);
}

/// SS_BITMAP: the bitmap, centered on the brush with SS_CENTERIMAGE;
/// nothing at all without one.
fn paint_bitmap(cpu: &mut Cpu, hdc: u32, r: Rect, brush: u32, style: u32, bitmap: u32) {
    let Some((cx, cy)) = bitmap_size(cpu, bitmap) else {
        return;
    };
    let at = if style & SS_CENTERIMAGE != 0 {
        paint::fill(cpu, hdc, r, brush);
        (r.left + (r.width() - cx) / 2, r.top + (r.height() - cy) / 2)
    } else {
        (r.left, r.top)
    };
    text::draw_bitmap(cpu, hdc, bitmap, at, (cx, cy));
}

/// The rectangles and frames, in the 3D colors (STATIC_PaintRectfn).
fn paint_rect(cpu: &mut Cpu, hdc: u32, r: Rect, style: u32) {
    let color = match type_of(style) {
        SS_BLACKRECT | SS_BLACKFRAME => COLOR_3DDKSHADOW,
        SS_GRAYRECT | SS_GRAYFRAME => COLOR_BTNSHADOW,
        _ => COLOR_BTNHIGHLIGHT,
    };
    if type_of(style) <= SS_WHITERECT {
        paint::fill_color(cpu, hdc, r, color);
        return;
    }
    if r.is_empty() {
        return;
    }
    for side in [
        Rect::new(r.left, r.top, r.right, r.top + 1),
        Rect::new(r.left, r.bottom - 1, r.right, r.bottom),
        Rect::new(r.left, r.top, r.left + 1, r.bottom),
        Rect::new(r.right - 1, r.top, r.right, r.bottom),
    ] {
        paint::fill_color(cpu, hdc, side, color);
    }
}

crate::state_fields!(Static { font, image });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_styles_format_as_wine_does() {
        assert_eq!(text_flags(SS_LEFT), DT_LEFT | DT_EXPANDTABS | DT_WORDBREAK);
        assert_eq!(
            text_flags(SS_SIMPLE | SS_CENTERIMAGE),
            DT_LEFT | DT_SINGLELINE
        );
        assert_eq!(
            text_flags(SS_CENTER | SS_NOPREFIX),
            DT_CENTER | DT_EXPANDTABS | DT_WORDBREAK | DT_NOPREFIX
        );
        let f = text_flags(SS_RIGHT | SS_CENTERIMAGE);
        assert_ne!(f & DT_VCENTER, 0);
        assert_ne!(f & DT_SINGLELINE, 0);
    }

    #[test]
    fn images_come_back_only_as_their_kind() {
        assert_eq!(image_of(SS_ICON, IMAGE_ICON, 7), 7);
        assert_eq!(image_of(SS_ICON, IMAGE_CURSOR, 7), 7);
        assert_eq!(image_of(SS_ICON, IMAGE_BITMAP, 7), 0);
        assert_eq!(image_of(SS_BITMAP, IMAGE_BITMAP, 7), 7);
        assert_eq!(image_of(SS_LEFT, IMAGE_ICON, 7), 0);
    }

    #[test]
    fn text_styles() {
        assert!(has_text(SS_OWNERDRAW));
        assert!(!has_text(SS_ICON));
        assert!(paints(SS_ETCHEDFRAME));
        assert!(!paints(0x0A));
        assert!(!paints(0x13));
    }
}
