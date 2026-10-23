//! Image lists: images of one size side by side in a bitmap of GDI's,
//! with a monochrome mask for those made with ILC_MASK, drawn with
//! BitBlt as the controls and the program ask. The bitmap holds four
//! images a row, as Wine's and later Windows' do; ImageList_Write
//! writes it so. The images are kept in GDI32's objects, the list
//! itself in `State` by its handle, a small block of the process's
//! heap. ImageList_Write and ImageList_Read call the program's IStream,
//! as scripts. Image lists of COMCTL32 4.70 have no alpha channel.
//!
//! Ported from Wine's imagelist.c:
//!
//! Copyright 1998 Eric Kohl
//! Copyright 2000 Jason Mawdsley
//! Copyright 2001, 2004 Michael Stefaniuc
//! Copyright 2001 Charles Loep for CodeWeavers
//! Copyright 2002 Dimitrie O. Paun
//! Copyright 2009 Owen Rudge for CodeWeavers
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

use super::misc::{heap_alloc, heap_free, process_heap};
use super::script::{self, Arg, Ctx, Op, Script, words_of};
use super::{IMAGELIST, call_builtin, state};
use crate::cpu::Cpu;
use crate::win32::gdi32;
use crate::win32::user32::{self, read_words, write_words};

pub const ILC_MASK: u32 = 0x0001;
pub const ILC_COLOR: u32 = 0x0000;
pub const ILC_COLORDDB: u32 = 0x00FE;
pub const ILC_COLOR4: u32 = 0x0004;
pub const ILC_COLOR32: u32 = 0x0020;

pub const ILD_NORMAL: u32 = 0x0000;
pub const ILD_TRANSPARENT: u32 = 0x0001;
pub const ILD_BLEND25: u32 = 0x0002;
pub const ILD_BLEND50: u32 = 0x0004;
pub const ILD_MASK: u32 = 0x0010;
pub const ILD_IMAGE: u32 = 0x0020;
pub const ILD_ROP: u32 = 0x0040;
pub const ILD_OVERLAYMASK: u32 = 0x0F00;
pub const ILD_SELECTED: u32 = ILD_BLEND50;

const ILCF_SWAP: u32 = 1;
const CLR_NONE: u32 = 0xFFFF_FFFF;
const CLR_DEFAULT: u32 = 0xFF00_0000;

const SRCCOPY: u32 = 0x00CC_0020;
const SRCPAINT: u32 = 0x00EE_0086;
const SRCAND: u32 = 0x0088_00C6;
const NOTSRCAND: u32 = 0x0022_0326;
const NOTSRCCOPY: u32 = 0x0033_0008;
const PATCOPY: u32 = 0x00F0_0021;
const BLACKNESS: u32 = 0x0000_0042;
const WHITENESS: u32 = 0x00FF_0062;
/// PSDPxax: the pattern where the mask is black.
const BLEND_ROP: u32 = 0x00B8_074A;

const IMAGE_BITMAP: u32 = 0;
const IMAGE_ICON: u32 = 1;
const IMAGE_CURSOR: u32 = 2;
const LR_COPYFROMRESOURCE: u32 = 0x4000;
const DI_DEFAULTSIZE: u32 = 0x0008;
const SM_CXICON: u32 = 11;
const COLOR_HIGHLIGHT: u32 = 13;

const MAX_OVERLAY: usize = 15;
const TILE: i32 = 4;
/// ILHEAD's usMagic, "IL".
const MAGIC: u16 = (b'L' as u16) << 8 | b'I' as u16;
const ILHEAD_SIZE: u32 = 28;

/// An image list.
#[derive(Clone, Debug, Default)]
pub struct ImageList {
    pub cx: i32,
    pub cy: i32,
    pub flags: u32,
    pub cur: i32,
    pub max: i32,
    pub initial: i32,
    pub grow: i32,
    pub bpp: u32,
    pub bk: u32,
    pub hdc_image: u32,
    pub hdc_mask: u32,
    pub hbm_image: u32,
    pub hbm_mask: u32,
    pub blend25: u32,
    pub blend50: u32,
    pub overlays: [i32; MAX_OVERLAY],
}

crate::state_fields!(ImageList {
    cx,
    cy,
    flags,
    cur,
    max,
    initial,
    grow,
    bpp,
    bk,
    hdc_image,
    hdc_mask,
    hbm_image,
    hbm_mask,
    blend25,
    blend50,
    overlays
});

/// The image being dragged (ImageList_BeginDrag), as Wine's
/// INTERNALDRAG.
#[derive(Clone, Debug, Default)]
pub struct Drag {
    pub hwnd: u32,
    pub himl: u32,
    pub x: i32,
    pub y: i32,
    pub dx: i32,
    pub dy: i32,
    pub show: bool,
    pub bg: u32,
}

crate::state_fields!(Drag {
    hwnd,
    himl,
    x,
    y,
    dx,
    dy,
    show,
    bg
});

fn gdi(cpu: &mut Cpu, name: &str, args: &[u32]) -> u32 {
    call_builtin(cpu, "GDI32.DLL", name, args).unwrap_or(0)
}

