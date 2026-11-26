//! MSACM32.DLL: the Audio Compression Manager, with the decoders of
//! Windows 98's two ADPCM codecs (`adpcm`). Oni keeps its sounds in MS
//! ADPCM and plays them through acmStreamConvert, so without them it is
//! silent; Test Drive 4 only asks acmMetrics for the largest wave
//! format, and won't start without an answer.
//!
//! A stream decodes MS ADPCM or IMA ADPCM to PCM of the same rate and
//! channels, 8 or 16 bits, as the codecs do; acmFormatSuggest suggests
//! that PCM. The codecs a program installed, such as the Windows Media
//! Audio codec, are drivers MSACM32 loads into the program and talks to
//! (`drivers`): acmFormatSuggest and acmStreamOpen ask them for what the
//! built-in codecs don't take, and acmDriverEnum, acmDriverOpen and the
//! other functions of drivers reach them. The built-in codecs aren't
//! drivers with handles of their own. The PCM converter built into
//! Windows' MSACM32 is here too (`pcm`): acmFormatSuggest suggests PCM
//! for PCM, and a stream converts PCM to PCM of other channels, bits and
//! rate. The rest answers as the code of MSACM32 4.03.1998 does with no
//! drivers: there is no encoder and no other codec built in (issue
//! 439). What it says of PCM without a driver and of bad arguments in
//! other calls isn't here (issue 443).
//!
//! Wine's MSACM32.DLL and its codecs convert too, as guest code:
//! `win32_dlls_replace=msacm32` puts them in place of this one.

pub mod adpcm;
pub mod drivers;
pub mod pcm;

use super::kernel32::virtual_memory::allows;
use super::stubs::{Api, Conv, Dll, Handler};
use super::user32::current_pid;
use super::winmm::{
    MMSYSERR_INVALFLAG, MMSYSERR_INVALHANDLE, MMSYSERR_INVALPARAM, MMSYSERR_NODRIVER,
    MMSYSERR_NOERROR, MMSYSERR_NOMEM, MMSYSERR_NOTSUPPORTED,
};
use crate::cpu::Cpu;
use crate::savestate::Since;
use adpcm::MS_ADPCM;
use pcm::Pcm;
use std::collections::BTreeMap;

pub static DLL: Dll = Dll {
    name: "MSACM32.DLL",
    base: 0xBF6A_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
    resources: None,
};

const ACMERR_NOTPOSSIBLE: u32 = 512;
const ACMERR_BUSY: u32 = 513;
const ACMERR_UNPREPARED: u32 = 514;

/// The versions acmGetVersion gives, retail: the constants in Windows
/// 95's MSACM32 4.00.950 and Windows 98 SE's 4.03.1998.
const VERSION_95: u32 = 0x0400_0000;
const VERSION_98: u32 = 0x0403_0000;

/// acmMetrics's metrics.
const ACM_METRIC_COUNT_DRIVERS: u32 = 1;
const ACM_METRIC_COUNT_HARDWARE: u32 = 6;
const ACM_METRIC_COUNT_LOCAL_DRIVERS: u32 = 20;
const ACM_METRIC_COUNT_LOCAL_DISABLED: u32 = 24;
const ACM_METRIC_HARDWARE_WAVE_INPUT: u32 = 30;
const ACM_METRIC_HARDWARE_WAVE_OUTPUT: u32 = 31;
const ACM_METRIC_MAX_SIZE_FORMAT: u32 = 50;
const ACM_METRIC_MAX_SIZE_FILTER: u32 = 51;
const ACM_METRIC_DRIVER_SUPPORT: u32 = 100;
const ACM_METRIC_DRIVER_PRIORITY: u32 = 101;

/// The largest format of Windows 98's codecs: MS ADPCM's, a
/// WAVEFORMATEX with its 7 coefficients. Programs size their buffers
/// for a wave file's format by this (Test Drive 4 allocates just that).
/// It is Windows' with its codecs; a real ACM without drivers says 0
/// and ACMERR_NOTPOSSIBLE, which that game takes for a fatal error.
const MAX_SIZE_FORMAT: u32 = 50;

