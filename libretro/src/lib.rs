//! rust-dos as a libretro core: the `retro_*` functions RetroArch and the
//! other libretro frontends call, over the machine in core.rs.
//!
//! The frontend calls them all from one thread, which keeps the machine
//! (`CORE`, which isn't `Send`) and the callbacks. A panic in the machine
//! is caught at the boundary: the frontend is told, and the content ends.

// The `retro_*` functions are safe to call as libretro.h says to call them.
#![allow(clippy::missing_safety_doc)]

mod content;
mod core;
mod disks;
pub mod ffi;
mod host;
mod keys;
mod memmap;
mod options;
mod state;
mod vfs;

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::ptr;

use crate::core::{Core, DEVICE_GAMEPORT, DEVICE_KEYS, FPS, MAX_SIZE};
use crate::ffi::*;

/// The frontend's functions, as it handed them over.
#[derive(Clone, Copy)]
pub struct Callbacks {
    pub env: retro_environment_t,
    video: Option<retro_video_refresh_t>,
    audio_batch: Option<retro_audio_sample_batch_t>,
    input_poll: Option<retro_input_poll_t>,
    input_state: Option<retro_input_state_t>,
    log: Option<retro_log_printf_t>,
}

unsafe extern "C" fn no_environment(_cmd: u32, _data: *mut c_void) -> bool {
    false
}

impl Default for Callbacks {
    fn default() -> Self {
        Self { env: no_environment, video: None, audio_batch: None, input_poll: None, input_state: None, log: None }
    }
}

impl Callbacks {
    /// An environment call.
    ///
    /// # Safety
    /// `data` must be what libretro.h says `cmd` takes.
    pub unsafe fn env(&self, cmd: u32, data: *mut c_void) -> bool {
        unsafe { (self.env)(cmd, data) }
    }

    pub fn log(&self, level: u32, message: &str) {
        let Ok(text) = CString::new(message.replace('\0', " ")) else { return };
        match self.log {
            // SAFETY: the frontend's printf-like log, with "%s\n" and one
            // C string.
            Some(log) => unsafe { log(level, c"%s\n".as_ptr(), text.as_ptr()) },
            None if level >= RETRO_LOG_WARN => eprintln!("[rust-dos] {}", message),
            None => {}
        }
    }

    /// A message on the screen, for a few seconds.
    pub fn message(&self, message: &str) {
        self.log(RETRO_LOG_INFO, message);
        let Ok(text) = CString::new(message.replace('\0', " ")) else { return };
        let mut version = 0u32;
        // SAFETY: the message calls take what libretro.h says.
        unsafe {
            if self.env(RETRO_ENVIRONMENT_GET_MESSAGE_INTERFACE_VERSION, &mut version as *mut u32 as *mut c_void)
                && version >= 1
            {
                let ext = retro_message_ext {
                    msg: text.as_ptr(),
                    duration: 3000,
                    priority: 1,
                    level: RETRO_LOG_INFO,
                    target: RETRO_MESSAGE_TARGET_ALL,
                    type_: RETRO_MESSAGE_TYPE_NOTIFICATION,
                    progress: -1,
                };
                self.env(RETRO_ENVIRONMENT_SET_MESSAGE_EXT, &ext as *const _ as *mut c_void);
            } else {
                let msg = retro_message { msg: text.as_ptr(), frames: 180 };
                self.env(RETRO_ENVIRONMENT_SET_MESSAGE, &msg as *const _ as *mut c_void);
            }
        }
    }

    pub fn input_poll(&self) {
        if let Some(poll) = self.input_poll {
            // SAFETY: the frontend's function, as it was handed over.
            unsafe { poll() }
        }
    }

    pub fn input(&self, port: u32, device: u32, index: u32, id: u32) -> i16 {
        // SAFETY: the frontend's function, as it was handed over.
        self.input_state.map_or(0, |state| unsafe { state(port, device, index, id) })
    }

    /// The picture, XRGB8888.
    pub fn video(&self, pixels: &[u32], width: u32, height: u32) {
        if let Some(video) = self.video {
            // SAFETY: `pixels` holds `width` x `height` pixels.
            unsafe { video(pixels.as_ptr() as *const c_void, width, height, width as usize * 4) }
        }
    }

