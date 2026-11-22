//! MSVFW32.DLL: the DrawDib functions, which draw a DIB to a DC and
//! decompress it first when it is compressed; ICInfo, ICOpen, ICLocate
//! and ICClose, which find and open the decompressors codec.rs has; and
//! ICSendMessage and ICDecompress, through which a program decompresses
//! Microsoft Video 1 frames itself. The DrawDib functions' behaviour
//! follows Wine's drawdib.c (LGPL 2.1 or later):
//!
//! Copyright 2000 Bradley Baetz
//!
//! A DrawDib keeps the picture it decompressed last in a 24-bit DIB in
//! the process heap, which DrawDibDraw then draws with StretchDIBits;
//! DDF_UPDATE draws it again and DDF_DONTDRAW decompresses without
//! drawing. DrawDibBegin fails for a DIB no codec of codec.rs takes. An
//! uncompressed DIB goes to StretchDIBits as it is. On a display of 256
//! colours a picture of more than 8 bits a pixel is dithered to the
//! halftone palette first (`gdi32::dither`). A save state keeps the
//! DIB but not the decoder: after a restore the decoder refuses the
//! frames up to the next key frame, and the picture stays until then.

use super::codec::{self, Codec};
use super::msvideo1;
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
    resources: None,
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
    stdcall("ICLocate", 35, 5, ic_locate),
    stdcall("ICSendMessage", 40, 4, ic_send_message),
    Api {
        name: "ICDecompress",
        ordinal: 26,
        args: 6,
        conv: Conv::Cdecl,
        handler: ic_decompress,
    },
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
    if kind != 0 && !video(kind) {
        return Some(0);
    }
    let found = if handler < 256 {
        codec::INSTALLED.get(handler as usize)
    } else {
        installed(handler)
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

/// Whether `kind` is `vidc`, the type of the codecs here, in either case.
fn video(kind: u32) -> bool {
    codec::upper(kind) == codec::upper(ICTYPE_VIDEO)
}

/// The installed codec of FourCC `handler`, compared without regard to
/// case, as Windows finds `vidc.<FourCC>` among SYSTEM.INI's [drivers32]
/// keys.
fn installed(handler: u32) -> Option<&'static (&'static [u8; 4], &'static str)> {
    let handler = codec::upper(handler);
    codec::INSTALLED
        .iter()
        .find(|(fourcc, _)| codec::upper(u32::from_le_bytes(**fourcc)) == handler)
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
/// or 0 when there is none. A mode that compresses or draws
/// (ICMODE_COMPRESS, ICMODE_FASTCOMPRESS, ICMODE_DRAW) gets 0 as well.
/// That is this module's choice: Windows 98 SE's Cinepak driver
/// compresses, and Wine's decoders open whatever the mode. But the
/// codecs here only decompress and there is no ICCompress or ICDraw, so
/// a handle would stop the program at its next call, where 0 tells it
/// that no codec does the job. Rent-A-Hero opens `vidc`/`IV50` to see
/// that Indeo 5 is there before it starts, and stops when it isn't.
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
    let Some((fourcc, _)) = installed(handler).filter(|_| video(kind)) else {
        return Some(0);
    };
    let list = &mut crate::win32::process(cpu).vfw.codecs;
    let slot = super::free_slot(list);
    list[slot] = Some(codec::upper(u32::from_le_bytes(**fourcc)));
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

/// The BITMAPINFO at `at`, its header and colours: None if the header
/// can't be read, and the colours as far as they can.
fn read_format(cpu: &mut Cpu, at: u32) -> Option<codec::Format> {
    if at == 0 || !allows(cpu, at, HEADER, false) {
        return None;
    }
    let mut header = [0; codec::HEADER];
    cpu.bus.guest_read_bytes(at, &mut header);
    let len = readable(cpu, at, codec::Format::size(&header) as u32);
    let mut info = vec![0; len as usize];
    cpu.bus.guest_read_bytes(at, &mut info);
    codec::Format::parse(&info)
}

/// The installed codec open as `hic`: its FourCC in capitals.
fn open_codec(cpu: &mut Cpu, hic: u32) -> Option<u32> {
    let slot = (hic as usize).checked_sub(1)?;
    *crate::win32::process(cpu).vfw.codecs.get(slot)?
}

/// Microsoft Video 1's FourCC as ICOpen keeps it, and its frames'.
const MSVC: u32 = u32::from_le_bytes(*b"MSVC");
const CRAM: u32 = u32::from_le_bytes(*b"CRAM");

/// The decompression messages: ICM_DECOMPRESS_GET_FORMAT, _QUERY,
/// _BEGIN, ICM_DECOMPRESS itself, _END and _SET_PALETTE.
const ICM_DECOMPRESS_GET_FORMAT: u32 = 0x400A;
const ICM_DECOMPRESS_QUERY: u32 = 0x400B;
const ICM_DECOMPRESS_BEGIN: u32 = 0x400C;
const ICM_DECOMPRESS: u32 = 0x400D;
const ICM_DECOMPRESS_END: u32 = 0x400E;
const ICM_DECOMPRESS_SET_PALETTE: u32 = 0x401D;
/// ICERR_UNSUPPORTED, ICERR_BADFORMAT and ICERR_BADPARAM.
const ICERR_UNSUPPORTED: u32 = -1i32 as u32;
const ICERR_BADFORMAT: u32 = -2i32 as u32;
const ICERR_BADPARAM: u32 = -6i32 as u32;
/// BI_BITFIELDS: 16 bits a pixel, with the colours' masks after the
/// header.
const BI_BITFIELDS: u32 = 3;

/// How a decompressed picture is laid out: the pixels, and whether its
/// rows run from the top.
type Layout = (msvideo1::Output, bool);

/// The formats of a Microsoft Video 1 decompression, from the
/// BITMAPINFOs at `input` and `output`: the stream's, and the layout of
/// the output (none for no `output`). An error for a pair it doesn't
/// decompress. The stream is CRAM (or MSVC) of 8 or 16 bits, and the
/// output an uncompressed DIB of its size: 8 bits for an 8-bit stream,
/// 16 (RGB555, or RGB565 by its BI_BITFIELDS masks), 24 or 32, from the
/// bottom or the top.
fn ms_video_1_formats(
    cpu: &mut Cpu,
    input: u32,
    output: u32,
) -> Result<(codec::Format, Option<Layout>), u32> {
    let source = read_format(cpu, input).ok_or(ICERR_BADPARAM)?;
    if !matches!(codec::upper(source.compression), MSVC | CRAM) || !codec::supported(&source) {
        return Err(ICERR_BADFORMAT);
    }
    let depth = msvideo1::Depth::of(source.bits).ok_or(ICERR_BADFORMAT)?;
    if output == 0 {
        return Ok((source, None));
    }
    let out = read_info(cpu, output).ok_or(ICERR_BADPARAM)?;
    // BI_BITFIELDS's masks follow a BITMAPINFOHEADER, and are the same
    // place in a BITMAPV4HEADER or V5 one: at 40 either way.
    let masks = output.wrapping_add(HEADER);
    let masks = allows(cpu, masks, 12, false).then(|| {
        let mut mask = |n: u32| cpu.bus.guest_read_32(masks + 4 * n);
        [mask(0), mask(1), mask(2)]
    });
    use msvideo1::Output;
    let layout = match (out.compression, out.bit_count) {
        (BI_RGB, 8) => Some(Output::Indexes),
        (BI_RGB, 16) => Some(Output::Rgb16 { green6: false }),
        (BI_RGB, 24) => Some(Output::Rgb24),
        (BI_RGB, 32) => Some(Output::Rgb32),
        (BI_BITFIELDS, 16) => match masks {
            Some([0xF800, 0x07E0, 0x001F]) => Some(Output::Rgb16 { green6: true }),
            Some([0x7C00, 0x03E0, 0x001F]) => Some(Output::Rgb16 { green6: false }),
            _ => None,
        },
        _ => None,
    };
    let size = (out.width.unsigned_abs(), out.height.unsigned_abs());
    match layout {
        Some(layout) if size == (source.width, source.height) && layout.takes(depth) => {
            Ok((source, Some((layout, out.height < 0))))
        }
        _ => Err(ICERR_BADFORMAT),
    }
}

/// ICM_DECOMPRESS_QUERY of the open codec `fourcc`: whether it
/// decompresses the format at `input` to the one at `output`, or to one
/// of its choice for 0. Only Microsoft Video 1 answers here: the other
/// codecs decode for DrawDib, AVIStreamGetFrame and MCI, and have no
/// ICDecompress yet.
fn decompress_query(cpu: &mut Cpu, fourcc: u32, input: u32, output: u32) -> u32 {
    if fourcc != MSVC {
        return ICERR_UNSUPPORTED;
    }
    match ms_video_1_formats(cpu, input, output) {
        Ok(_) => ICERR_OK,
        Err(e) => e,
    }
}

/// ICLocate(fccType, fccHandler, lpbiIn, lpbiOut, wFlags): a handle to
/// a codec that decompresses `lpbiIn` to `lpbiOut` (to a format of its
/// choice, for no `lpbiOut`), or 0. As in Windows, it asks the codec
/// `fccHandler` first, then the one of `lpbiIn`'s biCompression, then
/// every installed codec in turn, each with ICM_DECOMPRESS_QUERY. The
/// handle is one ICOpen would give, which ICClose closes. Only
/// ICMODE_DECOMPRESS and ICMODE_FASTDECOMPRESS find one, since no codec
/// here compresses or draws. Biosys finds the decoder of its Microsoft
/// Video 1 films so, and stops when there is none.
fn ic_locate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[kind, handler, input, output, mode] = args else {
        unreachable!()
    };
    if !video(kind) || !matches!(mode, ICMODE_DECOMPRESS | ICMODE_FASTDECOMPRESS) {
        return Some(0);
    }
    let compression = read_info(cpu, input).map_or(0, |i| i.compression);
    let listed = codec::INSTALLED
        .iter()
        .map(|(f, _)| u32::from_le_bytes(**f));
    for fourcc in [handler, compression].into_iter().chain(listed) {
        let Some((name, _)) = installed(fourcc) else {
            continue;
        };
        let fourcc = codec::upper(u32::from_le_bytes(**name));
        if decompress_query(cpu, fourcc, input, output) == ICERR_OK {
            let list = &mut crate::win32::process(cpu).vfw.codecs;
            let slot = super::free_slot(list);
            list[slot] = Some(fourcc);
            return Some(slot as u32 + 1);
        }
    }
    Some(0)
}