/// acmDriverPriority's flags.
const PRIORITY_FLAGS: u32 = 0x0003_0003;
const PRIORITY_DEFER_MASK: u32 = 0x0003_0000;
const PRIORITY_BEGIN: u32 = 0x0001_0000;
const PRIORITY_END: u32 = 0x0002_0000;
/// acmStreamOpen's flags, and the one that only asks.
const STREAM_OPEN_FLAGS: u32 = 0x0007_0007;
const STREAM_OPEN_QUERY: u32 = 1;
const STREAM_OPEN_ASYNC: u32 = 2;
/// The bytes of a wave format that every format has.
const WAVE_FORMAT_SIZE: u32 = 16;
const WAVE_FORMAT_PCM: u16 = 1;
/// acmFormatSuggest's flags: which of the destination's fields the
/// suggestion keeps.
const SUGGEST_TAG: u32 = 0x0001_0000;
const SUGGEST_CHANNELS: u32 = 0x0002_0000;
const SUGGEST_RATE: u32 = 0x0004_0000;
const SUGGEST_BITS: u32 = 0x0008_0000;
const SUGGEST_FLAGS: u32 = 0x000F_0000;
/// acmStreamSize's flags.
const SIZE_SOURCE: u32 = 0;
const SIZE_DESTINATION: u32 = 1;
/// acmStreamConvert's flags: whole blocks only, the start and the end of
/// a stream.
const CONVERT_BLOCKALIGN: u32 = 4;
const CONVERT_FLAGS: u32 = 0x34;
/// ACMSTREAMHEADER: its size, its fields and the bits of its status.
const HEADER_SIZE: u32 = 84;
const HEADER_STATUS: u32 = 4;
const HEADER_SOURCE: u32 = 0x0C;
const HEADER_SOURCE_LENGTH: u32 = 0x10;
const HEADER_SOURCE_USED: u32 = 0x14;
const HEADER_DESTINATION: u32 = 0x1C;
const HEADER_DESTINATION_LENGTH: u32 = 0x20;
const HEADER_DESTINATION_USED: u32 = 0x24;
const STATUS_DONE: u32 = 0x0001_0000;
const STATUS_PREPARED: u32 = 0x0002_0000;
const STATUS_INQUEUE: u32 = 0x0010_0000;
/// The first stream's handle; the others count up from it.
const FIRST_STREAM: u32 = 0x0AC0_0000;

/// The W32 version whose states have the installable drivers: the
/// processes that found theirs, the drivers, those open and the streams
/// of drivers.
pub const DRIVERS_SAVED: u16 = 110;

/// The streams open, by handle, and the installable drivers (`drivers`):
/// the processes that found theirs, the drivers each process has (by
/// HACMDRIVERID) and those it opened (by HACMDRIVER).
#[derive(Clone, Debug, Default)]
pub struct State {
    next: u32,
    streams: BTreeMap<u32, Stream>,
    registered: Since<DRIVERS_SAVED, Vec<u32>>,
    ids: Since<DRIVERS_SAVED, Vec<drivers::DriverId>>,
    drivers: Since<DRIVERS_SAVED, Vec<drivers::Driver>>,
}

crate::state_fields!(State {
    next,
    streams,
    registered,
    ids,
    drivers
});

/// A stream: the process that opened it, what it decodes and the bits
/// of the PCM it decodes to. A stream of an installable driver has the
/// driver's handle (a HACMDRIVER) instead, its ACMDRVSTREAMINSTANCE in
/// the system's heap, and whether it opened the driver itself, to close
/// it with the stream. A stream of the PCM converter has the PCM it
/// converts from and to instead (`from`, `to`).
#[derive(Clone, Debug, Default)]
struct Stream {
    pid: u32,
    source: adpcm::Format,
    bits: u16,
    driver: Since<DRIVERS_SAVED, u32>,
    instance: Since<DRIVERS_SAVED, u32>,
    owns: Since<DRIVERS_SAVED, bool>,
    from: Since<DRIVERS_SAVED, Pcm>,
    to: Since<DRIVERS_SAVED, Pcm>,
}

crate::state_fields!(Stream {
    pid,
    source,
    bits,
    driver,
    instance,
    owns,
    from,
    to
});

impl Stream {
    /// Whether it is a stream of the PCM converter.
    fn converts_pcm(&self) -> bool {
        self.from.rate != 0
    }

    /// The bytes of a frame of the PCM it writes.
    fn frame(&self) -> u32 {
        self.source.channels as u32 * self.bits as u32 / 8
    }

    /// The bytes of the PCM of a whole block.
    fn block_pcm(&self) -> u32 {
        self.source.samples_per_block as u32 * self.frame()
    }

    /// The PCM bytes that `adpcm` bytes of the source decode to, as
    /// msadp32.acm and imaadp32.acm size it: a partial block at the end
    /// counts as a whole one. None if `adpcm` holds no whole block, or
    /// if the whole blocks' PCM would pass 4 GB.
    fn pcm_size(&self, adpcm: u32) -> Option<u32> {
        let align = self.source.block_align as u32;
        let blocks = adpcm / align;
        if blocks == 0 || u32::MAX / self.block_pcm() < blocks {
            return None;
        }
        // The codecs check the whole blocks only, so one more can wrap.
        let partial = u32::from(!adpcm.is_multiple_of(align));
        Some((blocks + partial).wrapping_mul(self.block_pcm()))
    }

