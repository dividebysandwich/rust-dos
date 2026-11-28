//! MSVFW32.DLL: the DrawDib functions, which draw a DIB to a DC and
//! decompress it first when it is compressed; ICInfo, ICOpen, ICLocate,
//! ICGetInfo and ICClose, which find, open and describe the decompressors
//! codec.rs has; and ICSendMessage and ICDecompress, through which a
//! program decompresses frames itself. The DrawDib functions' behaviour
//! follows Wine's drawdib.c (LGPL 2.1 or later):
//!
//! Copyright 2000 Bradley Baetz
//!
//! A DrawDib keeps the picture it decompressed last in a 24-bit DIB in
//! the process heap, which DrawDibDraw then draws with StretchDIBits;
//! DDF_UPDATE draws it again and DDF_DONTDRAW decompresses without
//! drawing. DrawDibBegin fails for a DIB no codec of codec.rs takes. An
//! uncompressed DIB goes to StretchDIBits as it is. On a display of 256
//! colours a picture of more than 8 bits a pixel is stretched to the
//! destination and dithered to the halftone palette (`gdi32::dither`),
//! but for a Cinepak one: iccvid.dll decodes that to 8 bits itself, in
//! a palette of its own (`cinepak::palette_8`), at the size of the
//! picture or twice it, and DrawDib stretches that.
//! A save state keeps the DIB but not the decoder: after a restore the
//! decoder refuses the frames up to the next key frame, and the picture
//! stays until then.

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
    stdcall("ICGetInfo", 30, 3, ic_get_info),
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
pub(crate) fn ic_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
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
    write_info(cpu, info, fourcc, 0, "", "", Some(driver));
    Some(1)
}

/// An ICINFO at `at`: the codec `fourcc`'s type and handler, its flags
/// and versions, and its name, description and driver's file in UTF-16.
/// With no `driver`, szDriver is left as it is, as a driver answering
/// ICM_GETINFO leaves it for MSVFW32 to fill.
fn write_info(
    cpu: &mut Cpu,
    at: u32,
    fourcc: &[u8; 4],
    version: u32,
    name: &str,
    description: &str,
    driver: Option<&str>,
) {
    let mut bytes = Vec::with_capacity(ICINFO as usize);
    for value in [
        ICINFO,
        ICTYPE_VIDEO,
        u32::from_le_bytes(*fourcc),
        0,
        version,
        ICVERSION,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let texts = [
        Some((name, 16)),
        Some((description, 128)),
        driver.map(|driver| (driver, 128)),
    ];
    for (text, chars) in texts.into_iter().flatten() {
        let end = bytes.len() + 2 * chars;
        // The last character stays the terminating 0.
        for unit in text.encode_utf16().take(chars - 1) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.resize(end, 0);
    }
    cpu.bus.guest_write_bytes(at, &bytes);
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
    Some(open(cpu, codec::upper(u32::from_le_bytes(**fourcc))))
}

/// A new handle to the codec `fourcc` (in capitals), with no decoder
/// yet.
fn open(cpu: &mut Cpu, fourcc: u32) -> u32 {
    let vfw = &mut crate::win32::process(cpu).vfw;
    let slot = super::free_slot(&mut vfw.codecs);
    vfw.codecs[slot] = Some(fourcc);
    if vfw.decoders.len() <= slot {
        vfw.decoders.resize_with(slot + 1, Codec::default);
    }
    vfw.decoders[slot] = Codec::default();
    slot as u32 + 1
}

/// ICClose(hic): ICERR_OK, or ICERR_BADHANDLE for a handle that isn't
/// open.
fn ic_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let vfw = &mut crate::win32::process(cpu).vfw;
    let slot = (args[0] as usize).checked_sub(1);
    let open = slot
        .and_then(|slot| vfw.codecs.get_mut(slot))
        .and_then(Option::take);
    if let Some(decoder) = slot.and_then(|slot| vfw.decoders.get_mut(slot)) {
        *decoder = Codec::default();
    }
    Some(if open.is_some() {
        ICERR_OK
    } else {
        ICERR_BADHANDLE
    })
}

/// ICGetInfo(hic, lpicinfo, cb): the ICINFO of the open codec, which on
/// Windows its driver fills (ICM_GETINFO, `get_info`) with its name and
/// description, and MSVFW32 with the driver's file. It answers what the
/// driver does: ICINFO's size, or 0 when `cb` is smaller than an ICINFO.
/// A handle that isn't open gets ICERR_BADHANDLE, and no ICINFO
/// MMSYSERR_INVALPARAM, as Wine's ICGetInfo gives. Actua Soccer 2 asks
/// for its intro film's Microsoft Video 1 decoder's after ICLocate,
/// before it decompresses, and stops without it.
pub(crate) fn ic_get_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hic, info, size] = args else {
        unreachable!()
    };
    let Some(open) = open_codec(cpu, hic) else {
        return Some(ICERR_BADHANDLE);
    };
    if info == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    Some(get_info(cpu, open, info, size, true))
}

