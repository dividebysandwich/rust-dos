//! AVIFIL32.DLL: AVI files and their streams, read only. AVIFileOpenA
//! reads a file's headers and index (avi.rs) and gives an IAVIFile,
//! whose streams are IAVIStreams; the AVIFile and AVIStream functions go
//! to their methods. The interfaces' behaviour follows Wine's avifile.c
//! and api.c (LGPL 2.1 or later):
//!
//! Copyright 1999 Marcus Meissner
//! Copyright 2002-2003 Michael Günnewig
//!
//! A file's streams count as references to it, as in Wine: the file is
//! closed when the last of its interfaces is released. Writing, editing
//! and the clipboard functions aren't there; a file opened for writing
//! fails with AVIERR_UNSUPPORTED.
//!
//! AVIStreamGetFrameOpen gives an IGetFrame, which decompresses a video
//! stream's frames to a DIB of its own, as Wine's getframe.c does. It
//! takes 16, 24 or 32 bits a pixel; the default is 24.

use super::avi::{self, Avi};
use super::codec::{self, Codec, dib_stride};
use crate::cpu::Cpu;
use crate::win32::com::{self, Guid};
use crate::win32::kernel32;
use crate::win32::stubs::{self, Api, Conv, Dll, Vtable};

pub static DLL: Dll = Dll {
    name: "AVIFIL32.DLL",
    base: 0xBF78_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: VTABLES,
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

/// The functions, then the interfaces' methods. Append only (see
/// `stubs`).
static APIS: &[Api] = &[
    stdcall("AVIFileInit", 1, 0, nothing),
    stdcall("AVIFileExit", 2, 0, nothing),
    stdcall("AVIFileOpenA", 3, 4, avi_file_open),
    stdcall("AVIFileOpen", 4, 4, avi_file_open),
    stdcall("AVIFileAddRef", 5, 1, file_add_ref),
    stdcall("AVIFileRelease", 6, 1, file_release),
    stdcall("AVIFileGetStream", 7, 4, file_get_stream),
    stdcall("AVIFileInfoA", 8, 3, avi_file_info_a),
    stdcall("AVIFileInfo", 9, 3, avi_file_info_a),
    stdcall("AVIStreamAddRef", 10, 1, stream_add_ref),
    stdcall("AVIStreamRelease", 11, 1, stream_release),
    stdcall("AVIStreamInfoA", 12, 3, avi_stream_info_a),
    stdcall("AVIStreamInfo", 13, 3, avi_stream_info_a),
    stdcall("AVIStreamReadFormat", 14, 4, stream_read_format),
    stdcall("AVIStreamRead", 15, 7, stream_read),
    stdcall("AVIStreamStart", 16, 1, avi_stream_start),
    stdcall("AVIStreamLength", 17, 1, avi_stream_length),
    stdcall("AVIStreamBeginStreaming", 18, 4, avi_stream_streaming),
    stdcall("AVIStreamEndStreaming", 19, 1, avi_stream_streaming),
    stdcall("AVIStreamSampleToTime", 20, 2, avi_stream_sample_to_time),
    stdcall("AVIStreamTimeToSample", 21, 2, avi_stream_time_to_sample),
    stdcall("AVIStreamFindSample", 22, 3, stream_find_sample),
    stdcall("IAVIFile_QueryInterface", 0, 3, file_query_interface),
    stdcall("IAVIFile_AddRef", 0, 1, file_add_ref),
    stdcall("IAVIFile_Release", 0, 1, file_release),
    stdcall("IAVIFile_Info", 0, 3, file_info_w),
    stdcall("IAVIFile_GetStream", 0, 4, file_get_stream),
    stdcall("IAVIFile_CreateStream", 0, 3, read_only),
    stdcall("IAVIFile_WriteData", 0, 4, read_only),
    stdcall("IAVIFile_ReadData", 0, 4, no_data),
    stdcall("IAVIFile_EndRecord", 0, 1, read_only),
    stdcall("IAVIFile_DeleteStream", 0, 3, read_only),
    stdcall("IAVIStream_QueryInterface", 0, 3, stream_query_interface),
    stdcall("IAVIStream_AddRef", 0, 1, stream_add_ref),
    stdcall("IAVIStream_Release", 0, 1, stream_release),
    stdcall("IAVIStream_Create", 0, 3, unsupported),
    stdcall("IAVIStream_Info", 0, 3, stream_info_w),
    stdcall("IAVIStream_FindSample", 0, 3, stream_find_sample),
    stdcall("IAVIStream_ReadFormat", 0, 4, stream_read_format),
    stdcall("IAVIStream_SetFormat", 0, 4, read_only),
    stdcall("IAVIStream_Read", 0, 7, stream_read),
    stdcall("IAVIStream_Write", 0, 8, read_only),
    stdcall("IAVIStream_Delete", 0, 3, read_only),
    stdcall("IAVIStream_ReadData", 0, 4, no_data),
    stdcall("IAVIStream_WriteData", 0, 4, read_only),
    stdcall("IAVIStream_SetInfo", 0, 3, read_only),
    stdcall("AVIStreamGetFrameOpen", 23, 2, avi_stream_get_frame_open),
    stdcall("AVIStreamGetFrame", 24, 2, get_frame_get),
    stdcall("AVIStreamGetFrameClose", 25, 1, get_frame_release),
    stdcall("IGetFrame_QueryInterface", 0, 3, get_frame_query_interface),
    stdcall("IGetFrame_AddRef", 0, 1, get_frame_add_ref),
    stdcall("IGetFrame_Release", 0, 1, get_frame_release),
    stdcall("IGetFrame_GetFrame", 0, 2, get_frame_get),
    stdcall("IGetFrame_Begin", 0, 4, get_frame_nothing),
    stdcall("IGetFrame_End", 0, 1, get_frame_nothing),
    stdcall("IGetFrame_SetFormat", 0, 7, get_frame_set_format),
];

static VTABLES: &[Vtable] = &[
    Vtable {
        name: "IAVIFile",
        methods: &[
            "IAVIFile_QueryInterface",
            "IAVIFile_AddRef",
            "IAVIFile_Release",
            "IAVIFile_Info",
            "IAVIFile_GetStream",
            "IAVIFile_CreateStream",
            "IAVIFile_WriteData",
            "IAVIFile_ReadData",
            "IAVIFile_EndRecord",
            "IAVIFile_DeleteStream",
        ],
    },
    Vtable {
        name: "IAVIStream",
        methods: &[
            "IAVIStream_QueryInterface",
            "IAVIStream_AddRef",
            "IAVIStream_Release",
            "IAVIStream_Create",
            "IAVIStream_Info",
            "IAVIStream_FindSample",
            "IAVIStream_ReadFormat",
            "IAVIStream_SetFormat",
            "IAVIStream_Read",
            "IAVIStream_Write",
            "IAVIStream_Delete",
            "IAVIStream_ReadData",
            "IAVIStream_WriteData",
            "IAVIStream_SetInfo",
        ],
    },
    Vtable {
        name: "IGetFrame",
        methods: &[
            "IGetFrame_QueryInterface",
            "IGetFrame_AddRef",
            "IGetFrame_Release",
            "IGetFrame_GetFrame",
            "IGetFrame_Begin",
            "IGetFrame_End",
            "IGetFrame_SetFormat",
        ],
    },
];

const IAVIFILE: usize = 0;
const IAVISTREAM: usize = 1;
const IGETFRAME: usize = 2;

const fn guid_0002(d1: u32) -> Guid {
    Guid::new(d1, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46])
}
const IID_IAVIFILE: Guid = guid_0002(0x0002_0020);
const IID_IAVISTREAM: Guid = guid_0002(0x0002_0021);
const IID_IGETFRAME: Guid = guid_0002(0x0002_0023);

