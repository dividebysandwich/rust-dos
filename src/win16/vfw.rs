//! Video for Windows' 16-bit modules, as Windows 95 and 98 have them in
//! their SYSTEM directory: AVIFILE (AVI files and their streams) and
//! MSVIDEO (the Installable Compression Manager and DrawDib). Windows
//! 98 SE's MSVIDEO.DLL hands the codecs to the 32-bit MSVFW32; these go
//! on to the built-in AVIFIL32 and MSVFW32 (win32/vfw) for all of it.
//! With them in SYSTEM, a setup that brings Video for Windows 1.1e for
//! Windows 3.1, such as Civilization II's, finds newer files and keeps
//! them, as on Windows 98.
//!
//! The 32-bit functions take the 16-bit ones' structures where the two
//! are the same. Those with 16-bit fields or 16:16 pointers in them are
//! converted, as Wine's msvideo.dll16 converts them (LGPL 2.1 or later,
//! Copyright 1998 Marcus Meissner and others): AVISTREAMINFO's rcFrame,
//! ICINFO's strings, ICDECOMPRESS's and ICDECOMPRESSEX's pointers.
//!
//! An IAVIFile or IAVIStream a 16-bit program gets is the 32-bit
//! object's address, which the AVIFile functions take back. A program
//! that calls the objects' methods itself, through their vtables,
//! can't. AVIStreamGetFrame, which gives a flat pointer, the AVISave and
//! editing functions, the clipboard's and the capture functions
//! (videoOpen and the rest) aren't there yet.

use super::builtin::{Def, cdecl, missing, rust, variable, win32};
use super::ole2::ole_guid;
use crate::cpu::{Cpu, Seg};
use crate::win32::vfw::{avifil32, msvfw32};

const A: &str = "AVIFIL32.DLL";
const V: &str = "MSVFW32.DLL";

pub static AVIFILE: Def = Def {
    name: "AVIFILE",
    file: "AVIFILE.DLL",
    apis: &[
        missing("DllGetClassObject", 2),
        missing("DllCanUnloadNow", 3),
        variable("_IID_IAVIStream", 10, &ole_guid(0x0002_0021)),
        variable("_IID_IAVIFile", 11, &ole_guid(0x0002_0020)),
        variable("_IID_IAVIEditStream", 12, &ole_guid(0x0002_0024)),
        variable("_IID_IGetFrame", 13, &ole_guid(0x0002_0023)),
        variable("_CLSID_AVISimpleUnMarshal", 14, &ole_guid(0x0002_0009)),
        win32("AVIFileInit", 100, "", A, "AVIFileInit", b'w'),
        win32("AVIFileExit", 101, "", A, "AVIFileExit", b'w'),
        win32("AVIFileOpen", 102, "ppwp", A, "AVIFileOpenA", b'w'),
        win32(
            "AVIStreamOpenFromFile",
            103,
            "ppllwp",
            A,
            "AVIStreamOpenFromFileA",
            b'w',
        ),
        missing("AVIStreamCreate", 104),
        missing("AVIMakeCompressedStream", 105),
        missing("AVIMakeFileFromStreams", 106),
        missing("AVIMakeStreamFromClipboard", 107),
        missing("AVIStreamGetFrame", 110),
        missing("AVIStreamGetFrameClose", 111),
        missing("AVIStreamGetFrameOpen", 112),
        missing("_AVISave", 120),
        missing("AVISaveV", 121),
        missing("AVISaveOptions", 122),
        missing("AVIBuildFilter", 123),
        missing("AVISaveOptionsFree", 124),
        win32("AVIStreamStart", 130, "l", A, "AVIStreamStart", b'w'),
        win32("AVIStreamLength", 131, "l", A, "AVIStreamLength", b'w'),
        win32(
            "AVIStreamTimeToSample",
            132,
            "ll",
            A,
            "AVIStreamTimeToSample",
            b'w',
        ),
        win32(
            "AVIStreamSampleToTime",
            133,
            "ll",
            A,
            "AVIStreamSampleToTime",
            b'w',
        ),
        win32("AVIFileAddRef", 140, "l", A, "AVIFileAddRef", b'w'),
        win32("AVIFileRelease", 141, "l", A, "AVIFileRelease", b'w'),
        win32("AVIFileInfo", 142, "lpl", A, "AVIFileInfoA", b'w'),
        win32("AVIFileGetStream", 143, "lpll", A, "AVIFileGetStream", b'w'),
        missing("AVIFileCreateStream", 144),
        missing("AVIFileWriteData", 146),
        missing("AVIFileReadData", 147),
        missing("AVIFileEndRecord", 148),
        win32("AVIStreamAddRef", 160, "l", A, "AVIStreamAddRef", b'w'),
        win32("AVIStreamRelease", 161, "l", A, "AVIStreamRelease", b'w'),
        rust("AVIStreamInfo", 162, "lpl", avi_stream_info),
        win32(
            "AVIStreamFindSample",
            163,
            "lll",
            A,
            "AVIStreamFindSample",
            b'w',
        ),
        win32(
            "AVIStreamReadFormat",
            164,
            "llpp",
            A,
            "AVIStreamReadFormat",
            b'w',
        ),
        missing("AVIStreamReadData", 165),
        missing("AVIStreamWriteData", 166),
        win32("AVIStreamRead", 167, "lllplpp", A, "AVIStreamRead", b'w'),
        missing("AVIStreamWrite", 168),
        missing("AVIStreamSetFormat", 169),
        missing("EditStreamCopy", 180),
        missing("EditStreamCut", 181),
        missing("EditStreamPaste", 182),
        missing("CreateEditableStream", 184),
        missing("AVIPutFileOnClipboard", 185),
        missing("AVIGetFromClipboard", 187),
        missing("AVIClearClipboard", 188),
        missing("EditStreamClone", 190),
        missing("EditStreamSetName", 191),
        missing("EditStreamSetInfo", 192),
        win32(
            "AVIStreamBeginStreaming",
            200,
            "llll",
            A,
            "AVIStreamBeginStreaming",
            b'w',
        ),
        win32(
            "AVIStreamEndStreaming",
            201,
            "l",
            A,
            "AVIStreamEndStreaming",
            b'w',
        ),
    ],
    library: true,
};