    /// The ADPCM bytes of the whole blocks whose PCM fits in `pcm`
    /// bytes, None if not even one does.
    fn adpcm_size(&self, pcm: u32) -> Option<u32> {
        let blocks = pcm / self.block_pcm();
        (blocks != 0).then(|| blocks.wrapping_mul(self.source.block_align as u32))
    }
}

/// MSACM32's state.
fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.msacm32
}

/// The process `pid` ended: Windows frees the streams it left open, and
/// its drivers went with its memory.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    state(cpu).streams.retain(|_, s| s.pid != pid);
    drivers::process_ended(cpu, pid);
}

const fn stdcall(name: &'static str, ordinal: u16, args: u8, handler: Handler) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// The functions, with Windows 98 SE's ordinals. Append only (see
/// `stubs`). acmMessage32 and the thunk data, for 16-bit MSACM.DLL's
/// calls, are left out.
static APIS: &[Api] = &[
    stdcall("acmDriverAddA", 1, 5, add_refused),
    stdcall("acmDriverAddW", 2, 5, add_refused),
    stdcall("acmDriverClose", 3, 2, drivers::driver_close),
    stdcall("acmDriverDetailsA", 4, 3, driver_details_a),
    stdcall("acmDriverDetailsW", 5, 3, driver_details_w),
    stdcall("acmDriverEnum", 6, 3, drivers::driver_enum),
    stdcall("acmDriverID", 7, 3, drivers::driver_id),
    stdcall("acmDriverMessage", 8, 4, drivers::driver_message),
    stdcall("acmDriverOpen", 9, 3, drivers::driver_open),
    stdcall("acmDriverPriority", 10, 3, driver_priority),
    stdcall("acmDriverRemove", 11, 2, bad_handle),
    stdcall("acmFilterChooseA", 12, 1, not_possible),
    stdcall("acmFilterChooseW", 13, 1, not_possible),
    stdcall("acmFilterDetailsA", 14, 3, no_driver),
    stdcall("acmFilterDetailsW", 15, 3, no_driver),
    stdcall("acmFilterEnumA", 16, 5, nothing_to_enumerate),
    stdcall("acmFilterEnumW", 17, 5, nothing_to_enumerate),
    stdcall("acmFilterTagDetailsA", 18, 3, not_possible),
    stdcall("acmFilterTagDetailsW", 19, 3, not_possible),
    stdcall("acmFilterTagEnumA", 20, 5, nothing_to_enumerate),
    stdcall("acmFilterTagEnumW", 21, 5, nothing_to_enumerate),
    stdcall("acmFormatChooseA", 22, 1, not_possible),
    stdcall("acmFormatChooseW", 23, 1, not_possible),
    stdcall("acmFormatDetailsA", 24, 3, no_driver),
    stdcall("acmFormatDetailsW", 25, 3, no_driver),
    stdcall("acmFormatEnumA", 26, 5, not_possible),
    stdcall("acmFormatEnumW", 27, 5, not_possible),
    stdcall("acmFormatSuggest", 28, 5, format_suggest),
    stdcall("acmFormatTagDetailsA", 29, 3, not_possible),
    stdcall("acmFormatTagDetailsW", 30, 3, not_possible),
    stdcall("acmFormatTagEnumA", 31, 5, nothing_to_enumerate),
    stdcall("acmFormatTagEnumW", 32, 5, nothing_to_enumerate),
    stdcall("acmGetVersion", 33, 0, acm_get_version),
    stdcall("acmMetrics", 35, 3, acm_metrics),
    stdcall("acmStreamClose", 36, 2, stream_close),
    stdcall("acmStreamConvert", 37, 3, stream_convert),
    stdcall("acmStreamMessage", 38, 4, stream_message),
    stdcall("acmStreamOpen", 39, 8, stream_open),
    stdcall("acmStreamPrepareHeader", 40, 3, prepare_header),
    stdcall("acmStreamReset", 41, 2, stream_reset),
    stdcall("acmStreamSize", 42, 4, stream_size),
    stdcall("acmStreamUnprepareHeader", 43, 3, unprepare_header),
];

/// acmDriverAdd: a codec of a program's would need the codec interface.
fn add_refused(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_NOTSUPPORTED)
}

/// A function of a driver or a stream: there are none to have a handle.
fn bad_handle(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_INVALHANDLE)
}