fn usr(cpu: &mut Cpu, name: &str, args: &[u32]) -> u32 {
    call_builtin(cpu, "USER32.DLL", name, args).unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
fn blt(
    cpu: &mut Cpu,
    dst: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    src: u32,
    sx: i32,
    sy: i32,
    rop: u32,
) -> bool {
    let a = [
        dst, x as u32, y as u32, w as u32, h as u32, src, sx as u32, sy as u32, rop,
    ];
    gdi(cpu, "BitBlt", &a) != 0
}

#[allow(clippy::too_many_arguments)]
fn stretch(
    cpu: &mut Cpu,
    dst: u32,
    (x, y, w, h): (i32, i32, i32, i32),
    src: u32,
    (sx, sy, sw, sh): (i32, i32, i32, i32),
    rop: u32,
) {
    let a = [
        dst, x as u32, y as u32, w as u32, h as u32, src, sx as u32, sy as u32, sw as u32,
        sh as u32, rop,
    ];
    gdi(cpu, "StretchBlt", &a);
}

fn select(cpu: &mut Cpu, hdc: u32, object: u32) -> u32 {
    gdi(cpu, "SelectObject", &[hdc, object])
}

fn delete(cpu: &mut Cpu, object: u32) {
    if object != 0 {
        gdi(cpu, "DeleteObject", &[object]);
    }
}

fn new_dc(cpu: &mut Cpu) -> u32 {
    gdi(cpu, "CreateCompatibleDC", &[0])
}

fn delete_dc(cpu: &mut Cpu, hdc: u32) {
    if hdc != 0 {
        gdi(cpu, "DeleteDC", &[hdc]);
    }
}

fn mono_bitmap(cpu: &mut Cpu, w: i32, h: i32) -> u32 {
    gdi(cpu, "CreateBitmap", &[w as u32, h as u32, 1, 1, 0])
}

/// A bitmap's width, height and bits a pixel.
fn bitmap_size(cpu: &mut Cpu, hbm: u32) -> Option<(i32, i32, u32)> {
    let b = gdi32::gdi(cpu).bitmap(hbm)?;
    Some((b.width, b.height, b.bpp as u32 * b.planes.max(1) as u32))
}

/// Bytes put in the process's heap for a call that takes them: their
/// address, or 0.
fn temp(cpu: &mut Cpu, bytes: &[u8]) -> u32 {
    let heap = process_heap(cpu);
    let at = heap_alloc(cpu, heap, bytes.len().max(4) as u32);
    if at != 0 {
        write_words(cpu, at, &words_of(bytes));
    }
    at
}

fn free_temp(cpu: &mut Cpu, at: u32) {
    if at != 0 {
        let heap = process_heap(cpu);
        heap_free(cpu, heap, at);
    }
}

/// The image list `himl`, if it is one.
pub fn get(cpu: &mut Cpu, himl: u32) -> Option<ImageList> {
    state(cpu).imagelists.get(&himl).cloned()
}

fn put(cpu: &mut Cpu, himl: u32, il: ImageList) {
    state(cpu).imagelists.insert(himl, il);
}

fn rows(count: i32) -> i32 {
    (count.max(0) + TILE - 1) / TILE
}

fn point(il: &ImageList, index: i32) -> (i32, i32) {
    ((index % TILE) * il.cx, (index / TILE) * il.cy)
}

fn bitmap_dims(il: &ImageList, count: i32) -> (i32, i32) {
    (il.cx * TILE, rows(count) * il.cy)
}

/// Copy `count` images from `src` in `hdc_src` to `dest` in `hdc_dest`,
/// a column of the tiles at a time.
fn copy_images(
    cpu: &mut Cpu,
    il: &ImageList,
    hdc_src: u32,
    hdc_dest: u32,
    src: i32,
    count: i32,
    dest: i32,
) {
    for i in 0..TILE {
        let (sx, sy) = point(il, src + i);
        let (dx, dy) = point(il, dest + i);
        let h = il.cy * rows(count - i);
        blt(cpu, hdc_dest, dx, dy, il.cx, h, hdc_src, sx, sy, SRCCOPY);
    }
}

/// The color bitmap for `count` images (ImageList_CreateImage): a DIB
/// section of the list's depth, or for ILC_COLORDDB a bitmap of GDI's.
fn create_image(cpu: &mut Cpu, il: &ImageList, count: i32) -> u32 {
    let (w, h) = bitmap_dims(il, count);
    let ilc = il.flags & 0xFE;
    if !((ILC_COLOR4..=ILC_COLOR32).contains(&ilc) || ilc == ILC_COLOR) {
        return gdi(cpu, "CreateBitmap", &[w as u32, h as u32, 1, il.bpp, 0]);
    }
    let mut info = Vec::with_capacity(40 + 1024);
    for v in [40u32, w as u32, h as u32] {
        info.extend_from_slice(&v.to_le_bytes());
    }
    info.extend_from_slice(&1u16.to_le_bytes());
    info.extend_from_slice(&(il.bpp as u16).to_le_bytes());
    info.resize(40, 0);
    if il.bpp <= 8 {
        // The system's static colors: the sixteen of the VGA for four
        // bits, the first ten and the last ten of 256 for eight.
        let s = gdi32::static_colors();
        let colors = 1usize << il.bpp;
        let mut table = vec![0u32; colors];
        let mut set = |at: usize, from: usize| {
            let c = s[from];
            table[at] = (c & 0xFF) << 16 | (c & 0xFF00) | (c >> 16) & 0xFF;
        };
        match colors {
            2 => set(1, 19),
            16 => (0..16).for_each(|i| set(i, if i < 8 { i } else { i + 4 })),
            _ => (0..20).for_each(|i| set(if i < 10 { i } else { colors - 20 + i }, i)),
        }
        for q in table {
            info.extend_from_slice(&q.to_le_bytes());
        }
    }
    let at = temp(cpu, &info);
    if at == 0 {
        return 0;
    }
    let hbm = gdi(cpu, "CreateDIBSection", &[il.hdc_image, at, 0, 0, 0, 0]);
    free_temp(cpu, at);
    hbm
}

/// Make room for `n` more images (IMAGELIST_InternalExpandBitmaps).
fn expand(cpu: &mut Cpu, il: &mut ImageList, n: i32) {
    if il.cur + n < il.max {
        return;
    }
    let count = il.max + n.max(il.grow) + 1;
    let (w, h) = bitmap_dims(il, count);
    let hdc = new_dc(cpu);
    let hbm = create_image(cpu, il, count);
    if il.cur != 0 {
        let old = select(cpu, hdc, hbm);
        blt(cpu, hdc, 0, 0, w, h, il.hdc_image, 0, 0, SRCCOPY);
        select(cpu, hdc, old);
    }
    select(cpu, il.hdc_image, hbm);
    delete(cpu, il.hbm_image);
    il.hbm_image = hbm;
    if il.flags & ILC_MASK != 0 {
        let hbm = mono_bitmap(cpu, w, h);
        if il.cur != 0 {
            let old = select(cpu, hdc, hbm);
            blt(cpu, hdc, 0, 0, w, h, il.hdc_mask, 0, 0, SRCCOPY);
            select(cpu, hdc, old);
        }
        select(cpu, il.hdc_mask, hbm);
        delete(cpu, il.hbm_mask);
        il.hbm_mask = hbm;
    }
    il.max = count;
    delete_dc(cpu, hdc);
}

/// ImageList_Create's work: the new list's handle, or 0.
pub fn create(cpu: &mut Cpu, cx: i32, cy: i32, flags: u32, initial: i32, grow: i32) -> u32 {
    let heap = process_heap(cpu);
    let himl = heap_alloc(cpu, heap, 16);
    if himl == 0 {
        return 0;
    }
    write_words(cpu, himl, &[u32::from(MAGIC), 0, 0, 0]);
    let grow = if grow < 4 { 4 } else { (grow + 3) & !3 };
    let mut il = ImageList {
        cx,
        cy,
        flags,
        max: initial + 1,
        initial,
        grow,
        bk: CLR_NONE,
        overlays: [-1; MAX_OVERLAY],
        ..ImageList::default()
    };
    il.hdc_image = new_dc(cpu);
    if flags & ILC_MASK != 0 {
        il.hdc_mask = new_dc(cpu);
    }
    let mut ilc = flags & 0xFE;
    if ilc == ILC_COLOR {
        ilc = ILC_COLOR4;
    }
    il.bpp = if (ILC_COLOR4..=ILC_COLOR32).contains(&ilc) {
        ilc
    } else {
        gdi32::dc::device_depth(cpu)
    };
    if il.max > 0 {
        il.hbm_image = create_image(cpu, &il, il.max);
        select(cpu, il.hdc_image, il.hbm_image);
        if flags & ILC_MASK != 0 {
            let (w, h) = bitmap_dims(&il, il.max);
            il.hbm_mask = mono_bitmap(cpu, w, h);
            select(cpu, il.hdc_mask, il.hbm_mask);
        }
    }
    for (pattern, slot) in [
        ([0xAAu16, 0, 0x55, 0, 0xAA, 0, 0x55, 0], 0),
        ([0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA], 1),
    ] {
        let bytes: Vec<u8> = pattern.iter().flat_map(|w| w.to_le_bytes()).collect();
        let at = temp(cpu, &bytes);
        let hbm = gdi(cpu, "CreateBitmap", &[8, 8, 1, 1, at]);
        free_temp(cpu, at);
        let brush = gdi(cpu, "CreatePatternBrush", &[hbm]);
        delete(cpu, hbm);
        if slot == 0 {
            il.blend25 = brush;
        } else {
            il.blend50 = brush;
        }
    }
    put(cpu, himl, il);
    himl
}

/// ImageList_Destroy's work.
pub fn destroy(cpu: &mut Cpu, himl: u32) -> bool {
    let Some(il) = state(cpu).imagelists.remove(&himl) else {
        return false;
    };
    delete_dc(cpu, il.hdc_image);
    delete_dc(cpu, il.hdc_mask);
    for o in [il.hbm_image, il.hbm_mask, il.blend25, il.blend50] {
        delete(cpu, o);
    }
    write_words(cpu, himl, &[0]);
    free_temp(cpu, himl);
    true
}

/// ImageList_Add's work, with the mask's bitmap or 0.
fn add(cpu: &mut Cpu, himl: u32, image: u32, mask: u32) -> i32 {
    let Some(mut il) = get(cpu, himl) else {
        return -1;
    };
    let Some((w, h, _)) = bitmap_size(cpu, image) else {
        return -1;
    };
    if il.cx <= 0 {
        return -1;
    }
    let n = w / il.cx;
    expand(cpu, &mut il, n);
    let hdc = new_dc(cpu);
    select(cpu, hdc, image);
    let temp_dc = if il.hbm_mask != 0 {
        let d = new_dc(cpu);
        select(cpu, d, mask);
        d
    } else {
        0
    };
    for i in 0..n {
        let (x, y) = point(&il, il.cur + i);
        blt(
            cpu,
            il.hdc_image,
            x,
            y,
            il.cx,
            h,
            hdc,
            i * il.cx,
            0,
            SRCCOPY,
        );
        if il.hbm_mask == 0 {
            continue;
        }
        blt(
            cpu,
            il.hdc_mask,
            x,
            y,
            il.cx,
            h,
            temp_dc,
            i * il.cx,
            0,
            SRCCOPY,
        );
        blt(
            cpu,
            il.hdc_image,
            x,
            y,
            il.cx,
            h,
            il.hdc_mask,
            x,
            y,
            NOTSRCAND,
        );
    }
    delete_dc(cpu, temp_dc);
    delete_dc(cpu, hdc);
    let first = il.cur;
    il.cur += n;
    put(cpu, himl, il);
    first
}

/// ImageList_AddMasked's work.
pub fn add_masked(cpu: &mut Cpu, himl: u32, image: u32, color: u32) -> i32 {
    if !state(cpu).imagelists.contains_key(&himl) {
        return -1;
    }
    let Some((w, h, _)) = bitmap_size(cpu, image) else {
        return -1;
    };
    let hdc = new_dc(cpu);
    select(cpu, hdc, image);
    let hdc_mask = new_dc(cpu);
    let mask = mono_bitmap(cpu, w, h);
    select(cpu, hdc_mask, mask);
    let bk = if color != CLR_DEFAULT {
        color
    } else {
        gdi(cpu, "GetPixel", &[hdc, 0, 0])
    };
    gdi(cpu, "SetBkColor", &[hdc, bk]);
    blt(cpu, hdc_mask, 0, 0, w, h, hdc, 0, 0, SRCCOPY);
    gdi(cpu, "SetBkColor", &[hdc, 0xFF_FFFF]);
    // As Windows does, the program's bitmap loses its background too.
    blt(cpu, hdc, 0, 0, w, h, hdc_mask, 0, 0, NOTSRCAND);
    delete_dc(cpu, hdc);
    delete_dc(cpu, hdc_mask);
    let r = add(cpu, himl, image, mask);
    delete(cpu, mask);
    r
}

/// ImageList_ReplaceIcon's work: the index, or -1.
pub fn replace_icon(cpu: &mut Cpu, himl: u32, index: i32, icon: u32) -> i32 {
    let Some(mut il) = get(cpu, himl) else {
        return -1;
    };
    if index >= il.max || index < -1 {
        return -1;
    }
    let (cx, cy) = (il.cx as u32, il.cy as u32);
    let mut best = usr(
        cpu,
        "CopyImage",
        &[icon, IMAGE_ICON, cx, cy, LR_COPYFROMRESOURCE],
    );
    if best == 0 {
        best = usr(cpu, "CopyImage", &[icon, IMAGE_ICON, cx, cy, 0]);
    }
    if best == 0 {
        return -1;
    }
    let Some(ic) = user32::resource::icon(cpu, best) else {
        usr(cpu, "DestroyIcon", &[best]);
        return -1;
    };
    let Some((w, h, _)) = bitmap_size(cpu, ic.mask) else {
        usr(cpu, "DestroyIcon", &[best]);
        return -1;
    };
    let mut index = index;
    if index == -1 {
        if il.cur + 1 > il.max {
            expand(cpu, &mut il, 1);
        }
        index = il.cur;
        il.cur += 1;
    }
    let hdc = new_dc(cpu);
    let (x, y) = point(&il, index);
    gdi(cpu, "SetTextColor", &[il.hdc_image, 0]);
    gdi(cpu, "SetBkColor", &[il.hdc_image, 0xFF_FFFF]);
    let to = (x, y, il.cx, il.cy);
    if ic.color != 0 {
        let old = select(cpu, hdc, ic.color);
        stretch(cpu, il.hdc_image, to, hdc, (0, 0, w, h), SRCCOPY);
        if il.hbm_mask != 0 {
            select(cpu, hdc, ic.mask);
            stretch(cpu, il.hdc_mask, to, hdc, (0, 0, w, h), SRCCOPY);
        }
        select(cpu, hdc, old);
    } else {
        let half = h / 2;
        let old = select(cpu, hdc, ic.mask);
        stretch(cpu, il.hdc_image, to, hdc, (0, half, w, half), SRCCOPY);
        if il.hbm_mask != 0 {
            stretch(cpu, il.hdc_mask, to, hdc, (0, 0, w, half), SRCCOPY);
        }
        select(cpu, hdc, old);
    }
    delete_dc(cpu, hdc);
    usr(cpu, "DestroyIcon", &[best]);
    put(cpu, himl, il);
    index
}

/// What ImageList_DrawIndirect is given.
#[derive(Clone, Copy, Debug, Default)]
pub struct DrawParams {
    pub himl: u32,
    pub i: i32,
    pub hdc: u32,
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
    pub x_bitmap: i32,
    pub y_bitmap: i32,
    pub bk: u32,
    pub fg: u32,
    pub style: u32,
    pub rop: u32,
}

/// Draw an image: ImageList_DrawIndirect's work.
pub fn draw_indirect(cpu: &mut Cpu, p: &DrawParams) -> bool {
    let Some(il) = get(cpu, p.himl) else {
        return false;
    };
    if p.i < 0 || p.i >= il.cur {
        return false;
    }
    let (mut px, mut py) = point(&il, p.i);
    px += p.x_bitmap;
    py += p.y_bitmap;
    let style = p.style & !ILD_OVERLAYMASK;
    let cx = if p.cx == 0 { il.cx } else { p.cx };
    let cy = if p.cy == 0 { il.cy } else { p.cy };
    let transparent = style & ILD_TRANSPARENT != 0
        || p.bk == CLR_NONE
        || (p.bk == CLR_DEFAULT && il.bk == CLR_NONE);
    let mask_only = il.flags & ILC_MASK != 0 && style & ILD_MASK != 0;
    let blend = style & (ILD_BLEND25 | ILD_BLEND50) != 0 && !mask_only;
    let image_dc = gdi(cpu, "CreateCompatibleDC", &[p.hdc]);
    let image_bm = gdi(
        cpu,
        "CreateCompatibleBitmap",
        &[p.hdc, cx as u32, cy as u32],
    );
    let blend_bm = if blend { mono_bitmap(cpu, cx, cy) } else { 0 };
    let mut ok = false;
    if il.hdc_image != 0
        && image_dc != 0
        && image_bm != 0
        && (!blend || blend_bm != 0)
        && (il.hbm_mask == 0 || il.hdc_mask != 0)
    {
        ok = draw_with(
            cpu,
            p,
            &il,
            (px, py, cx, cy),
            transparent,
            mask_only,
            (image_dc, image_bm, blend_bm),
        );
    }
    delete(cpu, blend_bm);
    delete(cpu, image_bm);
    delete_dc(cpu, image_dc);
    ok
}

fn draw_with(
    cpu: &mut Cpu,
    p: &DrawParams,
    il: &ImageList,
    (px, py, cx, cy): (i32, i32, i32, i32),
    transparent: bool,
    mask_only: bool,
    (image_dc, image_bm, blend_bm): (u32, u32, u32),
) -> bool {
    let style = p.style & !ILD_OVERLAYMASK;
    let old_bm = select(cpu, image_dc, image_bm);
    let old_fg = gdi(cpu, "SetTextColor", &[image_dc, 0]);
    let old_bk = gdi(cpu, "SetBkColor", &[image_dc, 0xFF_FFFF]);
    let finish = |cpu: &mut Cpu| {
        gdi(cpu, "SetBkColor", &[image_dc, old_bk]);
        gdi(cpu, "SetTextColor", &[image_dc, old_fg]);
        select(cpu, image_dc, old_bm);
    };
    let solid = |cpu: &mut Cpu, color: u32| {
        let brush = gdi(cpu, "CreateSolidBrush", &[color]);
        let old = select(cpu, image_dc, brush);
        gdi(
            cpu,
            "PatBlt",
            &[image_dc, 0, 0, cx as u32, cy as u32, PATCOPY],
        );
        old
    };
    if mask_only {
        if il.hbm_mask != 0 {
            let fg = gdi(cpu, "GetTextColor", &[p.hdc]);
            let old = solid(cpu, fg);
            blt(cpu, image_dc, 0, 0, cx, cy, il.hdc_mask, px, py, SRCPAINT);
            let b = select(cpu, image_dc, old);
            delete(cpu, b);
            if transparent {
                blt(cpu, p.hdc, p.x, p.y, cx, cy, image_dc, 0, 0, SRCAND);
                finish(cpu);
                return true;
            }
        } else {
            let black = gdi(cpu, "GetStockObject", &[4]);
            let old = select(cpu, image_dc, black);
            gdi(
                cpu,
                "PatBlt",
                &[image_dc, 0, 0, cx as u32, cy as u32, PATCOPY],
            );
            select(cpu, image_dc, old);
        }
    } else {
        let mut color = 0;
        if !transparent {
            color = p.bk;
            if color == CLR_DEFAULT {
                color = il.bk;
            }
            if color == CLR_NONE {
                color = gdi(cpu, "GetBkColor", &[p.hdc]);
            }
        }
        let old = solid(cpu, color);
        if il.hbm_mask != 0 {
            blt(cpu, image_dc, 0, 0, cx, cy, il.hdc_mask, px, py, SRCAND);
            blt(cpu, image_dc, 0, 0, cx, cy, il.hdc_image, px, py, SRCPAINT);
        } else {
            blt(cpu, image_dc, 0, 0, cx, cy, il.hdc_image, px, py, SRCCOPY);
        }
        let b = select(cpu, image_dc, old);
        delete(cpu, b);
    }
    if blend_bm != 0 {
        // The blend mask, in the list's own image DC for the while.
        let mdc = il.hdc_image;
        let old_bitmap = select(cpu, mdc, blend_bm);
        let brush = if style & ILD_BLEND50 != 0 {
            il.blend50
        } else {
            il.blend25
        };
        let old = select(cpu, mdc, brush);
        gdi(cpu, "PatBlt", &[mdc, 0, 0, cx as u32, cy as u32, PATCOPY]);
        select(cpu, mdc, old);
        if il.hbm_mask != 0 {
            blt(cpu, mdc, 0, 0, cx, cy, il.hdc_mask, px, py, NOTSRCAND);
            blt(cpu, mdc, 0, 0, cx, cy, mdc, 0, 0, NOTSRCCOPY);
        }
        let mut color = p.fg;
        if color == CLR_DEFAULT {
            color = usr(cpu, "GetSysColor", &[COLOR_HIGHLIGHT]);
        } else if color == CLR_NONE {
            color = gdi(cpu, "GetTextColor", &[p.hdc]);
        }
        let b = gdi(cpu, "CreateSolidBrush", &[color]);
        let old = select(cpu, image_dc, b);
        blt(cpu, image_dc, 0, 0, cx, cy, mdc, 0, 0, BLEND_ROP);
        let b = select(cpu, image_dc, old);
        delete(cpu, b);
        select(cpu, mdc, old_bitmap);
    }
    let overlay = ((p.style & ILD_OVERLAYMASK) >> 8) as usize;
    if (1..=MAX_OVERLAY).contains(&overlay) {
        let index = il.overlays[overlay - 1];
        if index >= 0 && index < il.cur {
            let (mut ox, oy) = point(il, index);
            ox += p.x_bitmap;
            if il.hbm_mask != 0 && style & ILD_IMAGE == 0 {
                blt(cpu, image_dc, 0, 0, cx, cy, il.hdc_mask, ox, oy, SRCAND);
            }
            blt(cpu, image_dc, 0, 0, cx, cy, il.hdc_image, ox, oy, SRCPAINT);
        }
    }
    let mut rop = SRCCOPY;
    if il.hbm_mask != 0 && transparent {
        let fg = gdi(cpu, "SetTextColor", &[p.hdc, 0]);
        let bk = gdi(cpu, "SetBkColor", &[p.hdc, 0xFF_FFFF]);
        blt(cpu, p.hdc, p.x, p.y, cx, cy, il.hdc_mask, px, py, SRCAND);
        gdi(cpu, "SetBkColor", &[p.hdc, bk]);
        gdi(cpu, "SetTextColor", &[p.hdc, fg]);
        rop = SRCPAINT;
    }
    if style & ILD_ROP != 0 {
        rop = p.rop;
    }
    blt(cpu, p.hdc, p.x, p.y, cx, cy, image_dc, 0, 0, rop);
    finish(cpu);
    true
}

/// ImageList_DrawEx's work, for the controls.
#[allow(clippy::too_many_arguments)]
pub fn draw_ex(
    cpu: &mut Cpu,
    himl: u32,
    i: i32,
    hdc: u32,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    bk: u32,
    fg: u32,
    style: u32,
) -> bool {
    let p = DrawParams {
        himl,
        i,
        hdc,
        x,
        y,
        cx,
        cy,
        bk,
        fg,
        style,
        ..DrawParams::default()
    };
    draw_indirect(cpu, &p)
}

/// ImageList_Draw's work, for the controls.
pub fn draw(cpu: &mut Cpu, himl: u32, i: i32, hdc: u32, x: i32, y: i32, style: u32) -> bool {
    draw_ex(
        cpu,
        himl,
        i,
        hdc,
        x,
        y,
        0,
        0,
        CLR_DEFAULT,
        CLR_DEFAULT,
        style,
    )
}

/// The size of the images, if `himl` is an image list.
pub fn icon_size(cpu: &mut Cpu, himl: u32) -> Option<(i32, i32)> {
    get(cpu, himl).map(|il| (il.cx, il.cy))
}

/// How many images `himl` has.
pub fn count(cpu: &mut Cpu, himl: u32) -> i32 {
    get(cpu, himl).map_or(0, |il| il.cur)
}

fn merge(cpu: &mut Cpu, h1: u32, i1: i32, h2: u32, i2: i32, dx: i32, dy: i32) -> u32 {
    let (Some(a), Some(b)) = (get(cpu, h1), get(cpu, h2)) else {
        return 0;
    };
    let (cxd, xo1, xo2) = match dx {
        d if d > 0 => (a.cx.max(d + b.cx), 0, d),
        d if d < 0 => (b.cx.max(a.cx - d), -d, 0),
        _ => (a.cx.max(b.cx), 0, 0),
    };
    let (cyd, yo1, yo2) = match dy {
        d if d > 0 => (a.cy.max(d + b.cy), 0, d),
        d if d < 0 => (b.cy.max(a.cy - d), -d, 0),
        _ => (a.cy.max(b.cy), 0, 0),
    };
    let dst = create(cpu, cxd, cyd, ILC_MASK | ILC_COLOR, 1, 1);
    let Some(mut d) = get(cpu, dst) else {
        return 0;
    };
    let p1 = point(&a, i1);
    let p2 = point(&b, i2);
    let has1 = i1 >= 0 && i1 < a.cur;
    let has2 = i2 >= 0 && i2 < b.cur;
    blt(
        cpu,
        d.hdc_image,
        0,
        0,
        cxd,
        cyd,
        a.hdc_image,
        0,
        0,
        BLACKNESS,
    );
    if has1 {
        blt(
            cpu,
            d.hdc_image,
            xo1,
            yo1,
            a.cx,
            a.cy,
            a.hdc_image,
            p1.0,
            p1.1,
            SRCCOPY,
        );
    }
    if has2 {
        blt(
            cpu,
            d.hdc_image,
            xo2,
            yo2,
            b.cx,
            b.cy,
            b.hdc_mask,
            p2.0,
            p2.1,
            SRCAND,
        );
        blt(
            cpu,
            d.hdc_image,
            xo2,
            yo2,
            b.cx,
            b.cy,
            b.hdc_image,
            p2.0,
            p2.1,
            SRCPAINT,
        );
    }
    blt(cpu, d.hdc_mask, 0, 0, cxd, cyd, a.hdc_mask, 0, 0, WHITENESS);
    if has1 {
        blt(
            cpu, d.hdc_mask, xo1, yo1, a.cx, a.cy, a.hdc_mask, p1.0, p1.1, SRCCOPY,
        );
    }
    if has2 {
        blt(
            cpu, d.hdc_mask, xo2, yo2, b.cx, b.cy, b.hdc_mask, p2.0, p2.1, SRCAND,
        );
    }
    d.cur = 1;
    put(cpu, dst, d);
    dst
}

fn drag(cpu: &mut Cpu) -> Drag {
    state(cpu).drag.clone()
}

fn set_drag(cpu: &mut Cpu, d: Drag) {
    state(cpu).drag = d;
}

/// Draw the drag image at (x, y) on `hdc` (ImageList_InternalDragDraw).
fn drag_draw(cpu: &mut Cpu, himl: u32, hdc: u32, x: i32, y: i32) {
    let p = DrawParams {
        himl,
        hdc,
        x,
        y,
        bk: CLR_DEFAULT,
        fg: CLR_DEFAULT,
        ..DrawParams::default()
    };
    draw_indirect(cpu, &p);
}

const DCX_WINDOW: u32 = 0x0001;
const DCX_CACHE: u32 = 0x0002;
const DCX_LOCKWINDOWUPDATE: u32 = 0x0400;

fn drag_show(cpu: &mut Cpu, show: bool) -> bool {
    let mut d = drag(cpu);
    let Some(il) = get(cpu, d.himl) else {
        return false;
    };
    if d.show == show {
        return false;
    }
    let (x, y) = (d.x - d.dx, d.y - d.dy);
    let flags = DCX_WINDOW | DCX_CACHE | DCX_LOCKWINDOWUPDATE;
    let hdc = usr(cpu, "GetDCEx", &[d.hwnd, 0, flags]);
    if hdc == 0 {
        return false;
    }
    let bg_dc = gdi(cpu, "CreateCompatibleDC", &[hdc]);
    if d.bg == 0 {
        d.bg = gdi(
            cpu,
            "CreateCompatibleBitmap",
            &[hdc, il.cx as u32, il.cy as u32],
        );
    }
    select(cpu, bg_dc, d.bg);
    if show {
        blt(cpu, bg_dc, 0, 0, il.cx, il.cy, hdc, x, y, SRCCOPY);
        drag_draw(cpu, d.himl, hdc, x, y);
    } else {
        blt(cpu, hdc, x, y, il.cx, il.cy, bg_dc, 0, 0, SRCCOPY);
    }
    d.show = show;
    delete_dc(cpu, bg_dc);
    usr(cpu, "ReleaseDC", &[d.hwnd, hdc]);
    set_drag(cpu, d);
    true
}

fn end_drag(cpu: &mut Cpu) {
    let d = drag(cpu);
    destroy(cpu, d.himl);
    delete(cpu, d.bg);
    set_drag(cpu, Drag::default());
}

/// ILHEAD and the bitmaps after it, as ImageList_Write writes them.
fn bitmap_file(cpu: &mut Cpu, hbm: u32) -> Option<Vec<u8>> {
    let (w, h, bpp) = bitmap_size(cpu, hbm)?;
    let bits = if bpp == 1 { 1 } else { 24 };
    let stride = ((w * bits + 31) / 32 * 4) as u32;
    let size = stride * h.unsigned_abs();
    let mut header_len: u32 = 14 + 40;
    if bits != 24 {
        header_len += 4 << (bits as u32);
    }
    let mut out = vec![0u8; header_len as usize];
    out[0..2].copy_from_slice(b"BM");
    out[2..6].copy_from_slice(&header_len.to_le_bytes());
    out[10..14].copy_from_slice(&header_len.to_le_bytes());
    let info: [u32; 10] = [
        40,
        w as u32,
        h as u32,
        1 | (bits as u32) << 16,
        0,
        size,
        0,
        0,
        0,
        0,
    ];
    for (i, v) in info.iter().enumerate() {
        out[14 + 4 * i..18 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    // GetDIBits into the program's memory, then the bytes back.
    let mem = temp(cpu, &vec![0u8; (40 + 1024 + size) as usize]);
    if mem == 0 {
        return None;
    }
    write_words(cpu, mem, &info);
    let screen = usr(cpu, "GetDC", &[0]);
    let lines = gdi(
        cpu,
        "GetDIBits",
        &[screen, hbm, 0, h as u32, mem + 40 + 1024, mem, 0],
    );
    usr(cpu, "ReleaseDC", &[0, screen]);
    if lines != h as u32 {
        free_temp(cpu, mem);
        return None;
    }
    if bits == 1 {
        out[54..62].copy_from_slice(&[0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0]);
    }
    let mut data = vec![0u8; size as usize];
    for (i, b) in data.iter_mut().enumerate() {
        *b = cpu.bus.guest_read_8(mem + 40 + 1024 + i as u32);
    }
    free_temp(cpu, mem);
    out.extend_from_slice(&data);
    Some(out)
}

// The continuations of ImageList_Write and ImageList_Read.
const W_NEXT: u16 = 1;
const R_HEAD: u16 = 2;
const R_INFO: u16 = 3;
const R_PALETTE: u16 = 4;
const R_BITS: u16 = 5;

const E_FAILED: u32 = 0x8000_0000;
const IS_WRITE: u32 = 16;
const IS_READ: u32 = 12;

/// The method of the COM object `this` at `offset` in its vtable.
fn method(cpu: &mut Cpu, this: u32, offset: u32) -> u32 {
    let vtbl = cpu.bus.guest_read_32(this);
    cpu.bus.guest_read_32(vtbl.wrapping_add(offset))
}

/// The scratch space of ImageList_Read: ILHEAD, then BITMAPFILEHEADER,
/// BITMAPINFOHEADER and a color table.
const R_SCRATCH: u32 = 28 + 14 + 40 + 1024;
const R_FILE: u32 = 28;

/// The continuations.
pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    match id {
        W_NEXT => {
            // data: stream, buffer, offset, then the sizes left.
            let (stream, buf, off) = (c.word(0), c.word(1), c.word(2));
            if c.last & E_FAILED != 0 || c.data.len() < 4 {
                free_temp(cpu, buf);
                let ok = c.last & E_FAILED == 0;
                return vec![Op::Return(Arg::V(ok as u32))];
            }
            let n = c.word(3);
            let f = method(cpu, stream, IS_WRITE);
            let mut rest = vec![stream, buf, off + n];
            rest.extend_from_slice(&c.data[4..]);
            vec![
                Op::Call {
                    target: f,
                    args: vec![Arg::V(stream), Arg::V(buf + off), Arg::V(n), Arg::V(0)],
                },
                Op::Then {
                    id: W_NEXT,
                    data: rest,
                },
            ]
        }
        R_HEAD => read_head(cpu, c),
        R_INFO => read_info(cpu, c),
        R_PALETTE => read_palette(cpu, c),
        R_BITS => read_bits(cpu, c),
        _ => Vec::new(),
    }
}

fn fail() -> Vec<Op> {
    vec![Op::Return(Arg::V(0))]
}

fn read_call(cpu: &mut Cpu, stream: u32, at: Arg, n: u32) -> Op {
    Op::Call {
        target: method(cpu, stream, IS_READ),
        args: vec![Arg::V(stream), at, Arg::V(n), Arg::V(0)],
    }
}

/// After ILHEAD: make the list, then read the image's bitmap.
fn read_head(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let stream = c.word(0);
    if c.last & E_FAILED != 0 {
        return fail();
    }
    let at = c.scratch;
    let w: Vec<u16> = (0..14).map(|i| cpu.bus.guest_read_16(at + 2 * i)).collect();
    if w[0] != MAGIC || w[1] != 0x101 {
        return fail();
    }
    let (cur, max, cx, cy) = (w[2] as i32, w[3] as i32, w[5] as i32, w[6] as i32);
    let flags = w[9] as u32;
    let himl = create(cpu, cx, cy, flags, cur, max);
    if himl == 0 {
        return fail();
    }
    let read = read_call(cpu, stream, Arg::S(R_FILE), 54);
    vec![
        read,
        Op::Then {
            id: R_INFO,
            data: vec![stream, himl, 0],
        },
    ]
}

/// After a bitmap's headers: read its color table.
fn read_info(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (stream, himl, which) = (c.word(0), c.word(1), c.word(2));
    if c.last & E_FAILED != 0 {
        return fail();
    }
    let at = c.scratch + R_FILE;
    let kind = cpu.bus.guest_read_16(at);
    let size = cpu.bus.guest_read_32(at + 14);
    if kind != 0x4D42 || size != 40 {
        return fail();
    }
    let planes = cpu.bus.guest_read_16(at + 26) as u32;
    let bits = cpu.bus.guest_read_16(at + 28) as u32;
    let bpp = planes * bits;
    let palette = if bpp <= 8 { 4 << bpp } else { 0 };
    let mut ops = Vec::new();
    if palette > 0 {
        ops.push(read_call(cpu, stream, Arg::S(R_FILE + 54), palette));
    }
    ops.push(Op::Then {
        id: R_PALETTE,
        data: vec![stream, himl, which],
    });
    ops
}

fn dib_bytes(width: i32, height: i32, bpp: u32) -> u32 {
    let words = match bpp {
        1 => (width + 31) / 32,
        4 => (width + 7) / 8,
        8 => (width + 3) / 4,
        15 | 16 => (width + 1) / 2,
        24 => (width * 3 + 3) / 4,
        _ => width,
    };
    (4 * words.max(0)) as u32 * height.unsigned_abs()
}

/// After the color table: read the bits.
fn read_palette(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (stream, himl, which) = (c.word(0), c.word(1), c.word(2));
    if c.last & E_FAILED != 0 {
        return fail();
    }
    let info = c.scratch + R_FILE + 14;
    let w = cpu.bus.guest_read_32(info + 4) as i32;
    let h = cpu.bus.guest_read_32(info + 8) as i32;
    let planes = cpu.bus.guest_read_16(info + 12) as u32;
    let bits = cpu.bus.guest_read_16(info + 14) as u32;
    let size = dib_bytes(w, h, planes * bits);
    cpu.bus.guest_write_32(info + 20, size);
    let heap = process_heap(cpu);
    let buf = heap_alloc(cpu, heap, size.max(4));
    if buf == 0 {
        return fail();
    }
    let read = read_call(cpu, stream, Arg::V(buf), size);
    vec![
        read,
        Op::Then {
            id: R_BITS,
            data: vec![stream, himl, which, buf],
        },
    ]
}

/// After the bits: draw them into the list, then read the mask or
/// finish.
fn read_bits(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let (stream, himl, which, buf) = (c.word(0), c.word(1), c.word(2), c.word(3));
    if c.last & E_FAILED != 0 {
        free_temp(cpu, buf);
        return fail();
    }
    let Some(mut il) = get(cpu, himl) else {
        free_temp(cpu, buf);
        return fail();
    };
    let info = c.scratch + R_FILE + 14;
    let w = cpu.bus.guest_read_32(info + 4);
    let h = cpu.bus.guest_read_32(info + 8);
    let hdc = if which == 0 {
        il.hdc_image
    } else {
        il.hdc_mask
    };
    let a = [hdc, 0, 0, w, h, 0, 0, w, h, buf, info, 0, SRCCOPY];
    let drawn = gdi(cpu, "StretchDIBits", &a);
    free_temp(cpu, buf);
    if drawn == 0 {
        return fail();
    }
    if which == 0 && il.flags & ILC_MASK != 0 {
        let read = read_call(cpu, stream, Arg::S(R_FILE), 54);
        return vec![
            read,
            Op::Then {
                id: R_INFO,
                data: vec![stream, himl, 1],
            },
        ];
    }
    let at = c.scratch;
    let head: Vec<u16> = (0..14).map(|i| cpu.bus.guest_read_16(at + 2 * i)).collect();
    il.cur = head[2] as i32;
    il.max = head[3] as i32;
    il.bk = head[7] as u32 | (head[8] as u32) << 16;
    for i in 0..4 {
        let o = head[10 + i] as i16 as i32;
        if o == -1 || (o >= 0 && o <= il.cur) {
            il.overlays[i] = o;
        }
    }
    put(cpu, himl, il);
    vec![Op::Return(Arg::V(himl))]
}

// The exported functions.

/// ImageList_Create(cx, cy, flags, initial, grow).
pub fn il_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = |i: usize| args[i] as i32;
    Some(create(cpu, a(0), a(1), args[2], a(3), a(4)))
}

/// ImageList_Destroy(himl).
pub fn il_destroy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(destroy(cpu, args[0]) as u32)
}