pub static MSVIDEO: Def = Def {
    name: "MSVIDEO",
    file: "MSVIDEO.DLL",
    apis: &[
        rust("VideoForWindowsVersion", 2, "", vfw_version),
        missing("videoGetNumDevs", 20),
        missing("videoGetErrorText", 21),
        missing("videoCapDriverDescAndVer", 22),
        missing("videoOpen", 28),
        missing("videoClose", 29),
        missing("videoDialog", 30),
        missing("videoFrame", 31),
        missing("videoConfigure", 32),
        missing("videoConfigureStorage", 33),
        missing("videoGetChannelCaps", 34),
        missing("videoUpdate", 35),
        missing("videoStreamAddBuffer", 40),
        missing("videoStreamFini", 41),
        missing("videoStreamGetError", 42),
        missing("videoStreamGetPosition", 43),
        missing("videoStreamInit", 44),
        missing("videoStreamPrepareHeader", 46),
        missing("videoStreamReset", 47),
        missing("videoStreamStart", 49),
        missing("videoStreamStop", 50),
        missing("videoStreamUnprepareHeader", 51),
        missing("videoStreamAllocHdrAndBuffer", 52),
        missing("videoStreamFreeHdrAndBuffer", 53),
        missing("videoMessage", 60),
        win32("DrawDibOpen", 102, "", V, "DrawDibOpen", b'w'),
        win32("DrawDibClose", 103, "w", V, "DrawDibClose", b'b'),
        win32("DrawDibBegin", 104, "wwsspssw", V, "DrawDibBegin", b'b'),
        win32("DrawDibEnd", 105, "w", V, "DrawDibEnd", b'b'),
        win32("DrawDibDraw", 106, "wwssssppssssw", V, "DrawDibDraw", b'b'),
        win32("DrawDibGetPalette", 108, "w", V, "DrawDibGetPalette", b'w'),
        win32("DrawDibSetPalette", 110, "ww", V, "DrawDibSetPalette", b'b'),
        win32(
            "DrawDibChangePalette",
            111,
            "wssp",
            V,
            "DrawDibChangePalette",
            b'b',
        ),
        win32("DrawDibRealize", 112, "wws", V, "DrawDibRealize", b'w'),
        win32("DrawDibTime", 113, "wp", V, "DrawDibTime", b'b'),
        win32(
            "DrawDibProfileDisplay",
            114,
            "p",
            V,
            "DrawDibProfileDisplay",
            b'w',
        ),
        missing("StretchDIB", 115),
        win32("DrawDibStart", 118, "wl", V, "DrawDibStart", b'b'),
        win32("DrawDibStop", 119, "w", V, "DrawDibStop", b'b'),
        missing("DrawDibGetBuffer", 120),
        rust("ICInfo", 200, "llp", ic_info),
        missing("ICInstall", 201),
        missing("ICRemove", 202),
        win32("ICOpen", 203, "llw", V, "ICOpen", b'w'),
        win32("ICClose", 204, "w", V, "ICClose", b'w'),
        rust("ICSendMessage", 205, "wwll", ic_send_message),
        missing("ICOpenFunction", 206),
        cdecl("_ICMessage", 207, "www", ic_message),
        rust("ICGetInfo", 212, "wpl", ic_get_info),
        win32("ICLocate", 213, "llppw", V, "ICLocate", b'w'),
        missing("_ICCompress", 224),
        cdecl("_ICDecompress", 230, "wlSSSS", ic_decompress),
        missing("_ICDrawBegin", 232),
        missing("_ICDraw", 234),
        missing("ICGetDisplayFormat", 239),
        missing("ICImageCompress", 240),
        missing("ICImageDecompress", 241),
        missing("ICCompressorChoose", 242),
        missing("ICCompressorFree", 243),
        missing("ICSeqCompressFrameStart", 244),
        missing("ICSeqCompressFrameEnd", 245),
        missing("ICSeqCompressFrame", 246),
        missing("_MCIWndCreate", 250),
        missing("_MCIWndRegisterClass", 251),
        missing("GetOpenFileNamePreview", 252),
        missing("GetSaveFileNamePreview", 253),
    ],
    library: true,
};