/// ICSendMessage(hic, msg, dw1, dw2): the codec's answer. A codec here
/// answers Microsoft Video 1's decompression messages, and
/// ICERR_UNSUPPORTED to the others; a handle that isn't open gets
/// ICERR_BADHANDLE.
fn ic_send_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hic, msg, dw1, dw2] = args else {
        unreachable!()
    };
    let Some(fourcc) = open_codec(cpu, hic) else {
        return Some(ICERR_BADHANDLE);
    };
    if fourcc != MSVC {
        return Some(ICERR_UNSUPPORTED);
    }
    Some(match msg {
        ICM_DECOMPRESS_QUERY | ICM_DECOMPRESS_BEGIN => decompress_query(cpu, fourcc, dw1, dw2),
        ICM_DECOMPRESS_GET_FORMAT => decompress_get_format(cpu, dw1, dw2),
        ICM_DECOMPRESS => {
            // An ICDECOMPRESS: dwFlags, lpbiInput, lpInput, lpbiOutput,
            // lpOutput and ckid.
            if dw2 < 20 || !allows(cpu, dw1, 20, false) {
                return Some(ICERR_BADPARAM);
            }
            let field = |cpu: &mut Cpu, n: u32| cpu.bus.guest_read_32(dw1 + 4 * n);
            let (input, data) = (field(cpu, 1), field(cpu, 2));
            let (output, bits) = (field(cpu, 3), field(cpu, 4));
            decompress(cpu, input, data, output, bits)
        }
        // Nothing is kept between frames: each is drawn onto the output,
        // which holds the frame before.
        ICM_DECOMPRESS_END => ICERR_OK,
        // An 8-bit stream's palette indexes go out as they are, whatever
        // the output's palette.
        ICM_DECOMPRESS_SET_PALETTE => ICERR_UNSUPPORTED,
        _ => ICERR_UNSUPPORTED,
    })
}

