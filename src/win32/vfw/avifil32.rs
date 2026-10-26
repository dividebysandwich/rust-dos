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

use super::avi::{self, Avi};
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
];

const IAVIFILE: usize = 0;
const IAVISTREAM: usize = 1;

const fn guid_0002(d1: u32) -> Guid {
    Guid::new(d1, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46])
}
const IID_IAVIFILE: Guid = guid_0002(0x0002_0020);
const IID_IAVISTREAM: Guid = guid_0002(0x0002_0021);

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