fn driver_details_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    drivers::driver_details(cpu, args, false)
}

fn driver_details_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    drivers::driver_details(cpu, args, true)
}

/// What no codec can do. acmFormatEnum is one of these: it asks the
/// drivers for their largest format first, and without one ends there.
fn not_possible(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(ACMERR_NOTPOSSIBLE)
}

/// acmFormatDetails and acmFilterDetails: their answers are the drivers'.
fn no_driver(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_NODRIVER)
}

/// An enumeration of what codecs offer: it calls the callback for
/// nothing and succeeds.
fn nothing_to_enumerate(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_NOERROR)
}

/// Zero the handle or size at `out`, which a call that fails leaves so.
/// False if the program may not write there: Windows asks IsBadWritePtr,
/// so the call fails and the program doesn't.
fn clear(cpu: &mut Cpu, out: u32) -> bool {
    let writable = allows(cpu, out, 4, true);
    if writable {
        cpu.bus.guest_write_32(out, 0);
    }
    writable
}

/// acmDriverPriority(driver, priority, flags). With no driver, a program
/// holds back and lets go the notices of its changes, of which there are
/// none.
fn driver_priority(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[driver, _, flags] = args else {
        unreachable!()
    };
    if flags & !PRIORITY_FLAGS != 0 {
        return Some(MMSYSERR_INVALFLAG);
    }
    if driver != 0 {
        return Some(MMSYSERR_INVALHANDLE);
    }
    Some(match flags & PRIORITY_DEFER_MASK {
        PRIORITY_BEGIN | PRIORITY_END => MMSYSERR_NOERROR,
        _ => MMSYSERR_INVALFLAG,
    })
}

/// The ADPCM format at `at`, if it is one the decoders take: MS ADPCM
/// with its coefficients or IMA ADPCM, 4 bits a sample. Its rate is the
/// second number.
fn adpcm_format(cpu: &mut Cpu, at: u32) -> Option<(adpcm::Format, u32)> {
    let tag = cpu.bus.guest_read_16(at);
    let mut len = adpcm::WAVEFORMATEX_SIZE + adpcm::extra_fields(tag)?;
    if !allows(cpu, at, len as u32, false) {
        return None;
    }
    if tag == MS_ADPCM {
        // The coefficients, as many as wNumCoef says.
        let count = cpu.bus.guest_read_16(at + 20) as usize;
        if !allows(cpu, at + len as u32, 4 * count as u32, false) {
            return None;
        }
        len += 4 * count;
    }
    let mut wfx = vec![0; len];
    cpu.bus.guest_read_bytes(at, &mut wfx);
    adpcm::Format::from_wave_format(&wfx)
}

/// Write PCM of `channels`, `rate` and `bits` at `at`, as WAVEFORMATEX
/// with no extra bytes if `room` has space for its size.
fn write_pcm(cpu: &mut Cpu, at: u32, room: u32, channels: u16, rate: u32, bits: u16) {
    let align = channels * bits / 8;
    cpu.bus.guest_write_16(at, WAVE_FORMAT_PCM);
    cpu.bus.guest_write_16(at + 2, channels);
    cpu.bus.guest_write_32(at + 4, rate);
    cpu.bus.guest_write_32(at + 8, rate * align as u32);
    cpu.bus.guest_write_16(at + 12, align);
    cpu.bus.guest_write_16(at + 14, bits);
    if room >= 18 {
        cpu.bus.guest_write_16(at + 16, 0);
    }
}

/// acmFormatSuggest(driver, source, destination, size, flags): for an
/// ADPCM source, PCM of its rate and channels in 16 bits, or in 8 if
/// the flags keep the destination's 8. The flags keep the destination's
/// tag, channels, rate or bits; the codecs decode to nothing else. What
/// they don't suggest, the installed drivers may (`drivers`), and a
/// driver the program opened suggests on its own.
fn format_suggest(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[driver, source, destination, size, flags] = args else {
        unreachable!()
    };
    if flags & !SUGGEST_FLAGS != 0 {
        return Some(MMSYSERR_INVALFLAG);
    }
    if driver != 0 && drivers::driver(cpu, driver).is_none() {
        return Some(MMSYSERR_INVALHANDLE);
    }
    if !allows(cpu, source, WAVE_FORMAT_SIZE, false)
        || size < WAVE_FORMAT_SIZE
        || !allows(cpu, destination, size, true)
    {
        return Some(MMSYSERR_INVALPARAM);
    }
    if driver != 0 {
        return drivers::format_suggest(cpu, args);
    }
    match builtin_suggest(cpu, args) {
        MMSYSERR_NOERROR => Some(MMSYSERR_NOERROR),
        _ => drivers::format_suggest(cpu, args),
    }
}

