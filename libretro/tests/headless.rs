//! The core as a frontend runs it: loaded with content (or none), run
//! frame by frame, typed at, saved and loaded, with its disks changed and
//! its memory map read. A frontend of stubs keeps what the core hands it.
//! Each test runs on a thread of its own, which the core's state is per.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;

use rust_dos_libretro::ffi::*;
use rust_dos_libretro::*;

#[derive(Default)]
struct Frontend {
    system: CString,
    save: CString,
    /// The core options, and a value handed out, kept alive.
    variables: HashMap<String, CString>,
    options_changed: bool,
    keyboard: Option<retro_keyboard_event_t>,
    disks: Option<&'static retro_disk_control_ext_callback>,
    /// The memory map: each region's start, length and pointer.
    memory_map: Vec<(usize, usize, *const u8)>,
    shutdown: bool,
    messages: Vec<String>,
    geometry: Option<retro_game_geometry>,
    /// The last picture, and the sound frames handed over.
    width: u32,
    height: u32,
    pixels: Vec<u32>,
    audio_frames: usize,
    input: HashMap<(u32, u32, u32, u32), i16>,
    /// The folder `test://` paths are in, for a frontend with a VFS, and
    /// the calls made to it.
    vfs_root: Option<PathBuf>,
    vfs_calls: usize,
}

thread_local! {
    static FRONTEND: RefCell<Frontend> = RefCell::new(Frontend::default());
}

fn with<R>(f: impl FnOnce(&mut Frontend) -> R) -> R {
    FRONTEND.with(|fe| f(&mut fe.borrow_mut()))
}

unsafe extern "C" fn environment(cmd: u32, data: *mut c_void) -> bool {
    unsafe {
        match cmd {
            RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY => {
                *(data as *mut *const c_char) = with(|fe| fe.system.as_ptr());
                true
            }
            RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY => {
                *(data as *mut *const c_char) = with(|fe| fe.save.as_ptr());
                true
            }
            RETRO_ENVIRONMENT_GET_VARIABLE => {
                let var = &mut *(data as *mut retro_variable);
                let key = CStr::from_ptr(var.key).to_string_lossy().into_owned();
                match with(|fe| fe.variables.get(&key).map(|v| v.as_ptr())) {
                    Some(value) => {
                        var.value = value;
                        true
                    }
                    None => false,
                }
            }
            RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE => {
                *(data as *mut bool) = with(|fe| std::mem::take(&mut fe.options_changed));
                true
            }
            RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION => {
                *(data as *mut u32) = 2;
                true
            }
            RETRO_ENVIRONMENT_SET_KEYBOARD_CALLBACK => {
                let cb = &*(data as *const retro_keyboard_callback);
                with(|fe| fe.keyboard = cb.callback);
                true
            }
            RETRO_ENVIRONMENT_GET_DISK_CONTROL_INTERFACE_VERSION => {
                *(data as *mut u32) = 1;
                true
            }
            RETRO_ENVIRONMENT_SET_DISK_CONTROL_EXT_INTERFACE => {
                let disks = &*(data as *const retro_disk_control_ext_callback);
                with(|fe| fe.disks = Some(disks));
                true
            }
            RETRO_ENVIRONMENT_SET_MEMORY_MAPS => {
                let map = &*(data as *const retro_memory_map);
                let descriptors = std::slice::from_raw_parts(map.descriptors, map.num_descriptors as usize);
                let regions = descriptors.iter().map(|d| (d.start, d.len, (d.ptr as *const u8).add(d.offset))).collect();
                with(|fe| fe.memory_map = regions);
                true
            }
            RETRO_ENVIRONMENT_SET_GEOMETRY => {
                let geometry = *(data as *const retro_game_geometry);
                with(|fe| fe.geometry = Some(geometry));
                true
            }
            RETRO_ENVIRONMENT_SHUTDOWN => {
                with(|fe| fe.shutdown = true);
                true
            }
            RETRO_ENVIRONMENT_GET_MESSAGE_INTERFACE_VERSION => {
                *(data as *mut u32) = 1;
                true
            }
            RETRO_ENVIRONMENT_SET_MESSAGE_EXT => {
                let msg = &*(data as *const retro_message_ext);
                let text = CStr::from_ptr(msg.msg).to_string_lossy().into_owned();
                with(|fe| fe.messages.push(text));
                true
            }
            RETRO_ENVIRONMENT_GET_VFS_INTERFACE => {
                let info = &mut *(data as *mut retro_vfs_interface_info);
                if with(|fe| fe.vfs_root.is_none()) || info.required_interface_version > 3 {
                    return false;
                }
                info.iface = &raw const vfs::INTERFACE as *mut retro_vfs_interface;
                true
            }
            RETRO_ENVIRONMENT_GET_LOG_INTERFACE => false,
            _ => true,
        }
    }
}