/// ICDecompress(hic, dwFlags, lpbiFormat, lpData, lpbi, lpBits), which
/// sends the codec ICM_DECOMPRESS: ICERR_OK, or an error.
fn ic_decompress(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hic, _flags, input, data, output, bits] = args else {
        unreachable!()
    };
    Some(match open_codec(cpu, hic) {
        Some(MSVC) => decompress(cpu, input, data, output, bits),
        Some(_) => ICERR_UNSUPPORTED,
        None => ICERR_BADHANDLE,
    })
}

/// ICM_DECOMPRESS_GET_FORMAT: with no `output`, the bytes of the
/// format the codec decompresses `input` to; else that format written
/// at `output`: the stream's size and bits, uncompressed, with an 8-bit
/// stream's 256 colours.
fn decompress_get_format(cpu: &mut Cpu, input: u32, output: u32) -> u32 {
    let source = match ms_video_1_formats(cpu, input, 0) {
        Ok((source, _)) => source,
        Err(e) => return e,
    };
    let colours: u32 = if source.bits == 8 { 256 } else { 0 };
    let size = HEADER + 4 * colours;
    if output == 0 {
        return size;
    }
    if !allows(cpu, output, size, true) {
        return ICERR_BADPARAM;
    }
    let image = codec::dib_size(source.bits, source.width, source.height).unwrap_or(0);
    let mut out = vec![0u8; size as usize];
    out[0..4].copy_from_slice(&HEADER.to_le_bytes());
    out[4..8].copy_from_slice(&source.width.to_le_bytes());
    out[8..12].copy_from_slice(&source.height.to_le_bytes());
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&source.bits.to_le_bytes());
    out[20..24].copy_from_slice(&image.to_le_bytes());
    out[32..36].copy_from_slice(&colours.to_le_bytes());
    let palette = out[HEADER as usize..].as_chunks_mut::<4>().0;
    for (q, c) in palette.iter_mut().zip(&source.palette) {
        q[..3].copy_from_slice(c);
    }
    cpu.bus.guest_write_bytes(output, &out);
    ICERR_OK
}

