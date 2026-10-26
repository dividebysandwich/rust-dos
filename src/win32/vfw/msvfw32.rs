//! MSVFW32.DLL: the DrawDib functions, which draw a DIB to a DC and
//! decompress it first when it is compressed. The behaviour follows
//! Wine's drawdib.c (LGPL 2.1 or later):
//!
//! Copyright 2000 Bradley Baetz
//!
//! A DrawDib keeps the picture it decompressed last in a 24-bit DIB in
//! the process heap, which DrawDibDraw then draws with StretchDIBits;
//! DDF_UPDATE draws it again and DDF_DONTDRAW decompresses without
//! drawing. Indeo Video 3 (IV31, IV32) is the one codec; DrawDibBegin
//! fails for a DIB of another. An uncompressed DIB goes to StretchDIBits
//! as it is. A save state doesn't keep the decoder's picture: the frames
//! after a restore that change it start from black.

use crate::cpu::Cpu;
use crate::win32::gdi32::bitmap;
use crate::win32::stubs::{self, Api, Conv, Dll};
use rust_dos_indeo3::{Decoder, PixelFormat};

pub static DLL: Dll = Dll {
    name: "MSVFW32.DLL",
    base: 0xBF74_0000,
    apis: APIS,
    vars: &[],
    init: None,
    vtables: &[],
};

const fn stdcall(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// The functions. Append only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("DrawDibOpen", 1, 0, draw_dib_open),
    stdcall("DrawDibClose", 2, 1, draw_dib_close),
    stdcall("DrawDibBegin", 3, 8, draw_dib_begin),
    stdcall("DrawDibEnd", 4, 1, draw_dib_end),
    stdcall("DrawDibDraw", 5, 13, draw_dib_draw),
    stdcall("DrawDibStart", 6, 2, known),
    stdcall("DrawDibStop", 7, 1, known),
    stdcall("DrawDibRealize", 8, 3, zero),
    stdcall("DrawDibSetPalette", 9, 2, known),
    stdcall("DrawDibGetPalette", 10, 1, zero),
    stdcall("DrawDibChangePalette", 11, 4, known),
    stdcall("DrawDibGetBuffer", 12, 4, zero),
    stdcall("DrawDibTime", 13, 2, zero),
    stdcall("DrawDibProfileDisplay", 14, 1, profile_display),
];

/// DrawDibDraw's flags.
const DDF_UPDATE: u32 = 0x0002;
const DDF_SAME_HDC: u32 = 0x0004;
const DDF_SAME_DRAW: u32 = 0x0008;
const DDF_DONTDRAW: u32 = 0x0010;

/// BI_RGB, and the codecs' FourCCs.
const BI_RGB: u32 = 0;
const IV31: u32 = u32::from_le_bytes(*b"IV31");
const IV32: u32 = u32::from_le_bytes(*b"IV32");

/// BITMAPINFOHEADER's size.
const HEADER: u32 = 40;
const SRCCOPY: u32 = 0x00CC_0020;
const DIB_RGB_COLORS: u32 = 0;

/// DrawDibProfileDisplay's PD_CAN_DRAW_DIB and PD_CAN_STRETCHDIB.
const PD_CAN_DRAW_DIB: u32 = 0x0001;
const PD_CAN_STRETCHDIB: u32 = 0x0002;

/// The decompressor of a DrawDib, which a save state doesn't keep.
#[derive(Clone, Default)]
pub struct Codec(Option<Decoder>);

impl std::fmt::Debug for Codec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "Codec(Indeo 3)"
        } else {
            "Codec(None)"
        })
    }
}

/// A DrawDib handle's state.
#[derive(Clone, Debug, Default)]
pub struct DrawDib {
    /// What DrawDibBegin set up for: the DC, the size drawn to, and the
    /// source's compression and size.
    hdc: u32,
    dst: (u32, u32),
    compression: u32,
    width: u32,
    height: u32,
    /// The 24-bit DIB the picture is decompressed to: its header, then
    /// its rows. 0 for an uncompressed source.
    buffer: u32,
    /// Whether the decoder has a picture `buffer` doesn't have yet.
    dirty: bool,
    /// The last DrawDibDraw's source: its header and bits, which
    /// DDF_UPDATE of an uncompressed DIB draws again.
    last: (u32, u32),
    codec: Codec,
}

crate::state_fields!(DrawDib {
    hdc,
    dst,
    compression,
    width,
    height,
    buffer,
    dirty,
    last
} skip {
    codec
});

/// A BITMAPINFOHEADER's fields this needs.
struct Info {
    width: i32,
    height: i32,
    compression: u32,
    size_image: u32,
}

