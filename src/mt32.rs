//! The Roland MT-32 and CM-32L on the MPU-401 (`midisynth=mt32`), played by
//! munt's emulation of them, libmt32emu. rust-dos doesn't link the library:
//! it loads it when the MT-32 is chosen, so the program runs without munt
//! installed, and plays the MT-32 wherever it is. The synthesizer needs the
//! module's ROMs, a control ROM and a PCM ROM of the same model, which
//! munt identifies from their contents whatever the files are called.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use libloading::Library;

pub use crate::config::Mt32Model;

/// Frames munt renders at a time.
const BLOCK: usize = 64;

/// Where the ROMs are looked for without an `mt32roms` setting: rust-dos's
/// own directory, DOSBox Staging's, and where Linux packages of munt's ROM
/// data put them.
pub fn default_rom_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(user) = crate::config::user_dir() {
        dirs.push(user.join("mt32-roms"));
    }
    if let Some(config) = dirs::config_dir() {
        dirs.push(config.join("dosbox").join("mt32-roms"));
    }
    if cfg!(unix) {
        dirs.push(PathBuf::from("/usr/share/mt32-rom-data"));
        dirs.push(PathBuf::from("/usr/local/share/mt32-rom-data"));
    }
    dirs
}

// ---------------------------------------------------------------------------
// The library's C interface (mt32emu/c_interface/c_interface.h).

type Context = *mut c_void;

#[repr(C)]
struct RomInfo {
    control_rom_id: *const c_char,
    control_rom_description: *const c_char,
    control_rom_sha1_digest: *const c_char,
    pcm_rom_id: *const c_char,
    pcm_rom_description: *const c_char,
    pcm_rom_sha1_digest: *const c_char,
}

/// The first version of munt's report handler interface: the callbacks it
/// makes. The ones left out (None) get munt's own handling.
#[repr(C)]
struct ReportHandlerV0 {
    get_version_id: Option<extern "C" fn(ReportHandler) -> c_int>,
    // The va_list is passed as a pointer on every host rust-dos builds for;
    // it is never looked at.
    print_debug: Option<extern "C" fn(*mut c_void, *const c_char, *mut c_void)>,
    on_error_control_rom: Option<extern "C" fn(*mut c_void)>,
    on_error_pcm_rom: Option<extern "C" fn(*mut c_void)>,
    show_lcd_message: Option<extern "C" fn(*mut c_void, *const c_char)>,
    on_midi_message_played: Option<extern "C" fn(*mut c_void)>,
    on_midi_queue_overflow: Option<extern "C" fn(*mut c_void) -> c_int>,
    on_midi_system_realtime: Option<extern "C" fn(*mut c_void, u8)>,
    on_device_reset: Option<extern "C" fn(*mut c_void)>,
    on_device_reconfig: Option<extern "C" fn(*mut c_void)>,
    on_new_reverb_mode: Option<extern "C" fn(*mut c_void, u8)>,
    on_new_reverb_time: Option<extern "C" fn(*mut c_void, u8)>,
    on_new_reverb_level: Option<extern "C" fn(*mut c_void, u8)>,
    on_poly_state_changed: Option<extern "C" fn(*mut c_void, u8)>,
    on_program_changed: Option<extern "C" fn(*mut c_void, u8, *const c_char, *const c_char)>,
}

/// `mt32emu_report_handler_i`: a union of pointers to the interface's
/// versions, of which rust-dos gives the first.
#[repr(C)]
#[derive(Clone, Copy)]
struct ReportHandler {
    v0: *const ReportHandlerV0,
}

extern "C" fn report_version(_: ReportHandler) -> c_int {
    0
}

extern "C" fn print_debug(_: *mut c_void, _: *const c_char, _: *mut c_void) {}