/// ImageList_Add(himl, image, mask).
pub fn il_add(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(add(cpu, args[0], args[1], args[2]) as u32)
}

/// ImageList_AddMasked(himl, image, color).
pub fn il_add_masked(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(add_masked(cpu, args[0], args[1], args[2]) as u32)
}

/// ImageList_AddIcon(himl, icon).
pub fn il_add_icon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(replace_icon(cpu, args[0], -1, args[1]) as u32)
}

/// ImageList_ReplaceIcon(himl, index, icon).
pub fn il_replace_icon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(replace_icon(cpu, args[0], args[1] as i32, args[2]) as u32)
}

/// ImageList_GetImageCount(himl).
pub fn il_get_image_count(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(count(cpu, args[0]) as u32)
}

/// ImageList_GetIconSize(himl, cx, cy).
pub fn il_get_icon_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(il) = get(cpu, args[0]) else {
        return Some(0);
    };
    if il.cx <= 0 || il.cy <= 0 {
        return Some(0);
    }
    if args[1] != 0 {
        write_words(cpu, args[1], &[il.cx as u32]);
    }
    if args[2] != 0 {
        write_words(cpu, args[2], &[il.cy as u32]);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(1)
}

/// The image list's images all go and the list is made again for
/// `max` (ImageList_SetIconSize, ImageList_Remove(-1)).
fn reset(cpu: &mut Cpu, il: &mut ImageList) {
    il.cur = 0;
    il.overlays = [-1; MAX_OVERLAY];
    let hbm = create_image(cpu, il, il.max);
    select(cpu, il.hdc_image, hbm);
    delete(cpu, il.hbm_image);
    il.hbm_image = hbm;
    if il.hbm_mask != 0 {
        let (w, h) = bitmap_dims(il, il.max);
        let hbm = mono_bitmap(cpu, w, h);
        select(cpu, il.hdc_mask, hbm);
        delete(cpu, il.hbm_mask);
        il.hbm_mask = hbm;
    }
}

