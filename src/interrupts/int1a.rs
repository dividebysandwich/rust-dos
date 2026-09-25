use iced_x86::Register;
use crate::cpu::{Cpu, CpuFlags};
use chrono::{Datelike, NaiveDate, NaiveTime, Timelike};

fn bcd(value: u32) -> u8 {
    (((value / 10) % 10) << 4 | (value % 10)) as u8
}

/// The value of a BCD byte, if it is one.
fn from_bcd(value: u8) -> Option<u32> {
    let (high, low) = (value >> 4, value & 0x0F);
    (high < 10 && low < 10).then_some(high as u32 * 10 + low as u32)
}

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    match ah {
        0x00 => {
            // The tick count the timer interrupt maintains, as a real BIOS
            // does, so it agrees with programs reading 0040:006C directly.
            cpu.set_cx(cpu.bus.read_16(0x046E));
            cpu.set_dx(cpu.bus.read_16(0x046C));
            // AL = midnight flag, cleared by the read
            let midnight = cpu.bus.read_8(0x0470);
            cpu.bus.write_8(0x0470, 0);
            cpu.set_reg8(Register::AL, midnight);
        }
        0x01 => {
            // Set the tick count to CX:DX.
            cpu.bus.write_16(0x046E, cpu.cx());
            cpu.bus.write_16(0x046C, cpu.dx());
            cpu.bus.write_8(0x0470, 0);
        }
        0x02 => {
            // The real-time clock's time: CH hours, CL minutes, DH seconds
            // in BCD, DL no daylight saving.
            let now = cpu.bus.cmos.now();
            cpu.set_cx((bcd(now.hour()) as u16) << 8 | bcd(now.minute()) as u16);
            cpu.set_dx((bcd(now.second()) as u16) << 8);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        0x03 => {
            // Set the real-time clock's time from CH, CL and DH in BCD.
            let (cx, dx) = (cpu.cx(), cpu.dx());
            let time = from_bcd((cx >> 8) as u8)
                .zip(from_bcd(cx as u8))
                .zip(from_bcd((dx >> 8) as u8))
                .and_then(|((h, m), s)| NaiveTime::from_hms_opt(h, m, s));
            if let Some(time) = time {
                let date = cpu.bus.cmos.now().date();
                cpu.bus.cmos.set_now(date.and_time(time));
            }
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        0x04 => {
            // The real-time clock's date: CH century, CL year, DH month,
            // DL day in BCD.
            let now = cpu.bus.cmos.now();
            let year = now.year() as u32;
            cpu.set_cx((bcd(year / 100) as u16) << 8 | bcd(year % 100) as u16);
            cpu.set_dx((bcd(now.month()) as u16) << 8 | bcd(now.day()) as u16);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        0x05 => {
            // Set the real-time clock's date from CX and DX in BCD.
            let (cx, dx) = (cpu.cx(), cpu.dx());
            let date = from_bcd((cx >> 8) as u8)
                .zip(from_bcd(cx as u8))
                .zip(from_bcd((dx >> 8) as u8).zip(from_bcd(dx as u8)))
                .and_then(|((century, year), (month, day))| NaiveDate::from_ymd_opt((century * 100 + year) as i32, month, day));
            if let Some(date) = date {
                let time = cpu.bus.cmos.now().time();
                cpu.bus.cmos.set_now(date.and_time(time));
            }
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        _ => cpu.bus.log_string(&format!("[BIOS] Unhandled INT 1A AH={:02X}", ah)),
    }
}