/// VideoForWindowsVersion(): Video for Windows' version, the major and
/// minor numbers in the high word and the build in the low one, of
/// Windows 98 SE's MSVIDEO.DLL (4.03.1998); Windows 95's gives
/// 0x040003B6, as Wine's notes say.
fn vfw_version(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0x0403_07CE)
}

/// AVISTREAMINFO's sizes: the 32-bit one's rcFrame is a RECT of LONGs,
/// the 16-bit one's of WORDs, which moves the fields after it.
const STREAMINFO_32: usize = 140;
const STREAMINFO_16: usize = 132;
/// Where rcFrame is in both.
const RC_FRAME: usize = 52;

/// AVIStreamInfo(pavi, psi, lSize): the stream's AVISTREAMINFO, as much
/// of it as `lSize` bytes hold, in the 16-bit layout.
fn avi_stream_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [stream, at, size] = args[..] else {
        unreachable!()
    };
    let info = match avifil32::stream_info_ansi(cpu, stream) {
        Ok(info) => info,
        Err(e) => return Some(e),
    };
    debug_assert_eq!(info.len(), STREAMINFO_32);
    let mut short = Vec::with_capacity(STREAMINFO_16);
    short.extend_from_slice(&info[..RC_FRAME]);
    for i in 0..4 {
        let at = RC_FRAME + 4 * i;
        short.extend_from_slice(&info[at..at + 2]);
    }
    short.extend_from_slice(&info[RC_FRAME + 16..]);
    Some(avifil32::give_info(cpu, at, size, &short))
}

/// ICINFO's sizes: the 32-bit one's strings are UTF-16, the 16-bit
/// one's ANSI. Both start with six DWORDs, then szName[16],
/// szDescription[128] and szDriver[128].
const ICINFO_32: u32 = 24 + 2 * 272;
const ICINFO_16: u32 = 24 + 272;

/// A block of `size` bytes for the 32-bit side's copy of a structure:
/// its handle and its linear address.
fn scratch(cpu: &mut Cpu, size: u32) -> Option<(u16, u32)> {
    let handle = super::global::alloc(cpu, super::global::GMEM_ZEROINIT, size, 0, false);
    let base = super::global::get(cpu, handle)?.base;
    Some((handle, base))
}

/// The 32-bit ICINFO at `from` written at `to` in the 16-bit layout:
/// the strings to ANSI, those characters outside it '?'.
fn icinfo_to_16(cpu: &mut Cpu, from: u32, to: u32) {
    let Some(wide) = super::peek(cpu, from, ICINFO_32 as usize) else {
        return;
    };
    let mut out = wide[..24].to_vec();
    out[..4].copy_from_slice(&ICINFO_16.to_le_bytes());
    for unit in wide[24..].as_chunks::<2>().0 {
        let c = u16::from_le_bytes(*unit);
        out.push(if c < 0x80 { c as u8 } else { b'?' });
    }
    super::poke(cpu, to, &out);
}