/// ImageList_SetIconSize(himl, cx, cy).
pub fn il_set_icon_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(mut il) = get(cpu, args[0]) else {
        return Some(0);
    };
    il.max = il.initial + 1;
    il.cx = args[1] as i32;
    il.cy = args[2] as i32;
    reset(cpu, &mut il);
    put(cpu, args[0], il);
    Some(1)
}

/// ImageList_Remove(himl, index).
pub fn il_remove(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (himl, i) = (args[0], args[1] as i32);
    let Some(mut il) = get(cpu, himl) else {
        return Some(0);
    };
    if i < -1 || i >= il.cur {
        return Some(0);
    }
    if i == -1 {
        if il.cur == 0 {
            return Some(1);
        }
        il.max = il.initial + il.grow - 1;
        reset(cpu, &mut il);
        put(cpu, himl, il);
        return Some(1);
    }
    let image = create_image(cpu, &il, il.max);
    let (w, h) = bitmap_dims(&il, il.max);
    let mask = if il.hbm_mask != 0 {
        mono_bitmap(cpu, w, h)
    } else {
        0
    };
    let hdc = new_dc(cpu);
    if i > 0 {
        select(cpu, hdc, image);
        copy_images(cpu, &il, il.hdc_image, hdc, 0, i, 0);
        if mask != 0 {
            select(cpu, hdc, mask);
            copy_images(cpu, &il, il.hdc_mask, hdc, 0, i, 0);
        }
    }
    if i < il.cur - 1 {
        select(cpu, hdc, image);
        copy_images(cpu, &il, il.hdc_image, hdc, i + 1, il.cur - i, i);
        if mask != 0 {
            select(cpu, hdc, mask);
            copy_images(cpu, &il, il.hdc_mask, hdc, i + 1, il.cur - i, i);
        }
    }
    delete_dc(cpu, hdc);
    select(cpu, il.hdc_image, image);
    delete(cpu, il.hbm_image);
    il.hbm_image = image;
    if mask != 0 {
        select(cpu, il.hdc_mask, mask);
        delete(cpu, il.hbm_mask);
        il.hbm_mask = mask;
    }
    il.cur -= 1;
    put(cpu, himl, il);
    Some(1)
}

