//! A printer on the parallel port LPT1 (378h), as DOSBox-X has one
//! (`[printer]`): programs print through the port itself, strobing each
//! byte in, or through the BIOS (INT 17h) and DOS (PRN, LPT1). What they
//! print goes into a file as it is (`output=file`), or through an Epson
//! ESC/P 2 printer (escp.rs) onto pages that become PNG pictures, PDF
//! documents or a printout from the host's printer.
//!
//! A print job ends when the printer has had nothing for `timeout`
//! milliseconds of the machine's time, or when the user ejects the page;
//! then the page in the printer comes out, and the job's document is
//! finished.

pub mod escp;
pub mod font;
pub mod output;
pub mod page;
pub mod pdf;

use escp::Escp;
use output::{Event, Output};
use std::path::PathBuf;

/// Where printing goes (`output`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrinterOutput {
    /// No printer: LPT1 isn't there.
    None,
    /// The bytes as the program sent them, in a file.
    File,
    /// A PNG picture of each page.
    Png,
    /// A PDF document of each job.
    Pdf,
    /// The host's printer.
    Printer,
}

impl PrinterOutput {
    pub const ALL: [PrinterOutput; 5] =
        [PrinterOutput::None, PrinterOutput::Pdf, PrinterOutput::Png, PrinterOutput::Printer, PrinterOutput::File];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "false" => Some(PrinterOutput::None),
            "file" | "raw" => Some(PrinterOutput::File),
            "png" => Some(PrinterOutput::Png),
            "pdf" => Some(PrinterOutput::Pdf),
            "printer" => Some(PrinterOutput::Printer),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            PrinterOutput::None => "none",
            PrinterOutput::File => "file",
            PrinterOutput::Png => "png",
            PrinterOutput::Pdf => "pdf",
            PrinterOutput::Printer => "printer",
        }
    }

    /// As the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            PrinterOutput::None => "none",
            PrinterOutput::File => "file (as sent)",
            PrinterOutput::Png => "PNG pictures",
            PrinterOutput::Pdf => "PDF documents",
            PrinterOutput::Printer => "host printer",
        }
    }
}

/// The paper in the printer (`paper`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paper {
    Letter,
    Legal,
    A4,
    /// Width and height in hundredths of an inch.
    Custom(u32, u32),
}

impl Paper {
    pub const ALL: [Paper; 3] = [Paper::Letter, Paper::A4, Paper::Legal];

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "letter" => return Some(Paper::Letter),
            "legal" => return Some(Paper::Legal),
            "a4" => return Some(Paper::A4),
            _ => {}
        }
        let (w, h) = s.split_once('x')?;
        let inches = |v: &str| v.trim().parse::<f64>().ok().filter(|v| (1.0..=30.0).contains(v));
        Some(Paper::Custom((inches(w)? * 100.0).round() as u32, (inches(h)? * 100.0).round() as u32))
    }

    pub fn name(self) -> String {
        match self {
            Paper::Letter => "letter".into(),
            Paper::Legal => "legal".into(),
            Paper::A4 => "a4".into(),
            Paper::Custom(w, h) => format!("{}x{}", w as f64 / 100.0, h as f64 / 100.0),
        }
    }

    /// Width and height in inches.
    pub fn inches(self) -> (f64, f64) {
        match self {
            Paper::Letter => (8.5, 11.0),
            Paper::Legal => (8.5, 14.0),
            Paper::A4 => (210.0 / 25.4, 297.0 / 25.4),
            Paper::Custom(w, h) => (w as f64 / 100.0, h as f64 / 100.0),
        }
    }
}

/// The `[printer]` settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrinterSettings {
    pub output: PrinterOutput,
    pub dpi: u32,
    pub paper: Paper,
    /// Whether a job's pages go into one PDF document, or each into its own.
    pub multipage: bool,
    /// Milliseconds without printing that end a job (0: only ejecting does).
    pub timeout: u32,
    /// Where the files go; empty for the capture folder (`capture_dir`).
    pub docpath: PathBuf,
    /// Where the printer's fonts are (roman.ttf, courier.ttf, ...).
    pub fontpath: Option<PathBuf>,
    /// The host printer (`lp -d`), or the default one.
    pub device: Option<String>,
    /// What prints a finished document in place of lp.
    pub print_command: Option<String>,
    /// A program to open each file written with.
    pub open_with: Option<String>,
}

impl Default for PrinterSettings {
    fn default() -> Self {
        Self {
            output: PrinterOutput::Pdf,
            dpi: 360,
            paper: Paper::Letter,
            multipage: true,
            timeout: 3000,
            docpath: PathBuf::new(),
            fontpath: None,
            device: None,
            print_command: None,
            open_with: None,
        }
    }
}