unsafe extern "C" fn video(data: *const c_void, width: u32, height: u32, pitch: usize) {
    assert_eq!(pitch, width as usize * 4);
    let pixels = unsafe { std::slice::from_raw_parts(data as *const u32, (width * height) as usize) };
    with(|fe| {
        fe.width = width;
        fe.height = height;
        fe.pixels = pixels.to_vec();
    });
}

unsafe extern "C" fn audio_sample(_left: i16, _right: i16) {}

unsafe extern "C" fn audio_batch(_data: *const i16, frames: usize) -> usize {
    with(|fe| fe.audio_frames += frames);
    frames
}

unsafe extern "C" fn input_poll() {}

unsafe extern "C" fn input_state(port: u32, device: u32, index: u32, id: u32) -> i16 {
    with(|fe| fe.input.get(&(port, device, index, id)).copied().unwrap_or(0))
}

/// A frontend's VFS over a folder: `test://a/b` is `<vfs_root>/a/b`. As
/// Android's SAF, it can't rename, and its seek answers 0.
mod vfs {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};

    pub static INTERFACE: retro_vfs_interface = retro_vfs_interface {
        get_path: Some(get_path),
        open: Some(open),
        close: Some(close),
        size: Some(size),
        tell: Some(tell),
        seek: Some(seek),
        read: Some(read),
        write: Some(write),
        flush: Some(flush),
        remove: Some(remove),
        rename: Some(rename),
        truncate: Some(truncate),
        stat: Some(stat),
        mkdir: Some(mkdir),
        opendir: Some(opendir),
        readdir: Some(readdir),
        dirent_get_name: Some(dirent_get_name),
        dirent_is_dir: Some(dirent_is_dir),
        closedir: Some(closedir),
    };

    struct Dir {
        entries: Vec<(CString, bool)>,
        /// The entry readdir is at, plus one.
        at: usize,
    }

    /// The host path of a `test://` path.
    unsafe fn host(path: *const c_char) -> PathBuf {
        let path = unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned();
        let rest = path.strip_prefix("test://").unwrap_or_else(|| panic!("{} isn't a test:// path", path));
        with(|fe| {
            fe.vfs_calls += 1;
            fe.vfs_root.clone().unwrap().join(rest)
        })
    }

    unsafe fn file<'a>(stream: *mut retro_vfs_file_handle) -> &'a mut fs::File {
        unsafe { &mut *(stream as *mut fs::File) }
    }

    unsafe extern "C" fn get_path(_stream: *mut retro_vfs_file_handle) -> *const c_char {
        ptr::null()
    }

    unsafe extern "C" fn open(path: *const c_char, mode: u32, _hints: u32) -> *mut retro_vfs_file_handle {
        let path = unsafe { host(path) };
        let mut options = fs::OpenOptions::new();
        match mode {
            RETRO_VFS_FILE_ACCESS_READ => options.read(true),
            RETRO_VFS_FILE_ACCESS_WRITE => options.write(true).create(true).truncate(true),
            m if m == RETRO_VFS_FILE_ACCESS_READ_WRITE | RETRO_VFS_FILE_ACCESS_UPDATE_EXISTING => options.read(true).write(true),
            RETRO_VFS_FILE_ACCESS_READ_WRITE => options.read(true).write(true).create(true).truncate(true),
            m => panic!("access {}", m),
        };
        match options.open(path) {
            Ok(file) => Box::into_raw(Box::new(file)) as *mut retro_vfs_file_handle,
            Err(_) => ptr::null_mut(),
        }
    }

    unsafe extern "C" fn close(stream: *mut retro_vfs_file_handle) -> i32 {
        drop(unsafe { Box::from_raw(stream as *mut fs::File) });
        0
    }

    unsafe extern "C" fn size(stream: *mut retro_vfs_file_handle) -> i64 {
        unsafe { file(stream) }.metadata().map_or(-1, |m| m.len() as i64)
    }

    unsafe extern "C" fn tell(stream: *mut retro_vfs_file_handle) -> i64 {
        unsafe { file(stream) }.stream_position().map_or(-1, |p| p as i64)
    }

    unsafe extern "C" fn seek(stream: *mut retro_vfs_file_handle, offset: i64, whence: i32) -> i64 {
        let to = match whence {
            RETRO_VFS_SEEK_POSITION_START => SeekFrom::Start(offset as u64),
            RETRO_VFS_SEEK_POSITION_CURRENT => SeekFrom::Current(offset),
            _ => SeekFrom::End(offset),
        };
        unsafe { file(stream) }.seek(to).map_or(-1, |_| 0)
    }

    unsafe extern "C" fn read(stream: *mut retro_vfs_file_handle, s: *mut c_void, len: u64) -> i64 {
        let buf = unsafe { std::slice::from_raw_parts_mut(s as *mut u8, len as usize) };
        unsafe { file(stream) }.read(buf).map_or(-1, |n| n as i64)
    }

    unsafe extern "C" fn write(stream: *mut retro_vfs_file_handle, s: *const c_void, len: u64) -> i64 {
        let buf = unsafe { std::slice::from_raw_parts(s as *const u8, len as usize) };
        unsafe { file(stream) }.write(buf).map_or(-1, |n| n as i64)
    }

    unsafe extern "C" fn flush(stream: *mut retro_vfs_file_handle) -> i32 {
        unsafe { file(stream) }.flush().map_or(-1, |_| 0)
    }

    unsafe extern "C" fn remove(path: *const c_char) -> i32 {
        let path = unsafe { host(path) };
        let done = if path.is_dir() { fs::remove_dir(path) } else { fs::remove_file(path) };
        done.map_or(-1, |_| 0)
    }

    unsafe extern "C" fn rename(_old: *const c_char, _new: *const c_char) -> i32 {
        -1
    }

    unsafe extern "C" fn truncate(stream: *mut retro_vfs_file_handle, length: i64) -> i64 {
        unsafe { file(stream) }.set_len(length as u64).map_or(-1, |_| 0)
    }

    unsafe extern "C" fn stat(path: *const c_char, size: *mut i32) -> i32 {
        let Ok(meta) = fs::metadata(unsafe { host(path) }) else { return 0 };
        if !size.is_null() {
            unsafe { *size = meta.len() as i32 };
        }
        RETRO_VFS_STAT_IS_VALID | if meta.is_dir() { RETRO_VFS_STAT_IS_DIRECTORY } else { 0 }
    }

    unsafe extern "C" fn mkdir(dir: *const c_char) -> i32 {
        let path = unsafe { host(dir) };
        if path.exists() {
            return -2;
        }
        fs::create_dir(path).map_or(-1, |_| 0)
    }

    unsafe extern "C" fn opendir(dir: *const c_char, _hidden: bool) -> *mut retro_vfs_dir_handle {
        let Ok(read) = fs::read_dir(unsafe { host(dir) }) else { return ptr::null_mut() };
        let entries = read
            .flatten()
            .map(|e| (CString::new(e.file_name().to_string_lossy().as_bytes()).unwrap(), e.path().is_dir()))
            .collect();
        Box::into_raw(Box::new(Dir { entries, at: 0 })) as *mut retro_vfs_dir_handle
    }

    unsafe fn dir<'a>(dirstream: *mut retro_vfs_dir_handle) -> &'a mut Dir {
        unsafe { &mut *(dirstream as *mut Dir) }
    }

    unsafe extern "C" fn readdir(dirstream: *mut retro_vfs_dir_handle) -> bool {
        let dir = unsafe { dir(dirstream) };
        dir.at += 1;
        dir.at <= dir.entries.len()
    }

    unsafe extern "C" fn dirent_get_name(dirstream: *mut retro_vfs_dir_handle) -> *const c_char {
        let dir = unsafe { dir(dirstream) };
        dir.entries[dir.at - 1].0.as_ptr()
    }

    unsafe extern "C" fn dirent_is_dir(dirstream: *mut retro_vfs_dir_handle) -> bool {
        let dir = unsafe { dir(dirstream) };
        dir.entries[dir.at - 1].1
    }

    unsafe extern "C" fn closedir(dirstream: *mut retro_vfs_dir_handle) -> i32 {
        drop(unsafe { Box::from_raw(dirstream as *mut Dir) });
        0
    }
}