/// ImageList_Replace(himl, index, image, mask).
pub fn il_replace(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (himl, i, image, mask) = (args[0], args[1] as i32, args[2], args[3]);
    let Some(il) = get(cpu, himl) else {
        return Some(0);
    };
    if i >= il.max || i < 0 {
        return Some(0);
    }
    let Some((w, h, _)) = bitmap_size(cpu, image) else {
        return Some(0);
    };
    let hdc = new_dc(cpu);
    select(cpu, hdc, image);
    let (x, y) = point(&il, i);
    stretch(
        cpu,
        il.hdc_image,
        (x, y, il.cx, il.cy),
        hdc,
        (0, 0, w, h),
        SRCCOPY,
    );
    if il.hbm_mask != 0 {
        let t = new_dc(cpu);
        let old = select(cpu, t, mask);
        stretch(
            cpu,
            il.hdc_mask,
            (x, y, il.cx, il.cy),
            t,
            (0, 0, w, h),
            SRCCOPY,
        );
        select(cpu, t, old);
        delete_dc(cpu, t);
        blt(cpu, il.hdc_image, x, y, w, h, il.hdc_mask, x, y, NOTSRCAND);
    }
    delete_dc(cpu, hdc);
    Some(1)
}

/// ImageList_SetImageCount(himl, count).
pub fn il_set_image_count(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (himl, n) = (args[0], args[1]);
    let Some(mut il) = get(cpu, himl) else {
        return Some(0);
    };
    if il.max as u32 > n {
        il.cur = n as i32;
        put(cpu, himl, il);
        return Some(1);
    }
    let n = n as i32;
    let new_count = n + il.grow;
    let copy = il.cur.min(n);
    let hdc = new_dc(cpu);
    let hbm = create_image(cpu, &il, new_count);
    if hbm != 0 {
        let old = select(cpu, hdc, hbm);
        copy_images(cpu, &il, il.hdc_image, hdc, 0, copy, 0);
        select(cpu, hdc, old);
        select(cpu, il.hdc_image, hbm);
        delete(cpu, il.hbm_image);
        il.hbm_image = hbm;
    }
    if il.hbm_mask != 0 {
        let (w, h) = bitmap_dims(&il, new_count);
        let hbm = mono_bitmap(cpu, w, h);
        if hbm != 0 {
            let old = select(cpu, hdc, hbm);
            copy_images(cpu, &il, il.hdc_mask, hdc, 0, copy, 0);
            select(cpu, hdc, old);
            select(cpu, il.hdc_mask, hbm);
            delete(cpu, il.hbm_mask);
            il.hbm_mask = hbm;
        }
    }
    delete_dc(cpu, hdc);
    il.max = new_count;
    il.cur = n;
    put(cpu, himl, il);
    Some(1)
}

