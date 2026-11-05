//! Times: the clock's, and conversions between SYSTEMTIME, FILETIME and
//! DOS's dates and times. There are no time zones yet: local time is the
//! machine's clock, and UTC is the same.

use super::{FALSE, TRUE};
use crate::cpu::Cpu;
use crate::win32::convert::{self, SystemTime};
use chrono::{NaiveDateTime, TimeDelta};

/// The machine's clock as the system's time started from it: its time
/// then, the emulated microseconds then, and how far DOS had moved it.
#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    at: NaiveDateTime,
    micros: u64,
    offset: TimeDelta,
}

/// The system's time, as GetSystemTime and the C library's time() read
/// it. Windows reads the real-time clock once and moves the time on with
/// its timer, as GetTickCount's count, so this is the machine's clock
/// when it was first read, plus the emulated milliseconds since. The
/// host's clock, which the real-time clock follows, goes on whether the
/// machine runs or not, and a program that waits for this time to move
/// in a loop would wait in host time, calling the system all along. Here
/// what it reads moves on with the instructions run, once a millisecond,
/// so the idle detection skips the passes that see the same time
/// (`idle::pure_call`). The start is taken again when DOS sets the clock
/// (`Cmos::set_now`) and after a state is loaded.
pub(crate) fn now(cpu: &mut Cpu) -> NaiveDateTime {
    let micros = cpu.bus.clock.now_micros();
    let offset = cpu.bus.cmos.offset();
    let anchor = match cpu.bus.win32.system_clock {
        Some(a) if a.offset == offset && a.micros <= micros => a,
        _ => {
            let at = cpu.bus.cmos.now();
            let a = Anchor { at, micros, offset };
            cpu.bus.win32.system_clock = Some(a);
            a
        }
    };
    let ms = (micros - anchor.micros) / 1000;
    if cpu.bus.observe.on {
        let next = cpu.bus.clock.icount_at_micros(anchor.micros + (ms + 1) * 1000);
        crate::idle::pure_call(cpu, Some(next));
    }
    anchor.at + TimeDelta::milliseconds(ms as i64)
}

/// TIME_ZONE_INFORMATION's size, and GetTimeZoneInformation's answer when
/// there is no time zone.
const TIME_ZONE_INFORMATION: usize = 172;
const TIME_ZONE_ID_UNKNOWN: u32 = 0;

fn read_system_time(cpu: &mut Cpu, at: u32) -> Option<SystemTime> {
    let st = std::array::from_fn(|i| cpu.bus.guest_read_16(at + 2 * i as u32));
    (!cpu.bus.guest_faulted()).then_some(st)
}

fn write_system_time(cpu: &mut Cpu, at: u32, st: &SystemTime) {
    for (i, &value) in st.iter().enumerate() {
        cpu.bus.guest_write_16(at + 2 * i as u32, value);
    }
}

fn read_file_time(cpu: &mut Cpu, at: u32) -> Option<u64> {
    let low = cpu.bus.guest_read_32(at) as u64;
    let high = cpu.bus.guest_read_32(at + 4) as u64;
    (!cpu.bus.guest_faulted()).then_some(high << 32 | low)
}

fn write_file_time(cpu: &mut Cpu, at: u32, ticks: u64) {
    cpu.bus.guest_write_32(at, ticks as u32);
    cpu.bus.guest_write_32(at + 4, (ticks >> 32) as u32);
}

/// GetSystemTime and GetLocalTime: the system's time (`now`).
pub(super) fn get_system_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let st = convert::system_time_of(now(cpu));
    write_system_time(cpu, args[0], &st);
    Some(0)
}

/// Fails for a time that isn't one, leaving the last error as it was, as
/// Windows 95 does.
pub(super) fn system_time_to_file_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let st = read_system_time(cpu, args[0])?;
    let Some(ticks) = convert::system_time_to_ticks(&st) else {
        return Some(FALSE);
    };
    write_file_time(cpu, args[1], ticks);
    Some(TRUE)
}

/// Fails above the largest signed 64-bit time, as Windows does.
pub(super) fn file_time_to_system_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ticks = read_file_time(cpu, args[0])?;
    let Some(st) = (ticks < 1 << 63)
        .then(|| convert::ticks_to_system_time(ticks))
        .flatten()
    else {
        return Some(FALSE);
    };
    write_system_time(cpu, args[1], &st);
    Some(TRUE)
}

/// FileTimeToLocalFileTime and LocalFileTimeToFileTime: with no time zone,
/// the time as it is.
pub(super) fn copy_file_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ticks = read_file_time(cpu, args[0])?;
    write_file_time(cpu, args[1], ticks);
    Some(TRUE)
}

pub(super) fn dos_date_time_to_file_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(ticks) = convert::dos_to_ticks(args[0] as u16, args[1] as u16) else {
        return Some(FALSE);
    };
    write_file_time(cpu, args[2], ticks);
    Some(TRUE)
}

pub(super) fn file_time_to_dos_date_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ticks = read_file_time(cpu, args[0])?;
    let Some((date, time)) = convert::ticks_to_dos(ticks) else {
        return Some(FALSE);
    };
    cpu.bus.guest_write_16(args[1], date);
    cpu.bus.guest_write_16(args[2], time);
    Some(TRUE)
}

/// No time zone: no bias, no names, no changes to daylight saving time.
pub(super) fn get_time_zone_information(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    cpu.bus.guest_fill(args[0], TIME_ZONE_INFORMATION, 0);
    Some(TIME_ZONE_ID_UNKNOWN)
}

/// GetSystemTimeAsFileTime: the system's time (`now`) as a FILETIME.
pub(super) fn get_system_time_as_file_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let st = convert::system_time_of(now(cpu));
    let ticks = convert::system_time_to_ticks(&st).unwrap_or(0);
    write_file_time(cpu, args[0], ticks);
    Some(0)
}

/// CompareFileTime: -1, 0 or 1 as the first time is before, the same as
/// or after the second.
pub(super) fn compare_file_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = read_file_time(cpu, args[0])?;
    let b = read_file_time(cpu, args[1])?;
    Some(a.cmp(&b) as i32 as u32)
}

/// SetSystemTime and SetLocalTime: the machine's clock, which is both,
/// moves to the time given, if it is one.
pub(super) fn set_system_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let st = read_system_time(cpu, args[0])?;
    let Some(at) = convert::system_time_to_ticks(&st).and_then(|_| {
        chrono::NaiveDate::from_ymd_opt(st[0] as i32, st[1] as u32, st[3] as u32)?
            .and_hms_milli_opt(st[4] as u32, st[5] as u32, st[6] as u32, st[7] as u32)
    }) else {
        super::set_error(cpu, super::ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    };
    cpu.bus.cmos.set_now(at);
    Some(TRUE)
}