/// What the MT-32 shows on its display: the messages games send it
/// ("Insert Buckazoid" and the like).
extern "C" fn show_lcd_message(instance: *mut c_void, message: *const c_char) {
    if instance.is_null() || message.is_null() {
        return;
    }
    // SAFETY: `instance` is the `Mutex` of the `Mt32` that created the
    // context, which outlives it; munt passes a NUL-terminated string.
    let lcd = unsafe { &*(instance as *const Mutex<Option<String>>) };
    let text = unsafe { CStr::from_ptr(message) }.to_string_lossy().trim().to_string();
    if let Ok(mut lcd) = lcd.lock() {
        *lcd = Some(text);
    }
}

static REPORT_HANDLER: ReportHandlerV0 = ReportHandlerV0 {
    get_version_id: Some(report_version),
    print_debug: Some(print_debug),
    on_error_control_rom: None,
    on_error_pcm_rom: None,
    show_lcd_message: Some(show_lcd_message),
    on_midi_message_played: None,
    on_midi_queue_overflow: None,
    on_midi_system_realtime: None,
    on_device_reset: None,
    on_device_reconfig: None,
    on_new_reverb_mode: None,
    on_new_reverb_time: None,
    on_new_reverb_level: None,
    on_poly_state_changed: None,
    on_program_changed: None,
};

/// The library's functions rust-dos calls.
struct Api {
    version: unsafe extern "C" fn() -> *const c_char,
    create_context: unsafe extern "C" fn(ReportHandler, *mut c_void) -> Context,
    free_context: unsafe extern "C" fn(Context),
    identify_rom_file: unsafe extern "C" fn(*mut RomInfo, *const c_char, *const c_char) -> c_int,
    add_rom_file: unsafe extern "C" fn(Context, *const c_char) -> c_int,
    set_stereo_output_samplerate: unsafe extern "C" fn(Context, f64),
    open_synth: unsafe extern "C" fn(Context) -> c_int,
    close_synth: unsafe extern "C" fn(Context),
    play_msg: unsafe extern "C" fn(Context, u32) -> c_int,
    play_sysex: unsafe extern "C" fn(Context, *const u8, u32) -> c_int,
    render_float: unsafe extern "C" fn(Context, *mut f32, u32),
}

impl Api {
    /// Look the functions up in `lib`.
    fn load(lib: &Library) -> Result<Self, String> {
        // SAFETY: the types are those of the declarations in munt's
        // c_interface.h, which has kept them since version 2.5.
        unsafe {
            macro_rules! f {
                ($name:literal) => {
                    *lib.get($name).map_err(|e| format!("libmt32emu has no {}: {}", String::from_utf8_lossy($name), e))?
                };
            }
            Ok(Api {
                version: f!(b"mt32emu_get_library_version_string"),
                create_context: f!(b"mt32emu_create_context"),
                free_context: f!(b"mt32emu_free_context"),
                identify_rom_file: f!(b"mt32emu_identify_rom_file"),
                add_rom_file: f!(b"mt32emu_add_rom_file"),
                set_stereo_output_samplerate: f!(b"mt32emu_set_stereo_output_samplerate"),
                open_synth: f!(b"mt32emu_open_synth"),
                close_synth: f!(b"mt32emu_close_synth"),
                play_msg: f!(b"mt32emu_play_msg"),
                play_sysex: f!(b"mt32emu_play_sysex"),
                render_float: f!(b"mt32emu_render_float"),
            })
        }
    }
}

/// The names the library goes by, in the order they are tried.
fn library_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["mt32emu.dll", "libmt32emu.dll", "libmt32emu-2.dll"]
    } else if cfg!(target_os = "macos") {
        &[
            "libmt32emu.dylib",
            "libmt32emu.2.dylib",
            "/opt/homebrew/lib/libmt32emu.dylib",
            "/usr/local/lib/libmt32emu.dylib",
        ]
    } else {
        &["libmt32emu.so.2", "libmt32emu.so"]
    }
}

