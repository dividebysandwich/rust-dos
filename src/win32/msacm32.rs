//! MSACM32.DLL: the Audio Compression Manager, on a Windows with no
//! codecs. Programs that only load PCM wave files still import it, for
//! one number: Test Drive 4 asks acmMetrics for the largest wave format
//! and won't start without an answer. So the DLL is here with Windows
//! 98's export names, its drivers are none, and what would need a codec
//! fails with the error a real ACM gives when none can do it.
//!
//! Wine's MSACM32.DLL and its ADPCM codecs do convert, as guest code:
//! `win32_dlls_replace=msacm32` puts them in place of this one.

use super::stubs::{Api, Conv, Dll, Handler};
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

const MMSYSERR_NOERROR: u32 = 0;
const MMSYSERR_INVALHANDLE: u32 = 5;
const MMSYSERR_NOTSUPPORTED: u32 = 8;
const MMSYSERR_INVALPARAM: u32 = 11;
const ACMERR_NOTPOSSIBLE: u32 = 512;

/// The version acmGetVersion gives: 4.00, retail.
const VERSION: u32 = 0x0400_0000;

/// acmMetrics's metrics.
const ACM_METRIC_COUNT_DRIVERS: u32 = 1;
const ACM_METRIC_COUNT_HARDWARE: u32 = 6;
const ACM_METRIC_COUNT_LOCAL_DRIVERS: u32 = 20;
const ACM_METRIC_COUNT_LOCAL_DISABLED: u32 = 24;
const ACM_METRIC_MAX_SIZE_FORMAT: u32 = 50;
const ACM_METRIC_MAX_SIZE_FILTER: u32 = 51;

/// The largest format of Windows 98's codecs: MS ADPCM's, a
/// WAVEFORMATEX with its 7 coefficients. Programs size their buffers
/// for a wave file's format by this (Test Drive 4 allocates just that),
/// so the size stays Windows' though no such codec is here.
const MAX_SIZE_FORMAT: u32 = 50;

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
    stdcall("acmDriverAddA", 1, 5, no_driver),
    stdcall("acmDriverAddW", 2, 5, no_driver),
    stdcall("acmDriverClose", 3, 2, bad_handle),
    stdcall("acmDriverDetailsA", 4, 3, bad_handle),
    stdcall("acmDriverDetailsW", 5, 3, bad_handle),
    stdcall("acmDriverEnum", 6, 3, driver_enum),
    stdcall("acmDriverID", 7, 3, bad_handle),
    stdcall("acmDriverMessage", 8, 4, bad_handle),
    stdcall("acmDriverOpen", 9, 3, bad_handle),
    stdcall("acmDriverPriority", 10, 3, bad_handle),
    stdcall("acmDriverRemove", 11, 2, bad_handle),
    stdcall("acmFilterChooseA", 12, 1, not_possible),
    stdcall("acmFilterChooseW", 13, 1, not_possible),
    stdcall("acmFilterDetailsA", 14, 3, not_possible),
    stdcall("acmFilterDetailsW", 15, 3, not_possible),
    stdcall("acmFilterEnumA", 16, 5, nothing_to_enumerate),
    stdcall("acmFilterEnumW", 17, 5, nothing_to_enumerate),
    stdcall("acmFilterTagDetailsA", 18, 3, not_possible),
    stdcall("acmFilterTagDetailsW", 19, 3, not_possible),
    stdcall("acmFilterTagEnumA", 20, 5, nothing_to_enumerate),
    stdcall("acmFilterTagEnumW", 21, 5, nothing_to_enumerate),
    stdcall("acmFormatChooseA", 22, 1, not_possible),
    stdcall("acmFormatChooseW", 23, 1, not_possible),
    stdcall("acmFormatDetailsA", 24, 3, not_possible),
    stdcall("acmFormatDetailsW", 25, 3, not_possible),
    stdcall("acmFormatEnumA", 26, 5, nothing_to_enumerate),
    stdcall("acmFormatEnumW", 27, 5, nothing_to_enumerate),
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
    stdcall("acmStreamOpen", 39, 8, not_possible),
    stdcall("acmStreamPrepareHeader", 40, 3, bad_handle),
    stdcall("acmStreamReset", 41, 2, bad_handle),
    stdcall("acmStreamSize", 42, 4, bad_handle),
    stdcall("acmStreamUnprepareHeader", 43, 3, bad_handle),
];

/// acmDriverAdd: a codec of a program's would need the codec interface.
fn no_driver(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_NOTSUPPORTED)
}

/// A function of a driver or a stream: there are none to have a handle.
fn bad_handle(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_INVALHANDLE)
}

/// What no codec can do.
fn not_possible(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(ACMERR_NOTPOSSIBLE)
}

/// An enumeration of what codecs offer: it calls the callback for
/// nothing and succeeds.
fn nothing_to_enumerate(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_NOERROR)
}

/// acmDriverEnum(callback, instance, flags): no drivers, so no calls.
fn driver_enum(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if args[0] == 0 {
        MMSYSERR_INVALPARAM
    } else {
        MMSYSERR_NOERROR
    })
}

fn acm_get_version(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(VERSION)
}

/// acmMetrics(object, metric, output).
fn acm_metrics(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[object, metric, output] = args else {
        unreachable!()
    };
    if output == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if object != 0 {
        return Some(MMSYSERR_INVALHANDLE);
    }
    let value = match metric {
        ACM_METRIC_COUNT_DRIVERS..=ACM_METRIC_COUNT_HARDWARE => 0,
        ACM_METRIC_COUNT_LOCAL_DRIVERS..=ACM_METRIC_COUNT_LOCAL_DISABLED => 0,
        ACM_METRIC_MAX_SIZE_FORMAT => MAX_SIZE_FORMAT,
        ACM_METRIC_MAX_SIZE_FILTER => 0,
        _ => return Some(MMSYSERR_NOTSUPPORTED),
    };
    cpu.bus.guest_write_32(output, value);
    if cpu.bus.guest_faulted() {
        return Some(MMSYSERR_INVALPARAM);
    }
    Some(MMSYSERR_NOERROR)
}