/// ICM_GETINFO(lpicinfo, cb) of the open codec `open`: with no ICINFO,
/// its size; 0 when `cb` is smaller than an ICINFO; else the ICINFO
/// filled and its size. dwFlags is 0, since the codecs here don't
/// compress, and dwVersion is ICVERSION, as Wine's drivers give. The
/// driver leaves szDriver alone; ICGetInfo (`with_driver`) fills it.
fn get_info(cpu: &mut Cpu, open: u32, info: u32, size: u32, with_driver: bool) -> u32 {
    if info == 0 {
        return ICINFO;
    }
    if size < ICINFO || !allows(cpu, info, ICINFO, true) {
        return 0;
    }
    let Some(&(fourcc, driver)) = installed(open) else {
        return 0;
    };
    let (name, description) = describe(open);
    let driver = with_driver.then_some(driver);
    write_info(cpu, info, fourcc, ICVERSION, name, description, driver);
    ICINFO
}

/// ICM_GETINFO, and MMSYSERR_INVALPARAM.
const ICM_GETINFO: u32 = 0x5002;
const MMSYSERR_INVALPARAM: u32 = 11;

/// The name and description of the installed codec of FourCC `fourcc`
/// (in capitals), as Windows 98's drivers give them. The Indeo drivers'
/// aren't known here, and their ICINFO has none.
fn describe(fourcc: u32) -> (&'static str, &'static str) {
    match &fourcc.to_le_bytes() {
        b"MSVC" => ("MS-CRAM", "Microsoft Video 1"),
        b"CVID" => ("Cinepak", "Cinepak Codec by Radius"),
        _ => ("", ""),
    }
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

/// The layout of the uncompressed DIB the BITMAPINFO at `output`
/// describes, and its width and height: 8 bits, 16 (RGB555, or RGB565
/// by its BI_BITFIELDS masks), 24 or 32, from the bottom or the top. An
/// error for another format.
fn output_layout(cpu: &mut Cpu, output: u32) -> Result<(Layout, (u32, u32)), u32> {
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
        (BI_RGB, 8) => Output::Indexes,
        (BI_RGB, 16) => Output::Rgb16 { green6: false },
        (BI_RGB, 24) => Output::Rgb24,
        (BI_RGB, 32) => Output::Rgb32,
        (BI_BITFIELDS, 16) => match masks {
            Some([0xF800, 0x07E0, 0x001F]) => Output::Rgb16 { green6: true },
            Some([0x7C00, 0x03E0, 0x001F]) => Output::Rgb16 { green6: false },
            _ => return Err(ICERR_BADFORMAT),
        },
        _ => return Err(ICERR_BADFORMAT),
    };
    let size = (out.width.unsigned_abs(), out.height.unsigned_abs());
    Ok(((layout, out.height < 0), size))
}

/// The formats of a Microsoft Video 1 decompression, from the
/// BITMAPINFOs at `input` and `output`: the stream's, and the layout of
/// the output (none for no `output`). An error for a pair it doesn't
/// decompress. The stream is CRAM (or MSVC) of 8 or 16 bits, and the
/// output an uncompressed DIB of its size (`output_layout`), of 8 bits
/// only for an 8-bit stream.
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
    match output_layout(cpu, output)? {
        (layout, size) if size == (source.width, source.height) && layout.0.takes(depth) => {
            Ok((source, Some(layout)))
        }
        _ => Err(ICERR_BADFORMAT),
    }
}

/// Whether the codec opened as `fourcc` (in capitals) decodes frames of
/// biCompression `compression`: its own FourCC, which for Indeo 3's
/// driver is either of IV31 and IV32.
fn decodes(fourcc: u32, compression: u32) -> bool {
    const IV31: u32 = u32::from_le_bytes(*b"IV31");
    const IV32: u32 = u32::from_le_bytes(*b"IV32");
    let compression = codec::upper(compression);
    compression == fourcc || matches!((fourcc, compression), (IV31 | IV32, IV31 | IV32))
}