/// AVIERR_*: MAKE_SCODE(SEVERITY_ERROR, FACILITY_ITF, 0x4000 + n).
const fn avierr(n: u32) -> u32 {
    0x8004_4000 + n
}
const AVIERR_OK: u32 = 0;
const AVIERR_UNSUPPORTED: u32 = avierr(101);
const AVIERR_BADFORMAT: u32 = avierr(102);
const AVIERR_MEMORY: u32 = avierr(103);
const AVIERR_BADPARAM: u32 = avierr(106);
const AVIERR_BADSIZE: u32 = avierr(107);
const AVIERR_BADHANDLE: u32 = avierr(108);
const AVIERR_FILEREAD: u32 = avierr(109);
const AVIERR_FILEOPEN: u32 = avierr(111);
const AVIERR_READONLY: u32 = avierr(114);
const AVIERR_NODATA: u32 = avierr(115);
const AVIERR_BUFFERTOOSMALL: u32 = avierr(116);

/// OpenFile's modes that write.
const OF_WRITE: u32 = 0x0001;
const OF_READWRITE: u32 = 0x0002;
const OF_CREATE: u32 = 0x1000;

/// AVIFILECAPS_CANREAD, and the streams' types.
const AVIFILECAPS_CANREAD: u32 = 1;
const STREAMTYPE_VIDEO: u32 = u32::from_le_bytes(*b"vids");
const STREAMTYPE_AUDIO: u32 = u32::from_le_bytes(*b"auds");

/// AVISTREAMINFOA's and AVISTREAMINFOW's sizes; their names start at 76.
const AVISTREAMINFOA: usize = 140;
const AVISTREAMINFOW: usize = 204;
/// AVIFILEINFOA's and AVIFILEINFOW's; their file types start at 40.
const AVIFILEINFOA: usize = 104;
const AVIFILEINFOW: usize = 168;

/// AVIStreamRead's AVISTREAMREAD_CONVENIENT: as many samples as fit.
const AVISTREAMREAD_CONVENIENT: i32 = -1;

/// FindSample's flags: FIND_PREV, FIND_NEXT, FIND_FROM_START, and what
/// to find: FIND_KEY, FIND_ANY.
const FIND_DIR: u32 = 0x0F;
const FIND_NEXT: u32 = 0x01;
const FIND_PREV: u32 = 0x04;
const FIND_FROM_START: u32 = 0x08;
const FIND_TYPE: u32 = 0xF0;
const FIND_KEY: u32 = 0x10;
const FIND_ANY: u32 = 0x20;
const FIND_RET: u32 = 0xF000;
const FIND_POS: u32 = 0x0000;

/// BITMAPINFOHEADER's size, and BI_RGB.
const BITMAPINFOHEADER: usize = 40;
const BI_RGB: u32 = 0;

/// AVIStreamGetFrameOpen's AVIGETFRAMEF_BESTDISPLAYFMT, which comes as
/// the pointer to the wanted format.
const BESTDISPLAYFMT: u32 = 1;

/// An open AVI file.
#[derive(Clone, Debug, Default)]
pub struct AviFile {
    /// The file's handle, its headers and index, and its name.
    handle: u32,
    avi: Avi,
    name: String,
    /// References to it and to its streams.
    refs: u32,
    /// Its IAVIFile, and the IAVIStream of each stream once asked for.
    iface: u32,
    streams: Vec<u32>,
}

crate::state_fields!(AviFile {
    handle,
    avi,
    name,
    refs,
    iface,
    streams
});

fn dll_index() -> usize {
    stubs::find_dll(DLL.name).expect("AVIFIL32 is a built-in DLL")
}

fn files(cpu: &mut Cpu) -> &mut Vec<Option<AviFile>> {
    &mut crate::win32::process(cpu).vfw.files
}

