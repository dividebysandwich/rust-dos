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

/// Set the tick count at 0040:006C to `ticks` and clear the midnight
/// flag, as INT 1Ah AH=01h does. DOS sets it so when it sets the time.
pub fn set_tick_count(cpu: &mut Cpu, ticks: u32) {
    cpu.bus.guest_write_16(0x046E, (ticks >> 16) as u16);
    cpu.bus.guest_write_16(0x046C, ticks as u16);
    cpu.bus.guest_write_8(0x0470, 0);
}

/// The BIOS's time-of-day services, as an AT's BIOS has them: the tick
/// count (00h, 01h) and the real-time clock's time, date and alarm
/// (02h-07h). The AT's BIOS checks the function first and returns with CF
/// set and the registers unchanged for one it doesn't have; the services
/// it has return with CF clear unless they fail. The PCI BIOS (B1h) is
/// there on a machine with a PCI bus, and the PCjr's and Tandy 1000's
/// sound multiplexer (80h) on those machines.
pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    match ah {
        0x00 => {
            // The tick count the timer interrupt maintains, as a real BIOS
            // does, so it agrees with programs reading 0040:006C directly.
            let (high, low) = (cpu.bus.guest_read_16(0x046E), cpu.bus.guest_read_16(0x046C));
            cpu.set_cx(high);
            cpu.set_dx(low);
            // AL = midnight flag, cleared by the read
            let midnight = cpu.bus.guest_read_8(0x0470);
            cpu.bus.guest_write_8(0x0470, 0);
            cpu.set_reg8(Register::AL, midnight);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        0x01 => {
            // Set the tick count to CX:DX.
            let ticks = (cpu.cx() as u32) << 16 | cpu.dx() as u32;
            set_tick_count(cpu, ticks);
            cpu.set_cpu_flag(CpuFlags::CF, false);
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
        0x06 => set_alarm(cpu),
        0x07 => {
            // Reset the alarm: the alarm interrupt off.
            let status_b = cpu.bus.cmos.get(0x0B);
            cpu.bus.cmos.set(0x0B, status_b & !crate::cmos::AIE);
            cpu.bus.schedule_rtc();
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        // The PCjr's and the Tandy 1000's sound multiplexer: AL picks what
        // the speaker plays (0 the timer's channel 2, 1 the cassette, 2
        // the I/O channel's audio, 3 the sound chip), in bits 5 and 6 of
        // port 61h. Their BIOS returns with the caller's flags, as it does
        // for every function.
        0x80 if pcjr_or_tandy(cpu) => {
            let port_b = cpu.bus.io_read(0x61) & !0x60;
            cpu.bus.io_write(0x61, port_b | (cpu.get_al() & 0x03) << 5);
        }
        // The Tandy 1000 SL's and TL's digital sound (81h-84h: status,
        // record, play, stop; 85h resets it in DOSBox's Tandy DAC). No
        // machine here has the chip: CF set says so, as the BIOS of a
        // machine without it answers. Tandy drivers probe for it at
        // every start, so it isn't logged.
        0x81..=0x85 => cpu.set_cpu_flag(CpuFlags::CF, true),
        // The PCI BIOS, on a machine with a PCI bus.
        0xB1 => crate::pci::bios(cpu),
        _ => {
            cpu.bus.log_string(&format!("[BIOS] Unsupported INT 1A AH={:02X}", ah));
            cpu.set_cpu_flag(CpuFlags::CF, true);
        }
    }
}

/// Whether the machine is a PCjr or a Tandy 1000, whose BIOS has the
/// sound multiplexer.
fn pcjr_or_tandy(cpu: &Cpu) -> bool {
    use crate::video::adapter::Adapter;
    matches!(cpu.bus.vga.adapter, Adapter::Pcjr | Adapter::Tandy)
}

/// INT 1Ah AH=06h: set the alarm to CH hours, CL minutes and DH seconds
/// in BCD, as the AT's BIOS does: CF set if the alarm is set already,
/// else the alarm registers written, IRQ 8 unmasked at the slave PIC and
/// the alarm interrupt enabled. When the clock gets there, IRQ 8's
/// handler calls INT 4Ah.
fn set_alarm(cpu: &mut Cpu) {
    use crate::cmos::AIE;
    let status_b = cpu.bus.cmos.get(0x0B);
    if status_b & AIE != 0 {
        cpu.set_cpu_flag(CpuFlags::CF, true);
        return;
    }
    let (cx, dx) = (cpu.cx(), cpu.dx());
    cpu.bus.cmos.set(0x01, (dx >> 8) as u8);
    cpu.bus.cmos.set(0x03, cx as u8);
    cpu.bus.cmos.set(0x05, (cx >> 8) as u8);
    let mask = cpu.bus.io_read(0xA1);
    cpu.bus.io_write(0xA1, mask & !0x01);
    // The AT's BIOS clears the SET bit as it enables the alarm.
    cpu.bus.cmos.set(0x0B, (status_b & 0x7F) | AIE);
    cpu.bus.schedule_rtc();
    cpu.set_cpu_flag(CpuFlags::CF, false);
}