    /// Stereo samples, interleaved, all of them.
    pub fn audio(&self, samples: &[i16]) {
        let Some(batch) = self.audio_batch else { return };
        let frames = samples.len() / 2;
        let mut done = 0;
        while done < frames {
            // SAFETY: the samples from `done` on are there.
            let taken = unsafe { batch(samples[done * 2..].as_ptr(), frames - done) };
            if taken == 0 {
                break;
            }
            done += taken;
        }
    }
}

/// A key the frontend's keyboard callback reported, for the next frame.
#[derive(Clone, Copy, Debug)]
pub struct KeyEvent {
    pub down: bool,
    pub keycode: u32,
    pub character: u32,
    pub modifiers: u16,
}

thread_local! {
    static CALLBACKS: Cell<Callbacks> = Cell::new(Callbacks::default());
    static CORE: RefCell<Option<Box<Core>>> = const { RefCell::new(None) };
    static KEYS: RefCell<Vec<KeyEvent>> = const { RefCell::new(Vec::new()) };
    /// What the frontend set before the content was loaded: the devices
    /// in the ports and the disk to start with.
    static PORTS: Cell<[u32; 2]> = const { Cell::new([DEVICE_GAMEPORT; 2]) };
    pub(crate) static INITIAL_IMAGE: RefCell<Option<(usize, PathBuf)>> = const { RefCell::new(None) };
}

fn callbacks() -> Callbacks {
    CALLBACKS.with(Cell::get)
}

fn set_callbacks(f: impl FnOnce(&mut Callbacks)) {
    CALLBACKS.with(|cell| {
        let mut cb = cell.get();
        f(&mut cb);
        cell.set(cb);
    });
}

pub(crate) fn take_keys() -> Vec<KeyEvent> {
    KEYS.with(|keys| std::mem::take(&mut *keys.borrow_mut()))
}

/// Run `f` on the machine, if there is one and it isn't busy. A panic in
/// it ends the content.
pub(crate) fn with_core<R>(f: impl FnOnce(&mut Core) -> R) -> Option<R> {
    let result = CORE.with(|cell| {
        let mut core = cell.try_borrow_mut().ok()?;
        let core = core.as_mut()?;
        Some(catch_unwind(AssertUnwindSafe(|| f(core))))
    })?;
    match result {
        Ok(value) => Some(value),
        Err(panic) => {
            let what = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("a panic");
            let cb = callbacks();
            cb.log(RETRO_LOG_ERROR, &format!("The emulator failed: {}", what));
            cb.message(&format!("rust-dos failed: {}", what));
            CORE.with(|cell| {
                if let Ok(mut core) = cell.try_borrow_mut() {
                    *core = None;
                }
            });
            // SAFETY: SHUTDOWN takes no data.
            unsafe { cb.env(RETRO_ENVIRONMENT_SHUTDOWN, ptr::null_mut()) };
            None
        }
    }
}