/// The file an interface pointer is of, and the stream it is (None for
/// the file's own interface).
fn object(cpu: &mut Cpu, this: u32) -> Option<(usize, Option<usize>)> {
    let (vtable, id) = com::interface_of(cpu, dll_index(), this)?;
    if vtable == IGETFRAME {
        return None;
    }
    let slot = (id as usize).checked_sub(1)?;
    files(cpu).get(slot)?.as_ref()?;
    let stream = (vtable == IAVISTREAM).then(|| cpu.bus.guest_read_32(this + com::HEADER) as usize);
    Some((slot, stream))
}

fn file(cpu: &mut Cpu, slot: usize) -> &mut AviFile {
    files(cpu)[slot].as_mut().expect("an open AVI file")
}

fn write_32(cpu: &mut Cpu, at: u32, value: u32) {
    if at != 0 && cpu.bus.guest_probe(at, 4, true) {
        cpu.bus.guest_write_32(at, value);
    }
}

fn nothing(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn unsupported(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(AVIERR_UNSUPPORTED)
}

fn read_only(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(AVIERR_READONLY)
}

/// ReadData: a file's or stream's extra chunks, which aren't kept.
fn no_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    write_32(cpu, args[3], 0);
    Some(AVIERR_NODATA)
}

/// An IGetFrame: a video stream's frames as DIBs.
#[derive(Clone, Debug, Default)]
pub struct GetFrame {
    /// The file and stream, of which it holds a reference.
    file: usize,
    stream: usize,
    iface: u32,
    refs: u32,
    /// The source's compression (BI_RGB for none) and size.
    compression: u32,
    width: u32,
    height: u32,
    /// The bits a pixel of the DIB given out, its header's bytes (with
    /// the palette of an uncompressed source) and where it is.
    bits: u16,
    header: u32,
    buffer: u32,
    /// The last sample decoded, or -1.
    last: i64,
    codec: Codec,
}

crate::state_fields!(GetFrame {
    file,
    stream,
    iface,
    refs,
    compression,
    width,
    height,
    bits,
    header,
    buffer,
    last
} skip {
    codec
});

/// `count` bytes of the file `handle` at `offset`, or None when they
/// can't be read.
fn read_at(cpu: &mut Cpu, handle: u32, offset: u32, count: u32) -> Option<Vec<u8>> {
    kernel32::seek_file(cpu, handle, offset as i64, 0).ok()?;
    let mut bytes = Vec::with_capacity(count as usize);
    while (bytes.len() as u32) < count {
        let chunk = kernel32::read_file(cpu, handle, count - bytes.len() as u32).ok()?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes)
}

/// AVIFileOpenA(file, name, mode, handler): an IAVIFile of the AVI file
/// `name`, read only.
fn avi_file_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[out, name, mode, _] = args else {
        unreachable!()
    };
    if out == 0 || name == 0 {
        return Some(AVIERR_BADPARAM);
    }
    write_32(cpu, out, 0);
    if mode & (OF_WRITE | OF_READWRITE | OF_CREATE) != 0 {
        return Some(AVIERR_UNSUPPORTED);
    }
    let path = crate::win32::user32::string_at(cpu, name)?;
    let Ok(handle) = kernel32::open_handle(
        cpu,
        &path,
        kernel32::GENERIC_READ,
        kernel32::FILE_SHARE_READ,
        kernel32::OPEN_EXISTING,
    ) else {
        return Some(AVIERR_FILEOPEN);
    };
    let len = kernel32::seek_file(cpu, handle, 0, 2).unwrap_or(0);
    let parsed = avi::parse(len, &mut |at, n| read_at(cpu, handle, at, n));
    let avi = match parsed {
        Ok(avi) => avi,
        Err(e) => {
            kernel32::files::close(cpu, handle);
            return Some(match e {
                avi::Error::Read => AVIERR_FILEREAD,
                avi::Error::Format => AVIERR_BADFORMAT,
            });
        }
    };
    let streams = vec![0; avi.streams.len()];
    let file_name = path.rsplit(['\\', '/']).next().unwrap_or("").to_string();
    let list = files(cpu);
    let slot = list.iter().position(Option::is_none).unwrap_or(list.len());
    if slot == list.len() {
        list.push(None);
    }
    let Some(iface) = com::new_interface(cpu, dll_index(), IAVIFILE, slot as u32 + 1, 0) else {
        kernel32::files::close(cpu, handle);
        return Some(AVIERR_MEMORY);
    };
    files(cpu)[slot] = Some(AviFile {
        handle,
        avi,
        name: file_name,
        refs: 1,
        iface,
        streams,
    });
    write_32(cpu, out, iface);
    Some(AVIERR_OK)
}

/// AddRef of a file or a stream: the count of the file's references.
fn add_ref(cpu: &mut Cpu, this: u32) -> u32 {
    let Some((slot, _)) = object(cpu, this) else {
        return 0;
    };
    let f = file(cpu, slot);
    f.refs += 1;
    f.refs
}

/// Release of a file or a stream: the file is closed and its interfaces
/// freed when the count reaches 0.
fn release(cpu: &mut Cpu, this: u32) -> u32 {
    let Some((slot, _)) = object(cpu, this) else {
        return 0;
    };
    let f = file(cpu, slot);
    f.refs -= 1;
    if f.refs > 0 {
        return f.refs;
    }
    let f = files(cpu)[slot].take().expect("an open AVI file");
    kernel32::files::close(cpu, f.handle);
    for iface in std::iter::once(f.iface).chain(f.streams.iter().copied()) {
        if iface != 0 {
            com::free_interface(cpu, iface);
        }
    }
    0
}

fn file_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(add_ref(cpu, args[0]))
}

fn file_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(release(cpu, args[0]))
}

fn stream_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(add_ref(cpu, args[0]))
}

fn stream_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(release(cpu, args[0]))
}

/// QueryInterface of a file: IUnknown and IAVIFile are it.
fn file_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    query_interface(cpu, args, IID_IAVIFILE)
}

