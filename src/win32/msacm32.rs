//! MSACM32.DLL: the Audio Compression Manager, on a Windows with no
//! codecs. Programs that only load PCM wave files still import it, for
//! one number: Test Drive 4 asks acmMetrics for the largest wave format
//! and won't start without an answer. So the DLL is here with Windows
//! 98's export names, its drivers are none, and what would need a codec
//! fails with the error a real ACM gives when none can do it: the
//! answers are those of the code of MSACM32 4.03.1998 with no drivers,
//! for calls with sound arguments. What it says of PCM without a driver
//! and of bad arguments in other calls isn't here (issue 443).
//!
//! Wine's MSACM32.DLL and its ADPCM codecs do convert, as guest code:
//! `win32_dlls_replace=msacm32` puts them in place of this one.

use super::kernel32::virtual_memory::allows;
use super::stubs::{Api, Conv, Dll, Handler};
use super::winmm::{
    MMSYSERR_INVALFLAG, MMSYSERR_INVALHANDLE, MMSYSERR_INVALPARAM, MMSYSERR_NODRIVER,
    MMSYSERR_NOERROR, MMSYSERR_NOTSUPPORTED,
};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "MSACM32.DLL",
    base: 0xBF6A_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
};

const ACMERR_NOTPOSSIBLE: u32 = 512;

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
/// for a wave file's format by this (Test Drive 4 allocates just that),
/// so the size stays Windows' though no such codec is here. A real ACM
/// without drivers says 0 and ACMERR_NOTPOSSIBLE, which that game takes
/// for a fatal error.
const MAX_SIZE_FORMAT: u32 = 50;

/// acmDriverEnum's flags, the top four bits.
const DRIVER_ENUM_FLAGS: u32 = 0xF000_0000;
/// acmDriverPriority's flags.
const PRIORITY_FLAGS: u32 = 0x0003_0003;
const PRIORITY_DEFER_MASK: u32 = 0x0003_0000;
const PRIORITY_BEGIN: u32 = 0x0001_0000;
const PRIORITY_END: u32 = 0x0002_0000;
/// acmStreamOpen's flags, and the one that only asks.
const STREAM_OPEN_FLAGS: u32 = 0x0007_0007;
const STREAM_OPEN_QUERY: u32 = 1;
/// The bytes of a wave format that every format has.
const WAVE_FORMAT_SIZE: u32 = 16;

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
    stdcall("acmDriverClose", 3, 2, bad_handle),
    stdcall("acmDriverDetailsA", 4, 3, bad_handle),
    stdcall("acmDriverDetailsW", 5, 3, bad_handle),
    stdcall("acmDriverEnum", 6, 3, driver_enum),
    stdcall("acmDriverID", 7, 3, bad_handle),
    stdcall("acmDriverMessage", 8, 4, bad_handle),
    stdcall("acmDriverOpen", 9, 3, driver_open),
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
    stdcall("acmFormatSuggest", 28, 5, not_possible),
    stdcall("acmFormatTagDetailsA", 29, 3, not_possible),
    stdcall("acmFormatTagDetailsW", 30, 3, not_possible),
    stdcall("acmFormatTagEnumA", 31, 5, nothing_to_enumerate),
    stdcall("acmFormatTagEnumW", 32, 5, nothing_to_enumerate),
    stdcall("acmGetVersion", 33, 0, acm_get_version),
    stdcall("acmMetrics", 35, 3, acm_metrics),
    stdcall("acmStreamClose", 36, 2, bad_handle),
    stdcall("acmStreamConvert", 37, 3, bad_handle),
    stdcall("acmStreamMessage", 38, 4, bad_handle),
    stdcall("acmStreamOpen", 39, 8, stream_open),
    stdcall("acmStreamPrepareHeader", 40, 3, bad_handle),
    stdcall("acmStreamReset", 41, 2, bad_handle),
    stdcall("acmStreamSize", 42, 4, stream_size),
    stdcall("acmStreamUnprepareHeader", 43, 3, bad_handle),
];

/// acmDriverAdd: a codec of a program's would need the codec interface.
fn add_refused(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_NOTSUPPORTED)
}

/// A function of a driver or a stream: there are none to have a handle.
fn bad_handle(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_INVALHANDLE)
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

/// acmDriverEnum(callback, instance, flags): no drivers, so no calls.
fn driver_enum(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[callback, _, flags] = args else {
        unreachable!()
    };
    Some(if !allows(cpu, callback, 1, false) {
        MMSYSERR_INVALPARAM
    } else if flags & !DRIVER_ENUM_FLAGS != 0 {
        MMSYSERR_INVALFLAG
    } else {
        MMSYSERR_NOERROR
    })
}

/// acmDriverOpen(out, driver, flags).
fn driver_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if clear(cpu, args[0]) {
        MMSYSERR_INVALHANDLE
    } else {
        MMSYSERR_INVALPARAM
    })
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

/// acmStreamOpen(out, driver, source, destination, filter, callback,
/// instance, flags). A query needs no `out`.
fn stream_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[out, driver, source, destination, _, _, _, flags] = args else {
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
    Some(if driver != 0 {
        MMSYSERR_INVALHANDLE
    } else {
        ACMERR_NOTPOSSIBLE
    })
}

/// acmStreamSize(stream, size, out, flags).
fn stream_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if clear(cpu, args[2]) {
        MMSYSERR_INVALHANDLE
    } else {
        MMSYSERR_INVALPARAM
    })
}

fn acm_get_version(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
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