/// Decodes the Microsoft Video 1 frame `data`, of the format at `input`,
/// onto the DIB `bits` of the format at `output`. Like msvidc32.dll it
/// draws in place: the blocks a frame skips keep what the DIB had, which
/// is the frame before when the program decodes every frame to one
/// buffer. A frame cut short draws the blocks before its end and is
/// logged.
fn decompress(cpu: &mut Cpu, input: u32, data: u32, output: u32, bits: u32) -> u32 {
    let (source, (layout, top_down)) = match ms_video_1_formats(cpu, input, output) {
        Ok((source, Some(layout))) => (source, layout),
        Ok((_, None)) => return ICERR_BADPARAM,
        Err(e) => return e,
    };
    let depth = msvideo1::Depth::of(source.bits).expect("a depth the query took");
    let (width, height) = (source.width, source.height);
    let Some(size) = codec::dib_size(layout.bits(), width, height) else {
        return ICERR_BADFORMAT;
    };
    if bits == 0 || !allows(cpu, bits, size, true) {
        return ICERR_BADPARAM;
    }
    // The frame's size is the input's biSizeImage, as for DrawDibDraw's
    // frames, and is at most a code and eight 16-bit colours a block. A
    // biSizeImage of 0 says nothing: the frame ends where its last block
    // does.
    let most = (width / 4) * (height / 4) * 18;
    let most = match read_info(cpu, input).map_or(0, |i| i.size_image) {
        0 => most,
        size => size.min(most),
    };
    let mut frame = vec![0; readable(cpu, data, most) as usize];
    cpu.bus.guest_read_bytes(data, &mut frame);
    let mut pixels = vec![0; size as usize];
    cpu.bus.guest_read_bytes(bits, &mut pixels);
    let mut target = msvideo1::Target {
        pixels: &mut pixels,
        width,
        height,
        output: layout,
        top_down,
    };
    if let Err(e) = target.decode(&frame, depth, &source.palette) {
        cpu.bus.log_string(&format!("[W32] ICDecompress: {e}"));
    }
    cpu.bus.guest_write_bytes(bits, &pixels);
    ICERR_OK
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
    /// Whether the compressed source has more than 8 bits a pixel, so
    /// its picture is dithered on a display of 256 colours. Set again
    /// from the format when the decoder is (after a restore).
    deep: bool,
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
    codec,
    deep
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
    let slot = super::free_slot(list);
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
        // The same codec and size can be another stream, with colours
        // of its own.
        if d.buffer != 0
            && let Some(format) = read_format(cpu, at)
        {
            let d = dd(cpu, slot);
            d.codec.follow(&format);
            d.deep = format.bits > 8;
        }
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
    let Some(format) = read_format(cpu, at) else {
        return false;
    };
    if !codec::supported(&format) {
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
    d.codec = Codec::new(&format);
    d.deep = format.bits > 8;
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
    let (src_info, src_bits, deep) = if d.buffer != 0 {
        if flags & DDF_UPDATE == 0 && bits != 0 {
            decode(cpu, slot, info, bits);
        }
        let d = dd(cpu, slot);
        if d.dirty && flags & DDF_DONTDRAW == 0 {
            write_picture(cpu, slot);
        }
        let d = dd(cpu, slot);
        (d.buffer, d.buffer + HEADER, d.deep)
    } else {
        if flags & DDF_UPDATE == 0 {
            d.last = (info, bits);
        }
        let (at, bits) = d.last;
        let deep = read_info(cpu, at).is_some_and(|i| i.bit_count > 8);
        (at, bits, deep)
    };
    if flags & DDF_DONTDRAW != 0 || src_bits == 0 {
        return Some(1);
    }
    // On a display of 256 colours, a picture of more bits is dithered to
    // the halftone palette. DrawDib asks the display, as GetDeviceCaps
    // tells every DC the display's depth.
    let halftone = deep && crate::win32::gdi32::palette::palette_device(cpu);
    let drawn = bitmap::stretch_dib(
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
        halftone,
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

/// Decompresses the frame `bits` that the BITMAPINFO at `at` describes.
fn decode(cpu: &mut Cpu, slot: usize, at: u32, bits: u32) {
    let Some(info) = read_info(cpu, at) else {
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
    // A copy's (after a restore) starts again.
    if dd(cpu, slot).codec.is_empty() {
        let format = read_format(cpu, at).unwrap_or_default();
        let d = dd(cpu, slot);
        d.codec = Codec::new(&format);
        d.deep = format.bits > 8;
    }
    let d = dd(cpu, slot);
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