/// QueryInterface of a stream: IUnknown and IAVIStream are it.
fn stream_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    query_interface(cpu, args, IID_IAVISTREAM)
}

fn query_interface(cpu: &mut Cpu, args: &[u32], own: Guid) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(com::E_POINTER);
    };
    if out == 0 {
        return Some(com::E_POINTER);
    }
    if iid == com::IID_IUNKNOWN || iid == own {
        add_ref(cpu, this);
        write_32(cpu, out, this);
        return Some(com::S_OK);
    }
    write_32(cpu, out, 0);
    Some(com::E_NOINTERFACE)
}

/// AVIFileGetStream and IAVIFile::GetStream(file, out, type, n): the
/// IAVIStream of the `n`th stream of type `type` (of any type for 0).
fn file_get_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out, kind, n] = args else {
        unreachable!()
    };
    if out == 0 {
        return Some(AVIERR_BADPARAM);
    }
    write_32(cpu, out, 0);
    let Some((slot, None)) = object(cpu, this) else {
        return Some(AVIERR_BADHANDLE);
    };
    let found = file(cpu, slot)
        .avi
        .streams
        .iter()
        .enumerate()
        .filter(|(_, s)| kind == 0 || s.kind() == kind)
        .nth(n as usize)
        .map(|(i, _)| i);
    let Some(stream) = found else {
        return Some(AVIERR_NODATA);
    };
    let mut iface = file(cpu, slot).streams[stream];
    if iface == 0 {
        let Some(new) = com::new_interface(cpu, dll_index(), IAVISTREAM, slot as u32 + 1, 4)
        else {
            return Some(AVIERR_MEMORY);
        };
        cpu.bus.guest_write_32(new + com::HEADER, stream as u32);
        file(cpu, slot).streams[stream] = new;
        iface = new;
    }
    file(cpu, slot).refs += 1;
    write_32(cpu, out, iface);
    Some(AVIERR_OK)
}

/// The stream an IAVIStream is, as (file, stream).
fn stream(cpu: &mut Cpu, this: u32) -> Option<(usize, usize)> {
    match object(cpu, this)? {
        (slot, Some(s)) if s < file(cpu, slot).avi.streams.len() => Some((slot, s)),
        _ => None,
    }
}

/// The stream's AVISTREAMINFO, as Wine's handler fills it from the
/// stream's header, its name in `wide` or ANSI characters.
fn stream_info(cpu: &mut Cpu, slot: usize, n: usize, wide: bool) -> Vec<u8> {
    let f = file(cpu, slot);
    let s = &f.avi.streams[n];
    let h = &s.header;
    let mut info = vec![0u8; if wide { AVISTREAMINFOW } else { AVISTREAMINFOA }];
    let put = |info: &mut Vec<u8>, at: usize, v: u32| {
        info[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };
    let field = |at: usize| u32::from_le_bytes([h[at], h[at + 1], h[at + 2], h[at + 3]]);
    put(&mut info, 0, field(0));
    put(&mut info, 4, field(4));
    put(&mut info, 8, field(8));
    put(&mut info, 12, AVIFILECAPS_CANREAD);
    info[16..20].copy_from_slice(&h[12..16]);
    put(&mut info, 20, field(20));
    put(&mut info, 24, field(24));
    put(&mut info, 28, field(28));
    put(&mut info, 32, field(32));
    put(&mut info, 36, field(16));
    let largest = s.chunks.iter().map(|c| c.size).max().unwrap_or(0);
    put(&mut info, 40, largest);
    put(&mut info, 44, field(40));
    put(&mut info, 48, field(44));
    // rcFrame, from the header's 16-bit one, or the frame's size.
    let short = |at: usize| i16::from_le_bytes([h[at], h[at + 1]]) as i32;
    let mut rect = [short(48), short(50), short(52), short(54)];
    if (rect[2] <= rect[0] || rect[3] <= rect[1]) && s.kind() == STREAMTYPE_VIDEO {
        let fmt = &s.format;
        if fmt.len() >= 12 {
            let w = i32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
            let h = i32::from_le_bytes([fmt[8], fmt[9], fmt[10], fmt[11]]);
            rect = [0, 0, w, h.abs()];
        }
    }
    for (i, v) in rect.iter().enumerate() {
        put(&mut info, 52 + 4 * i, *v as u32);
    }
    // The name: the file's strn, or "file Video #1" as Wine makes it.
    let name = if s.name.is_empty() {
        let kind = s.kind();
        let count = f.avi.streams[..=n].iter().filter(|o| o.kind() == kind).count();
        let what = match kind {
            STREAMTYPE_VIDEO => "Video".to_string(),
            STREAMTYPE_AUDIO => "Audio".to_string(),
            k => String::from_utf8_lossy(&k.to_le_bytes()).into_owned(),
        };
        format!("{} {} #{}", f.name, what, count)
    } else {
        s.name.clone()
    };
    if wide {
        for (i, c) in name.encode_utf16().take(63).enumerate() {
            info[76 + 2 * i..78 + 2 * i].copy_from_slice(&c.to_le_bytes());
        }
    } else {
        let bytes = crate::dosstr::to_bytes(&name);
        let len = bytes.len().min(63);
        info[76..76 + len].copy_from_slice(&bytes[..len]);
    }
    info
}

/// Write `info` at `at`, as much of it as `size` bytes hold.
fn give_info(cpu: &mut Cpu, at: u32, size: u32, info: &[u8]) -> u32 {
    if (size as i32) < 0 {
        return AVIERR_BADSIZE;
    }
    let n = (size as usize).min(info.len());
    if at == 0 || !cpu.bus.guest_probe(at, n, true) {
        return AVIERR_BADPARAM;
    }
    cpu.bus.guest_write_bytes(at, &info[..n]);
    AVIERR_OK
}

/// AVIStreamInfoA(stream, info, size).
fn avi_stream_info_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((slot, n)) = stream(cpu, args[0]) else {
        return Some(AVIERR_BADHANDLE);
    };
    let info = stream_info(cpu, slot, n, false);
    Some(give_info(cpu, args[1], args[2], &info))
}