/// The formats of a decompression by the codec `fourcc` other than
/// Microsoft Video 1, as `ms_video_1_formats` gives them: the stream is
/// one of the codec's that codec.rs decodes, and the output an
/// uncompressed DIB of its size of 16, 24 or 32 bits, or of 8 bits
/// with a colour table that can be read (`output_colours`). codec.rs
/// gives a picture of 24 bits, which is written in the output's format,
/// in 8 bits as the nearest of the DIB's colours. iccvid.dll gives 8
/// bits dithered to a palette of its own, or the one
/// ICM_DECOMPRESS_SET_PALETTE gives (#2202, #2325).
fn codec_formats(
    cpu: &mut Cpu,
    fourcc: u32,
    input: u32,
    output: u32,
) -> Result<(codec::Format, Option<Layout>), u32> {
    let source = read_format(cpu, input).ok_or(ICERR_BADPARAM)?;
    if !decodes(fourcc, source.compression) || !codec::supported(&source) {
        return Err(ICERR_BADFORMAT);
    }
    if output == 0 {
        return Ok((source, None));
    }
    match output_layout(cpu, output)? {
        (layout, size)
            if size == (source.width, source.height)
                && (layout.0 != msvideo1::Output::Indexes
                    || output_colours(cpu, output).is_some()) =>
        {
            Ok((source, Some(layout)))
        }
        _ => Err(ICERR_BADFORMAT),
    }
}

/// The colours of the 8-bit DIB whose BITMAPINFO is at `output`, red,
/// green and blue: biClrUsed of them, or 256 for 0. A picture these
/// codecs decode goes out in 8 bits as the nearest of them
/// (`from_rgb24`). None for a DIB whose colours can't be read.
fn output_colours(cpu: &mut Cpu, output: u32) -> Option<Vec<[u8; 3]>> {
    let info = read_info(cpu, output)?;
    let count = match info.clr_used {
        0 => 256,
        n => n.min(256),
    };
    let at = output.wrapping_add(info.size);
    if !allows(cpu, at, 4 * count, false) {
        return None;
    }
    let mut quads = vec![0; 4 * count as usize];
    cpu.bus.guest_read_bytes(at, &mut quads);
    Some(
        quads
            .as_chunks::<4>()
            .0
            .iter()
            .map(|q| [q[2], q[1], q[0]])
            .collect(),
    )
}

/// The formats of a decompression by the codec `fourcc`.
fn formats(
    cpu: &mut Cpu,
    fourcc: u32,
    input: u32,
    output: u32,
) -> Result<(codec::Format, Option<Layout>), u32> {
    if fourcc == MSVC {
        ms_video_1_formats(cpu, input, output)
    } else {
        codec_formats(cpu, fourcc, input, output)
    }
}

/// ICM_DECOMPRESS_QUERY of the open codec `fourcc`: whether it
/// decompresses the format at `input` to the one at `output`, or to one
/// of its choice for 0.
fn decompress_query(cpu: &mut Cpu, fourcc: u32, input: u32, output: u32) -> u32 {
    match formats(cpu, fourcc, input, output) {
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
/// Video 1 films so, and Outlaw Racers that of its Cinepak ones, and
/// both stop or show nothing when there is none.
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
            return Some(open(cpu, fourcc));
        }
    }
    Some(0)
}

/// ICSendMessage(hic, msg, dw1, dw2): the codec's answer. Every codec
/// here answers ICM_GETINFO and the decompression messages, those of
/// ICDecompressEx too, and ICERR_UNSUPPORTED to the others; a handle
/// that isn't open gets ICERR_BADHANDLE.
pub(crate) fn ic_send_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hic, msg, dw1, dw2] = args else {
        unreachable!()
    };
    let Some(fourcc) = open_codec(cpu, hic) else {
        return Some(ICERR_BADHANDLE);
    };
    if msg == ICM_GETINFO {
        return Some(get_info(cpu, fourcc, dw1, dw2, false));
    }
    Some(match msg {
        ICM_DECOMPRESS_QUERY | ICM_DECOMPRESS_BEGIN => decompress_query(cpu, fourcc, dw1, dw2),
        ICM_DECOMPRESS_GET_FORMAT => decompress_get_format(cpu, fourcc, dw1, dw2),
        ICM_DECOMPRESS => {
            // An ICDECOMPRESS: dwFlags, lpbiInput, lpInput, lpbiOutput,
            // lpOutput and ckid.
            if dw2 < 20 || !allows(cpu, dw1, 20, false) {
                return Some(ICERR_BADPARAM);
            }
            let field = |cpu: &mut Cpu, n: u32| cpu.bus.guest_read_32(dw1 + 4 * n);
            let (flags, input, data) = (field(cpu, 0), field(cpu, 1), field(cpu, 2));
            let (output, bits) = (field(cpu, 3), field(cpu, 4));
            let frame = Frame { flags, data };
            decompress_with(cpu, hic, fourcc, input, frame, output, bits)
        }
        // Microsoft Video 1 keeps nothing between frames: each is drawn
        // onto the output, which holds the frame before. The other
        // codecs' decoders stay with the handle until it is closed.
        ICM_DECOMPRESS_END | ICM_DECOMPRESSEX_END => ICERR_OK,
        ICM_DECOMPRESSEX_QUERY | ICM_DECOMPRESSEX_BEGIN => {
            decompress_ex_query(cpu, fourcc, dw1, dw2)
        }
        ICM_DECOMPRESSEX => decompress_ex(cpu, hic, fourcc, dw1, dw2),
        // An 8-bit stream's palette indexes go out as they are, whatever
        // the output's palette.
        ICM_DECOMPRESS_SET_PALETTE => ICERR_UNSUPPORTED,
        _ => ICERR_UNSUPPORTED,
    })
}