/// ImageList_SetBkColor(himl, color): the color before.
pub fn il_set_bk_color(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(mut il) = get(cpu, args[0]) else {
        return Some(CLR_NONE);
    };
    let old = std::mem::replace(&mut il.bk, args[1]);
    put(cpu, args[0], il);
    Some(old)
}

/// ImageList_GetBkColor(himl).
pub fn il_get_bk_color(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(get(cpu, args[0]).map_or(CLR_NONE, |il| il.bk))
}

/// ImageList_SetOverlayImage(himl, image, overlay).
pub fn il_set_overlay_image(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (image, overlay) = (args[1] as i32, args[2] as i32);
    let Some(mut il) = get(cpu, args[0]) else {
        return Some(0);
    };
    if !(1..=MAX_OVERLAY as i32).contains(&overlay) {
        return Some(0);
    }
    if image != -1 && (image < 0 || image > il.cur) {
        return Some(0);
    }
    il.overlays[overlay as usize - 1] = image;
    put(cpu, args[0], il);
    Some(1)
}

/// ImageList_Draw(himl, i, hdc, x, y, style).
pub fn il_draw(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = |i: usize| args[i] as i32;
    Some(draw(cpu, args[0], a(1), args[2], a(3), a(4), args[5]) as u32)
}

/// ImageList_DrawEx(himl, i, hdc, x, y, dx, dy, bk, fg, style).
pub fn il_draw_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = |i: usize| args[i] as i32;
    let (bk, fg, style) = (args[7], args[8], args[9]);
    Some(draw_ex(
        cpu,
        args[0],
        a(1),
        args[2],
        a(3),
        a(4),
        a(5),
        a(6),
        bk,
        fg,
        style,
    ) as u32)
}