impl PrinterSettings {
    /// Take `key=value` of the `[printer]` section, `~/` in paths being
    /// in `home`.
    pub fn set(&mut self, key: &str, value: &str, home: Option<&std::path::Path>) -> Result<(), String> {
        let key = key.to_ascii_lowercase();
        let text = value.trim().trim_matches('"');
        let optional = |v: &str| (!v.is_empty()).then(|| v.to_string());
        let path = |v: &str| match (v.strip_prefix("~/"), home) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => PathBuf::from(v),
        };
        match key.as_str() {
            "output" => {
                self.output = PrinterOutput::parse(text)
                    .ok_or_else(|| format!("invalid output '{}' (none, pdf, png, printer or file)", value))?
            }
            "dpi" => {
                self.dpi = text
                    .parse()
                    .ok()
                    .filter(|d| (60..=720).contains(d))
                    .ok_or_else(|| format!("invalid dpi '{}' (60 to 720)", value))?
            }
            "paper" => {
                self.paper = Paper::parse(text)
                    .ok_or_else(|| format!("invalid paper '{}' (letter, a4, legal or <width>x<height> in inches)", value))?
            }
            "multipage" => {
                self.multipage = match text.to_ascii_lowercase().as_str() {
                    "true" | "on" | "yes" | "1" => true,
                    "false" | "off" | "no" | "0" => false,
                    _ => return Err(format!("invalid multipage '{}' (true or false)", value)),
                }
            }
            "timeout" => {
                self.timeout = text.parse().map_err(|_| format!("invalid timeout '{}' (milliseconds, 0 for none)", value))?
            }
            "docpath" => self.docpath = if text.is_empty() { PathBuf::new() } else { path(text) },
            "fontpath" => self.fontpath = (!text.is_empty()).then(|| path(text)),
            "device" => self.device = optional(text),
            "print_command" => self.print_command = optional(value.trim()),
            "open_with" => self.open_with = optional(value.trim()),
            _ => return Err(format!("unknown setting '{}'", key)),
        }
        Ok(())
    }

    /// The settings as the configuration file has them.
    pub fn entries(&self, home: Option<&std::path::Path>) -> Vec<(&'static str, Option<String>)> {
        let path = |p: &PathBuf| crate::mount::contract_home(p, home);
        vec![
            ("output", Some(self.output.name().to_string())),
            ("dpi", Some(self.dpi.to_string())),
            ("paper", Some(self.paper.name())),
            ("multipage", Some(self.multipage.to_string())),
            ("timeout", Some(self.timeout.to_string())),
            ("docpath", (!self.docpath.as_os_str().is_empty()).then(|| path(&self.docpath))),
            ("fontpath", self.fontpath.as_ref().map(path)),
            ("device", self.device.clone()),
            ("print_command", self.print_command.clone()),
            ("open_with", self.open_with.clone()),
        ]
    }

    /// Whether LPT1 has the printer on it.
    pub fn present(&self) -> bool {
        self.output != PrinterOutput::None
    }
}

/// The status port's bits: not busy (7), acknowledge (6, low for a
/// moment after each byte), selected (4) and no error (3).
const STATUS_IDLE: u8 = 0xDF;
const STATUS_ACK: u8 = 0x40;
/// The control port's bits: strobe (0), auto feed (1), initialize (2,
/// active low).
const CONTROL_STROBE: u8 = 0x01;
const CONTROL_AUTOFEED: u8 = 0x02;
const CONTROL_INIT: u8 = 0x04;
/// What the file of `output=file` gets at a time.
const RAW_CHUNK: usize = 64 * 1024;

/// The printer on LPT1.
pub struct Printer {
    pub settings: PrinterSettings,
    data: u8,
    control: u8,
    /// A byte was taken and the next status read says so.
    ack: bool,
    /// The Epson printer, from the first byte it gets.
    escp: Option<Box<Escp>>,
    raw: Vec<u8>,
    output: Output,
    /// A job is printing, the last byte at this time (PIT ticks).
    job: Option<u64>,
    /// Bytes printed and pages fed out since the machine started.
    pub bytes: u64,
    pub pages: u64,
    pub jobs: u64,
    /// The last file written, or the last job sent to the printer.
    pub last: Option<String>,
    /// What to show the user, and what goes to the log.
    notices: Vec<String>,
    log: Vec<String>,
}

