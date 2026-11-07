//! MSVFW32.DLL: the DrawDib functions, which draw a DIB to a DC and
//! decompress it first when it is compressed, and ICInfo, ICOpen and
//! ICClose, which find and open the decompressors codec.rs has. The
//! DrawDib functions' behaviour follows Wine's drawdib.c (LGPL 2.1 or
//! later):
//!
//! Copyright 2000 Bradley Baetz
//!
//! A DrawDib keeps the picture it decompressed last in a 24-bit DIB in
//! the process heap, which DrawDibDraw then draws with StretchDIBits;
//! DDF_UPDATE draws it again and DDF_DONTDRAW decompresses without
//! drawing. DrawDibBegin fails for a DIB no codec of codec.rs takes. An
//! uncompressed DIB goes to StretchDIBits as it is. A save state keeps the
//! DIB but not the decoder: after a restore the decoder refuses the
//! frames up to the next key frame, and the picture stays until then.

use super::codec::{self, Codec};
use crate::cpu::Cpu;
use crate::win32::gdi32::bitmap;
use crate::win32::kernel32::virtual_memory::allows;
use crate::win32::stubs::{self, Api, Conv, Dll};

pub static DLL: Dll = Dll {
    name: "MSVFW32.DLL",
    base: 0xBF6C_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
};

const fn stdcall(name: &'static str, ordinal: u16, args: u8, handler: stubs::Handler) -> Api {
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
    stdcall("DrawDibOpen", 10, 0, draw_dib_open),
    stdcall("DrawDibClose", 5, 1, draw_dib_close),
    stdcall("DrawDibBegin", 3, 8, draw_dib_begin),
    stdcall("DrawDibEnd", 7, 1, draw_dib_end),
    stdcall("DrawDibDraw", 6, 13, draw_dib_draw),
    stdcall("DrawDibStart", 14, 2, known),
    stdcall("DrawDibStop", 15, 1, known),
    stdcall("DrawDibRealize", 12, 3, zero),
    stdcall("DrawDibSetPalette", 13, 2, known),
    stdcall("DrawDibGetPalette", 9, 1, zero),
    stdcall("DrawDibChangePalette", 4, 4, known),
    stdcall("DrawDibGetBuffer", 8, 4, zero),
    stdcall("DrawDibTime", 16, 2, zero),
    stdcall("DrawDibProfileDisplay", 11, 1, profile_display),
    stdcall("ICInfo", 33, 3, ic_info),
    stdcall("ICOpen", 37, 3, ic_open),
    stdcall("ICClose", 22, 1, ic_close),
];

/// ICINFO's size: six DWORDs, then szName[16], szDescription[128] and
/// szDriver[128] in UTF-16.
const ICINFO: u32 = 24 + 2 * (16 + 128 + 128);
const ICTYPE_VIDEO: u32 = u32::from_le_bytes(*b"vidc");
/// ICINFO's dwVersionICM: the ICM version, 1.04.
const ICVERSION: u32 = 0x0104;

/// ICInfo(fccType, fccHandler, lpicinfo): whether a codec of the type
/// (`vidc`, or any with 0) is installed. A handler under 256 is an index
/// into the type's codecs, which a program counts up from 0 until the
/// call fails; any other is the codec's FourCC, compared without regard
/// to case. The ICINFO filled is as Wine's ICInfo fills it: the type and
/// handler, dwVersionICM and the driver's file, with no name or
/// description (ICGetInfo's). Age of Empires asks for `iv41`, Indeo 4,
/// before it plays its films.
fn ic_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[kind, handler, info] = args else {
        unreachable!()
    };
    if info == 0 || !allows(cpu, info, ICINFO, true) {
        return Some(0);
    }
    if kind != 0 && codec::upper(kind) != codec::upper(ICTYPE_VIDEO) {
        return Some(0);
    }
    let found = if handler < 256 {
        codec::INSTALLED.get(handler as usize)
    } else {
        codec::INSTALLED
            .iter()
            .find(|(fourcc, _)| codec::upper(u32::from_le_bytes(**fourcc)) == codec::upper(handler))
    };
    let Some(&(fourcc, driver)) = found else {
        return Some(0);
    };
    let mut bytes = Vec::with_capacity(ICINFO as usize);
    for value in [ICINFO, ICTYPE_VIDEO, u32::from_le_bytes(*fourcc), 0, 0, ICVERSION] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.resize(24 + 2 * (16 + 128), 0);
    for unit in driver.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes.resize(ICINFO as usize, 0);
    cpu.bus.guest_write_bytes(info, &bytes);
    Some(1)
}

/// The installed codec of the type and FourCC `handler`, compared
/// without regard to case: the FourCC in capitals.
fn installed(kind: u32, handler: u32) -> Option<u32> {
    if codec::upper(kind) != codec::upper(ICTYPE_VIDEO) {
        return None;
    }
    let handler = codec::upper(handler);
    codec::INSTALLED
        .iter()
        .map(|(fourcc, _)| codec::upper(u32::from_le_bytes(**fourcc)))
        .find(|&fourcc| fourcc == handler)
}