/// A folder for a test, on disk (the build's target folder).
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-headless").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The core set up by the stub frontend, with the options `options`, but
/// no content yet.
fn start(dir: &Path, options: &[(&str, &str)]) {
    with(|fe| {
        fe.system = CString::new(dir.join("system").to_string_lossy().as_bytes()).unwrap();
        fe.save = CString::new(dir.join("saves").to_string_lossy().as_bytes()).unwrap();
        // A fixed speed and the interpreter, for runs that come out the
        // same every time.
        let fixed = [("rust_dos_cycles", "3000"), ("rust_dos_core", "normal")];
        for (key, value) in fixed.iter().chain(options) {
            fe.variables.insert(key.to_string(), CString::new(*value).unwrap());
        }
    });
    retro_set_environment(environment);
    retro_set_video_refresh(video);
    retro_set_audio_sample(audio_sample);
    retro_set_audio_sample_batch(audio_batch);
    retro_set_input_poll(input_poll);
    retro_set_input_state(input_state);
    retro_init();
}

/// Load `content` (None: none), as the frontend does.
fn load(content: Option<&Path>) -> bool {
    let path = content.map(|p| CString::new(p.to_string_lossy().as_bytes()).unwrap());
    let info = path.as_ref().map(|path| retro_game_info {
        path: path.as_ptr(),
        data: ptr::null(),
        size: 0,
        meta: ptr::null(),
    });
    unsafe { retro_load_game(info.as_ref().map_or(ptr::null(), |i| i as *const _)) }
}