/// What the built-in codecs suggest for acmFormatSuggest's checked
/// arguments: MMSYSERR_NOERROR with the destination written, or
/// ACMERR_NOTPOSSIBLE.
fn builtin_suggest(cpu: &mut Cpu, args: &[u32]) -> u32 {
    let &[_, source, destination, size, flags] = args else {
        unreachable!()
    };
    if cpu.bus.guest_read_16(source) == WAVE_FORMAT_PCM {
        return pcm_suggest(cpu, args);
    }
    let Some((f, rate)) = adpcm_format(cpu, source) else {
        return ACMERR_NOTPOSSIBLE;
    };
    let kept = |flag: u32, wanted: u32, ours: u32| flags & flag == 0 || wanted == ours;
    let bits = if flags & SUGGEST_BITS != 0 {
        cpu.bus.guest_read_16(destination + 14)
    } else {
        16
    };
    let tag = cpu.bus.guest_read_16(destination) as u32;
    let channels = cpu.bus.guest_read_16(destination + 2) as u32;
    if !kept(SUGGEST_TAG, tag, WAVE_FORMAT_PCM as u32)
        || !kept(SUGGEST_CHANNELS, channels, f.channels as u32)
        || !kept(SUGGEST_RATE, cpu.bus.guest_read_32(destination + 4), rate)
        || !matches!(bits, 8 | 16)
    {
        return ACMERR_NOTPOSSIBLE;
    }
    write_pcm(cpu, destination, size, f.channels, rate, bits);
    MMSYSERR_NOERROR
}

/// The PCM format at `at`, if the PCM converter takes it (`Pcm::new`).
fn pcm_format(cpu: &mut Cpu, at: u32) -> Option<Pcm> {
    if cpu.bus.guest_read_16(at) != WAVE_FORMAT_PCM {
        return None;
    }
    let channels = cpu.bus.guest_read_16(at + 2);
    let rate = cpu.bus.guest_read_32(at + 4);
    let align = cpu.bus.guest_read_16(at + 12);
    Pcm::new(channels, rate, cpu.bus.guest_read_16(at + 14), align)
}

/// What the PCM converter built into Windows' MSACM32 suggests for a PCM
/// source: PCM with the source's channels, rate and bits, but those the
/// flags keep from the destination. ACMERR_NOTPOSSIBLE for a source or
/// a suggestion the converter's streams don't take (`Pcm::new`), or for
/// a tag kept that isn't PCM's. Demolition Racer's wmaudsdk.dll asks
/// this of the PCM the WMA codec suggested. Which rates Windows'
/// converter takes isn't checked here: any but 0 is.
fn pcm_suggest(cpu: &mut Cpu, args: &[u32]) -> u32 {
    let &[_, source, destination, size, flags] = args else {
        unreachable!()
    };
    if pcm_format(cpu, source).is_none() {
        return ACMERR_NOTPOSSIBLE;
    }
    let field = |cpu: &mut Cpu, flag: u32, offset: u32, read: fn(&mut Cpu, u32) -> u32| {
        let from = if flags & flag != 0 {
            destination
        } else {
            source
        };
        read(cpu, from + offset)
    };
    let read_16 = |cpu: &mut Cpu, at| cpu.bus.guest_read_16(at) as u32;
    let read_32 = |cpu: &mut Cpu, at| cpu.bus.guest_read_32(at);
    let tag = field(cpu, SUGGEST_TAG, 0, read_16);
    let channels = field(cpu, SUGGEST_CHANNELS, 2, read_16);
    let rate = field(cpu, SUGGEST_RATE, 4, read_32);
    let bits = field(cpu, SUGGEST_BITS, 14, read_16);
    if tag != WAVE_FORMAT_PCM as u32
        || !matches!(channels, 1 | 2)
        || !matches!(bits, 8 | 16)
        || rate == 0
    {
        return ACMERR_NOTPOSSIBLE;
    }
    write_pcm(cpu, destination, size, channels as u16, rate, bits as u16);
    MMSYSERR_NOERROR
}

/// The stream whose handle is `handle`.
fn stream(cpu: &mut Cpu, handle: u32) -> Option<Stream> {
    state(cpu).streams.get(&handle).cloned()
}