/// ICInfo(fccType, fccHandler, lpicinfo): as MSVFW32's, in a 16-bit
/// ICINFO.
fn ic_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [kind, handler, at] = args[..] else {
        unreachable!()
    };
    if at == 0 {
        return Some(0);
    }
    let Some((block, wide)) = scratch(cpu, ICINFO_32) else {
        return Some(0);
    };
    let found = msvfw32::ic_info(cpu, &[kind, handler, wide]).unwrap_or(0);
    if found != 0 {
        icinfo_to_16(cpu, wide, at);
    }
    super::global::free(cpu, block);
    Some(found)
}

/// ICGetInfo(hic, lpicinfo, cb): as MSVFW32's, in a 16-bit ICINFO; its
/// size, or 0, or MSVFW32's error (ICERR_BADHANDLE for a handle that
/// isn't open).
fn ic_get_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hic, at, size] = args[..] else {
        unreachable!()
    };
    if at == 0 || size < ICINFO_16 {
        return Some(0);
    }
    let Some((block, wide)) = scratch(cpu, ICINFO_32) else {
        return Some(0);
    };
    let got = msvfw32::ic_get_info(cpu, &[hic, wide, ICINFO_32]).unwrap_or(0);
    let result = info_to_16(cpu, got, wide, at);
    super::global::free(cpu, block);
    Some(result)
}

/// What a 16-bit ICGetInfo or ICM_GETINFO gives for MSVFW32's answer
/// `got`, with the 32-bit ICINFO it filled at `wide`: a whole ICINFO
/// written at `at` in the 16-bit layout and its size, or the answer as
/// it is (0, or an error such as ICERR_BADHANDLE), with nothing
/// written.
fn info_to_16(cpu: &mut Cpu, got: u32, wide: u32, at: u32) -> u32 {
    if got != ICINFO_32 {
        return got;
    }
    icinfo_to_16(cpu, wide, at);
    ICINFO_16
}

/// The messages whose parameters are converted.
const ICM_GETINFO: u32 = 0x5002;
const ICM_DECOMPRESS: u32 = 0x400D;
const ICM_DECOMPRESSEX_BEGIN: u32 = 0x403C;
const ICM_DECOMPRESSEX_QUERY: u32 = 0x403D;
const ICM_DECOMPRESSEX: u32 = 0x403E;
/// Those whose two parameters are pointers to BITMAPINFOs, or the
/// second 0: ICM_COMPRESS_BEGIN, _GET_FORMAT, _GET_SIZE and _QUERY,
/// and ICM_DECOMPRESS_GET_FORMAT, _QUERY, _BEGIN, _SET_PALETTE and
/// _GET_PALETTE.
const TWO_FORMATS: [u32; 9] = [
    0x4007, 0x4004, 0x4005, 0x4006, 0x400A, 0x400B, 0x400C, 0x401D, 0x401E,
];
/// ICDECOMPRESS: dwFlags, then lpbiInput, lpInput, lpbiOutput and
/// lpOutput, then ckid.
const ICDECOMPRESS: u32 = 24;
/// ICDECOMPRESSEX: dwFlags and four pointers, then eight coordinates,
/// INT16s in the 16-bit one and INTs in the 32-bit one.
const ICDECOMPRESSEX_16: u32 = 20 + 16;
const ICDECOMPRESSEX_32: u32 = 20 + 32;
/// ICERR_BADPARAM.
const ICERR_BADPARAM: u32 = -6i32 as u32;

/// ICSendMessage(hic, msg, dw1, dw2): the message to MSVFW32's codec,
/// its parameters' 16:16 pointers made flat and the structures they
/// point to converted.
fn ic_send_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hic, msg, dw1, dw2] = args[..] else {
        unreachable!()
    };
    let first = super::linear(cpu, dw1);
    Some(send(cpu, hic, msg, first, dw2))
}

/// _ICMessage(hic, msg, cb, ...): the `cb` bytes of arguments after
/// `cb` sent to the codec as a structure, as ICSendMessage(hic, msg,
/// &args, cb), as the ICDecompressEx and ICDecompressExQuery macros
/// send theirs.
fn ic_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hic, msg, size] = args[..] else {
        unreachable!()
    };
    // The far return address and the three words, then the structure.
    let base = cpu.seg_cache(Seg::SS).base;
    let at = base.wrapping_add(cpu.sp() as u32 + 10);
    Some(send(cpu, hic, msg, at, size))
}

