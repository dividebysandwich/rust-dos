//! DATE and TIME: show the machine's clock and set it. The clock is the
//! host's, moved by what they (and DOS's and the BIOS's services) set it
//! to (`Cmos::now`); files keep the host's times.

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::shell::{ShellWait, enter_wait};
use crate::video::print_string;
use chrono::{Datelike, NaiveDate, NaiveTime, Timelike};

/// The line DATE or TIME asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinePurpose {
    Date,
    Time,
}

/// A date as DATE takes it: month, day and year, separated by '-', '/' or
/// '.'; a year of two digits is one from 1980 to 2079.
pub fn parse_date(text: &str) -> Option<NaiveDate> {
    let parts: Vec<u32> = text.trim().split(['-', '/', '.']).map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    let [month, day, year] = parts[..] else { return None };
    let year = match year {
        0..=79 => 2000 + year,
        80..=99 => 1900 + year,
        _ => year,
    };
    NaiveDate::from_ymd_opt(year as i32, month, day).filter(|d| (1980..=2099).contains(&d.year()))
}

/// A time as TIME takes it: hours[:minutes[:seconds[.hundredths]]], in
/// 24 hours or with 'a' or 'p' after it.
pub fn parse_time(text: &str) -> Option<NaiveTime> {
    let text = text.trim().to_ascii_lowercase();
    let (text, half) = match text.strip_suffix('m').unwrap_or(&text) {
        t if t.ends_with('a') => (t[..t.len() - 1].trim_end().to_string(), Some(false)),
        t if t.ends_with('p') => (t[..t.len() - 1].trim_end().to_string(), Some(true)),
        t => (t.to_string(), None),
    };
    let (hms, hundredths) = text.split_once('.').unwrap_or((&text, "0"));
    let parts: Vec<u32> = hms.split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let hundredths: u32 = format!("{:0<2}", hundredths.trim()).get(..2)?.parse().ok()?;
    let mut hour = parts[0];
    match half {
        Some(pm) if (1..=12).contains(&hour) => hour = hour % 12 + if pm { 12 } else { 0 },
        Some(_) => return None,
        None => {}
    }
    let (minute, second) = (parts.get(1).copied().unwrap_or(0), parts.get(2).copied().unwrap_or(0));
    NaiveTime::from_hms_milli_opt(hour, minute, second, hundredths * 10)
}

/// The date as DATE shows it: "Thu 09-25-2026".
fn show_date(cpu: &Cpu) -> String {
    cpu.bus.cmos.now().format("%a %m-%d-%Y").to_string()
}

/// The time as TIME shows it: " 2:03:05.12p".
fn show_time(cpu: &Cpu) -> String {
    let now = cpu.bus.cmos.now();
    let (pm, hour) = now.hour12();
    let hundredths = now.nanosecond() / 10_000_000 % 100;
    format!("{:>2}:{:02}:{:02}.{:02}{}", hour, now.minute(), now.second(), hundredths, if pm { 'p' } else { 'a' })
}

fn set_date(cpu: &mut Cpu, text: &str) -> bool {
    let Some(date) = parse_date(text) else { return false };
    let time = cpu.bus.cmos.now().time();
    cpu.bus.cmos.set_now(date.and_time(time));
    true
}

fn set_time(cpu: &mut Cpu, text: &str) -> bool {
    let Some(time) = parse_time(text) else { return false };
    let date = cpu.bus.cmos.now().date();
    cpu.bus.cmos.set_now(date.and_time(time));
    true
}

/// Ask for a new date or time on a line of its own.
fn ask(cpu: &mut Cpu, purpose: LinePurpose) {
    print_string(cpu, match purpose {
        LinePurpose::Date => "Enter new date (mm-dd-yy): ",
        LinePurpose::Time => "Enter new time: ",
    });
}

/// The line typed after DATE or TIME asked for one: an empty line leaves
/// the clock as it is, one it can't read asks again.
pub fn line_entered(cpu: &mut Cpu, purpose: LinePurpose, line: &str) {
    if line.trim().is_empty() {
        return;
    }
    let set = match purpose {
        LinePurpose::Date => set_date(cpu, line),
        LinePurpose::Time => set_time(cpu, line),
    };
    if !set {
        print_string(cpu, if purpose == LinePurpose::Date { "Invalid date\r\n" } else { "Invalid time\r\n" });
        ask(cpu, purpose);
        // The shell's code goes back to its line editor from the command
        // trap, and reads the next line for this.
        cpu.shell_wait = Some(ShellWait::Line(purpose));
    }
}

/// DATE [mm-dd-yy]: set the date, or show it and ask for a new one.
pub struct DateCommand;
impl ShellCommand for DateCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        if args.trim().is_empty() {
            let text = format!("Current date is {}\r\n", show_date(cpu));
            print_string(cpu, &text);
            ask(cpu, LinePurpose::Date);
            enter_wait(cpu, ShellWait::Line(LinePurpose::Date));
        } else if !set_date(cpu, args) {
            print_string(cpu, "Invalid date\r\n");
        }
    }
}

/// TIME [hh:mm[:ss[.xx]]]: set the time, or show it and ask for a new one.
pub struct TimeCommand;
impl ShellCommand for TimeCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        if args.trim().is_empty() {
            let text = format!("Current time is {}\r\n", show_time(cpu));
            print_string(cpu, &text);
            ask(cpu, LinePurpose::Time);
            enter_wait(cpu, ShellWait::Line(LinePurpose::Time));
        } else if !set_time(cpu, args) {
            print_string(cpu, "Invalid time\r\n");
        }
    }
}

crate::state_enum!(LinePurpose { LinePurpose::Date, LinePurpose::Time });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_times_are_read_as_dos_reads_them() {
        assert_eq!(parse_date("12-24-1993"), NaiveDate::from_ymd_opt(1993, 12, 24));
        assert_eq!(parse_date("1/2/03"), NaiveDate::from_ymd_opt(2003, 1, 2));
        assert_eq!(parse_date("7.4.85"), NaiveDate::from_ymd_opt(1985, 7, 4));
        assert_eq!(parse_date("13-01-1990"), None);
        assert_eq!(parse_date("01-01-1979"), None);
        assert_eq!(parse_time("14:30"), NaiveTime::from_hms_opt(14, 30, 0));
        assert_eq!(parse_time("2:30p"), NaiveTime::from_hms_opt(14, 30, 0));
        assert_eq!(parse_time("12:05 am"), NaiveTime::from_hms_opt(0, 5, 0));
        assert_eq!(parse_time("9:08:07.5"), NaiveTime::from_hms_milli_opt(9, 8, 7, 500));
        assert_eq!(parse_time("7"), NaiveTime::from_hms_opt(7, 0, 0));
        assert_eq!(parse_time("25:00"), None);
        assert_eq!(parse_time("13p"), None);
    }
}
