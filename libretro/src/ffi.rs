//! The part of the libretro API (libretro-common's include/libretro.h)
//! the core uses: its types, and the numbers of its calls and devices as
//! the header defines them.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_void};

pub const RETRO_API_VERSION: u32 = 1;

pub type retro_environment_t = unsafe extern "C" fn(cmd: u32, data: *mut c_void) -> bool;
pub type retro_video_refresh_t = unsafe extern "C" fn(data: *const c_void, width: u32, height: u32, pitch: usize);
pub type retro_audio_sample_t = unsafe extern "C" fn(left: i16, right: i16);
pub type retro_audio_sample_batch_t = unsafe extern "C" fn(data: *const i16, frames: usize) -> usize;
pub type retro_input_poll_t = unsafe extern "C" fn();
pub type retro_input_state_t = unsafe extern "C" fn(port: u32, device: u32, index: u32, id: u32) -> i16;
pub type retro_log_printf_t = unsafe extern "C" fn(level: u32, fmt: *const c_char, ...);
pub type retro_keyboard_event_t = unsafe extern "C" fn(down: bool, keycode: u32, character: u32, modifiers: u16);

// Environment calls.
pub const RETRO_ENVIRONMENT_EXPERIMENTAL: u32 = 0x10000;
pub const RETRO_ENVIRONMENT_SET_MESSAGE: u32 = 6;
pub const RETRO_ENVIRONMENT_SHUTDOWN: u32 = 7;
pub const RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY: u32 = 9;
pub const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: u32 = 10;
pub const RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS: u32 = 11;
pub const RETRO_ENVIRONMENT_SET_KEYBOARD_CALLBACK: u32 = 12;
pub const RETRO_ENVIRONMENT_SET_DISK_CONTROL_INTERFACE: u32 = 13;
pub const RETRO_ENVIRONMENT_GET_VARIABLE: u32 = 15;
pub const RETRO_ENVIRONMENT_SET_VARIABLES: u32 = 16;
pub const RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE: u32 = 17;
pub const RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME: u32 = 18;
pub const RETRO_ENVIRONMENT_GET_LOG_INTERFACE: u32 = 27;
pub const RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY: u32 = 31;
pub const RETRO_ENVIRONMENT_SET_SYSTEM_AV_INFO: u32 = 32;
pub const RETRO_ENVIRONMENT_SET_CONTROLLER_INFO: u32 = 35;
pub const RETRO_ENVIRONMENT_SET_MEMORY_MAPS: u32 = 36 | RETRO_ENVIRONMENT_EXPERIMENTAL;
pub const RETRO_ENVIRONMENT_SET_GEOMETRY: u32 = 37;
pub const RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS: u32 = 42 | RETRO_ENVIRONMENT_EXPERIMENTAL;
pub const RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION: u32 = 52;
pub const RETRO_ENVIRONMENT_GET_DISK_CONTROL_INTERFACE_VERSION: u32 = 57;
pub const RETRO_ENVIRONMENT_SET_DISK_CONTROL_EXT_INTERFACE: u32 = 58;
pub const RETRO_ENVIRONMENT_GET_MESSAGE_INTERFACE_VERSION: u32 = 59;
pub const RETRO_ENVIRONMENT_SET_MESSAGE_EXT: u32 = 60;
pub const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2: u32 = 67;

pub const RETRO_PIXEL_FORMAT_XRGB8888: u32 = 1;

pub const RETRO_LOG_DEBUG: u32 = 0;
pub const RETRO_LOG_INFO: u32 = 1;
pub const RETRO_LOG_WARN: u32 = 2;
pub const RETRO_LOG_ERROR: u32 = 3;

pub const RETRO_REGION_NTSC: u32 = 0;

// Devices, and their buttons and axes.
pub const RETRO_DEVICE_TYPE_SHIFT: u32 = 8;
pub const RETRO_DEVICE_NONE: u32 = 0;
pub const RETRO_DEVICE_JOYPAD: u32 = 1;
pub const RETRO_DEVICE_MOUSE: u32 = 2;
pub const RETRO_DEVICE_KEYBOARD: u32 = 3;
pub const RETRO_DEVICE_ANALOG: u32 = 5;
pub const RETRO_DEVICE_POINTER: u32 = 6;

pub const fn retro_device_subclass(base: u32, id: u32) -> u32 {
    ((id + 1) << RETRO_DEVICE_TYPE_SHIFT) | base
}