/// ICOpen's modes a decompressor takes: ICMODE_DECOMPRESS,
/// ICMODE_FASTDECOMPRESS and ICMODE_QUERY.
const ICMODE_DECOMPRESS: u32 = 2;
const ICMODE_FASTDECOMPRESS: u32 = 3;
const ICMODE_QUERY: u32 = 4;
/// ICClose's ICERR_OK and ICERR_BADHANDLE.
const ICERR_OK: u32 = 0;
const ICERR_BADHANDLE: u32 = -8i32 as u32;

/// ICOpen(fccType, fccHandler, wMode): a handle to the installed codec,
/// or 0 when there is none. The codecs here only decompress, so a mode
/// that compresses or draws (ICMODE_COMPRESS, ICMODE_FASTCOMPRESS,
/// ICMODE_DRAW) finds none, as a decompressor's driver refuses to open
/// for one. Rent-A-Hero opens `vidc`/`IV50` to see that Indeo 5 is
/// there before it starts, and stops when it isn't.
fn ic_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[kind, handler, mode] = args else {
        unreachable!()
    };
    if !matches!(
        mode,
        ICMODE_DECOMPRESS | ICMODE_FASTDECOMPRESS | ICMODE_QUERY
    ) {
        return Some(0);
    }
    let Some(fourcc) = installed(kind, handler) else {
        return Some(0);
    };
    let list = &mut crate::win32::process(cpu).vfw.codecs;
    let slot = list.iter().position(Option::is_none).unwrap_or(list.len());
    if slot == list.len() {
        list.push(None);
    }
    list[slot] = Some(fourcc);
    Some(slot as u32 + 1)
}

/// ICClose(hic): ICERR_OK, or ICERR_BADHANDLE for a handle that isn't
/// open.
fn ic_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let list = &mut crate::win32::process(cpu).vfw.codecs;
    let open = (args[0] as usize)
        .checked_sub(1)
        .and_then(|slot| list.get_mut(slot))
        .and_then(Option::take);
    Some(if open.is_some() {
        ICERR_OK
    } else {
        ICERR_BADHANDLE
    })
}

/// DrawDibDraw's flags.
const DDF_UPDATE: u32 = 0x0002;
const DDF_SAME_HDC: u32 = 0x0004;
const DDF_SAME_DRAW: u32 = 0x0008;
const DDF_DONTDRAW: u32 = 0x0010;

const BI_RGB: u32 = 0;

/// BITMAPINFOHEADER's size.
const HEADER: u32 = 40;
const PAGE: u32 = 0x1000;
const SRCCOPY: u32 = 0x00CC_0020;
const DIB_RGB_COLORS: u32 = 0;

/// DrawDibProfileDisplay's PD_CAN_DRAW_DIB and PD_CAN_STRETCHDIB.
const PD_CAN_DRAW_DIB: u32 = 0x0001;
const PD_CAN_STRETCHDIB: u32 = 0x0002;

/// A DrawDib handle's state.
#[derive(Clone, Debug, Default)]
pub struct DrawDib {
    /// What DrawDibBegin set up for: the DC, and the source's compression
    /// and size.
    hdc: u32,
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
    size: u32,
    width: i32,
    height: i32,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    clr_used: u32,
}

fn read_info(cpu: &mut Cpu, at: u32) -> Option<Info> {
    if at == 0 || !cpu.bus.guest_probe(at, HEADER as usize, false) {
        return None;
    }
    Some(Info {
        size: cpu.bus.guest_read_32(at),
        width: cpu.bus.guest_read_32(at + 4) as i32,
        height: cpu.bus.guest_read_32(at + 8) as i32,
        bit_count: cpu.bus.guest_read_16(at + 14),
        compression: cpu.bus.guest_read_32(at + 16),
        size_image: cpu.bus.guest_read_32(at + 20),
        clr_used: cpu.bus.guest_read_32(at + 32),
    })
}

/// Where a packed DIB's bits are, after its header and colours.
fn packed_bits(cpu: &mut Cpu, at: u32) -> Option<u32> {
    let info = read_info(cpu, at)?;
    let colours: u32 = match (info.clr_used, info.bit_count) {
        (0, bits @ 0..=8) => 1 << bits,
        (used, _) => used,
    };
    Some(
        at.wrapping_add(info.size)
            .wrapping_add(colours.wrapping_mul(4)),
    )
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
    let &[hdd, hdc, _, _, info, ..] = args else {
        unreachable!()
    };
    let Some(slot) = slot(cpu, hdd) else {
        return Some(0);
    };
    Some(begin(cpu, slot, hdc, info) as u32)
}