/// ICDecompress(hic, dwFlags, lpbiFormat, lpData, lpbi, lpBits), which
/// sends the codec ICM_DECOMPRESS: ICERR_OK, or an error.
pub(crate) fn ic_decompress(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hic, flags, input, data, output, bits] = args else {
        unreachable!()
    };
    let frame = Frame { flags, data };
    Some(match open_codec(cpu, hic) {
        Some(fourcc) => decompress_with(cpu, hic, fourcc, input, frame, output, bits),
        None => ICERR_BADHANDLE,
    })
}

/// ICDECOMPRESS's dwFlags ICDECOMPRESS_NULLFRAME: the frame has no data,
/// and the picture stays as it is: an AVI's empty chunk, for one, whose
/// `lpInput` may still hold the frame before.
const ICDECOMPRESS_NULLFRAME: u32 = 0x1000_0000;

/// A frame to decompress: ICDECOMPRESS's dwFlags and lpInput.
#[derive(Clone, Copy)]
struct Frame {
    flags: u32,
    data: u32,
}

impl Frame {
    /// Whether the frame has no data to decode (ICDECOMPRESS_NULLFRAME).
    fn null(self) -> bool {
        self.flags & ICDECOMPRESS_NULLFRAME != 0
    }
}

/// ICM_DECOMPRESS of the codec `fourcc` open as `hic`.
fn decompress_with(
    cpu: &mut Cpu,
    hic: u32,
    fourcc: u32,
    input: u32,
    frame: Frame,
    output: u32,
    bits: u32,
) -> u32 {
    if fourcc == MSVC {
        decompress(cpu, input, frame, output, bits)
    } else {
        decompress_picture(cpu, hic, fourcc, input, frame, output, bits, None)
    }
}

/// ICM_DECOMPRESSEX_BEGIN, _QUERY, ICM_DECOMPRESSEX itself and _END,
/// whose first parameter is an ICDECOMPRESSEX: dwFlags, lpbiSrc, lpSrc,
/// lpbiDst and lpDst, then the destination's rectangle and the
/// source's, each a left, top, width and height.
const ICM_DECOMPRESSEX_BEGIN: u32 = 0x403C;
const ICM_DECOMPRESSEX_QUERY: u32 = 0x403D;
const ICM_DECOMPRESSEX: u32 = 0x403E;
const ICM_DECOMPRESSEX_END: u32 = 0x403F;
const ICDECOMPRESSEX: u32 = 52;

/// An ICDECOMPRESSEX's fields.
struct DecompressEx {
    frame: Frame,
    input: u32,
    output: u32,
    bits: u32,
    dest: [i32; 4],
    source: [i32; 4],
}

/// The ICDECOMPRESSEX at `at`, of `size` bytes: None if it is shorter
/// than one or can't be read.
fn read_ex(cpu: &mut Cpu, at: u32, size: u32) -> Option<DecompressEx> {
    if size < ICDECOMPRESSEX || !allows(cpu, at, ICDECOMPRESSEX, false) {
        return None;
    }
    let mut field = |n: u32| cpu.bus.guest_read_32(at + 4 * n);
    let frame = Frame {
        flags: field(0),
        data: field(2),
    };
    let (input, output, bits) = (field(1), field(3), field(4));
    let dest = [5, 6, 7, 8].map(|n| field(n) as i32);
    let source = [9, 10, 11, 12].map(|n| field(n) as i32);
    Some(DecompressEx {
        frame,
        input,
        output,
        bits,
        dest,
        source,
    })
}

/// Where an ICDECOMPRESSEX puts the frame in the output: the
/// destination rectangle's left and top, and 2 for a frame zoomed by
/// two, else 1.
#[derive(Clone, Copy)]
struct Place {
    left: u32,
    top: u32,
    zoom: u32,
}