/// IAVIStream::Info(stream, info, size), with AVISTREAMINFOW.
fn stream_info_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((slot, n)) = stream(cpu, args[0]) else {
        return Some(AVIERR_BADHANDLE);
    };
    let info = stream_info(cpu, slot, n, true);
    Some(give_info(cpu, args[1], args[2], &info))
}

/// The file's AVIFILEINFO, as Wine's handler fills it from the main
/// header, its type's name in `wide` or ANSI characters.
fn file_info(cpu: &mut Cpu, slot: usize, wide: bool) -> Vec<u8> {
    let f = file(cpu, slot);
    let m = &f.avi.main;
    let field = |at: usize| u32::from_le_bytes([m[at], m[at + 1], m[at + 2], m[at + 3]]);
    let mut info = vec![0u8; if wide { AVIFILEINFOW } else { AVIFILEINFOA }];
    // MaxBytesPerSec, Flags, Caps, Streams, SuggestedBufferSize, Width,
    // Height, Scale, Rate, Length.
    let largest = f
        .avi
        .streams
        .iter()
        .flat_map(|s| s.chunks.iter().map(|c| c.size))
        .max()
        .unwrap_or(0);
    let values = [
        field(4),
        field(12),
        AVIFILECAPS_CANREAD,
        f.avi.streams.len() as u32,
        largest,
        field(32),
        field(36),
        1_000_000,
        field(0),
        field(16),
    ];
    for (i, v) in values.iter().enumerate() {
        info[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    let kind = "AVI file";
    if wide {
        for (i, c) in kind.encode_utf16().enumerate() {
            info[44 + 2 * i..46 + 2 * i].copy_from_slice(&c.to_le_bytes());
        }
    } else {
        info[44..44 + kind.len()].copy_from_slice(kind.as_bytes());
    }
    info
}

/// AVIFileInfoA(file, info, size).
fn avi_file_info_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((slot, None)) = object(cpu, args[0]) else {
        return Some(AVIERR_BADHANDLE);
    };
    let info = file_info(cpu, slot, false);
    Some(give_info(cpu, args[1], args[2], &info))
}

/// IAVIFile::Info(file, info, size), with AVIFILEINFOW.
fn file_info_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((slot, None)) = object(cpu, args[0]) else {
        return Some(AVIERR_BADHANDLE);
    };
    let info = file_info(cpu, slot, true);
    Some(give_info(cpu, args[1], args[2], &info))
}

/// AVIStreamReadFormat and IAVIStream::ReadFormat(stream, position,
/// format, size): the stream's format, or its size with no buffer.
fn stream_read_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, _, format, size] = args else {
        unreachable!()
    };
    let Some((slot, n)) = stream(cpu, this) else {
        return Some(AVIERR_BADHANDLE);
    };
    if size == 0 || !cpu.bus.guest_probe(size, 4, true) {
        return Some(AVIERR_BADPARAM);
    }
    let bytes = file(cpu, slot).avi.streams[n].format.clone();
    let room = cpu.bus.guest_read_32(size);
    cpu.bus.guest_write_32(size, bytes.len() as u32);
    if format == 0 {
        return Some(AVIERR_OK);
    }
    let n = (room as usize).min(bytes.len());
    if !cpu.bus.guest_probe(format, n, true) {
        return Some(AVIERR_BADPARAM);
    }
    cpu.bus.guest_write_bytes(format, &bytes[..n]);
    Some(if n < bytes.len() {
        AVIERR_BUFFERTOOSMALL
    } else {
        AVIERR_OK
    })
}

/// AVIStreamRead and IAVIStream::Read(stream, start, samples, buffer,
/// size, bytes, read): samples from `start`, as Wine reads them. A
/// stream of fixed-size samples reads across its chunks, one of
/// variable-size samples one sample, its chunk whole. With no buffer,
/// how much would be read.
fn stream_read(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, start, samples, buffer, size, bytes_out, samples_out] = args else {
        unreachable!()
    };
    write_32(cpu, bytes_out, 0);
    write_32(cpu, samples_out, 0);
    let Some((slot, n)) = stream(cpu, this) else {
        return Some(AVIERR_BADHANDLE);
    };
    let s = &file(cpu, slot).avi.streams[n];
    let (first, length, sample_size) = (s.start(), s.length(), s.sample_size());
    let start = start as i32;
    if start < first as i32 || start as u32 > first.saturating_add(length) {
        return Some(AVIERR_NODATA);
    }
    let mut samples = samples as i32;
    if samples == AVISTREAMREAD_CONVENIENT {
        if bytes_out == 0 && samples_out == 0 {
            return Some(AVIERR_BADPARAM);
        }
        samples = size.checked_div(sample_size).map_or(1, |n| n as i32);
    }
    let from = (start as u32 - first) as i64;
    let samples = (samples.max(0) as i64).min(length as i64 - from).max(0) as u32;
    if samples == 0 {
        return Some(AVIERR_OK);
    }
    let handle = file(cpu, slot).handle;
    if sample_size != 0 {
        if buffer == 0 {
            write_32(cpu, bytes_out, samples * sample_size);
            write_32(cpu, samples_out, samples);
            return Some(AVIERR_OK);
        }
        let Some((mut chunk, mut offset)) = file(cpu, slot).avi.streams[n].locate(from as u32)
        else {
            return Some(AVIERR_NODATA);
        };
        let mut want = samples as u64 * sample_size as u64;
        let mut room = size as u64;
        let (mut at, mut done) = (buffer, 0u32);
        while want > 0 && room > 0 {
            let Some(c) = file(cpu, slot).avi.streams[n].chunks.get(chunk).copied() else {
                break;
            };
            let take = want.min(room).min((c.size - offset) as u64) as u32;
            let data = read_at(cpu, handle, c.offset + offset, take)?;
            if data.len() as u32 != take {
                return Some(AVIERR_FILEREAD);
            }
            if !cpu.bus.guest_probe(at, take as usize, true) {
                return Some(AVIERR_BADPARAM);
            }
            cpu.bus.guest_write_bytes(at, &data);
            at += take;
            done += take;
            want -= take as u64;
            room -= take as u64;
            chunk += 1;
            offset = 0;
        }
        write_32(cpu, bytes_out, done);
        write_32(cpu, samples_out, done / sample_size);
        return Some(if want == 0 {
            AVIERR_OK
        } else {
            AVIERR_BUFFERTOOSMALL
        });
    }
    // One sample of its own size.
    let Some(c) = file(cpu, slot).avi.streams[n].chunks.get(from as usize).copied() else {
        return Some(AVIERR_NODATA);
    };
    if buffer != 0 {
        if size < c.size {
            return Some(AVIERR_BUFFERTOOSMALL);
        }
        let data = read_at(cpu, handle, c.offset, c.size)?;
        if data.len() as u32 != c.size {
            return Some(AVIERR_FILEREAD);
        }
        if !cpu.bus.guest_probe(buffer, data.len(), true) {
            return Some(AVIERR_BADPARAM);
        }
        cpu.bus.guest_write_bytes(buffer, &data);
    }
    write_32(cpu, bytes_out, c.size);
    write_32(cpu, samples_out, 1);
    Some(AVIERR_OK)
}