pub const RETRO_DEVICE_ID_JOYPAD_B: u32 = 0;
pub const RETRO_DEVICE_ID_JOYPAD_Y: u32 = 1;
pub const RETRO_DEVICE_ID_JOYPAD_SELECT: u32 = 2;
pub const RETRO_DEVICE_ID_JOYPAD_START: u32 = 3;
pub const RETRO_DEVICE_ID_JOYPAD_UP: u32 = 4;
pub const RETRO_DEVICE_ID_JOYPAD_DOWN: u32 = 5;
pub const RETRO_DEVICE_ID_JOYPAD_LEFT: u32 = 6;
pub const RETRO_DEVICE_ID_JOYPAD_RIGHT: u32 = 7;
pub const RETRO_DEVICE_ID_JOYPAD_A: u32 = 8;
pub const RETRO_DEVICE_ID_JOYPAD_X: u32 = 9;
pub const RETRO_DEVICE_ID_JOYPAD_L: u32 = 10;
pub const RETRO_DEVICE_ID_JOYPAD_R: u32 = 11;
pub const RETRO_DEVICE_ID_JOYPAD_L2: u32 = 12;
pub const RETRO_DEVICE_ID_JOYPAD_R2: u32 = 13;
pub const RETRO_DEVICE_ID_JOYPAD_L3: u32 = 14;
pub const RETRO_DEVICE_ID_JOYPAD_R3: u32 = 15;

pub const RETRO_DEVICE_INDEX_ANALOG_LEFT: u32 = 0;
pub const RETRO_DEVICE_INDEX_ANALOG_RIGHT: u32 = 1;
pub const RETRO_DEVICE_ID_ANALOG_X: u32 = 0;
pub const RETRO_DEVICE_ID_ANALOG_Y: u32 = 1;

pub const RETRO_DEVICE_ID_MOUSE_X: u32 = 0;
pub const RETRO_DEVICE_ID_MOUSE_Y: u32 = 1;
pub const RETRO_DEVICE_ID_MOUSE_LEFT: u32 = 2;
pub const RETRO_DEVICE_ID_MOUSE_RIGHT: u32 = 3;
pub const RETRO_DEVICE_ID_MOUSE_WHEELUP: u32 = 4;
pub const RETRO_DEVICE_ID_MOUSE_WHEELDOWN: u32 = 5;
pub const RETRO_DEVICE_ID_MOUSE_MIDDLE: u32 = 6;

pub const RETRO_DEVICE_ID_POINTER_X: u32 = 0;
pub const RETRO_DEVICE_ID_POINTER_Y: u32 = 1;
pub const RETRO_DEVICE_ID_POINTER_PRESSED: u32 = 2;

pub const RETRO_MEMORY_SYSTEM_RAM: u32 = 2;
pub const RETRO_MEMDESC_SYSTEM_RAM: u64 = 1 << 2;

// Keyboard modifiers (retro_mod).
pub const RETROKMOD_SHIFT: u16 = 0x01;
pub const RETROKMOD_CTRL: u16 = 0x02;
pub const RETROKMOD_ALT: u16 = 0x04;

/// The keys of `enum retro_key` the core knows.
pub mod key {
    pub const BACKSPACE: u32 = 8;
    pub const TAB: u32 = 9;
    pub const RETURN: u32 = 13;
    pub const PAUSE: u32 = 19;
    pub const ESCAPE: u32 = 27;
    pub const SPACE: u32 = 32;
    pub const QUOTE: u32 = 39;
    pub const COMMA: u32 = 44;
    pub const MINUS: u32 = 45;
    pub const PERIOD: u32 = 46;
    pub const SLASH: u32 = 47;
    pub const N0: u32 = 48;
    pub const N9: u32 = 57;
    pub const SEMICOLON: u32 = 59;
    pub const EQUALS: u32 = 61;
    pub const LEFTBRACKET: u32 = 91;
    pub const BACKSLASH: u32 = 92;
    pub const RIGHTBRACKET: u32 = 93;
    pub const BACKQUOTE: u32 = 96;
    pub const A: u32 = 97;
    pub const Z: u32 = 122;
    pub const DELETE: u32 = 127;
    pub const KP0: u32 = 256;
    pub const KP9: u32 = 265;
    pub const KP_PERIOD: u32 = 266;
    pub const KP_DIVIDE: u32 = 267;
    pub const KP_MULTIPLY: u32 = 268;
    pub const KP_MINUS: u32 = 269;
    pub const KP_PLUS: u32 = 270;
    pub const KP_ENTER: u32 = 271;
    pub const UP: u32 = 273;
    pub const DOWN: u32 = 274;
    pub const RIGHT: u32 = 275;
    pub const LEFT: u32 = 276;
    pub const INSERT: u32 = 277;
    pub const HOME: u32 = 278;
    pub const END: u32 = 279;
    pub const PAGEUP: u32 = 280;
    pub const PAGEDOWN: u32 = 281;
    pub const F1: u32 = 282;
    pub const F2: u32 = 283;
    pub const F4: u32 = 285;
    pub const F10: u32 = 291;
    pub const F11: u32 = 292;
    pub const F12: u32 = 293;
    pub const NUMLOCK: u32 = 300;
    pub const CAPSLOCK: u32 = 301;
    pub const SCROLLOCK: u32 = 302;
    pub const RSHIFT: u32 = 303;
    pub const LSHIFT: u32 = 304;
    pub const RCTRL: u32 = 305;
    pub const LCTRL: u32 = 306;
    pub const RALT: u32 = 307;
    pub const LALT: u32 = 308;
    pub const LSUPER: u32 = 311;
    pub const RSUPER: u32 = 312;
    pub const MENU: u32 = 319;
    pub const OEM_102: u32 = 323;
}