/// ImageList_DrawIndirect(params): of the structure, the fields
/// COMCTL32 4.70 has (its size isn't checked).
pub fn il_draw_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    let w = read_words(cpu, args[0], 14);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let p = DrawParams {
        himl: w[1],
        i: w[2] as i32,
        hdc: w[3],
        x: w[4] as i32,
        y: w[5] as i32,
        cx: w[6] as i32,
        cy: w[7] as i32,
        x_bitmap: w[8] as i32,
        y_bitmap: w[9] as i32,
        bk: w[10],
        fg: w[11],
        style: w[12],
        rop: w[13],
    };
    if p.himl == 0 {
        return Some(0);
    }
    Some(draw_indirect(cpu, &p) as u32)
}

/// ImageList_Copy(dst, idst, src, isrc, flags).
pub fn il_copy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hd, id, hs, is, flags) = (args[0], args[1] as i32, args[2], args[3] as i32, args[4]);
    let (Some(d), Some(s)) = (get(cpu, hd), get(cpu, hs)) else {
        return Some(0);
    };
    if id < 0 || id >= d.cur || is < 0 || is >= s.cur {
        return Some(0);
    }
    let (dx, dy) = point(&d, id);
    let (sx, sy) = point(&s, is);
    let to = (dx, dy, d.cx, d.cy);
    let from = (sx, sy, s.cx, s.cy);
    if flags & ILCF_SWAP != 0 {
        let hdc = new_dc(cpu);
        let t_image = gdi(
            cpu,
            "CreateBitmap",
            &[s.cx as u32, s.cy as u32, 1, s.bpp, 0],
        );
        let t_mask = mono_bitmap(cpu, s.cx, s.cy);
        select(cpu, hdc, t_image);
        stretch(cpu, hdc, (0, 0, s.cx, s.cy), d.hdc_image, to, SRCCOPY);
        select(cpu, hdc, t_mask);
        stretch(cpu, hdc, (0, 0, s.cx, s.cy), d.hdc_mask, to, SRCCOPY);
        stretch(cpu, d.hdc_image, to, s.hdc_image, from, SRCCOPY);
        stretch(cpu, d.hdc_mask, to, s.hdc_mask, from, SRCCOPY);
        blt(cpu, s.hdc_mask, sx, sy, s.cx, s.cy, hdc, 0, 0, SRCCOPY);
        blt(cpu, s.hdc_image, sx, sy, s.cx, s.cy, hdc, 0, 0, SRCCOPY);
        delete(cpu, t_mask);
        delete(cpu, t_image);
        delete_dc(cpu, hdc);
    } else {
        stretch(cpu, d.hdc_image, to, s.hdc_image, from, SRCCOPY);
        stretch(cpu, d.hdc_mask, to, s.hdc_mask, from, SRCCOPY);
    }
    Some(1)
}

/// ImageList_Duplicate(himl).
pub fn il_duplicate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(s) = get(cpu, args[0]) else {
        return Some(0);
    };
    let dst = create(cpu, s.cx, s.cy, s.flags, s.cur, s.grow);
    let Some(mut d) = get(cpu, dst) else {
        return Some(0);
    };
    let (w, h) = bitmap_dims(&s, s.cur);
    blt(cpu, d.hdc_image, 0, 0, w, h, s.hdc_image, 0, 0, SRCCOPY);
    if d.hbm_mask != 0 {
        blt(cpu, d.hdc_mask, 0, 0, w, h, s.hdc_mask, 0, 0, SRCCOPY);
    }
    d.cur = s.cur;
    put(cpu, dst, d);
    Some(dst)
}

/// ImageList_GetIcon(himl, i, style).
pub fn il_get_icon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (himl, i) = (args[0], args[1] as i32);
    let Some(il) = get(cpu, himl) else {
        return Some(0);
    };
    if i < 0 || i >= il.cur {
        return Some(0);
    }
    let screen = usr(cpu, "GetDC", &[0]);
    let color = gdi(
        cpu,
        "CreateCompatibleBitmap",
        &[screen, il.cx as u32, il.cy as u32],
    );
    usr(cpu, "ReleaseDC", &[0, screen]);
    let hdc = new_dc(cpu);
    let (x, y) = point(&il, i);
    let mask = mono_bitmap(cpu, il.cx, il.cy);
    let old = select(cpu, hdc, mask);
    if il.hbm_mask != 0 {
        blt(cpu, hdc, 0, 0, il.cx, il.cy, il.hdc_mask, x, y, SRCCOPY);
    } else {
        let a = [hdc, 0, 0, il.cx as u32, il.cy as u32, BLACKNESS];
        gdi(cpu, "PatBlt", &a);
    }
    select(cpu, hdc, color);
    blt(cpu, hdc, 0, 0, il.cx, il.cy, il.hdc_image, x, y, SRCCOPY);
    select(cpu, hdc, old);
    let info: Vec<u8> = [1u32, 0, 0, mask, color]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let at = temp(cpu, &info);
    let icon = usr(cpu, "CreateIconIndirect", &[at]);
    free_temp(cpu, at);
    delete(cpu, mask);
    delete(cpu, color);
    delete_dc(cpu, hdc);
    Some(icon)
}

/// ImageList_GetImageInfo(himl, i, info).
pub fn il_get_image_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (himl, i, at) = (args[0], args[1] as i32, args[2]);
    let Some(il) = get(cpu, himl) else {
        return Some(0);
    };
    if at == 0 || i < 0 || i >= il.cur {
        return Some(0);
    }
    let (x, y) = point(&il, i);
    let w = [il.hbm_image, il.hbm_mask, 0, 0, x as u32, y as u32];
    write_words(cpu, at, &w);
    write_words(cpu, at + 24, &[(x + il.cx) as u32, (y + il.cy) as u32]);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(1)
}

/// ImageList_GetImageRect(himl, i, rect).
pub fn il_get_image_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (himl, i, at) = (args[0], args[1] as i32, args[2]);
    let Some(il) = get(cpu, himl) else {
        return Some(0);
    };
    if at == 0 || i < 0 || i >= il.cur {
        return Some(0);
    }
    let (x, y) = point(&il, i);
    let r = [x, y, x + il.cx, y + il.cy].map(|v| v as u32);
    write_words(cpu, at, &r);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(1)
}

/// ImageList_LoadImageA(instance, name, cx, grow, mask, type, flags).
pub fn il_load_image(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (inst, name, cx, grow, mask, kind, flags) = (
        args[0],
        args[1],
        args[2] as i32,
        args[3] as i32,
        args[4],
        args[5],
        args[6],
    );
    let handle = usr(cpu, "LoadImageA", &[inst, name, kind, 0, 0, flags]);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if handle == 0 {
        return Some(0);
    }
    let mut himl = 0;
    if kind == IMAGE_BITMAP {
        let (w, h, _) = bitmap_size(cpu, handle).unwrap_or((0, 0, 0));
        let mut cx = cx;
        if cx == 0 {
            cx = if flags & DI_DEFAULTSIZE != 0 {
                usr(cpu, "GetSystemMetrics", &[SM_CXICON]) as i32
            } else {
                h
            };
        }
        let n = if cx > 0 { w / cx } else { 0 };
        himl = create(cpu, cx, h, ILC_MASK | ILC_COLOR, n, grow);
        if himl != 0 {
            add_masked(cpu, himl, handle, mask);
        }
        delete(cpu, handle);
    } else if kind == IMAGE_ICON || kind == IMAGE_CURSOR {
        if let Some(ic) = user32::resource::icon(cpu, handle) {
            himl = create(cpu, ic.width, ic.height, ILC_MASK | ILC_COLOR, 1, grow);
            if himl != 0 {
                add(cpu, himl, ic.color, ic.mask);
            }
        }
        usr(cpu, "DestroyIcon", &[handle]);
    }
    Some(himl)
}