/// IAVIStream::FindSample and AVIStreamFindSample(stream, position,
/// flags): the nearest key (or any, or non-empty) sample from
/// `position` the way the flags say, or -1.
fn stream_find_sample(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, position, flags] = args else {
        unreachable!()
    };
    let Some((slot, n)) = stream(cpu, this) else {
        return Some(u32::MAX);
    };
    if flags & FIND_RET != FIND_POS {
        return Some(u32::MAX);
    }
    let s = &file(cpu, slot).avi.streams[n];
    let first = s.start() as i64;
    let len = s.chunks.len() as i64;
    let fits = |i: i64| {
        let c = &s.chunks[i as usize];
        match flags & FIND_TYPE {
            FIND_KEY => c.flags & avi::AVIIF_KEYFRAME != 0,
            FIND_ANY => true,
            _ => c.size != 0,
        }
    };
    if s.sample_size() != 0 {
        // Every sample of fixed size is a key one.
        let p = position as i32 as i64;
        let end = first + s.length() as i64;
        return Some(if p >= first && p < end { p as u32 } else { u32::MAX });
    }
    // FIND_FROM_START searches forward from the stream's first sample,
    // as in Wine.
    let p = if flags & FIND_FROM_START != 0 {
        0
    } else {
        position as i32 as i64 - first
    };
    let found = match flags & FIND_DIR {
        d if d & FIND_FROM_START != 0 => (0..len).find(|&i| fits(i)),
        d if d & FIND_PREV != 0 => (0..len.min(p + 1)).rev().find(|&i| fits(i)),
        d if d & FIND_NEXT != 0 => (p.max(0)..len).find(|&i| fits(i)),
        _ => (p >= 0 && p < len && fits(p)).then_some(p),
    };
    Some(found.map_or(u32::MAX, |i| (i + first) as u32))
}

/// The stream's header fields for the functions that read them.
fn header(cpu: &mut Cpu, this: u32) -> Option<(u32, u32, u32, u32)> {
    let (slot, n) = stream(cpu, this)?;
    let s = &file(cpu, slot).avi.streams[n];
    Some((s.start(), s.length(), s.scale(), s.rate()))
}

/// AVIStreamStart(stream): its first sample, or -1.
fn avi_stream_start(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(header(cpu, args[0]).map_or(u32::MAX, |h| h.0))
}

/// AVIStreamLength(stream): its samples, or -1.
fn avi_stream_length(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(header(cpu, args[0]).map_or(u32::MAX, |h| h.1))
}

/// AVIStreamBeginStreaming and AVIStreamEndStreaming: the file handler
/// has no IAVIStreaming, so streaming needs no preparing.
fn avi_stream_streaming(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(AVIERR_OK)
}

/// AVIStreamSampleToTime(stream, sample): its time in milliseconds,
/// computed in floating point as Wine does, or -1.
fn avi_stream_sample_to_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((start, length, scale, rate)) = header(cpu, args[0]) else {
        return Some(u32::MAX);
    };
    if rate == 0 {
        return Some(u32::MAX);
    }
    let sample = (args[1] as i32 as i64).clamp(start as i64, start as i64 + length as i64);
    let product = sample as f32 * scale as f32 * 1000.0;
    let time = if rate / scale.max(1) < 1000 {
        product / rate as f32
    } else {
        (product + (rate - 1) as f32) / rate as f32
    };
    Some(time as i32 as u32)
}

/// AVIStreamTimeToSample(stream, time): the sample at `time`
/// milliseconds, or -1.
fn avi_stream_time_to_sample(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((start, length, scale, rate)) = header(cpu, args[0]) else {
        return Some(u32::MAX);
    };
    let time = args[1] as i32;
    if scale == 0 || time < 0 {
        return Some(u32::MAX);
    }
    let product = time as f64 * rate as f64;
    let sample = if rate / scale < 1000 {
        product / (scale as f64 * 1000.0)
    } else {
        (product + (scale as f64 * 1000.0 - 1.0)) / (scale as f64 * 1000.0)
    } as i64;
    let sample = sample.clamp(start as i64, start as i64 + length as i64);
    Some(sample as u32)
}

fn frames(cpu: &mut Cpu) -> &mut Vec<Option<GetFrame>> {
    &mut crate::win32::process(cpu).vfw.frames
}

