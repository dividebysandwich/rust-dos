//! The host's local time, as the real-time clock, DOS's date and time
//! services and file times read it. A thread can fix it (`fix`), so that
//! two machines run side by side see the same time: the tests that compare
//! the interpreter with the dynamic recompiler do.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use std::cell::Cell;

thread_local! {
    static FIXED: Cell<Option<NaiveDateTime>> = const { Cell::new(None) };
}

/// The current local time, or the fixed one.
pub fn now() -> DateTime<Local> {
    match FIXED.with(Cell::get) {
        Some(at) => Local.from_local_datetime(&at).earliest().unwrap_or_else(Local::now),
        None => Local::now(),
    }
}

/// Make `now` return `at` on this thread, or the real time again (None).
pub fn fix(at: Option<NaiveDateTime>) {
    FIXED.with(|fixed| fixed.set(at));
}