fn run(frames: usize) {
    for _ in 0..frames {
        retro_run();
    }
}

fn stop() {
    retro_unload_game();
    retro_deinit();
}

/// Type `text` on the frontend's keyboard, a key a frame.
fn type_text(text: &str) {
    let keyboard = with(|fe| fe.keyboard).expect("the core takes the keyboard");
    for c in text.chars() {
        let keycode = match c {
            '\r' => key::RETURN,
            c => c.to_ascii_lowercase() as u32,
        };
        let shift = if c.is_ascii_uppercase() { RETROKMOD_SHIFT } else { 0 };
        if shift != 0 {
            unsafe { keyboard(true, key::LSHIFT, 0, shift) };
        }
        unsafe { keyboard(true, keycode, c as u32, shift) };
        run(2);
        unsafe { keyboard(false, keycode, 0, shift) };
        if shift != 0 {
            unsafe { keyboard(false, key::LSHIFT, 0, 0) };
        }
        run(2);
    }
}

fn picture_hash() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    with(|fe| (fe.width, fe.height, &fe.pixels).hash(&mut hasher));
    hasher.finish()
}

fn memory_hash() -> u64 {
    use std::hash::{Hash, Hasher};
    let data = retro_get_memory_data(RETRO_MEMORY_SYSTEM_RAM) as *const u8;
    let size = retro_get_memory_size(RETRO_MEMORY_SYSTEM_RAM);
    let ram = unsafe { std::slice::from_raw_parts(data, size) };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    ram.hash(&mut hasher);
    hasher.finish()
}

/// The word at `address` of the memory map, as RetroAchievements reads it.
fn map_word(address: usize) -> Option<u16> {
    with(|fe| {
        let &(start, _, ptr) = fe.memory_map.iter().find(|(start, len, _)| (*start..start + len).contains(&address))?;
        let at = unsafe { ptr.add(address - start) };
        Some(unsafe { u16::from_le_bytes([*at, *at.add(1)]) })
    })
}