#[repr(C)]
pub struct retro_system_info {
    pub library_name: *const c_char,
    pub library_version: *const c_char,
    pub valid_extensions: *const c_char,
    pub need_fullpath: bool,
    pub block_extract: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct retro_game_geometry {
    pub base_width: u32,
    pub base_height: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub aspect_ratio: f32,
}

#[repr(C)]
pub struct retro_system_timing {
    pub fps: f64,
    pub sample_rate: f64,
}

#[repr(C)]
pub struct retro_system_av_info {
    pub geometry: retro_game_geometry,
    pub timing: retro_system_timing,
}

#[repr(C)]
pub struct retro_game_info {
    pub path: *const c_char,
    pub data: *const c_void,
    pub size: usize,
    pub meta: *const c_char,
}

#[repr(C)]
pub struct retro_variable {
    pub key: *const c_char,
    pub value: *const c_char,
}

pub const RETRO_NUM_CORE_OPTION_VALUES_MAX: usize = 128;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_core_option_value {
    pub value: *const c_char,
    pub label: *const c_char,
}

#[repr(C)]
pub struct retro_core_option_v2_category {
    pub key: *const c_char,
    pub desc: *const c_char,
    pub info: *const c_char,
}

#[repr(C)]
pub struct retro_core_option_v2_definition {
    pub key: *const c_char,
    pub desc: *const c_char,
    pub desc_categorized: *const c_char,
    pub info: *const c_char,
    pub info_categorized: *const c_char,
    pub category_key: *const c_char,
    pub values: [retro_core_option_value; RETRO_NUM_CORE_OPTION_VALUES_MAX],
    pub default_value: *const c_char,
}

#[repr(C)]
pub struct retro_core_options_v2 {
    pub categories: *const retro_core_option_v2_category,
    pub definitions: *const retro_core_option_v2_definition,
}

#[repr(C)]
pub struct retro_log_callback {
    pub log: Option<retro_log_printf_t>,
}

#[repr(C)]
pub struct retro_keyboard_callback {
    pub callback: Option<retro_keyboard_event_t>,
}

#[repr(C)]
pub struct retro_input_descriptor {
    pub port: u32,
    pub device: u32,
    pub index: u32,
    pub id: u32,
    pub description: *const c_char,
}

#[repr(C)]
pub struct retro_controller_description {
    pub desc: *const c_char,
    pub id: u32,
}

#[repr(C)]
pub struct retro_controller_info {
    pub types: *const retro_controller_description,
    pub num_types: u32,
}

#[repr(C)]
pub struct retro_message {
    pub msg: *const c_char,
    pub frames: u32,
}

#[repr(C)]
pub struct retro_message_ext {
    pub msg: *const c_char,
    pub duration: u32,
    pub priority: u32,
    pub level: u32,
    pub target: u32,
    pub type_: u32,
    pub progress: i8,
}

pub const RETRO_MESSAGE_TARGET_ALL: u32 = 0;
pub const RETRO_MESSAGE_TYPE_NOTIFICATION: u32 = 0;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_memory_descriptor {
    pub flags: u64,
    pub ptr: *mut c_void,
    pub offset: usize,
    pub start: usize,
    pub select: usize,
    pub disconnect: usize,
    pub len: usize,
    pub addrspace: *const c_char,
}

#[repr(C)]
pub struct retro_memory_map {
    pub descriptors: *const retro_memory_descriptor,
    pub num_descriptors: u32,
}

#[repr(C)]
pub struct retro_disk_control_ext_callback {
    pub set_eject_state: unsafe extern "C" fn(ejected: bool) -> bool,
    pub get_eject_state: unsafe extern "C" fn() -> bool,
    pub get_image_index: unsafe extern "C" fn() -> u32,
    pub set_image_index: unsafe extern "C" fn(index: u32) -> bool,
    pub get_num_images: unsafe extern "C" fn() -> u32,
    pub replace_image_index: unsafe extern "C" fn(index: u32, info: *const retro_game_info) -> bool,
    pub add_image_index: unsafe extern "C" fn() -> bool,
    pub set_initial_image: unsafe extern "C" fn(index: u32, path: *const c_char) -> bool,
    pub get_image_path: unsafe extern "C" fn(index: u32, s: *mut c_char, len: usize) -> bool,
    pub get_image_label: unsafe extern "C" fn(index: u32, s: *mut c_char, len: usize) -> bool,
}