/// The slot of the IGetFrame `this`.
fn frame_slot(cpu: &mut Cpu, this: u32) -> Option<usize> {
    let (vtable, id) = com::interface_of(cpu, dll_index(), this)?;
    let slot = (id as usize).checked_sub(1)?;
    (vtable == IGETFRAME && frames(cpu).get(slot)?.is_some()).then_some(slot)
}

fn frame(cpu: &mut Cpu, slot: usize) -> &mut GetFrame {
    frames(cpu)[slot].as_mut().expect("an open IGetFrame")
}

/// The size of a DIB's rows.
fn dib_size(bits: u16, width: u32, height: u32) -> u32 {
    (width * bits as u32).div_ceil(32) * 4 * height
}

/// Sets `slot`'s frame up for the format `wanted`, as IGetFrame::SetFormat
/// does: a BITMAPINFOHEADER, or 0 or AVIGETFRAMEF_BESTDISPLAYFMT for the
/// default. False for one it can't give, which it logs.
fn set_format(cpu: &mut Cpu, slot: usize, wanted: u32) -> bool {
    let (file_slot, stream) = (frame(cpu, slot).file, frame(cpu, slot).stream);
    let format = file(cpu, file_slot).avi.streams[stream].format.clone();
    if format.len() < BITMAPINFOHEADER {
        return false;
    }
    let read = |at: usize| u32::from_le_bytes([format[at], format[at + 1], format[at + 2], format[at + 3]]);
    let (width, height, compression) = (read(4) as i32, read(8) as i32, read(16));
    let (width, height) = (width.unsigned_abs(), height.unsigned_abs());
    let source_bits = u16::from_le_bytes([format[14], format[15]]);
    let bits = if wanted == 0 || wanted == BESTDISPLAYFMT {
        if compression == BI_RGB { source_bits } else { 24 }
    } else {
        if !cpu.bus.guest_probe(wanted, BITMAPINFOHEADER, false) {
            return false;
        }
        let top_down = (cpu.bus.guest_read_32(wanted + 8) as i32) < 0;
        let kind = cpu.bus.guest_read_32(wanted + 16);
        let bits = cpu.bus.guest_read_16(wanted + 14);
        let decoded = compression != BI_RGB && matches!(bits, 16 | 24 | 32);
        if top_down || kind != BI_RGB || !(decoded || bits == source_bits) {
            cpu.bus.log_string(&format!(
                "[W32] AVIStreamGetFrameOpen: no {bits}-bit format {kind:#X}"
            ));
            return false;
        }
        bits
    };
    if compression != BI_RGB && !codec::supported(compression, width, height) {
        let fourcc = String::from_utf8_lossy(&compression.to_le_bytes()).into_owned();
        cpu.bus
            .log_string(&format!("[W32] AVIStreamGetFrameOpen: no codec for {fourcc:?}"));
        return false;
    }
    // An uncompressed source's header goes out with its palette.
    let mut header = if compression == BI_RGB {
        format
    } else {
        format[..BITMAPINFOHEADER].to_vec()
    };
    let image = dib_size(bits, width, height);
    header[0..4].copy_from_slice(&(BITMAPINFOHEADER as u32).to_le_bytes());
    header[8..12].copy_from_slice(&height.to_le_bytes());
    header[14..16].copy_from_slice(&bits.to_le_bytes());
    header[16..20].copy_from_slice(&BI_RGB.to_le_bytes());
    header[20..24].copy_from_slice(&image.to_le_bytes());
    let size = header.len() as u32 + image;
    let Some(buffer) =
        crate::win32::with_process(cpu, |p, bus| p.heap.alloc(&mut p.memory, bus, size))
    else {
        return false;
    };
    cpu.bus.guest_fill(buffer, size as usize, 0);
    cpu.bus.guest_write_bytes(buffer, &header);
    let f = frame(cpu, slot);
    let old = std::mem::replace(&mut f.buffer, buffer);
    f.compression = compression;
    f.width = width;
    f.height = height;
    f.bits = bits;
    f.header = header.len() as u32;
    f.last = -1;
    f.codec = Codec::new(compression);
    if old != 0 {
        crate::win32::with_process(cpu, |p, _| p.heap.free(old));
    }
    true
}

/// AVIStreamGetFrameOpen(stream, wanted): an IGetFrame of a video
/// stream, or 0.
fn avi_stream_get_frame_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, wanted] = args else { unreachable!() };
    let Some((file_slot, stream_index)) = stream(cpu, this) else {
        return Some(0);
    };
    if file(cpu, file_slot).avi.streams[stream_index].kind() != STREAMTYPE_VIDEO {
        return Some(0);
    }
    let list = frames(cpu);
    let slot = list.iter().position(Option::is_none).unwrap_or(list.len());
    if slot == list.len() {
        list.push(None);
    }
    let Some(iface) = com::new_interface(cpu, dll_index(), IGETFRAME, slot as u32 + 1, 0) else {
        return Some(0);
    };
    frames(cpu)[slot] = Some(GetFrame {
        file: file_slot,
        stream: stream_index,
        iface,
        refs: 1,
        ..GetFrame::default()
    });
    file(cpu, file_slot).refs += 1;
    if !set_format(cpu, slot, wanted) {
        release_frame(cpu, iface);
        return Some(0);
    }
    Some(iface)
}

fn release_frame(cpu: &mut Cpu, this: u32) -> u32 {
    let Some(slot) = frame_slot(cpu, this) else {
        return 0;
    };
    let f = frame(cpu, slot);
    f.refs -= 1;
    if f.refs > 0 {
        return f.refs;
    }
    let f = frames(cpu)[slot].take().expect("an open IGetFrame");
    if f.buffer != 0 {
        crate::win32::with_process(cpu, |p, _| p.heap.free(f.buffer));
    }
    com::free_interface(cpu, f.iface);
    // Its reference to the file, through the file's own interface.
    let iface = file(cpu, f.file).iface;
    release(cpu, iface);
    0
}