/// ImageList_LoadImageW: Windows 95 has no Unicode resources' names to
/// give it; it fails.
pub fn il_load_image_w(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// ImageList_Merge(himl1, i1, himl2, i2, dx, dy).
pub fn il_merge(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = |i: usize| args[i] as i32;
    Some(merge(cpu, args[0], a(1), args[2], a(3), a(4), a(5)))
}

/// ImageList_BeginDrag(himl, i, dx, dy).
pub fn il_begin_drag(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (himl, i, dx, dy) = (args[0], args[1] as i32, args[2] as i32, args[3] as i32);
    let Some(t) = get(cpu, himl) else {
        return Some(0);
    };
    if state(cpu).drag.himl != 0 {
        end_drag(cpu);
    }
    let new = create(cpu, t.cx, t.cy, t.flags, 1, 1);
    let Some(mut d) = get(cpu, new) else {
        return Some(0);
    };
    blt(
        cpu,
        d.hdc_image,
        0,
        0,
        t.cx,
        t.cy,
        t.hdc_image,
        i * t.cx,
        0,
        SRCCOPY,
    );
    blt(
        cpu,
        d.hdc_mask,
        0,
        0,
        t.cx,
        t.cy,
        t.hdc_mask,
        i * t.cx,
        0,
        SRCCOPY,
    );
    d.cur = 1;
    put(cpu, new, d);
    set_drag(
        cpu,
        Drag {
            himl: new,
            dx,
            dy,
            ..Drag::default()
        },
    );
    Some(1)
}

/// ImageList_EndDrag().
pub fn il_end_drag(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    end_drag(cpu);
    Some(0)
}

/// ImageList_DragEnter(hwnd, x, y).
pub fn il_drag_enter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut d = drag(cpu);
    if get(cpu, d.himl).is_none() {
        return Some(0);
    }
    d.hwnd = if args[0] != 0 {
        args[0]
    } else {
        usr(cpu, "GetDesktopWindow", &[])
    };
    d.x = args[1] as i32;
    d.y = args[2] as i32;
    set_drag(cpu, d);
    Some(drag_show(cpu, true) as u32)
}

/// ImageList_DragLeave(hwnd).
pub fn il_drag_leave(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    drag_show(cpu, false);
    Some(1)
}

/// ImageList_DragShowNolock(show).
pub fn il_drag_show_nolock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(drag_show(cpu, args[0] != 0) as u32)
}

/// ImageList_DragMove(x, y): the image moves, drawn on what was under
/// it.
pub fn il_drag_move(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (x, y) = (args[0] as i32, args[1] as i32);
    let mut d = drag(cpu);
    let Some(il) = get(cpu, d.himl) else {
        return Some(0);
    };
    if d.show {
        let (nx, ny) = (x - d.dx, y - d.dy);
        let (ox, oy) = (d.x - d.dx, d.y - d.dy);
        let (rx, ry) = (nx.min(ox), ny.min(oy));
        let (rw, rh) = (il.cx + (x - d.x).abs(), il.cy + (y - d.y).abs());
        let flags = DCX_WINDOW | DCX_CACHE | DCX_LOCKWINDOWUPDATE;
        let hdc = usr(cpu, "GetDCEx", &[d.hwnd, 0, flags]);
        let off = gdi(cpu, "CreateCompatibleDC", &[hdc]);
        let bg_dc = gdi(cpu, "CreateCompatibleDC", &[hdc]);
        let off_bm = gdi(cpu, "CreateCompatibleBitmap", &[hdc, rw as u32, rh as u32]);
        select(cpu, off, off_bm);
        select(cpu, bg_dc, d.bg);
        blt(cpu, off, 0, 0, rw, rh, hdc, rx, ry, SRCCOPY);
        blt(
            cpu,
            off,
            ox - rx,
            oy - ry,
            il.cx,
            il.cy,
            bg_dc,
            0,
            0,
            SRCCOPY,
        );
        blt(
            cpu,
            bg_dc,
            0,
            0,
            il.cx,
            il.cy,
            off,
            nx - rx,
            ny - ry,
            SRCCOPY,
        );
        drag_draw(cpu, d.himl, off, nx - rx, ny - ry);
        blt(cpu, hdc, rx, ry, rw, rh, off, 0, 0, SRCCOPY);
        delete_dc(cpu, bg_dc);
        delete_dc(cpu, off);
        delete(cpu, off_bm);
        usr(cpu, "ReleaseDC", &[d.hwnd, hdc]);
    }
    d.x = x;
    d.y = y;
    set_drag(cpu, d);
    Some(1)
}

/// ImageList_SetDragCursorImage(himl, i, dx, dy): the drag image
/// becomes itself with the image `i` over it, at the offset.
pub fn il_set_drag_cursor_image(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (himl, i, dx, dy) = (args[0], args[1] as i32, args[2] as i32, args[3] as i32);
    let mut d = drag(cpu);
    let Some(old) = get(cpu, d.himl) else {
        return Some(0);
    };
    if get(cpu, himl).is_none() {
        return Some(0);
    }
    let visible = d.show;
    let merged = merge(cpu, d.himl, 0, himl, i, dx, dy);
    if visible {
        drag_show(cpu, false);
        d = drag(cpu);
    }
    if let Some(m) = get(cpu, merged)
        && (m.cx != old.cx || m.cy != old.cy)
    {
        delete(cpu, d.bg);
        d.bg = 0;
    }
    destroy(cpu, d.himl);
    d.himl = merged;
    set_drag(cpu, d);
    if visible {
        drag_show(cpu, true);
    }
    Some(1)
}

/// ImageList_GetDragImage(pt, hotspot).
pub fn il_get_drag_image(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let d = drag(cpu);
    if get(cpu, d.himl).is_none() {
        return Some(0);
    }
    if args[0] != 0 {
        write_words(cpu, args[0], &[d.x as u32, d.y as u32]);
    }
    if args[1] != 0 {
        write_words(cpu, args[1], &[d.dx as u32, d.dy as u32]);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(d.himl)
}

/// ImageList_SetFilter and ImageList_SetFlags: nothing to set.
pub fn il_set_filter(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// ImageList_Write(himl, stream): ILHEAD, the image's bitmap and the
/// mask's, each through IStream::Write.
pub fn il_write(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (himl, stream) = (args[0], args[1]);
    let Some(il) = get(cpu, himl) else {
        return Some(0);
    };
    let mut head = Vec::with_capacity(ILHEAD_SIZE as usize);
    for v in [MAGIC, 0x101, il.cur as u16, il.max as u16, il.grow as u16] {
        head.extend_from_slice(&v.to_le_bytes());
    }
    head.extend_from_slice(&(il.cx as u16).to_le_bytes());
    head.extend_from_slice(&(il.cy as u16).to_le_bytes());
    head.extend_from_slice(&il.bk.to_le_bytes());
    head.extend_from_slice(&(il.flags as u16).to_le_bytes());
    for o in &il.overlays[..4] {
        head.extend_from_slice(&(*o as i16).to_le_bytes());
    }
    let Some(image) = bitmap_file(cpu, il.hbm_image) else {
        return Some(0);
    };
    let mut sizes = vec![ILHEAD_SIZE, image.len() as u32];
    let mut all = head;
    all.extend_from_slice(&image);
    if il.flags & ILC_MASK != 0 {
        let Some(mask) = bitmap_file(cpu, il.hbm_mask) else {
            return Some(0);
        };
        sizes.push(mask.len() as u32);
        all.extend_from_slice(&mask);
    }
    let buf = temp(cpu, &all);
    if buf == 0 {
        return Some(0);
    }
    let mut data = vec![stream, buf, 0];
    data.extend(sizes);
    let mut s = Script::new(0);
    s.then(W_NEXT, &data);
    script::run(cpu, IMAGELIST, s)
}

/// ImageList_Read(stream): an image list as ImageList_Write wrote it.
pub fn il_read(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let stream = args[0];
    let mut s = Script::new(0);
    let head = s.alloc(R_SCRATCH);
    let f = method(cpu, stream, IS_READ);
    if cpu.bus.guest_faulted() {
        return None;
    }
    s.call(
        f,
        &[Arg::V(stream), Arg::S(head), Arg::V(ILHEAD_SIZE), Arg::V(0)],
    )
    .then(R_HEAD, &[stream]);
    script::run(cpu, IMAGELIST, s)
}