/// _ICDecompress(hic, dwFlags, lpbiFormat, lpData, lpbi, lpBits):
/// ICM_DECOMPRESS with an ICDECOMPRESS of these.
fn ic_decompress(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hic, flags, input, data, output, bits] = args[..] else {
        unreachable!()
    };
    let flat = [
        hic,
        flags,
        super::linear(cpu, input),
        super::linear(cpu, data),
        super::linear(cpu, output),
        super::linear(cpu, bits),
    ];
    msvfw32::ic_decompress(cpu, &flat)
}

/// Send `msg` to the codec `hic`: `first` is the linear address the
/// first parameter points to, `second` the second parameter as the
/// 16-bit program gave it.
fn send(cpu: &mut Cpu, hic: u32, msg: u32, first: u32, second: u32) -> u32 {
    let call = |cpu: &mut Cpu, a: u32, b: u32| {
        msvfw32::ic_send_message(cpu, &[hic, msg, a, b]).unwrap_or(0)
    };
    match msg {
        ICM_GETINFO => {
            if first == 0 {
                return ICINFO_16;
            }
            if second < ICINFO_16 {
                return 0;
            }
            let Some((block, wide)) = scratch(cpu, ICINFO_32) else {
                return 0;
            };
            let got = call(cpu, wide, ICINFO_32);
            let result = info_to_16(cpu, got, wide, first);
            super::global::free(cpu, block);
            result
        }
        ICM_DECOMPRESS => {
            let Some(fields) = super::peek(cpu, first, ICDECOMPRESS as usize) else {
                return ICERR_BADPARAM;
            };
            let field = |n: usize| {
                let b = &fields[4 * n..4 * n + 4];
                u32::from_le_bytes([b[0], b[1], b[2], b[3]])
            };
            let mut out = Vec::with_capacity(ICDECOMPRESS as usize);
            for n in 0..6 {
                let v = if (1..=4).contains(&n) {
                    super::linear(cpu, field(n))
                } else {
                    field(n)
                };
                out.extend_from_slice(&v.to_le_bytes());
            }
            with_copy(cpu, &out, |cpu, at| call(cpu, at, ICDECOMPRESS))
        }
        ICM_DECOMPRESSEX_BEGIN | ICM_DECOMPRESSEX_QUERY | ICM_DECOMPRESSEX => {
            let Some(out) = decompress_ex(cpu, first) else {
                return ICERR_BADPARAM;
            };
            with_copy(cpu, &out, |cpu, at| call(cpu, at, ICDECOMPRESSEX_32))
        }
        m if TWO_FORMATS.contains(&m) => {
            let second = super::linear(cpu, second);
            call(cpu, first, second)
        }
        _ => call(cpu, first, second),
    }
}

/// The 16-bit ICDECOMPRESSEX at `at` in the 32-bit layout: its pointers
/// flat and its coordinates widened.
fn decompress_ex(cpu: &mut Cpu, at: u32) -> Option<Vec<u8>> {
    let fields = super::peek(cpu, at, ICDECOMPRESSEX_16 as usize)?;
    let mut out = Vec::with_capacity(ICDECOMPRESSEX_32 as usize);
    out.extend_from_slice(&fields[..4]);
    for n in 1..5 {
        let b = &fields[4 * n..4 * n + 4];
        let v = super::linear(cpu, u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        out.extend_from_slice(&v.to_le_bytes());
    }
    for c in fields[20..].as_chunks::<2>().0 {
        let v = i16::from_le_bytes(*c) as i32;
        out.extend_from_slice(&v.to_le_bytes());
    }
    Some(out)
}

/// Run `f` with `data` copied to a block of its own, at the block's
/// linear address; ICERR_BADPARAM when there is no memory for it.
fn with_copy(cpu: &mut Cpu, data: &[u8], f: impl FnOnce(&mut Cpu, u32) -> u32) -> u32 {
    let Some((block, at)) = scratch(cpu, data.len() as u32) else {
        return ICERR_BADPARAM;
    };
    super::poke(cpu, at, data);
    let result = f(cpu, at);
    super::global::free(cpu, block);
    result
}