/// Load libmt32emu: from `path` (`mt32lib`), or wherever the system finds
/// it under its usual names.
fn open_library(path: Option<&Path>) -> Result<Library, String> {
    // SAFETY: loading runs the library's initializers, which munt's are
    // fine with.
    if let Some(path) = path {
        return unsafe { Library::new(path) }.map_err(|e| format!("{}: {}", path.display(), e));
    }
    let mut last = String::new();
    for name in library_names() {
        match unsafe { Library::new(name) } {
            Ok(lib) => return Ok(lib),
            Err(e) => last = e.to_string(),
        }
    }
    Err(format!("munt's libmt32emu isn't installed ({})", last))
}

/// A ROM munt identified: its id, e.g. "ctrl_mt32_1_07" or "pcm_cm32l".
struct Rom {
    id: String,
    path: PathBuf,
}

/// The ROMs in `dir` that munt knows.
fn identify_roms(api: &Api, dir: &Path) -> Result<Vec<Rom>, String> {
    let listing = std::fs::read_dir(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let mut roms = Vec::new();
    for entry in listing.flatten() {
        let path = entry.path();
        // The ROMs are 32 KB to 1 MB: skip what can't be one.
        if !entry.metadata().is_ok_and(|m| m.is_file() && (16 * 1024..=2 * 1024 * 1024).contains(&m.len())) {
            continue;
        }
        let Some(c_path) = path.to_str().and_then(|p| CString::new(p).ok()) else { continue };
        let mut info = RomInfo {
            control_rom_id: std::ptr::null(),
            control_rom_description: std::ptr::null(),
            control_rom_sha1_digest: std::ptr::null(),
            pcm_rom_id: std::ptr::null(),
            pcm_rom_description: std::ptr::null(),
            pcm_rom_sha1_digest: std::ptr::null(),
        };
        // SAFETY: a valid out-structure and a NUL-terminated path; the ids
        // point into the library's static tables.
        let rc = unsafe { (api.identify_rom_file)(&mut info, c_path.as_ptr(), std::ptr::null()) };
        if rc < 0 {
            continue;
        }
        for id in [info.control_rom_id, info.pcm_rom_id] {
            if !id.is_null() {
                let id = unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned();
                roms.push(Rom { id, path: path.clone() });
            }
        }
    }
    roms.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(roms)
}

/// The control and PCM ROM to play `model` with. Halves of ROMs (the
/// `_a`/`_b` and `_l`/`_h` dumps) aren't put together.
fn pick_roms(roms: &[Rom], model: Mt32Model) -> Result<(&Rom, &Rom), String> {
    // The MT-32's own ROMs before the CM-32L's, the versions games were
    // written for first.
    const MT32: [&str; 9] = [
        "ctrl_mt32_1_07",
        "ctrl_mt32_1_06",
        "ctrl_mt32_1_05",
        "ctrl_mt32_1_04",
        "ctrl_mt32_bluer",
        "ctrl_mt32_2_07",
        "ctrl_mt32_2_06",
        "ctrl_mt32_2_04",
        "ctrl_mt32_2_03",
    ];
    const CM32L: [&str; 3] = ["ctrl_cm32l_1_02", "ctrl_cm32l_1_00", "ctrl_cm32ln_1_00"];
    let pair = |controls: &[&str], pcm: &str| {
        let control = controls.iter().find_map(|id| roms.iter().find(|r| r.id == *id))?;
        let pcm = roms.iter().find(|r| r.id == pcm)?;
        Some((control, pcm))
    };
    let found = match model {
        Mt32Model::Mt32 => pair(&MT32, "pcm_mt32"),
        Mt32Model::Cm32l => pair(&CM32L, "pcm_cm32l"),
        Mt32Model::Auto => pair(&CM32L, "pcm_cm32l").or_else(|| pair(&MT32, "pcm_mt32")),
    };
    found.ok_or_else(|| {
        let what = match model {
            Mt32Model::Mt32 => "an MT-32 control and PCM ROM",
            Mt32Model::Cm32l => "a CM-32L control and PCM ROM",
            Mt32Model::Auto => "a control and PCM ROM of the MT-32 or the CM-32L",
        };
        if roms.is_empty() {
            format!("no ROMs munt knows; it needs {}", what)
        } else {
            let ids: Vec<&str> = roms.iter().map(|r| r.id.as_str()).collect();
            format!("{} needed, found {}", what, ids.join(", "))
        }
    })
}

/// A running MT-32.
pub struct Mt32 {
    api: Api,
    context: Context,
    /// The message on the display, until it is taken. munt keeps the
    /// address of the mutex, which the Arc doesn't move.
    lcd: Arc<Mutex<Option<String>>>,
    /// A block of rendered frames (left, right interleaved) and how many
    /// were used.
    block: [f32; BLOCK * 2],
    at: usize,
    /// What plays, for the log: library version, model and ROMs.
    description: String,
    // Dropped last: the functions above live in it.
    _lib: Library,
}

// SAFETY: the context is only ever used from the thread that owns the
// `Mt32`, one call at a time; munt keeps no thread-local state.
unsafe impl Send for Mt32 {}

impl Mt32 {
    /// Start an MT-32 of `model` with the ROMs in `rom_dir` (or the first of
    /// `default_rom_dirs` with ROMs), at `rate` frames a second, from the
    /// library at `lib` or the system's.
    pub fn open(rom_dir: Option<&Path>, model: Mt32Model, lib: Option<&Path>, rate: u32) -> Result<Self, String> {
        let library = open_library(lib)?;
        let api = Api::load(&library)?;
        // SAFETY: returns a static string.
        let version = unsafe { CStr::from_ptr((api.version)()) }.to_string_lossy().into_owned();

        let dirs = match rom_dir {
            Some(dir) => vec![dir.to_path_buf()],
            None => default_rom_dirs(),
        };
        let mut problems = Vec::new();
        let mut chosen = None;
        for dir in &dirs {
            match identify_roms(&api, dir) {
                Ok(roms) => match pick_roms(&roms, model) {
                    Ok((control, pcm)) => {
                        chosen = Some((control.id.clone(), control.path.clone(), pcm.path.clone()));
                        break;
                    }
                    Err(e) => problems.push(format!("{}: {}", dir.display(), e)),
                },
                Err(e) if rom_dir.is_some() => problems.push(e),
                // A default directory that isn't there.
                Err(_) => {}
            }
        }
        let Some((control_id, control, pcm)) = chosen else {
            if problems.is_empty() {
                let tried: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
                return Err(format!("no MT-32 ROMs (mt32roms), looked in {}", tried.join(", ")));
            }
            return Err(problems.join("; "));
        };

        let lcd: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let handler = ReportHandler { v0: &REPORT_HANDLER };
        // SAFETY: the handler is static and the instance data (the mutex in
        // the Arc) lives as long as the context, which Drop frees first.
        let context = unsafe { (api.create_context)(handler, Arc::as_ptr(&lcd) as *mut c_void) };
        if context.is_null() {
            return Err("libmt32emu couldn't make a synthesizer".to_string());
        }
        let mut synth = Mt32 {
            api,
            context,
            lcd,
            block: [0.0; BLOCK * 2],
            at: BLOCK,
            description: String::new(),
            _lib: library,
        };
        for rom in [&control, &pcm] {
            let c_path = CString::new(rom.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
            // SAFETY: a live context and a NUL-terminated path.
            let rc = unsafe { (synth.api.add_rom_file)(synth.context, c_path.as_ptr()) };
            if rc < 0 {
                return Err(format!("{}: libmt32emu refused it (error {})", rom.display(), rc));
            }
        }
        // SAFETY: a live context.
        unsafe { (synth.api.set_stereo_output_samplerate)(synth.context, rate as f64) };
        let rc = unsafe { (synth.api.open_synth)(synth.context) };
        if rc != 0 {
            // SAFETY: never opened, only freed by Drop.
            return Err(format!("libmt32emu couldn't start the synthesizer (error {})", rc));
        }
        let name = if control_id.starts_with("ctrl_cm32l") { "CM-32L" } else { "MT-32" };
        synth.description = format!(
            "{} ({}) from {} with munt {}",
            name,
            control_id.trim_start_matches("ctrl_"),
            control.parent().unwrap_or(Path::new("")).display(),
            version
        );
        Ok(synth)
    }

    /// The model, ROM and library that play, for the log.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// A channel message.
    pub fn message(&mut self, status: u8, d1: u8, d2: u8) {
        let msg = status as u32 | (d1 as u32) << 8 | (d2 as u32) << 16;
        // SAFETY: an open synthesizer.
        unsafe { (self.api.play_msg)(self.context, msg) };
    }

    /// A System Exclusive message, without its F0h and F7h.
    pub fn sysex(&mut self, body: &[u8]) {
        let mut framed = Vec::with_capacity(body.len() + 2);
        framed.push(0xF0);
        framed.extend_from_slice(body);
        framed.push(0xF7);
        // SAFETY: an open synthesizer and a buffer of the length given.
        unsafe { (self.api.play_sysex)(self.context, framed.as_ptr(), framed.len() as u32) };
    }

    /// Silence the notes, as when a program ends. The timbres and patches
    /// programs sent stay, as they do in a real module.
    pub fn notes_off(&mut self) {
        for channel in 0..16u8 {
            self.message(0xB0 | channel, 64, 0);
            self.message(0xB0 | channel, 123, 0);
        }
    }

    /// The message the display shows, once, when it changes.
    pub fn take_lcd_message(&mut self) -> Option<String> {
        self.lcd.lock().ok()?.take()
    }

    /// One stereo frame at the rate the synthesizer was opened with, on the
    /// scale of 16-bit samples.
    #[inline]
    pub fn render(&mut self) -> (f32, f32) {
        if self.at == BLOCK {
            // SAFETY: an open synthesizer and room for BLOCK stereo frames.
            unsafe { (self.api.render_float)(self.context, self.block.as_mut_ptr(), BLOCK as u32) };
            self.at = 0;
        }
        let frame = (self.block[self.at * 2] * 32767.0, self.block[self.at * 2 + 1] * 32767.0);
        self.at += 1;
        frame
    }
}

impl Drop for Mt32 {
    fn drop(&mut self) {
        // SAFETY: the context is live until here; closing one that never
        // opened is allowed.
        unsafe {
            (self.api.close_synth)(self.context);
            (self.api.free_context)(self.context);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom(id: &str) -> Rom {
        Rom { id: id.to_string(), path: PathBuf::from(id) }
    }

    #[test]
    fn models_pick_their_pairs() {
        let roms = [rom("ctrl_cm32l_1_02"), rom("ctrl_mt32_1_04"), rom("ctrl_mt32_1_07"), rom("pcm_cm32l"), rom("pcm_mt32")];
        let ids = |model| {
            let (c, p) = pick_roms(&roms, model).unwrap();
            (c.id.as_str(), p.id.as_str())
        };
        assert_eq!(ids(Mt32Model::Mt32), ("ctrl_mt32_1_07", "pcm_mt32"));
        assert_eq!(ids(Mt32Model::Cm32l), ("ctrl_cm32l_1_02", "pcm_cm32l"));
        assert_eq!(ids(Mt32Model::Auto), ("ctrl_cm32l_1_02", "pcm_cm32l"));
        // Auto falls back to the MT-32 without a pair of the CM-32L's.
        let roms = [rom("ctrl_mt32_1_04"), rom("pcm_cm32l"), rom("pcm_mt32")];
        assert_eq!(pick_roms(&roms, Mt32Model::Auto).unwrap().0.id, "ctrl_mt32_1_04");
    }

    #[test]
    fn a_control_rom_alone_is_not_enough() {
        let roms = [rom("ctrl_mt32_1_07")];
        let err = pick_roms(&roms, Mt32Model::Mt32).err().unwrap();
        assert!(err.contains("ctrl_mt32_1_07"), "{}", err);
        assert!(pick_roms(&[], Mt32Model::Auto).err().unwrap().contains("no ROMs"));
    }
}
