//! Where printed pages go (`output`): PNG pictures, PDF documents, a
//! host printer, or, before any of it, the bytes the program sent in a
//! file. The files are written away from the machine, on a thread of
//! their own, as compressing a page takes a while; the machine hears back
//! what became of them in `Event`s.

use super::page::Page;
use super::pdf::PdfWriter;
use super::{PrinterOutput, PrinterSettings};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// What became of a page or a print job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A file was written.
    Saved(PathBuf),
    /// A job went to the host's printer.
    Printed(String),
    Error(String),
}

enum Work {
    /// Bytes for the file of what the program sent (`output=file`).
    Raw(Vec<u8>),
    Page(Box<Page>),
    /// The job ends: its document or file closes.
    EndJob,
    /// Everything before it is done.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Sync(std::sync::mpsc::Sender<()>),
}

/// What turns pages into files: a thread with a `Sink`, or the sink
/// itself where there are no threads.
pub struct Output {
    #[cfg(not(target_arch = "wasm32"))]
    work: Option<std::sync::mpsc::Sender<Work>>,
    #[cfg(not(target_arch = "wasm32"))]
    events: std::sync::mpsc::Receiver<Event>,
    #[cfg(not(target_arch = "wasm32"))]
    thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(target_arch = "wasm32")]
    sink: Sink,
    #[cfg(target_arch = "wasm32")]
    events: Vec<Event>,
}

impl Output {
    pub fn new(settings: &PrinterSettings) -> Self {
        let sink = Sink::new(settings.clone());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (work_tx, work_rx) = std::sync::mpsc::channel::<Work>();
            let (event_tx, event_rx) = std::sync::mpsc::channel();
            let thread = std::thread::Builder::new()
                .name("printer".into())
                .spawn(move || {
                    let mut sink = sink;
                    for work in work_rx {
                        for event in sink.take(work) {
                            let _ = event_tx.send(event);
                        }
                    }
                    for event in sink.take(Work::EndJob) {
                        let _ = event_tx.send(event);
                    }
                })
                .ok();
            Self { work: Some(work_tx), events: event_rx, thread }
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self { sink, events: Vec::new() }
        }
    }

    fn send(&mut self, work: Work) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(tx) = &self.work {
            let _ = tx.send(work);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let events = self.sink.take(work);
            self.events.extend(events);
        }
    }

    pub fn raw(&mut self, bytes: Vec<u8>) {
        self.send(Work::Raw(bytes));
    }

    pub fn page(&mut self, page: Page) {
        self.send(Work::Page(Box::new(page)));
    }

    pub fn end_job(&mut self) {
        self.send(Work::EndJob);
    }

    /// Wait until the pages and jobs sent so far are done with.
    pub fn sync(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            self.send(Work::Sync(tx));
            let _ = rx.recv();
        }
    }

    /// What became of the pages since the last call.
    pub fn events(&mut self) -> Vec<Event> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.events.try_iter().collect()
        }
        #[cfg(target_arch = "wasm32")]
        {
            std::mem::take(&mut self.events)
        }
    }

    /// End the job, and wait for the files to be written. Returns what
    /// became of the last pages.
    pub fn close(&mut self) -> Vec<Event> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.work.take();
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
        #[cfg(target_arch = "wasm32")]
        self.end_job();
        self.events()
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        for event in self.close() {
            if let Event::Error(e) = event {
                eprintln!("[PRINTER] {}", e);
            }
        }
    }
}

/// Makes the files, a page or a job at a time.
struct Sink {
    settings: PrinterSettings,
    /// The document of the job printing (PDF, and the host printer's).
    pdf: Option<PdfWriter>,
    /// The file of what the program sent (`output=file`).
    raw: Option<(PathBuf, File)>,
}

impl Sink {
    fn new(settings: PrinterSettings) -> Self {
        Self { settings, pdf: None, raw: None }
    }

    fn take(&mut self, work: Work) -> Vec<Event> {
        let result = match work {
            Work::Raw(bytes) => self.raw(&bytes).map(|()| Vec::new()),
            Work::Page(page) => self.page(&page),
            Work::EndJob => self.end_job(),
            Work::Sync(done) => {
                let _ = done.send(());
                Ok(Vec::new())
            }
        };
        result.unwrap_or_else(|e| vec![Event::Error(e)])
    }