fn read_info(cpu: &mut Cpu, at: u32) -> Option<Info> {
    if at == 0 || !cpu.bus.guest_probe(at, HEADER as usize, false) {
        return None;
    }
    Some(Info {
        width: cpu.bus.guest_read_32(at + 4) as i32,
        height: cpu.bus.guest_read_32(at + 8) as i32,
        compression: cpu.bus.guest_read_32(at + 16),
        size_image: cpu.bus.guest_read_32(at + 20),
    })
}

fn drawdibs(cpu: &mut Cpu) -> &mut Vec<Option<DrawDib>> {
    &mut crate::win32::process(cpu).vfw.drawdibs
}

/// The slot of the handle `hdd`.
fn slot(cpu: &mut Cpu, hdd: u32) -> Option<usize> {
    let slot = (hdd as usize).checked_sub(1)?;
    drawdibs(cpu).get(slot)?.as_ref()?;
    Some(slot)
}

fn dd(cpu: &mut Cpu, slot: usize) -> &mut DrawDib {
    drawdibs(cpu)[slot].as_mut().expect("an open DrawDib")
}

fn zero(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// A function that does nothing here, for a handle that is open.
fn known(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(slot(cpu, args[0]).is_some() as u32)
}

/// DrawDibProfileDisplay(info): every DIB can be drawn and stretched.
fn profile_display(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(PD_CAN_DRAW_DIB | PD_CAN_STRETCHDIB)
}

/// DrawDibOpen(): a new handle.
fn draw_dib_open(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let list = drawdibs(cpu);
    let slot = list.iter().position(Option::is_none).unwrap_or(list.len());
    if slot == list.len() {
        list.push(None);
    }
    list[slot] = Some(DrawDib::default());
    Some(slot as u32 + 1)
}

/// DrawDibClose(hdd).
fn draw_dib_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(slot) = slot(cpu, args[0]) else {
        return Some(0);
    };
    end(cpu, slot);
    drawdibs(cpu)[slot] = None;
    Some(1)
}

/// DrawDibEnd(hdd): the decompressor and its picture go.
fn draw_dib_end(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(slot) = slot(cpu, args[0]) else {
        return Some(0);
    };
    end(cpu, slot);
    Some(1)
}

fn end(cpu: &mut Cpu, slot: usize) {
    let d = dd(cpu, slot);
    let buffer = std::mem::take(&mut d.buffer);
    *d = DrawDib::default();
    if buffer != 0 {
        crate::win32::with_process(cpu, |p, _| p.heap.free(buffer));
    }
}

/// DrawDibBegin(hdd, hdc, dxdst, dydst, info, dxsrc, dysrc, flags):
/// sets up for drawing DIBs like `info`.
fn draw_dib_begin(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hdd, hdc, dxd, dyd, info, ..] = args else {
        unreachable!()
    };
    let Some(slot) = slot(cpu, hdd) else {
        return Some(0);
    };
    Some(begin(cpu, slot, hdc, (dxd, dyd), info) as u32)
}

fn begin(cpu: &mut Cpu, slot: usize, hdc: u32, dst: (u32, u32), at: u32) -> bool {
    let Some(info) = read_info(cpu, at) else {
        return false;
    };
    let (width, height) = (info.width.unsigned_abs(), info.height.unsigned_abs());
    let d = dd(cpu, slot);
    let same = d.compression == info.compression && d.width == width && d.height == height;
    d.hdc = hdc;
    d.dst = dst;
    if same && (d.buffer != 0 || info.compression == BI_RGB) {
        return true;
    }
    end(cpu, slot);
    let d = dd(cpu, slot);
    d.hdc = hdc;
    d.dst = dst;
    d.compression = info.compression;
    d.width = width;
    d.height = height;
    if info.compression == BI_RGB {
        return true;
    }
    if !is_indeo3(info.compression) {
        let fourcc = String::from_utf8_lossy(&info.compression.to_le_bytes()).into_owned();
        cpu.bus
            .log_string(&format!("[W32] DrawDibBegin: no codec for {fourcc:?}"));
        return false;
    }
    let Ok(decoder) = Decoder::new(width, height) else {
        return false;
    };
    let stride = PixelFormat::Bgr24.dib_stride(width) as u32;
    let size = HEADER + stride * height;
    let Some(buffer) =
        crate::win32::with_process(cpu, |p, bus| p.heap.alloc(&mut p.memory, bus, size))
    else {
        return false;
    };
    // A BITMAPINFOHEADER of a bottom-up 24-bit DIB.
    let mut header = [0u8; HEADER as usize];
    header[0..4].copy_from_slice(&HEADER.to_le_bytes());
    header[4..8].copy_from_slice(&width.to_le_bytes());
    header[8..12].copy_from_slice(&height.to_le_bytes());
    header[12..14].copy_from_slice(&1u16.to_le_bytes());
    header[14..16].copy_from_slice(&24u16.to_le_bytes());
    header[20..24].copy_from_slice(&(stride * height).to_le_bytes());
    cpu.bus.guest_write_bytes(buffer, &header);
    cpu.bus
        .guest_fill(buffer + HEADER, (stride * height) as usize, 0);
    let d = dd(cpu, slot);
    d.buffer = buffer;
    d.codec = Codec(Some(decoder));
    true
}

