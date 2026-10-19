//! Times: the clock's, and conversions between SYSTEMTIME, FILETIME and
//! DOS's dates and times. There are no time zones yet: local time is the
//! machine's clock, and UTC is the same.

use super::{FALSE, TRUE};
use crate::cpu::Cpu;
use crate::win32::convert::{self, SystemTime};

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

/// GetSystemTime and GetLocalTime: the machine's clock.
pub(super) fn get_system_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let st = convert::system_time_of(cpu.bus.cmos.now());
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
