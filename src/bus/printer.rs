//! The printer on the bus: its port at LPT1 (378h-37Ah) while the DAC
//! isn't there, the BIOS data area's LPT1, its print jobs timing out
//! with the machine's time, and what it has to tell.

use super::Bus;
use crate::lpt_dac::{CONTROL, DATA, LPT1, STATUS};
use crate::printer::{Printer, PrinterSettings};

/// Parallel port I/O takes an ISA bus cycle.
const LPT_NS: u64 = 1000;

impl Bus {
    /// Put the `[printer]` settings in place: a job printing ends, and the
    /// printer starts anew with them. Its files go where `docpath` says,
    /// or else in `printer_dir` if the front end has one, or else in the
    /// capture folder.
    pub fn configure_printer(&mut self, settings: &PrinterSettings, capture_dir: &std::path::Path) {
        let mut settings = settings.clone();
        if settings.docpath.as_os_str().is_empty() {
            settings.docpath = self.printer_dir.clone().unwrap_or_else(|| capture_dir.to_path_buf());
        }
        if settings == self.printer_settings && self.printer.is_some() == self.printer_wanted() {
            return;
        }
        self.printer_settings = settings;
        self.place_printer();
    }

    /// Whether LPT1 should have the printer: the settings want one and
    /// no DAC is plugged in there. (The browser has nowhere for the
    /// printouts to go.)
    fn printer_wanted(&self) -> bool {
        self.printer_settings.present() && self.lpt_dac.is_none() && !cfg!(target_arch = "wasm32")
    }

    /// Plug the printer into LPT1 or take it out, as the settings and the
    /// DAC have it, and tell the BIOS data area.
    pub(crate) fn place_printer(&mut self) {
        if let Some(mut old) = self.printer.take() {
            self.printer_done(&mut old);
        }
        if self.printer_wanted() {
            self.log_string(&format!("[PRINTER] LPT1 at {:X}h: {}", LPT1, self.printer_settings.output.describe()));
            self.printer = Some(Printer::new(self.printer_settings.clone()));
        } else if self.printer_settings.present() {
            self.log_string("[PRINTER] No printer: LPT1 has the DAC (lpt_dac)");
        }
        if self.boot.is_none() {
            self.write_lpt_bda();
        }
    }

    /// The `[printer]` settings in place.
    pub fn printer_output_settings(&self) -> &PrinterSettings {
        &self.printer_settings
    }

    /// Whether LPT1 has anything on it.
    pub fn lpt1_present(&self) -> bool {
        self.lpt_dac.is_some() || self.printer.is_some()
    }

    /// LPT1 in the BIOS data area (40:08, its timeout at 40:78) and the
    /// equipment word (bits 14-15).
    pub(crate) fn write_lpt_bda(&mut self) {
        let present = self.lpt1_present();
        self.write_16(0x0408, if present { LPT1 } else { 0 });
        self.write_8(0x0478, if present { 20 } else { 0 });
        let equipment = self.read_16(0x0410) & !0xC000;
        self.write_16(0x0410, equipment | if present { 0x4000 } else { 0 });
    }

    #[inline]
    pub(crate) fn printer_claims(&self, port: u16) -> bool {
        (DATA..=CONTROL).contains(&port) && self.printer.is_some()
    }

    pub(crate) fn printer_read(&mut self, port: u16) -> u8 {
        self.clock.stall(LPT_NS);
        let Some(p) = &mut self.printer else { return 0xFF };
        match port {
            DATA => p.read_data(),
            STATUS => p.read_status(),
            _ => p.read_control(),
        }
    }

    pub(crate) fn printer_write(&mut self, port: u16, value: u8) {
        self.clock.stall(LPT_NS);
        let now = self.clock.now_ticks();
        let Some(p) = &mut self.printer else { return };
        match port {
            DATA => p.write_data(value),
            CONTROL => p.write_control(value, now),
            _ => {}
        }
    }

    /// Print `byte` for the BIOS or DOS. False without a printer.
    pub fn printer_put(&mut self, byte: u8) -> bool {
        let now = self.clock.now_ticks();
        match &mut self.printer {
            Some(p) => {
                p.put_byte(byte, now);
                true
            }
            None => false,
        }
    }

    /// At the start of a batch: end the print job the timeout is over
    /// for, and log what the printer has to say.
    pub(crate) fn printer_poll(&mut self) {
        let now = self.clock.now_ticks();
        let Some(p) = &mut self.printer else { return };
        p.poll(now);
        for line in p.take_log() {
            self.log_string(&line);
        }
    }

    /// Eject the page and end the print job (the user's eject key).
    /// Returns what became of it, or None without a printer.
    pub fn printer_eject(&mut self) -> Option<String> {
        let p = self.printer.as_mut()?;
        let busy = p.busy();
        p.eject();
        Some(if busy { "Printing the page".to_string() } else { "Nothing to print".to_string() })
    }

    /// What the printer has to tell the user since the last call.
    pub fn printer_notices(&mut self) -> Vec<String> {
        self.printer.as_mut().map(Printer::take_notices).unwrap_or_default()
    }

    /// The machine goes away: the job printing is finished and written.
    /// Returns what became of it.
    pub fn finish_printing(&mut self) -> Vec<String> {
        match self.printer.take() {
            Some(mut p) => self.printer_done(&mut p),
            None => Vec::new(),
        }
    }

    fn printer_done(&mut self, p: &mut Printer) -> Vec<String> {
        let notices = p.finish();
        for line in p.take_log() {
            self.log_string(&line);
        }
        notices
    }
}