/// Whether the FourCC `compression` is Indeo Video 3's, in either case.
fn is_indeo3(compression: u32) -> bool {
    let upper = u32::from_le_bytes(compression.to_le_bytes().map(|b| b.to_ascii_uppercase()));
    upper == IV31 || upper == IV32
}

/// DrawDibDraw(hdd, hdc, xdst, ydst, dxdst, dydst, info, bits, xsrc,
/// ysrc, dxsrc, dysrc, flags).
fn draw_dib_draw(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hdd, hdc, xd, yd, dxd, dyd, info, bits, xs, ys, dxs, dys, flags] =
        args.try_into().unwrap();
    let Some(slot) = slot(cpu, hdd) else {
        return Some(0);
    };
    let d = dd(cpu, slot);
    let same = flags & (DDF_SAME_HDC | DDF_SAME_DRAW) != 0 && d.hdc == hdc;
    let set_up = d.buffer != 0 || (d.compression == BI_RGB && d.width != 0);
    if (!same || !set_up) && flags & DDF_UPDATE == 0 && !begin(cpu, slot, hdc, (dxd, dyd), info)
    {
        return Some(0);
    }
    let d = dd(cpu, slot);
    let (width, height) = (d.width, d.height);
    // -1 for a size is the source's own.
    let (dxs, dys) = if dxs == u32::MAX {
        (width, height)
    } else {
        (dxs, dys)
    };
    let (dxd, dyd) = if dxd == u32::MAX { (dxs, dys) } else { (dxd, dyd) };
    let (src_info, src_bits) = if d.buffer != 0 {
        if flags & DDF_UPDATE == 0 && bits != 0 {
            decode(cpu, slot, info, bits);
        }
        let d = dd(cpu, slot);
        if d.dirty && flags & DDF_DONTDRAW == 0 {
            write_picture(cpu, slot);
        }
        let buffer = dd(cpu, slot).buffer;
        (buffer, buffer + HEADER)
    } else {
        if flags & DDF_UPDATE == 0 {
            d.last = (info, bits);
        }
        d.last
    };
    if flags & DDF_DONTDRAW != 0 || src_bits == 0 {
        return Some(1);
    }
    let drawn = bitmap::stretch_di_bits(
        cpu,
        &[
            hdc,
            xd,
            yd,
            dxd,
            dyd,
            xs,
            ys,
            dxs,
            dys,
            src_bits,
            src_info,
            DIB_RGB_COLORS,
            SRCCOPY,
        ],
    )?;
    Some((drawn != 0) as u32)
}

/// Decompresses the frame `bits` that `info` describes.
fn decode(cpu: &mut Cpu, slot: usize, info: u32, bits: u32) {
    let Some(info) = read_info(cpu, info) else {
        return;
    };
    let d = dd(cpu, slot);
    // A frame's own header has its size: the most it can be is what
    // biSizeImage says, or an uncompressed picture's when that is 0.
    let size = if info.size_image != 0 {
        info.size_image
    } else {
        d.width * d.height * 3
    };
    let mut frame = vec![0; size as usize];
    cpu.bus.guest_read_bytes(bits, &mut frame);
    let d = dd(cpu, slot);
    if d.codec.0.is_none() {
        // After a restore.
        d.codec = Codec(Decoder::new(d.width, d.height).ok());
    }
    let Some(decoder) = d.codec.0.as_mut() else {
        return;
    };
    match decoder.decode(&frame) {
        Ok(()) => d.dirty = true,
        Err(e) => cpu.bus.log_string(&format!("[W32] DrawDibDraw: {e}")),
    }
}

/// Writes the decoder's picture to the DrawDib's DIB.
fn write_picture(cpu: &mut Cpu, slot: usize) {
    let d = dd(cpu, slot);
    d.dirty = false;
    let (buffer, width, height) = (d.buffer, d.width, d.height);
    let Some(decoder) = d.codec.0.as_ref() else {
        return;
    };
    let stride = PixelFormat::Bgr24.dib_stride(width);
    let mut out = vec![0; stride * height as usize];
    if decoder
        .write_picture(PixelFormat::Bgr24, true, stride, &mut out)
        .is_ok()
    {
        cpu.bus.guest_write_bytes(buffer + HEADER, &out);
    }
}