/// acmStreamOpen(out, driver, source, destination, filter, callback,
/// instance, flags). A query needs no `out`. The stream decodes ADPCM to
/// PCM of the same rate and channels, or converts PCM to PCM
/// (`pcm_stream_open`), with no filter; what else is asked goes to the
/// installed drivers (`drivers`), and a driver the program opened takes
/// every stream asked of it. A stream that calls back as it converts
/// (ACM_STREAMOPENF_ASYNC) isn't here: Windows' ADPCM codecs convert
/// while the call waits, and the ACM's own queue for them isn't written.
fn stream_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[out, driver, source, destination, filter, _, _, flags] = args else {
        unreachable!()
    };
    if out != 0 && !clear(cpu, out) {
        return Some(MMSYSERR_INVALPARAM);
    }
    if flags & !STREAM_OPEN_FLAGS != 0 {
        return Some(MMSYSERR_INVALFLAG);
    }
    if out == 0 && flags & STREAM_OPEN_QUERY == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    let readable = |cpu: &Cpu, format| allows(cpu, format, WAVE_FORMAT_SIZE, false);
    if !readable(cpu, source) || !readable(cpu, destination) {
        return Some(MMSYSERR_INVALPARAM);
    }
    if driver != 0 && drivers::driver(cpu, driver).is_none() {
        return Some(MMSYSERR_INVALHANDLE);
    }
    if flags & STREAM_OPEN_ASYNC != 0 {
        return Some(ACMERR_NOTPOSSIBLE);
    }
    if driver != 0 {
        return drivers::stream_open(cpu, args);
    }
    if cpu.bus.guest_read_16(source) == WAVE_FORMAT_PCM {
        return pcm_stream_open(cpu, args);
    }
    let Some((f, rate)) = adpcm_format(cpu, source) else {
        return drivers::stream_open(cpu, args);
    };
    let bits = cpu.bus.guest_read_16(destination + 14);
    let pcm = cpu.bus.guest_read_16(destination) == WAVE_FORMAT_PCM
        && cpu.bus.guest_read_16(destination + 2) == f.channels
        && cpu.bus.guest_read_32(destination + 4) == rate
        && matches!(bits, 8 | 16)
        && cpu.bus.guest_read_16(destination + 12) == f.channels * bits / 8;
    if !pcm || filter != 0 {
        return drivers::stream_open(cpu, args);
    }
    if flags & STREAM_OPEN_QUERY != 0 {
        return Some(MMSYSERR_NOERROR);
    }
    let pid = current_pid(cpu);
    let handle = drivers::new_handle(cpu, FIRST_STREAM);
    let stream = Stream {
        pid,
        source: f,
        bits,
        ..Stream::default()
    };
    state(cpu).streams.insert(handle, stream);
    cpu.bus.guest_write_32(out, handle);
    Some(MMSYSERR_NOERROR)
}

/// acmStreamOpen's checked arguments for a PCM source: a stream of the
/// PCM converter if it takes both formats (`Pcm::new`) and there is no
/// filter, else what the installed drivers say.
fn pcm_stream_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[out, _, source, destination, filter, _, _, flags] = args else {
        unreachable!()
    };
    let (Some(from), Some(to)) = (pcm_format(cpu, source), pcm_format(cpu, destination)) else {
        return drivers::stream_open(cpu, args);
    };
    if filter != 0 {
        return drivers::stream_open(cpu, args);
    }
    if flags & STREAM_OPEN_QUERY != 0 {
        return Some(MMSYSERR_NOERROR);
    }
    let pid = current_pid(cpu);
    let handle = drivers::new_handle(cpu, FIRST_STREAM);
    let stream = Stream {
        pid,
        from: Since(from),
        to: Since(to),
        ..Stream::default()
    };
    state(cpu).streams.insert(handle, stream);
    cpu.bus.guest_write_32(out, handle);
    Some(MMSYSERR_NOERROR)
}

/// acmStreamClose(stream, flags).
fn stream_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(s) = stream(cpu, args[0]) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if args[1] != 0 {
        return Some(MMSYSERR_INVALFLAG);
    }
    if *s.driver != 0 {
        return drivers::stream_close(cpu, args[0], &s);
    }
    state(cpu).streams.remove(&args[0]);
    Some(MMSYSERR_NOERROR)
}

/// acmStreamReset(stream, flags): a stream that converts while the call
/// waits has nothing in a queue to give back.
fn stream_reset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if stream(cpu, args[0]).is_none() {
        MMSYSERR_INVALHANDLE
    } else if args[1] != 0 {
        MMSYSERR_INVALFLAG
    } else {
        MMSYSERR_NOERROR
    })
}

/// acmStreamMessage(stream, message, first, second): the ADPCM codecs
/// take no messages of their own.
fn stream_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match stream(cpu, args[0]) {
        Some(_) => MMSYSERR_NOTSUPPORTED,
        None => MMSYSERR_INVALHANDLE,
    })
}