impl Printer {
    pub fn new(settings: PrinterSettings) -> Self {
        let output = Output::new(&settings);
        Self {
            settings,
            data: 0,
            control: CONTROL_INIT,
            ack: false,
            escp: None,
            raw: Vec::new(),
            output,
            job: None,
            bytes: 0,
            pages: 0,
            jobs: 0,
            last: None,
            notices: Vec::new(),
            log: Vec::new(),
        }
    }

    pub fn read_data(&self) -> u8 {
        self.data
    }

    pub fn write_data(&mut self, value: u8) {
        self.data = value;
    }

    pub fn read_status(&mut self) -> u8 {
        if std::mem::take(&mut self.ack) { STATUS_IDLE & !STATUS_ACK } else { STATUS_IDLE }
    }

    pub fn read_control(&self) -> u8 {
        0xE0 | self.control
    }

    /// A byte goes in on the falling edge of strobe; the printer starts
    /// over on the rising edge of initialize.
    pub fn write_control(&mut self, value: u8, now: u64) {
        let old = self.control;
        self.control = value & 0x1F;
        if value & CONTROL_INIT != 0 && old & CONTROL_INIT == 0 {
            self.init();
        }
        if value & CONTROL_STROBE == 0 && old & CONTROL_STROBE != 0 {
            self.put_byte(self.data, now);
        }
    }

    /// Print `byte` (as the port takes it, and INT 17h and DOS give it).
    pub fn put_byte(&mut self, byte: u8, now: u64) {
        self.ack = true;
        self.bytes += 1;
        if self.job.is_none() {
            self.jobs += 1;
        }
        self.job = Some(now);
        if self.settings.output == PrinterOutput::File {
            self.raw.push(byte);
            // The port's auto feed adds line feeds, as the printer does.
            if byte == b'\r' && self.control & CONTROL_AUTOFEED != 0 {
                self.raw.push(b'\n');
            }
            if self.raw.len() >= RAW_CHUNK {
                self.output.raw(std::mem::take(&mut self.raw));
            }
            return;
        }
        let settings = &self.settings;
        let escp = self.escp.get_or_insert_with(|| {
            let (w, h) = settings.paper.inches();
            Box::new(Escp::new(w, h, settings.dpi, settings.fontpath.clone()))
        });
        escp.auto_feed = self.control & CONTROL_AUTOFEED != 0;
        escp.print(byte);
        self.take_pages();
    }

    /// The pages the printer fed out, to the output; what it didn't
    /// understand, to the log.
    fn take_pages(&mut self) {
        let Some(escp) = &mut self.escp else { return };
        for what in escp.unknown.drain(..) {
            self.log.push(format!("[PRINTER] Ignored {}", what));
        }
        for page in escp.done.drain(..) {
            self.pages += 1;
            self.output.page(page);
        }
    }

    /// Start over with the printer's settings (INIT, INT 17h AH=01h).
    pub fn init(&mut self) {
        if let Some(escp) = &mut self.escp {
            escp.reset();
        }
    }

    /// Whether a job is printing.
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// End the job: the page in the printer comes out if anything is on
    /// it, and the job's document or file is finished.
    pub fn eject(&mut self) {
        if let Some(escp) = &mut self.escp {
            escp.form_feed();
        }
        self.take_pages();
        if !self.raw.is_empty() {
            self.output.raw(std::mem::take(&mut self.raw));
        }
        self.output.end_job();
        self.job = None;
    }

    /// At `now` (PIT ticks): end a job the timeout is over for, and hear
    /// what became of the pages.
    pub fn poll(&mut self, now: u64) {
        if let Some(last) = self.job
            && self.settings.timeout > 0
            && now.saturating_sub(last) >= self.settings.timeout as u64 * crate::timer::PIT_HZ / 1000
        {
            self.eject();
        }
        let events = self.output.events();
        self.take_events(events);
    }

    fn take_events(&mut self, events: Vec<Event>) {
        for event in events {
            let line = match event {
                Event::Saved(path) => {
                    self.last = Some(path.display().to_string());
                    format!("Printed to {}", path.display())
                }
                Event::Printed(message) => {
                    self.last = Some(message.clone());
                    message
                }
                Event::Error(e) => format!("Printing failed: {}", e),
            };
            self.log.push(format!("[PRINTER] {}", line));
            self.notices.push(line);
        }
    }

    /// Wait until what was printed so far is written (for the debugger
    /// and tests).
    pub fn sync(&mut self) {
        self.output.sync();
        let events = self.output.events();
        self.take_events(events);
    }

    /// The job ends, and its files are written before this returns: the
    /// machine is going away.
    pub fn finish(&mut self) -> Vec<String> {
        self.eject();
        let events = self.output.close();
        self.take_events(events);
        self.take_notices()
    }

    /// What to tell the user since the last call.
    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    /// What to log since the last call.
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }
}