fn get_frame_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(slot) = frame_slot(cpu, args[0]) else {
        return Some(0);
    };
    let f = frame(cpu, slot);
    f.refs += 1;
    Some(f.refs)
}

fn get_frame_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(release_frame(cpu, args[0]))
}

/// QueryInterface of an IGetFrame: IUnknown and IGetFrame are it.
fn get_frame_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(com::E_POINTER);
    };
    if out == 0 {
        return Some(com::E_POINTER);
    }
    if iid == com::IID_IUNKNOWN || iid == IID_IGETFRAME {
        get_frame_add_ref(cpu, &[this]);
        write_32(cpu, out, this);
        return Some(com::S_OK);
    }
    write_32(cpu, out, 0);
    Some(com::E_NOINTERFACE)
}

/// IGetFrame::Begin and End: streaming needs no preparing.
fn get_frame_nothing(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(AVIERR_OK)
}

/// IGetFrame::SetFormat(this, wanted, bits, x, y, dx, dy). The DIB is
/// always the frame's whole picture in a buffer of its own.
fn get_frame_set_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(slot) = frame_slot(cpu, args[0]) else {
        return Some(AVIERR_BADHANDLE);
    };
    if args[2] != 0 {
        return Some(AVIERR_UNSUPPORTED);
    }
    Some(if set_format(cpu, slot, args[1]) {
        AVIERR_OK
    } else {
        AVIERR_BADFORMAT
    })
}

/// AVIStreamGetFrame and IGetFrame::GetFrame(this, position): the DIB of
/// the frame at `position`, its header first, or 0. Frames are decoded
/// from the last key frame, or from the last one decoded when that is
/// nearer.
fn get_frame_get(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, position] = args else {
        unreachable!()
    };
    let Some(slot) = frame_slot(cpu, this) else {
        return Some(0);
    };
    let (file_slot, stream_index) = (frame(cpu, slot).file, frame(cpu, slot).stream);
    let f = file(cpu, file_slot);
    let handle = f.handle;
    let s = &f.avi.streams[stream_index];
    let first = s.start() as i64;
    let wanted = position as i32 as i64 - first;
    if wanted < 0 || wanted >= s.chunks.len() as i64 {
        return Some(0);
    }
    let key = (0..=wanted)
        .rev()
        .find(|&i| s.chunks[i as usize].flags & avi::AVIIF_KEYFRAME != 0)
        .unwrap_or(0);
    let f = frame(cpu, slot);
    // A copy's codec (after a restore) starts again.
    if f.compression != BI_RGB && f.codec.is_empty() {
        f.codec = Codec::new(f.compression);
        f.last = -1;
    }
    let (compression, width, height, bits, header, buffer, last) = (
        f.compression,
        f.width,
        f.height,
        f.bits,
        f.header,
        f.buffer,
        f.last,
    );
    if last == wanted {
        return Some(buffer);
    }
    let from = if compression == BI_RGB {
        wanted
    } else if key <= last && last < wanted {
        last + 1
    } else {
        key
    };
    let mut raw = None;
    for i in from..=wanted {
        let c = file(cpu, file_slot).avi.streams[stream_index].chunks[i as usize];
        // An empty chunk repeats the frame before.
        if c.size == 0 {
            continue;
        }
        let data = read_at(cpu, handle, c.offset, c.size)?;
        if compression == BI_RGB {
            raw = Some(data);
        } else if let Err(e) = frame(cpu, slot).codec.decode(&data) {
            cpu.bus
                .log_string(&format!("[W32] AVIStreamGetFrame: frame {i}: {e}"));
        }
    }
    frame(cpu, slot).last = wanted;
    let picture = if compression == BI_RGB {
        raw
    } else {
        frame(cpu, slot)
            .codec
            .picture(width, height)
            .map(|p| convert(&p, bits, width, height))
    };
    if let Some(picture) = picture {
        let n = picture.len().min(dib_size(bits, width, height) as usize);
        cpu.bus.guest_write_bytes(buffer + header, &picture[..n]);
    }
    Some(buffer)
}

/// A 24-bit DIB in `bits` bits a pixel.
fn convert(dib: &[u8], bits: u16, width: u32, height: u32) -> Vec<u8> {
    if bits == 24 {
        return dib.to_vec();
    }
    let (w, stride) = (width as usize, dib_stride(width));
    let out_stride = dib_size(bits, width, 1) as usize;
    let mut out = vec![0; out_stride * height as usize];
    for (from, to) in dib.chunks_exact(stride).zip(out.chunks_exact_mut(out_stride)) {
        let pixels = from[..w * 3].as_chunks::<3>().0.iter();
        if bits == 16 {
            for (p, d) in pixels.zip(to.as_chunks_mut::<2>().0) {
                let [b, g, r] = p.map(|c| c as u16 >> 3);
                *d = ((r << 10) | (g << 5) | b).to_le_bytes();
            }
        } else {
            for (p, d) in pixels.zip(to.as_chunks_mut::<4>().0) {
                d[..3].copy_from_slice(p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One blue pixel (0xFF, 0, 0 in DIB order), one white, 2 by 1.
    const DIB: [u8; 8] = [0xFF, 0, 0, 0xFF, 0xFF, 0xFF, 0, 0];

    #[test]
    fn a_dib_converts_to_16_and_32_bits() {
        assert_eq!(convert(&DIB, 24, 2, 1), DIB);
        assert_eq!(convert(&DIB, 16, 2, 1), [0x1F, 0, 0xFF, 0x7F]);
        assert_eq!(
            convert(&DIB, 32, 2, 1),
            [0xFF, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0]
        );
    }

    #[test]
    fn dib_rows_are_padded_to_4_bytes() {
        assert_eq!(dib_size(24, 2, 3), 24);
        assert_eq!(dib_size(16, 3, 2), 16);
        assert_eq!(dib_size(32, 640, 480), 640 * 480 * 4);
    }
}