/// A program that puts BEEFh at offset 200h of its segment, then waits
/// there for ever.
const MARKER: &[u8] = &[0xC7, 0x06, 0x00, 0x02, 0xEF, 0xBE, 0xEB, 0xFE];

#[test]
fn without_content_it_starts_at_the_prompt() {
    let dir = scratch("prompt");
    start(&dir, &[]);
    assert!(load(None));
    run(120);
    with(|fe| {
        assert_eq!((fe.width, fe.height), (640, 400));
        assert!(fe.pixels.iter().any(|&p| p != 0), "something on the screen");
        // 735 frames of sound a frame: 44100 Hz at 60 frames a second.
        let expected = 735 * 120;
        assert!(fe.audio_frames.abs_diff(expected) <= 120, "{} sound frames", fe.audio_frames);
    });
    assert!(dir.join("saves/rust-dos/drive_c").is_dir(), "an empty C:");
    assert_eq!(retro_get_memory_size(RETRO_MEMORY_SYSTEM_RAM), 16 << 20);
    assert!(!retro_get_memory_data(RETRO_MEMORY_SYSTEM_RAM).is_null());

    // Typing at the prompt changes what it shows.
    let before = picture_hash();
    type_text("VER\r");
    run(30);
    assert_ne!(picture_hash(), before);
    stop();
}

#[test]
fn a_program_given_as_content_runs_and_is_where_achievements_look() {
    let dir = scratch("program");
    fs::create_dir_all(dir.join("game")).unwrap();
    fs::write(dir.join("game/MARKER.COM"), MARKER).unwrap();
    start(&dir, &[]);
    assert!(load(Some(&dir.join("game/MARKER.COM"))));
    run(180);
    // The game's memory starts 120h bytes below its PSP, so its segment's
    // 200h is at 320h.
    let regions: Vec<usize> = with(|fe| fe.memory_map.iter().map(|r| r.0).collect());
    assert_eq!(regions, [0, 0x100000, 0x200000]);
    assert_eq!(map_word(0x320), Some(0xBEEF));
    stop();
}

#[test]
fn save_states_come_back_as_they_were() {
    let dir = scratch("states");
    fs::write(dir.join("MARKER.COM"), MARKER).unwrap();
    start(&dir, &[]);
    assert!(load(Some(&dir.join("MARKER.COM"))));
    run(60);
    let size = retro_serialize_size();
    assert!(size > 16 << 20, "the memory is in it");
    let mut state = vec![0u8; size];
    assert!(unsafe { retro_serialize(state.as_mut_ptr() as *mut c_void, size) });
    run(30);
    let after = memory_hash();
    assert!(unsafe { retro_unserialize(state.as_ptr() as *const c_void, size) });
    run(30);
    assert_eq!(memory_hash(), after, "the same 30 frames again");
    assert!(!unsafe { retro_unserialize([0u8; 64].as_ptr() as *const c_void, 64) });
    stop();
}