/// Where the ICDECOMPRESSEX `ex` puts the frame in the output. The
/// source rectangle is the whole frame, and the destination one is of
/// the frame's size or twice it, inside the output: Indeo's drivers
/// zoom by two, which Leadfoot asks of Indeo 5 for its 320 by 240
/// films on a 640 by 480 surface, and the other codecs here take it
/// too, each pixel twice in each direction. Microsoft Video 1
/// draws onto the frame before, so its output is the frame's size.
/// Anything else is ICERR_BADFORMAT; a query without an output asks
/// about the stream alone.
fn ex_place(cpu: &mut Cpu, fourcc: u32, ex: &DecompressEx) -> Result<Place, u32> {
    let (source, _) = formats(cpu, fourcc, ex.input, 0)?;
    let (width, height) = (source.width as i32, source.height as i32);
    if ex.source != [0, 0, width, height] {
        return Err(ICERR_BADFORMAT);
    }
    let whole = Place {
        left: 0,
        top: 0,
        zoom: 1,
    };
    if ex.output == 0 {
        return Ok(whole);
    }
    if fourcc == MSVC {
        formats(cpu, fourcc, ex.input, ex.output)?;
        return match ex.dest {
            [0, 0, w, h] if (w, h) == (width, height) => Ok(whole),
            _ => Err(ICERR_BADFORMAT),
        };
    }
    let ((layout, _), (out_width, out_height)) = output_layout(cpu, ex.output)?;
    let [left, top, w, h] = ex.dest;
    let zoom = match (w, h) {
        _ if (w, h) == (width, height) => 1,
        _ if (i64::from(w), i64::from(h)) == (2 * i64::from(width), 2 * i64::from(height)) => 2,
        _ => return Err(ICERR_BADFORMAT),
    };
    let fits =
        |at: i32, size: i32, out: u32| at >= 0 && i64::from(at) + i64::from(size) <= i64::from(out);
    let no_colours =
        layout == msvideo1::Output::Indexes && output_colours(cpu, ex.output).is_none();
    if !fits(left, w, out_width) || !fits(top, h, out_height) || no_colours {
        return Err(ICERR_BADFORMAT);
    }
    Ok(Place {
        left: left as u32,
        top: top as u32,
        zoom,
    })
}

/// ICM_DECOMPRESSEX_QUERY and _BEGIN: whether the codec `fourcc`
/// decompresses as the ICDECOMPRESSEX at `at` asks.
fn decompress_ex_query(cpu: &mut Cpu, fourcc: u32, at: u32, size: u32) -> u32 {
    let Some(ex) = read_ex(cpu, at, size) else {
        return ICERR_BADPARAM;
    };
    match ex_place(cpu, fourcc, &ex) {
        Ok(_) => ICERR_OK,
        Err(e) => {
            let out =
                read_info(cpu, ex.output).map(|i| (i.width, i.height, i.bit_count, i.compression));
            cpu.bus.log_string(&format!(
                "[W32] ICDecompressEx: refused, destination {:?} of {:?}, source {:?}",
                ex.dest, out, ex.source
            ));
            e
        }
    }
}

/// ICM_DECOMPRESSEX: the frame of the ICDECOMPRESSEX at `at` into its
/// place in the output.
fn decompress_ex(cpu: &mut Cpu, hic: u32, fourcc: u32, at: u32, size: u32) -> u32 {
    let Some(ex) = read_ex(cpu, at, size) else {
        return ICERR_BADPARAM;
    };
    if ex.output == 0 {
        return ICERR_BADPARAM;
    }
    let place = match ex_place(cpu, fourcc, &ex) {
        Ok(place) => place,
        Err(e) => return e,
    };
    let (input, frame, output, bits) = (ex.input, ex.frame, ex.output, ex.bits);
    if fourcc == MSVC {
        decompress(cpu, input, frame, output, bits)
    } else {
        decompress_picture(cpu, hic, fourcc, input, frame, output, bits, Some(place))
    }
}