/// A printer going away (the machine, or its settings, changing) finishes
/// its job: the page in it comes out, and the files are written.
impl Drop for Printer {
    fn drop(&mut self) {
        self.eject();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(output: PrinterOutput, dir: &std::path::Path) -> PrinterSettings {
        PrinterSettings {
            output,
            dpi: 90,
            docpath: dir.to_path_buf(),
            fontpath: Some(dir.join("no fonts")),
            ..Default::default()
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rust-dos-printer-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Strobe `byte` in as a program does: data, strobe low, strobe high.
    fn strobe(printer: &mut Printer, byte: u8) {
        printer.write_data(byte);
        printer.write_control(0x0D, 0);
        printer.write_control(0x0C, 0);
        printer.write_control(0x0D, 0);
    }

    #[test]
    fn the_port_takes_bytes_on_strobe_and_acknowledges_them() {
        let dir = temp_dir("port");
        let mut printer = Printer::new(settings(PrinterOutput::File, &dir));
        assert_eq!(printer.read_status(), 0xDF);
        printer.write_control(0x0D, 0);
        // Data alone prints nothing.
        printer.write_data(b'A');
        assert_eq!(printer.bytes, 0);
        printer.write_control(0x0C, 0);
        assert_eq!(printer.bytes, 1);
        // Acknowledge, once.
        assert_eq!(printer.read_status(), 0x9F);
        assert_eq!(printer.read_status(), 0xDF);
        // Strobe going up again prints nothing more.
        printer.write_control(0x0D, 0);
        assert_eq!(printer.bytes, 1);
        assert_eq!(printer.read_control(), 0xED);
        // Auto feed adds line feeds after carriage returns.
        printer.write_control(0x0F, 0);
        printer.write_data(b'\r');
        printer.write_control(0x0E, 0);
        printer.eject();
        printer.sync();
        let written = printer.last.clone().unwrap();
        assert!(written.ends_with(".prn"), "{}", written);
        assert_eq!(std::fs::read(&written).unwrap(), b"A\r\n");
        assert_eq!(printer.take_notices(), vec![format!("Printed to {}", written)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_job_ends_after_the_timeout() {
        let dir = temp_dir("timeout");
        let mut printer = Printer::new(settings(PrinterOutput::Png, &dir));
        for &b in b"Hello" {
            printer.put_byte(b, 1000);
        }
        assert!(printer.busy());
        printer.poll(1000 + crate::timer::PIT_HZ);
        assert!(printer.busy());
        printer.poll(1000 + 3 * crate::timer::PIT_HZ);
        assert!(!printer.busy());
        printer.sync();
        assert_eq!(printer.pages, 1);
        let written = printer.last.clone().unwrap();
        assert!(written.ends_with(".png"), "{}", written);
        let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&written).unwrap()));
        let info = decoder.read_info().unwrap();
        assert_eq!((info.info().width, info.info().height), (765, 990));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pdf_takes_the_jobs_pages() {
        let dir = temp_dir("pdf");
        let mut printer = Printer::new(settings(PrinterOutput::Pdf, &dir));
        for &b in b"One\x0cTwo\x0cThree" {
            strobe(&mut printer, b);
        }
        let notices = printer.finish();
        assert_eq!(printer.pages, 3);
        assert_eq!(notices.len(), 1, "{:?}", notices);
        let written = printer.last.clone().unwrap();
        let text = String::from_utf8_lossy(&std::fs::read(&written).unwrap()).to_string();
        assert!(text.contains("/Count 3"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_read_and_write() {
        let mut s = PrinterSettings::default();
        s.set("output", "png", None).unwrap();
        s.set("paper", "a4", None).unwrap();
        s.set("dpi", "180", None).unwrap();
        s.set("timeout", "0", None).unwrap();
        s.set("device", "Office", None).unwrap();
        s.set("print_command", "lpr -P x", None).unwrap();
        assert!(s.set("output", "fax", None).is_err());
        assert!(s.set("paper", "0x3", None).is_err());
        assert_eq!(Paper::parse("8.5x14"), Some(Paper::Custom(850, 1400)));
        let entries = s.entries(None);
        assert!(entries.contains(&("output", Some("png".into()))));
        assert!(entries.contains(&("paper", Some("a4".into()))));
        assert!(entries.contains(&("print_command", Some("lpr -P x".into()))));
        assert!(entries.contains(&("docpath", None)));
        let mut again = PrinterSettings::default();
        for (key, value) in entries {
            if let Some(value) = value {
                again.set(key, &value, None).unwrap();
            }
        }
        assert_eq!(again, s);
    }
}