/// A zip archive of `files`, stored.
fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let (mut data, mut central) = (Vec::new(), Vec::new());
    for &(name, contents) in files {
        let offset = data.len() as u32;
        let header = |sig: u32| {
            let mut h = sig.to_le_bytes().to_vec();
            h.extend([20, 0, 0, 0, 0, 0]);
            h.extend([0x00, 0x60, 0x21, 0x2A]);
            h.extend([0; 4]);
            h.extend((contents.len() as u32).to_le_bytes());
            h.extend((contents.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend([0, 0]);
            h
        };
        data.extend(header(0x0403_4B50));
        data.extend(name.as_bytes());
        data.extend(contents);
        let mut entry = vec![0x50, 0x4B, 0x01, 0x02, 20, 0];
        entry.extend(&header(0)[4..]);
        entry.extend([0; 10]);
        entry.extend(offset.to_le_bytes());
        entry.extend(name.as_bytes());
        central.extend(entry);
    }
    let at = data.len() as u32;
    data.extend(&central);
    data.extend([0x50, 0x4B, 0x05, 0x06, 0, 0, 0, 0]);
    data.extend((files.len() as u16).to_le_bytes());
    data.extend((files.len() as u16).to_le_bytes());
    data.extend((central.len() as u32).to_le_bytes());
    data.extend(at.to_le_bytes());
    data.extend([0, 0]);
    data
}

#[test]
fn an_archive_is_a_game_made_once_and_launched() {
    let dir = scratch("zip");
    fs::write(dir.join("Marker Game.zip"), zip(&[("MARKER.COM", MARKER), ("README.TXT", b"hello")])).unwrap();
    for _ in 0..2 {
        start(&dir, &[]);
        assert!(load(Some(&dir.join("Marker Game.zip"))));
        run(180);
        assert_eq!(map_word(0x320), Some(0xBEEF), "the game runs");
        stop();
    }
    let games: Vec<String> = fs::read_dir(dir.join("saves/rust-dos/games"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(games, ["marker-game.conf"], "its profile, and no folder");
}

/// GAME.conf beside GAME.dosz, which has the archive as C:
/// before its commands, and moves it to D:.
#[test]
fn a_configuration_beside_a_dosz_can_remount_the_archive() {
    let dir = scratch("dosz-remount");
    fs::write(dir.join("Marker.dosz"), zip(&[("MARKER.COM", MARKER)])).unwrap();
    fs::write(dir.join("Marker.conf"), "[dosbox]\nmachine=vga\n[autoexec]\nremount c d\nd:\nmarker\n").unwrap();
    start(&dir, &[]);
    assert!(load(Some(&dir.join("Marker.dosz"))));
    run(180);
    assert_eq!(map_word(0x320), Some(0xBEEF), "the game runs from D:");
    stop();
    let profile = fs::read_to_string(dir.join("saves/rust-dos/games/marker.conf")).unwrap();
    assert!(profile.contains("\nD="), "{}", profile);
    assert!(!profile.contains("\nC="), "{}", profile);
}

#[test]
fn the_disks_of_a_playlist_change_through_disk_control() {
    let dir = scratch("disks");
    // Two formatted floppies, as MAKEIMG would make them.
    let spec = rust_dos::makeimg::ImageSpec { preset: rust_dos::makeimg::preset("fd_1440kb"), ..Default::default() };
    let floppy = rust_dos::makeimg::plan(&spec).unwrap();
    for name in ["one.img", "two.img"] {
        rust_dos::makeimg::write(&dir.join(name), &floppy, true).unwrap();
    }
    fs::write(dir.join("game.m3u"), "one.img\ntwo.img\n").unwrap();
    start(&dir, &[]);
    assert!(load(Some(&dir.join("game.m3u"))));
    run(30);
    let disks = with(|fe| fe.disks).expect("disk control");
    unsafe {
        assert_eq!((disks.get_num_images)(), 2);
        assert_eq!((disks.get_image_index)(), 0);
        assert!(!(disks.set_image_index)(1), "the drive is closed");
        assert!((disks.set_eject_state)(true));
        assert!((disks.set_image_index)(1));
        assert!((disks.set_eject_state)(false));
        assert_eq!((disks.get_image_index)(), 1);
        let mut label = [0 as c_char; 64];
        assert!((disks.get_image_label)(1, label.as_mut_ptr(), label.len()));
        assert_eq!(CStr::from_ptr(label.as_ptr()).to_str().unwrap(), "two.img");
    }
    run(2);
    assert!(with(|fe| fe.messages.iter().any(|m| m.contains("disk 2 of 2"))), "{:?}", with(|fe| fe.messages.clone()));
    stop();
}

#[test]
fn changed_options_reach_the_machine() {
    let dir = scratch("options");
    start(&dir, &[("rust_dos_aspect", "false")]);
    assert!(load(None));
    run(10);
    assert_eq!(with(|fe| fe.geometry.map(|g| g.aspect_ratio)), Some(640.0 / 400.0));
    with(|fe| {
        fe.variables.insert("rust_dos_aspect".into(), CString::new("true").unwrap());
        fe.options_changed = true;
    });
    run(2);
    assert_eq!(with(|fe| fe.geometry.map(|g| g.aspect_ratio)), Some(4.0 / 3.0));
    stop();
}

#[test]
fn exit_at_the_prompt_ends_the_content() {
    let dir = scratch("exit");
    start(&dir, &[]);
    assert!(load(None));
    run(60);
    type_text("EXIT\r");
    run(30);
    assert!(with(|fe| fe.shutdown));
    stop();
}


/// A test frontend with a VFS over `dir/vfs`, where `test://` paths are.
fn with_vfs(dir: &Path) -> PathBuf {
    let root = dir.join("vfs");
    fs::create_dir_all(&root).unwrap();
    with(|fe| fe.vfs_root = Some(root.clone()));
    root
}

#[test]
fn a_folder_the_frontend_keeps_is_c() {
    let dir = scratch("vfs-folder");
    let root = with_vfs(&dir);
    fs::create_dir_all(root.join("Game")).unwrap();
    fs::write(root.join("Game/MARKER.COM"), MARKER).unwrap();
    fs::write(root.join("Game/README.TXT"), "read me").unwrap();
    fs::write(
        root.join("Game/rust-dos.conf"),
        "[game]\noverlay=false\n[autoexec]\nECHO hello>NEW.TXT\nREN NEW.TXT DONE.TXT\nMD SUB\nCOPY DONE.TXT SUB\\COPY.TXT\nDEL README.TXT\nMARKER\n",
    )
    .unwrap();
    start(&dir, &[]);
    assert!(load(Some(Path::new("test://Game"))));
    run(600);
    assert_eq!(map_word(0x320), Some(0xBEEF), "the game runs");
    let game = root.join("Game");
    assert_eq!(fs::read_to_string(game.join("DONE.TXT")).unwrap().trim(), "hello");
    assert!(!game.join("NEW.TXT").exists(), "renamed by copying");
    assert_eq!(fs::read_to_string(game.join("SUB/COPY.TXT")).unwrap().trim(), "hello");
    assert!(!game.join("README.TXT").exists());
    assert!(with(|fe| fe.vfs_calls) > 0);
    stop();
}

#[test]
fn a_disk_image_the_frontend_keeps_is_read_and_written() {
    let dir = scratch("vfs-image");
    let root = with_vfs(&dir);
    let spec = rust_dos::makeimg::ImageSpec { preset: rust_dos::makeimg::preset("fd_1440kb"), ..Default::default() };
    let floppy = rust_dos::makeimg::plan(&spec).unwrap();
    rust_dos::makeimg::write(&root.join("disk.img"), &floppy, true).unwrap();
    let before = fs::read(root.join("disk.img")).unwrap();
    fs::create_dir_all(dir.join("system/rust-dos")).unwrap();
    fs::write(dir.join("system/rust-dos/rust-dos.conf"), "[autoexec]\nECHO X>A:\\T.TXT\n").unwrap();
    start(&dir, &[]);
    assert!(load(Some(Path::new("test://disk.img"))));
    run(300);
    assert!(with(|fe| fe.vfs_calls) > 0);
    stop();
    let after = fs::read(root.join("disk.img")).unwrap();
    assert_eq!(after.len(), before.len());
    assert!(after != before, "the file is on the disk");
}

#[test]
fn an_archive_the_frontend_keeps_is_read_where_it_is() {
    let dir = scratch("vfs-zip");
    let root = with_vfs(&dir);
    fs::write(root.join("Marker Game.zip"), zip(&[("MARKER.COM", MARKER)])).unwrap();
    start(&dir, &[]);
    assert!(load(Some(Path::new("test://Marker Game.zip"))));
    run(180);
    assert_eq!(map_word(0x320), Some(0xBEEF), "the game runs");
    assert!(!dir.join("saves/rust-dos/games/marker-game").exists(), "not unpacked");
    let profile = fs::read_to_string(dir.join("saves/rust-dos/games/marker-game.conf")).unwrap();
    assert!(profile.contains("C=\"test://Marker Game.zip\""), "{}", profile);
    stop();
}