/// acmStreamSize(stream, size, out, flags): the PCM bytes that `size`
/// bytes of ADPCM decode to, a partial block at the end counted whole,
/// or the ADPCM of the whole blocks whose PCM fits in `size`.
/// ACMERR_NOTPOSSIBLE if there is no whole block, or if the PCM would
/// pass 4 GB. The PCM converter sizes by bytes a second (`pcm::size`),
/// and a driver's stream asks its driver.
fn stream_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, size, out, flags] = args else {
        unreachable!()
    };
    if !clear(cpu, out) {
        return Some(MMSYSERR_INVALPARAM);
    }
    let Some(s) = stream(cpu, handle) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if *s.driver != 0 {
        if !matches!(flags, SIZE_SOURCE | SIZE_DESTINATION) {
            return Some(MMSYSERR_INVALFLAG);
        }
        return drivers::stream_size(cpu, &s, size, out, flags);
    }
    let bytes = match flags {
        SIZE_SOURCE if s.converts_pcm() => pcm::size(*s.from, *s.to, size),
        SIZE_DESTINATION if s.converts_pcm() => pcm::size(*s.to, *s.from, size),
        SIZE_SOURCE => s.pcm_size(size),
        SIZE_DESTINATION => s.adpcm_size(size),
        _ => return Some(MMSYSERR_INVALFLAG),
    };
    let Some(bytes) = bytes else {
        return Some(ACMERR_NOTPOSSIBLE);
    };
    cpu.bus.guest_write_32(out, bytes);
    Some(MMSYSERR_NOERROR)
}

/// The stream `handle` and the status of the ACMSTREAMHEADER at
/// `header`: the checks the header functions share.
fn header_of(cpu: &mut Cpu, handle: u32, header: u32) -> Result<(Stream, u32), u32> {
    let s = stream(cpu, handle).ok_or(MMSYSERR_INVALHANDLE)?;
    if !allows(cpu, header, HEADER_SIZE, true) || cpu.bus.guest_read_32(header) < HEADER_SIZE {
        return Err(MMSYSERR_INVALPARAM);
    }
    Ok((s, cpu.bus.guest_read_32(header + HEADER_STATUS)))
}

/// The header's buffers, (source, its length, destination, its length),
/// if they are the program's: the source to read, the destination to
/// write.
fn buffers(cpu: &mut Cpu, header: u32) -> Option<(u32, u32, u32, u32)> {
    let mut read = |field| cpu.bus.guest_read_32(header + field);
    let buffers = (
        read(HEADER_SOURCE),
        read(HEADER_SOURCE_LENGTH),
        read(HEADER_DESTINATION),
        read(HEADER_DESTINATION_LENGTH),
    );
    let (source, source_length, destination, destination_length) = buffers;
    (allows(cpu, source, source_length, false)
        && allows(cpu, destination, destination_length, true))
    .then_some(buffers)
}

/// acmStreamPrepareHeader(stream, header, flags). Preparing a prepared
/// header does nothing. A driver's stream's driver prepares it too.
fn prepare_header(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, header, flags] = args else {
        unreachable!()
    };
    let (s, status) = match header_of(cpu, handle, header) {
        Ok(found) => found,
        Err(e) => return Some(e),
    };
    if flags != 0 {
        return Some(MMSYSERR_INVALFLAG);
    }
    if buffers(cpu, header).is_none() {
        return Some(MMSYSERR_INVALPARAM);
    }
    if *s.driver != 0 {
        return drivers::stream_header(cpu, &s, drivers::PREPARE, header, flags);
    }
    let status = status | STATUS_PREPARED;
    cpu.bus.guest_write_32(header + HEADER_STATUS, status);
    Some(MMSYSERR_NOERROR)
}

/// acmStreamUnprepareHeader(stream, header, flags). A driver's stream's
/// driver unprepares it too.
fn unprepare_header(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, header, flags] = args else {
        unreachable!()
    };
    let (s, status) = match header_of(cpu, handle, header) {
        Ok(found) => found,
        Err(e) => return Some(e),
    };
    if flags != 0 {
        return Some(MMSYSERR_INVALFLAG);
    }
    if *s.driver != 0 {
        return drivers::stream_header(cpu, &s, drivers::UNPREPARE, header, flags);
    }
    if status & STATUS_PREPARED == 0 {
        return Some(ACMERR_UNPREPARED);
    }
    let status = status & !STATUS_PREPARED;
    cpu.bus.guest_write_32(header + HEADER_STATUS, status);
    Some(MMSYSERR_NOERROR)
}