/// ICM_DECOMPRESS_GET_FORMAT: with no `output`, the bytes of the
/// format the codec decompresses `input` to; else that format written
/// at `output`: the stream's size, uncompressed, in the stream's bits
/// with an 8-bit stream's 256 colours for Microsoft Video 1, and in 24
/// bits for the other codecs, whose pictures codec.rs gives so.
fn decompress_get_format(cpu: &mut Cpu, fourcc: u32, input: u32, output: u32) -> u32 {
    let source = match formats(cpu, fourcc, input, 0) {
        Ok((source, _)) => source,
        Err(e) => return e,
    };
    let bits = if fourcc == MSVC { source.bits } else { 24 };
    let colours: u32 = if bits == 8 { 256 } else { 0 };
    let size = HEADER + 4 * colours;
    if output == 0 {
        return size;
    }
    if !allows(cpu, output, size, true) {
        return ICERR_BADPARAM;
    }
    let image = codec::dib_size(bits, source.width, source.height).unwrap_or(0);
    let mut out = vec![0u8; size as usize];
    out[0..4].copy_from_slice(&HEADER.to_le_bytes());
    out[4..8].copy_from_slice(&source.width.to_le_bytes());
    out[8..12].copy_from_slice(&source.height.to_le_bytes());
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&bits.to_le_bytes());
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
/// logged. A null frame draws nothing.
fn decompress(cpu: &mut Cpu, input: u32, frame: Frame, output: u32, bits: u32) -> u32 {
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
    // The DIB holds the picture before, which a null frame keeps.
    if frame.null() {
        return ICERR_OK;
    }
    let data = frame.data;
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

/// Decodes the frame `data`, of the format at `input`, with the decoder
/// of the handle `hic` of the codec `fourcc`, and writes its picture to
/// the DIB `bits` of the format at `output`. The decoder keeps the
/// picture between frames, as the codecs' drivers do, so a frame that
/// changes part of it gives the whole picture. A frame the decoder
/// refuses is logged, and the DIB gets the picture as it stands, as it
/// does for a null frame; with no picture yet (after a restore, before a
/// key frame) the DIB is left as it is. With `at`, the output may be
/// larger than the frame, which goes to that place of it, zoomed or not
/// (ICM_DECOMPRESSEX, which `ex_place` checked); the rest of the DIB
/// keeps what it has.
#[allow(clippy::too_many_arguments)]
fn decompress_picture(
    cpu: &mut Cpu,
    hic: u32,
    fourcc: u32,
    input: u32,
    frame: Frame,
    output: u32,
    bits: u32,
    at: Option<Place>,
) -> u32 {
    let formats = match at {
        None => codec_formats(cpu, fourcc, input, output).map(|(source, layout)| {
            let size = (source.width, source.height);
            (source, layout.map(|l| (l, size)))
        }),
        Some(_) => codec_formats(cpu, fourcc, input, 0)
            .and_then(|(source, _)| Ok((source, Some(output_layout(cpu, output)?)))),
    };
    let (source, ((layout, top_down), out_size)) = match formats {
        Ok((source, Some(layout))) => (source, layout),
        Ok((_, None)) => return ICERR_BADPARAM,
        Err(e) => return e,
    };
    let (width, height) = (source.width, source.height);
    let Some(size) = codec::dib_size(layout.bits(), out_size.0, out_size.1) else {
        return ICERR_BADFORMAT;
    };
    if bits == 0 || !allows(cpu, bits, size, true) {
        return ICERR_BADPARAM;
    }
    // As for DrawDibDraw's frames: a frame's own header has its size,
    // and biSizeImage (or a 24-bit picture's size when that is 0) is
    // only the most it can be. A null frame has none.
    let most = match read_info(cpu, input).map_or(0, |i| i.size_image) {
        _ if frame.null() => 0,
        0 => codec::dib_size(24, width, height).unwrap_or(0),
        size => size,
    };
    let data = frame.data;
    let mut frame = vec![0; readable(cpu, data, most) as usize];
    cpu.bus.guest_read_bytes(data, &mut frame);
    let slot = hic as usize - 1;
    let decoders = &mut crate::win32::process(cpu).vfw.decoders;
    if decoders.len() <= slot {
        decoders.resize_with(slot + 1, Codec::default);
    }
    let decoder = &mut decoders[slot];
    if decoder.is_empty() {
        *decoder = Codec::new(&source);
    }
    let decoded = if frame.is_empty() {
        Ok(())
    } else {
        decoder.decode(&frame)
    };
    let picture = decoder.picture(width, height);
    if let Err(e) = decoded {
        cpu.bus.log_string(&format!("[W32] ICDecompress: {e}"));
    }
    let Some(picture) = picture else {
        return ICERR_OK;
    };
    let Place { left, top, zoom } = at.unwrap_or(Place {
        left: 0,
        top: 0,
        zoom: 1,
    });
    let (picture, width, height) = match zoom {
        2 => (zoom_by_two(&picture, width, height), 2 * width, 2 * height),
        _ => (picture, width, height),
    };
    let colours = match layout {
        msvideo1::Output::Indexes => output_colours(cpu, output).unwrap_or_default(),
        _ => Vec::new(),
    };
    let pixels = from_rgb24(&picture, width, height, layout, top_down, &colours);
    if (left, top) == (0, 0) && out_size == (width, height) {
        cpu.bus.guest_write_bytes(bits, &pixels);
        return ICERR_OK;
    }
    // The frame's rows into the output's, counted from its top.
    let bytes = u32::from(layout.bits() / 8);
    let from = crate::win32::gdi32::surface::dib_stride(width as i32, layout.bits().into());
    let to = crate::win32::gdi32::surface::dib_stride(out_size.0 as i32, layout.bits().into());
    let row_of = |y: u32, rows: u32| if top_down { y } else { rows - 1 - y };
    for y in 0..height {
        let source = &pixels[row_of(y, height) as usize * from..][..(width * bytes) as usize];
        let row = row_of(top + y, out_size.1);
        let at = bits + row * to as u32 + left * bytes;
        cpu.bus.guest_write_bytes(at, source);
    }
    ICERR_OK
}

/// The 24-bit DIB `picture` of `width` by `height` at twice its size,
/// each pixel twice in each direction.
fn zoom_by_two(picture: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let from = crate::win32::gdi32::surface::dib_stride(width as i32, 24);
    let to = crate::win32::gdi32::surface::dib_stride(2 * width as i32, 24);
    let mut out = vec![0; to * 2 * h];
    for y in 0..2 * h {
        let source = picture[y / 2 * from..][..w * 3].as_chunks::<3>().0;
        let dest = out[y * to..][..w * 6].as_chunks_mut::<6>().0;
        for (d, s) in dest.iter_mut().zip(source) {
            d[..3].copy_from_slice(s);
            d[3..].copy_from_slice(s);
        }
    }
    out
}

/// A bottom-up 24-bit DIB of `width` by `height` in `layout`, its rows
/// from the top when `top_down`. In 8 bits each pixel is the index of
/// the nearest of `colours` (red, green and blue), found for its colour
/// at 4 bits a channel.
fn from_rgb24(
    picture: &[u8],
    width: u32,
    height: u32,
    layout: msvideo1::Output,
    top_down: bool,
    colours: &[[u8; 3]],
) -> Vec<u8> {
    use msvideo1::Output;
    let nearest = match layout {
        Output::Indexes => nearest_table(colours),
        _ => std::rc::Rc::from([]),
    };
    let (w, h) = (width as usize, height as usize);
    let from = crate::win32::gdi32::surface::dib_stride(width as i32, 24);
    let to = crate::win32::gdi32::surface::dib_stride(width as i32, u32::from(layout.bits()));
    let bytes = usize::from(layout.bits() / 8);
    let mut out = vec![0; to * h];
    for y in 0..h {
        let row = if top_down { h - 1 - y } else { y };
        let source = picture[y * from..][..w * 3].as_chunks::<3>().0;
        let dest = &mut out[row * to..][..w * bytes];
        for (o, &[b, g, r]) in dest.chunks_exact_mut(bytes).zip(source) {
            match layout {
                Output::Rgb16 { green6 } => {
                    let [r, g, b] = [r, g, b].map(u16::from);
                    let word = if green6 {
                        (r >> 3) << 11 | (g >> 2) << 5 | b >> 3
                    } else {
                        (r >> 3) << 10 | (g >> 3) << 5 | b >> 3
                    };
                    o.copy_from_slice(&word.to_le_bytes());
                }
                Output::Rgb24 | Output::Rgb32 => o[..3].copy_from_slice(&[b, g, r]),
                Output::Indexes => {
                    let [r, g, b] = [r, g, b].map(|c| usize::from(c >> 4));
                    o[0] = nearest[r << 8 | g << 4 | b];
                }
            }
        }
    }
    out
}

thread_local! {
    /// The colours `nearest_table` last made a table for, and the table.
    /// A film's frames go to one DIB, whose colours stay from frame to
    /// frame, and a table takes 4096 times as many sums as there are
    /// colours: about a million for 256.
    static NEAREST: std::cell::RefCell<(Vec<[u8; 3]>, std::rc::Rc<[u8]>)> =
        std::cell::RefCell::new((Vec::new(), std::rc::Rc::from([0; 4096])));
}

/// For each colour of 4 bits a channel (red, green and blue, from the
/// high bits), the index of the nearest of `colours` to the colour each
/// channel's 4 bits stand for: 17 times them, so that black is 0 and
/// white 255 in every channel.
fn nearest_table(colours: &[[u8; 3]]) -> std::rc::Rc<[u8]> {
    NEAREST.with(|last| {
        let mut last = last.borrow_mut();
        if last.0 != colours {
            *last = (colours.to_vec(), make_nearest_table(colours).into());
        }
        last.1.clone()
    })
}

/// `nearest_table`'s table of `colours`, made.
fn make_nearest_table(colours: &[[u8; 3]]) -> Vec<u8> {
    let mut table = vec![0; 4096];
    for (key, slot) in table.iter_mut().enumerate() {
        let channel = |shift: usize| ((key >> shift) & 15) as i32 * 17;
        let (r, g, b) = (channel(8), channel(4), channel(0));
        let distance = |c: &[u8; 3]| {
            let [dr, dg, db] = [c[0] as i32 - r, c[1] as i32 - g, c[2] as i32 - b];
            dr * dr + dg * dg + db * db
        };
        *slot = (0..colours.len())
            .min_by_key(|&i| distance(&colours[i]))
            .unwrap_or(0) as u8;
    }
    table
}

/// DrawDibDraw's flags.
const DDF_UPDATE: u32 = 0x0002;
const DDF_SAME_HDC: u32 = 0x0004;
const DDF_SAME_DRAW: u32 = 0x0008;
const DDF_DONTDRAW: u32 = 0x0010;
const DDF_BACKGROUNDPAL: u32 = 0x0200;

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
    /// The palette of a Cinepak picture's 8-bit colours, made when it is
    /// first drawn on a display of 256 colours (`draw_cinepak_8`).
    palette: u32,
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
    deep,
    palette
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
    drop_palette(cpu, slot);
    let d = dd(cpu, slot);
    let buffer = std::mem::take(&mut d.buffer);
    *d = DrawDib::default();
    if buffer != 0 {
        crate::win32::with_process(cpu, |p, bus| p.heap.free(&p.memory, bus, buffer));
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
            let deep = format.bits > 8;
            if d.deep != deep {
                // A Cinepak stream of the other kind (grey or colour)
                // takes the other palette.
                d.deep = deep;
                drop_palette(cpu, slot);
            }
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
    let src_rect = (xs as i32, ys as i32, dxs as i32, dys as i32);
    let dst_rect = (xd as i32, yd as i32, dxd as i32, dyd as i32);
    let background = flags & DDF_BACKGROUNDPAL != 0;
    if draw_cinepak_8(cpu, slot, hdc, dst_rect, src_rect, background) {
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

/// On a display of 256 colours, draws the picture of a Cinepak source as
/// Windows 98's DrawDib does: decoded by the codec to 8 bits in its
/// palette, at twice the size when the whole picture is drawn at twice
/// its size, and stretched to the destination `dst` (x, y, width,
/// height) from the source rectangle `src` (x and y from the picture's
/// bottom left). The codec's palette is selected into the DC and
/// realized for the drawing, so the pixels keep their indexes, which on
/// Windows land on the same entries of the system palette. It is
/// realized as a foreground palette unless `background`
/// (DDF_BACKGROUNDPAL), as Wine's DrawDibDraw selects it. False when the
/// picture isn't drawn so.
fn draw_cinepak_8(
    cpu: &mut Cpu,
    slot: usize,
    hdc: u32,
    (xd, yd, dxd, dyd): (i32, i32, i32, i32),
    (xs, ys, dxs, dys): (i32, i32, i32, i32),
    background: bool,
) -> bool {
    use crate::win32::gdi32::{self, dither, draw, palette};
    let d = dd(cpu, slot);
    let cinepak = codec::upper(d.compression) == codec::upper(u32::from_le_bytes(*b"cvid"));
    if d.buffer == 0 || !cinepak || !palette::palette_device(cpu) {
        return false;
    }
    let Some(dc) = gdi32::gdi(cpu).dc(hdc).cloned() else {
        return false;
    };
    let d = dd(cpu, slot);
    let (w, h) = (d.width as i32, d.height as i32);
    let (x0, y0) = dc.to_device(xd, yd);
    let (x1, y1) = dc.to_device(xd + dxd, yd + dyd);
    let (pw, ph) = (x1 - x0, y1 - y0);
    if pw <= 0 || ph <= 0 || dxs <= 0 || dys <= 0 {
        return false;
    }
    let top = h - ys - dys;
    let from = (xs, top, xs + dxs, top + dys);
    let zoom = if from == (0, 0, w, h) && (pw, ph) == (2 * w, 2 * h) {
        2
    } else {
        1
    };
    let Some(indexes) = d.codec.indexes_8(d.width, d.height, zoom as u32) else {
        return false;
    };
    let grey = !d.deep;
    let mut hpal = d.palette;
    let colors = super::cinepak::palette_8(gdi32::static_colors(), grey);
    if gdi32::gdi(cpu).palette(hpal).is_none() {
        hpal = palette::system_palette_of(cpu, &colors);
        dd(cpu, slot).palette = hpal;
    }
    let from = if zoom == 2 { (0, 0, pw, ph) } else { from };
    let surface = dither::stretch_8(&indexes, w * zoom, colors, from, pw, ph);
    let source = draw::Source {
        surface,
        bk_color: 0xFF_FFFF,
        origin: (0, 0),
    };
    let old = palette::select_palette(cpu, &[hdc, hpal, background as u32]).filter(|&o| o != 0);
    palette::realize_posted(cpu, hdc);
    draw::blit_from(
        cpu,
        hdc,
        (x0, y0, x1, y1),
        Some((source, (0, 0, pw, ph))),
        SRCCOPY,
    );
    if let Some(old) = old {
        palette::select_palette(cpu, &[hdc, old, 1]);
    }
    true
}

/// Deletes the palette of a DrawDib's Cinepak pictures (`draw_cinepak_8`),
/// which no process owns, when the DrawDib ends or its stream changes
/// kind.
fn drop_palette(cpu: &mut Cpu, slot: usize) {
    let hpal = std::mem::take(&mut dd(cpu, slot).palette);
    if crate::win32::gdi32::gdi(cpu).palette(hpal).is_some() {
        crate::win32::gdi32::delete_object(cpu, &[hpal]);
    }
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