fn begin(cpu: &mut Cpu, slot: usize, hdc: u32, at: u32) -> bool {
    let Some(info) = read_info(cpu, at) else {
        return false;
    };
    let (width, height) = (info.width.unsigned_abs(), info.height.unsigned_abs());
    let d = dd(cpu, slot);
    let same = d.compression == info.compression && d.width == width && d.height == height;
    d.hdc = hdc;
    if same && (d.buffer != 0 || info.compression == BI_RGB) {
        return true;
    }
    end(cpu, slot);
    let d = dd(cpu, slot);
    d.hdc = hdc;
    d.compression = info.compression;
    d.width = width;
    d.height = height;
    if info.compression == BI_RGB {
        return true;
    }
    if !codec::supported(info.compression, width, height) {
        let fourcc = String::from_utf8_lossy(&info.compression.to_le_bytes()).into_owned();
        cpu.bus
            .log_string(&format!("[W32] DrawDibBegin: no codec for {fourcc:?}"));
        return false;
    }
    let Some(image) = codec::dib_size(24, width, height) else {
        return false;
    };
    let Some(size) = HEADER.checked_add(image) else {
        return false;
    };
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
    header[20..24].copy_from_slice(&image.to_le_bytes());
    cpu.bus.guest_write_bytes(buffer, &header);
    cpu.bus.guest_fill(buffer + HEADER, image as usize, 0);
    let d = dd(cpu, slot);
    d.buffer = buffer;
    d.codec = Codec::new(info.compression);
    true
}

/// DrawDibDraw(hdd, hdc, xdst, ydst, dxdst, dydst, info, bits, xsrc,
/// ysrc, dxsrc, dysrc, flags). With no `bits` the DIB is a packed one, as
/// in Wine: what AVIStreamGetFrame gives can be drawn so.
fn draw_dib_draw(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [
        hdd,
        hdc,
        xd,
        yd,
        dxd,
        dyd,
        info,
        bits,
        xs,
        ys,
        dxs,
        dys,
        flags,
    ] = args.try_into().unwrap();
    let Some(slot) = slot(cpu, hdd) else {
        return Some(0);
    };
    let d = dd(cpu, slot);
    let same = flags & (DDF_SAME_HDC | DDF_SAME_DRAW) != 0 && d.hdc == hdc;
    let set_up = d.buffer != 0 || (d.compression == BI_RGB && d.width != 0);
    if (!same || !set_up) && flags & DDF_UPDATE == 0 && !begin(cpu, slot, hdc, info) {
        return Some(0);
    }
    let bits = match bits {
        0 if flags & DDF_UPDATE == 0 => packed_bits(cpu, info).unwrap_or(0),
        bits => bits,
    };
    let d = dd(cpu, slot);
    let (width, height) = (d.width, d.height);
    // -1 for a size is the source's own.
    let (dxs, dys) = if dxs == u32::MAX {
        (width, height)
    } else {
        (dxs, dys)
    };
    let (dxd, dyd) = if dxd == u32::MAX {
        (dxs, dys)
    } else {
        (dxd, dyd)
    };
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

/// How many of the `len` bytes at `at` the program can read, up to the
/// first page it can't.
fn readable(cpu: &Cpu, at: u32, len: u32) -> u32 {
    let mut n = 0;
    while n < len {
        let here = at.wrapping_add(n);
        let run = (PAGE - (here & (PAGE - 1))).min(len - n);
        if !allows(cpu, here, run, false) {
            break;
        }
        n += run;
    }
    n
}

/// Decompresses the frame `bits` that `info` describes.
fn decode(cpu: &mut Cpu, slot: usize, info: u32, bits: u32) {
    let Some(info) = read_info(cpu, info) else {
        return;
    };
    let d = dd(cpu, slot);
    // A frame's own header has its size, and a codec reads no further.
    // biSizeImage (or an uncompressed picture's size when that is 0) is
    // only the most it can be: a format's biSizeImage may say more than
    // the buffer of a frame holds, so what follows the buffer in memory
    // may not be there.
    let most = if info.size_image != 0 {
        info.size_image
    } else {
        d.width * d.height * 3
    };
    let mut frame = vec![0; readable(cpu, bits, most) as usize];
    cpu.bus.guest_read_bytes(bits, &mut frame);
    if cpu.bus.guest_faulted() {
        return;
    }
    let d = dd(cpu, slot);
    // A copy's (after a restore) starts again.
    if d.codec.is_empty() {
        d.codec = Codec::new(d.compression);
    }
    if frame.is_empty() {
        return;
    }
    match d.codec.decode(&frame) {
        Ok(()) => d.dirty = true,
        Err(e) => cpu.bus.log_string(&format!("[W32] DrawDibDraw: {e}")),
    }
}

/// Writes the decoder's picture to the DrawDib's DIB.
fn write_picture(cpu: &mut Cpu, slot: usize) {
    let d = dd(cpu, slot);
    d.dirty = false;
    let (buffer, width, height) = (d.buffer, d.width, d.height);
    let Some(out) = d.codec.picture(width, height) else {
        return;
    };
    cpu.bus.guest_write_bytes(buffer + HEADER, &out);
}