/// acmStreamConvert(stream, header, flags): decode the header's source
/// into its destination, and say how much of each was used. As the
/// codecs do, it fails with ACMERR_NOTPOSSIBLE unless the destination
/// holds the PCM of all the source's blocks, a partial one counted
/// whole, as acmStreamSize gives it. All of the source is used, or its
/// whole blocks if the flags ask for those; a partial block at the end
/// decodes as `adpcm::Format::decode_buffer` says. The PCM converter
/// converts the whole frames that fit (`pcm::convert`), and a driver's
/// stream's driver converts.
fn stream_convert(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, header, flags] = args else {
        unreachable!()
    };
    let (s, status) = match header_of(cpu, handle, header) {
        Ok(found) => found,
        Err(e) => return Some(e),
    };
    if flags & !CONVERT_FLAGS != 0 {
        return Some(MMSYSERR_INVALFLAG);
    }
    if status & STATUS_PREPARED == 0 {
        return Some(ACMERR_UNPREPARED);
    }
    let Some((at, length, destination, room)) = buffers(cpu, header) else {
        return Some(MMSYSERR_INVALPARAM);
    };
    if *s.driver != 0 {
        return drivers::stream_header(cpu, &s, drivers::CONVERT, header, flags);
    }
    let (used, bytes) = if s.converts_pcm() {
        let mut source = vec![0; length as usize];
        cpu.bus.guest_read_bytes(at, &mut source);
        pcm::convert(*s.from, *s.to, &source, room)
    } else {
        // The codecs size the destination's source first and refuse a
        // source longer than that, so the PCM always fits.
        if s.adpcm_size(room).unwrap_or(0) < length {
            return Some(ACMERR_NOTPOSSIBLE);
        }
        let used = if flags & CONVERT_BLOCKALIGN != 0 {
            length - length % s.source.block_align as u32
        } else {
            length
        };
        let mut source = vec![0; used as usize];
        cpu.bus.guest_read_bytes(at, &mut source);
        let pcm = s.source.decode_buffer(&source);
        let bytes: Vec<u8> = if s.bits == 8 {
            pcm.iter().map(|&v| ((v >> 8) as u8) ^ 0x80).collect()
        } else {
            pcm.iter().flat_map(|v| v.to_le_bytes()).collect()
        };
        (used, bytes)
    };
    cpu.bus.guest_write_bytes(destination, &bytes);
    cpu.bus.guest_write_32(header + HEADER_SOURCE_USED, used);
    let written = bytes.len() as u32;
    cpu.bus
        .guest_write_32(header + HEADER_DESTINATION_USED, written);
    let status = (status | STATUS_DONE) & !STATUS_INQUEUE;
    cpu.bus.guest_write_32(header + HEADER_STATUS, status);
    Some(MMSYSERR_NOERROR)
}

pub(super) fn acm_get_version(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(if super::version::is_98(cpu) {
        VERSION_98
    } else {
        VERSION_95
    })
}

/// acmMetrics(object, metric, output): Windows checks the object, then
/// the metric, then the output, which gets a value from the metrics that
/// fail too. Those of a driver fail for want of a driver.
fn acm_metrics(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[object, metric, output] = args else {
        unreachable!()
    };
    if object != 0 {
        return Some(MMSYSERR_INVALHANDLE);
    }
    let (value, result) = match metric {
        ACM_METRIC_COUNT_DRIVERS..=ACM_METRIC_COUNT_HARDWARE => (0, MMSYSERR_NOERROR),
        ACM_METRIC_COUNT_LOCAL_DRIVERS..=ACM_METRIC_COUNT_LOCAL_DISABLED => (0, MMSYSERR_NOERROR),
        ACM_METRIC_HARDWARE_WAVE_INPUT | ACM_METRIC_HARDWARE_WAVE_OUTPUT => {
            (u32::MAX, MMSYSERR_INVALHANDLE)
        }
        ACM_METRIC_MAX_SIZE_FORMAT => (MAX_SIZE_FORMAT, MMSYSERR_NOERROR),
        ACM_METRIC_MAX_SIZE_FILTER => (0, ACMERR_NOTPOSSIBLE),
        ACM_METRIC_DRIVER_SUPPORT | ACM_METRIC_DRIVER_PRIORITY => (0, MMSYSERR_INVALHANDLE),
        _ => return Some(MMSYSERR_NOTSUPPORTED),
    };
    if !allows(cpu, output, 4, true) {
        return Some(MMSYSERR_INVALPARAM);
    }
    cpu.bus.guest_write_32(output, value);
    Some(result)
}