// ----------------------------------------------------------------------
// The frontend's callbacks
// ----------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_environment(env: retro_environment_t) {
    set_callbacks(|cb| cb.env = env);
    let cb = callbacks();
    let mut yes = true;
    // SAFETY: each call takes what libretro.h says.
    unsafe {
        cb.env(RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME, &mut yes as *mut bool as *mut c_void);
    }
    options::declare(env);

    struct Types([retro_controller_description; 3]);
    struct Info([retro_controller_info; 3]);
    // SAFETY: the pointers are to statics, and nothing writes them.
    unsafe impl Sync for Types {}
    unsafe impl Sync for Info {}
    static TYPES: Types = Types([
        retro_controller_description { desc: c"Gamepad (game port joystick)".as_ptr(), id: DEVICE_GAMEPORT },
        retro_controller_description { desc: c"Gamepad as keyboard".as_ptr(), id: DEVICE_KEYS },
        retro_controller_description { desc: c"None".as_ptr(), id: RETRO_DEVICE_NONE },
    ]);
    static PORT_TYPES: Info = Info([
        retro_controller_info { types: TYPES.0.as_ptr(), num_types: 3 },
        retro_controller_info { types: TYPES.0.as_ptr(), num_types: 3 },
        retro_controller_info { types: ptr::null(), num_types: 0 },
    ]);
    // SAFETY: SET_CONTROLLER_INFO takes an array of retro_controller_info
    // ended by an empty one.
    unsafe {
        cb.env(RETRO_ENVIRONMENT_SET_CONTROLLER_INFO, PORT_TYPES.0.as_ptr() as *mut c_void);
    }

    let mut keyboard = retro_keyboard_callback { callback: Some(retro_keyboard_event) };
    // SAFETY: SET_KEYBOARD_CALLBACK takes a retro_keyboard_callback.
    unsafe {
        cb.env(RETRO_ENVIRONMENT_SET_KEYBOARD_CALLBACK, &mut keyboard as *mut _ as *mut c_void);
    }
    disks::declare(&cb);
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_video_refresh(f: retro_video_refresh_t) {
    set_callbacks(|cb| cb.video = Some(f));
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_audio_sample(_f: retro_audio_sample_t) {}

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_audio_sample_batch(f: retro_audio_sample_batch_t) {
    set_callbacks(|cb| cb.audio_batch = Some(f));
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_input_poll(f: retro_input_poll_t) {
    set_callbacks(|cb| cb.input_poll = Some(f));
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_input_state(f: retro_input_state_t) {
    set_callbacks(|cb| cb.input_state = Some(f));
}

/// A key went down or up on the frontend's keyboard: for the next frame,
/// as the frontend may call this in the middle of one.
pub extern "C" fn retro_keyboard_event(down: bool, keycode: u32, character: u32, modifiers: u16) {
    KEYS.with(|keys| keys.borrow_mut().push(KeyEvent { down, keycode, character, modifiers }));
}

// ----------------------------------------------------------------------
// The core
// ----------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn retro_api_version() -> u32 {
    RETRO_API_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_init() {
    let cb = callbacks();
    let mut log = retro_log_callback { log: None };
    // SAFETY: GET_LOG_INTERFACE takes a retro_log_callback.
    if unsafe { cb.env(RETRO_ENVIRONMENT_GET_LOG_INTERFACE, &mut log as *mut _ as *mut c_void) } {
        set_callbacks(|cb| cb.log = log.log);
    }
    vfs::install(&callbacks());
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_deinit() {
    CORE.with(|cell| cell.borrow_mut().take());
    KEYS.with(|keys| keys.borrow_mut().clear());
    rust_dos::hostfs::set_backend(None);
}

/// The extensions of the content the core takes.
const EXTENSIONS: &CStr = c"exe|com|bat|zip|dosz|7z|conf|img|ima|vfd|flp|dsk|86f|vhd|hdd|iso|cue|ins|chd|m3u|m3u8";

#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_get_system_info(info: *mut retro_system_info) {
    static VERSION: &CStr = match CStr::from_bytes_with_nul(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes()) {
        Ok(version) => version,
        Err(_) => c"",
    };
    // SAFETY: the frontend hands over a retro_system_info to fill in.
    unsafe {
        *info = retro_system_info {
            library_name: c"rust-dos".as_ptr(),
            library_version: VERSION.as_ptr(),
            valid_extensions: EXTENSIONS.as_ptr(),
            // Drives are the host's folders, archives and images, read
            // where they are.
            need_fullpath: true,
            block_extract: true,
        };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_get_system_av_info(info: *mut retro_system_av_info) {
    let geometry = with_core(|core| core.geometry_now()).unwrap_or(retro_game_geometry {
        base_width: rust_dos::video::SCREEN_WIDTH,
        base_height: rust_dos::video::SCREEN_HEIGHT,
        max_width: MAX_SIZE.0,
        max_height: MAX_SIZE.1,
        aspect_ratio: rust_dos::video::SCREEN_WIDTH as f32 / rust_dos::video::SCREEN_HEIGHT as f32,
    });
    // SAFETY: the frontend hands over a retro_system_av_info to fill in.
    unsafe {
        *info = retro_system_av_info {
            geometry,
            timing: retro_system_timing { fps: FPS, sample_rate: rust_dos::opl::RATE as f64 },
        };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_controller_port_device(port: u32, device: u32) {
    if port >= 2 {
        return;
    }
    PORTS.with(|ports| {
        let mut all = ports.get();
        all[port as usize] = device;
        ports.set(all);
    });
    with_core(|core| {
        core.ports[port as usize] = device;
        if device != DEVICE_GAMEPORT {
            core.m.cpu.bus.joystick.set_pad(port as usize, None);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_reset() {
    let cb = callbacks();
    // A power cycle: the machine as the content started it.
    let Some(content) = with_core(|core| core.content.clone()) else { return };
    let ports = PORTS.with(Cell::get);
    match Core::new(content, &cb, options::read(cb.env), None) {
        Ok(mut core) => {
            core.ports = ports;
            CORE.with(|cell| *cell.borrow_mut() = Some(core));
        }
        Err(e) => cb.message(&format!("rust-dos can't start again: {}", e)),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_run() {
    let cb = callbacks();
    with_core(|core| core.run_frame(&cb));
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_serialize_size() -> usize {
    with_core(|core| {
        if core.state_size == 0 {
            core.state_size = state::size(&core.m);
        }
        core.state_size
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_serialize(data: *mut c_void, size: usize) -> bool {
    if data.is_null() {
        return false;
    }
    // SAFETY: the frontend hands over `size` bytes to fill.
    let buf = unsafe { std::slice::from_raw_parts_mut(data as *mut u8, size) };
    with_core(|core| {
        let saved = state::save(&core.m, buf);
        if !saved {
            // The next size asked for has room for it.
            core.state_size = state::size(&core.m);
        }
        saved
    })
    .unwrap_or(false)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_unserialize(data: *const c_void, size: usize) -> bool {
    if data.is_null() {
        return false;
    }
    // SAFETY: the frontend hands over `size` bytes of a state.
    let buf = unsafe { std::slice::from_raw_parts(data as *const u8, size) };
    let cb = callbacks();
    with_core(|core| match state::load(&mut core.m, buf) {
        Ok(_) => {
            core.rebase();
            true
        }
        Err(e) => {
            cb.message(&format!("The state can't be loaded: {}", e));
            false
        }
    })
    .unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_cheat_reset() {}

#[unsafe(no_mangle)]
pub extern "C" fn retro_cheat_set(_index: u32, _enabled: bool, _code: *const c_char) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_load_game(game: *const retro_game_info) -> bool {
    let cb = callbacks();
    let mut format = RETRO_PIXEL_FORMAT_XRGB8888;
    // SAFETY: SET_PIXEL_FORMAT takes a retro_pixel_format.
    if !unsafe { cb.env(RETRO_ENVIRONMENT_SET_PIXEL_FORMAT, &mut format as *mut u32 as *mut c_void) } {
        cb.log(RETRO_LOG_ERROR, "The frontend doesn't take XRGB8888 pictures");
        return false;
    }
    // SAFETY: the frontend hands over the content's info, or none.
    let content = unsafe { game.as_ref() }.filter(|g| !g.path.is_null()).map(|g| {
        let path = unsafe { CStr::from_ptr(g.path) };
        PathBuf::from(path.to_string_lossy().into_owned())
    });
    let initial = INITIAL_IMAGE.with(|i| i.borrow_mut().take());
    let values = options::read(cb.env);
    // As Dolphin does where Apple's rules keep a JIT out.
    if rust_dos::dynrec::AVAILABLE && !rust_dos::dynrec::usable() && values.get("core") != Some("normal") {
        cb.message("JIT is disabled on this device: the interpreter runs everything, and protected-mode games run slower");
    }
    let core = catch_unwind(AssertUnwindSafe(|| Core::new(content, &cb, values, initial)));
    let mut core = match core {
        Ok(Ok(core)) => core,
        Ok(Err(e)) => {
            cb.log(RETRO_LOG_ERROR, &e);
            cb.message(&e);
            return false;
        }
        Err(_) => {
            cb.log(RETRO_LOG_ERROR, "The emulator failed as it started");
            return false;
        }
    };
    core.ports = PORTS.with(Cell::get);
    let mut yes = true;
    // SAFETY: the calls take what libretro.h says.
    unsafe {
        cb.env(RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS, &mut yes as *mut bool as *mut c_void);
    }
    set_input_descriptors(&cb);
    core.memmap.refresh(&mut core.m.cpu.bus, cb.env);
    CORE.with(|cell| *cell.borrow_mut() = Some(core));
    true
}

/// What the gamepad's buttons do, for the frontend's menu.
fn set_input_descriptors(cb: &Callbacks) {
    let mut descriptors = Vec::new();
    for port in 0..2 {
        let joypad = |id, description: &'static CStr| retro_input_descriptor {
            port,
            device: RETRO_DEVICE_JOYPAD,
            index: 0,
            id,
            description: description.as_ptr(),
        };
        descriptors.extend([
            joypad(RETRO_DEVICE_ID_JOYPAD_UP, c"Up"),
            joypad(RETRO_DEVICE_ID_JOYPAD_DOWN, c"Down"),
            joypad(RETRO_DEVICE_ID_JOYPAD_LEFT, c"Left"),
            joypad(RETRO_DEVICE_ID_JOYPAD_RIGHT, c"Right"),
            joypad(RETRO_DEVICE_ID_JOYPAD_B, c"Button 1 / Ctrl"),
            joypad(RETRO_DEVICE_ID_JOYPAD_A, c"Button 2 / Alt"),
            joypad(RETRO_DEVICE_ID_JOYPAD_Y, c"Button 3 / Space"),
            joypad(RETRO_DEVICE_ID_JOYPAD_X, c"Button 4 / Shift"),
            joypad(RETRO_DEVICE_ID_JOYPAD_START, c"Enter"),
            joypad(RETRO_DEVICE_ID_JOYPAD_SELECT, c"Esc"),
            joypad(RETRO_DEVICE_ID_JOYPAD_L, c"Page Up"),
            joypad(RETRO_DEVICE_ID_JOYPAD_R, c"Page Down"),
            joypad(RETRO_DEVICE_ID_JOYPAD_L2, c"Tab / Mouse right button"),
            joypad(RETRO_DEVICE_ID_JOYPAD_R2, c"Backspace / Mouse left button"),
        ]);
        let analog = |index, id, description: &'static CStr| retro_input_descriptor {
            port,
            device: RETRO_DEVICE_ANALOG,
            index,
            id,
            description: description.as_ptr(),
        };
        descriptors.extend([
            analog(RETRO_DEVICE_INDEX_ANALOG_LEFT, RETRO_DEVICE_ID_ANALOG_X, c"Joystick X"),
            analog(RETRO_DEVICE_INDEX_ANALOG_LEFT, RETRO_DEVICE_ID_ANALOG_Y, c"Joystick Y"),
            analog(RETRO_DEVICE_INDEX_ANALOG_RIGHT, RETRO_DEVICE_ID_ANALOG_X, c"Joystick 2 X / Mouse X"),
            analog(RETRO_DEVICE_INDEX_ANALOG_RIGHT, RETRO_DEVICE_ID_ANALOG_Y, c"Joystick 2 Y / Mouse Y"),
        ]);
    }
    descriptors.push(retro_input_descriptor {
        port: 0,
        device: RETRO_DEVICE_JOYPAD,
        index: 0,
        id: RETRO_DEVICE_ID_JOYPAD_R3,
        description: c"Settings window (with L3)".as_ptr(),
    });
    descriptors.push(retro_input_descriptor { port: 0, device: 0, index: 0, id: 0, description: ptr::null() });
    // SAFETY: SET_INPUT_DESCRIPTORS takes an array ended by one without a
    // description; the frontend copies it.
    unsafe { cb.env(RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS, descriptors.as_mut_ptr() as *mut c_void) };
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_load_game_special(_type: u32, _info: *const retro_game_info, _num: usize) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_unload_game() {
    CORE.with(|cell| cell.borrow_mut().take());
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_get_region() -> u32 {
    RETRO_REGION_NTSC
}

/// The machine's RAM, all of it, for the frontend's cheats and memory
/// viewer; RetroAchievements has the memory map.
#[unsafe(no_mangle)]
pub extern "C" fn retro_get_memory_data(id: u32) -> *mut c_void {
    if id != RETRO_MEMORY_SYSTEM_RAM {
        return ptr::null_mut();
    }
    with_core(|core| core.m.cpu.bus.ram_mut().as_mut_ptr() as *mut c_void).unwrap_or(ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_get_memory_size(id: u32) -> usize {
    if id != RETRO_MEMORY_SYSTEM_RAM {
        return 0;
    }
    with_core(|core| core.m.cpu.bus.ram().len()).unwrap_or(0)
}