    fn raw(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.raw.is_none() {
            let path = crate::capture::capture_path(&self.settings.docpath, "print", "prn")?;
            let file = File::create(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
            self.raw = Some((path, file));
        }
        let (path, file) = self.raw.as_mut().unwrap();
        file.write_all(bytes).map_err(|e| format!("{}: {}", path.display(), e))
    }

    fn page(&mut self, page: &Page) -> Result<Vec<Event>, String> {
        match self.settings.output {
            PrinterOutput::Png => {
                let path = crate::capture::capture_path(&self.settings.docpath, "print", "png")?;
                save_png(page, &path)?;
                Ok(self.saved(path))
            }
            PrinterOutput::Pdf | PrinterOutput::Printer => {
                if self.pdf.is_none() {
                    let path = if self.settings.output == PrinterOutput::Printer {
                        let dir = std::env::temp_dir().join("rust-dos-print");
                        crate::capture::capture_path(&dir, "print", "pdf")?
                    } else {
                        crate::capture::capture_path(&self.settings.docpath, "print", "pdf")?
                    };
                    self.pdf = Some(PdfWriter::create(&path)?);
                }
                self.pdf.as_mut().unwrap().add_page(page)?;
                // One page a document unless pages go on in one.
                if self.settings.output == PrinterOutput::Pdf && !self.settings.multipage {
                    return self.end_job();
                }
                Ok(Vec::new())
            }
            PrinterOutput::File | PrinterOutput::None => Ok(Vec::new()),
        }
    }

    fn end_job(&mut self) -> Result<Vec<Event>, String> {
        let mut events = Vec::new();
        if let Some((path, file)) = self.raw.take() {
            drop(file);
            events.extend(self.saved(path));
        }
        if let Some(pdf) = self.pdf.take() {
            let path = pdf.finish()?;
            if self.settings.output == PrinterOutput::Printer {
                events.push(match spool(&self.settings, &path) {
                    Ok(message) => Event::Printed(message),
                    Err(e) => Event::Error(e),
                });
            } else {
                events.extend(self.saved(path));
            }
        }
        Ok(events)
    }

    /// A file written, opened with the `open_with` program if there is one.
    fn saved(&self, path: PathBuf) -> Vec<Event> {
        let mut events = Vec::new();
        if let Some(program) = self.settings.open_with.as_deref().filter(|p| !p.trim().is_empty())
            && let Err(e) = run_command(program, &path, false)
        {
            events.push(Event::Error(e));
        }
        events.insert(0, Event::Saved(path));
        events
    }
}

/// `page` as a PNG file at `path`, in gray if it has no colours.
fn save_png(page: &Page, path: &Path) -> Result<(), String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {}", path.display(), e);
    let file = File::create(path).map_err(|e| err(&e))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), page.width as u32, page.height as u32);
    let gray = page.is_gray();
    encoder.set_color(if gray { png::ColorType::Grayscale } else { png::ColorType::Rgb });
    encoder.set_depth(png::BitDepth::Eight);
    // The resolution, so viewers and printers know the page's size.
    let per_metre = (page.dpi as f64 / 0.0254).round() as u32;
    encoder.set_pixel_dims(Some(png::PixelDimensions { xppu: per_metre, yppu: per_metre, unit: png::Unit::Meter }));
    let mut writer = encoder.write_header().map_err(|e| err(&e))?;
    let pixels = if gray { page.to_gray() } else { page.to_rgb() };
    writer.write_image_data(&pixels).map_err(|e| err(&e))?;
    writer.finish().map_err(|e| err(&e))
}

/// Send the finished document at `path` to the host's printer: with
/// `print_command` if there is one, or else `lp` (CUPS) and on Windows
/// the shell's print verb. Returns what to tell the user.
fn spool(settings: &PrinterSettings, path: &Path) -> Result<String, String> {
    let printer = settings.device.as_deref().filter(|d| !d.trim().is_empty());
    let target = printer.map_or("the printer".to_string(), |d| format!("printer {}", d));
    if let Some(command) = settings.print_command.as_deref().filter(|c| !c.trim().is_empty()) {
        run_command(command, path, true)?;
        // The command is done with it.
        let _ = std::fs::remove_file(path);
        return Ok(format!("Sent the printout to {}", target));
    }
    #[cfg(windows)]
    {
        let script = format!("Start-Process -Verb Print -FilePath '{}'", path.display().to_string().replace('\'', "''"));
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .map_err(|e| format!("Can't start PowerShell to print: {}", e))?;
        if !status.success() {
            return Err(format!("Printing {} failed: no program prints PDF files", path.display()));
        }
        Ok(format!("Sent the printout to {}", target))
    }
    #[cfg(all(not(windows), not(target_arch = "wasm32")))]
    {
        let mut lp = std::process::Command::new("lp");
        if let Some(printer) = printer {
            lp.args(["-d", printer]);
        }
        lp.args(["-t", "rust-dos"]).arg(path);
        let out = lp.output().map_err(|e| format!("Can't run lp to print: {}", e))?;
        if !out.status.success() {
            let message = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(format!("lp failed: {}", message));
        }
        // lp took a copy.
        let _ = std::fs::remove_file(path);
        let job = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(if job.is_empty() { format!("Sent the printout to {}", target) } else { job })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (path, target);
        Err("There is no printer here".to_string())
    }
}

/// Run `command` on the file at `path`, through the shell: in place of
/// `{file}`, or after the command. `wait` waits for it and fails if it
/// does.
fn run_command(command: &str, path: &Path, wait: bool) -> Result<(), String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (command, path, wait);
        Err("Programs can't be run here".to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let file = path.display().to_string();
        let quoted = if cfg!(windows) { format!("\"{}\"", file) } else { format!("'{}'", file.replace('\'', "'\\''")) };
        let line =
            if command.contains("{file}") { command.replace("{file}", &quoted) } else { format!("{} {}", command, quoted) };
        let mut shell = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", &line]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", &line]);
            c
        };
        if wait {
            let status = shell.status().map_err(|e| format!("Can't run {}: {}", command, e))?;
            if !status.success() {
                return Err(format!("{} failed ({})", line, status));
            }
        } else {
            shell.spawn().map_err(|e| format!("Can't run {}: {}", command, e))?;
        }
        Ok(())
    }
}
