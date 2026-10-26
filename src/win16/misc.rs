//! The smaller system modules: KEYBOARD, SOUND, SYSTEM, WIN87EM,
//! TOOLHELP, SHELL, COMMDLG, MMSYSTEM, LZEXPAND, VER, WING, DDEML and
//! DISPLAY. Most of their functions go on to the built-in DLL that has
//! them in Win32 (USER32, WINMM, COMDLG32, VERSION); the rest are done
//! here, or answer as Windows 3.1 does when there is no such device.

use super::builtin::{Def, register, rust, win32};
use super::thunk;
use super::user::{P_MISC, P_WORDS};
use crate::cpu::Cpu;
use crate::dpmi::Context;

const U: &str = "USER32.DLL";
const K: &str = "KERNEL32.DLL";
const G: &str = "GDI32.DLL";
const MM: &str = "WINMM.DLL";
const SH: &str = "SHELL32.DLL";
const CD: &str = "COMDLG32.DLL";
const V: &str = "VERSION.DLL";

pub static KEYBOARD: Def = Def {
    name: "KEYBOARD",
    file: "KEYBOARD.DRV",
    apis: &[
        win32("ToAscii", 4, "wwppw", U, "ToAscii", b'w'),
        win32("AnsiToOem", 5, "pp", U, "CharToOemA", b'w'),
        win32("OemToAnsi", 6, "pp", U, "OemToCharA", b'w'),
        rust("SetSpeed", 7, "w", minus_one),
        win32("OemKeyScan", 128, "w", U, "OemKeyScan", b'w'),
        win32("VkKeyScan", 129, "w", U, "VkKeyScanA", b'w'),
        win32("GetKeyboardType", 130, "s", U, "GetKeyboardType", b'w'),
        win32("MapVirtualKey", 131, "ww", U, "MapVirtualKeyA", b'w'),
        rust("GetKBCodePage", 132, "", code_page),
        win32("GetKeyNameText", 133, "lps", U, "GetKeyNameTextA", b'w'),
        win32("AnsiToOemBuff", 134, "ppw", U, "CharToOemBuffA", b'w'),
        win32("OemToAnsiBuff", 135, "ppw", U, "OemToCharBuffA", b'w'),
    ],
    library: true,
};

/// SOUND: the PC speaker's voices, which no program gets: OpenSound
/// answers S_SERDVNA, the device is in use.
pub static SOUND: Def = Def {
    name: "SOUND",
    file: "SOUND.DRV",
    apis: &[
        rust("OpenSound", 1, "", minus_one),
        rust("CloseSound", 2, "", zero),
        rust("SetVoiceQueueSize", 3, "ww", zero),
        rust("SetVoiceNote", 4, "wwww", zero),
        rust("SetVoiceAccent", 5, "wwwww", zero),
        rust("SetVoiceEnvelope", 6, "www", zero),
        rust("SetSoundNoise", 7, "ww", zero),
        rust("SetVoiceSound", 8, "wlw", zero),
        rust("StartSound", 9, "", zero),
        rust("StopSound", 10, "", zero),
        rust("WaitSoundState", 11, "w", zero),
        rust("SyncAllVoices", 12, "", zero),
        rust("CountVoiceNotes", 13, "w", zero),
    ],
    library: true,
};

pub static SYSTEM: Def = Def {
    name: "SYSTEM",
    file: "SYSTEM.DRV",
    apis: &[
        rust("InquireSystem", 1, "ww", inquire_system),
        rust("CreateSystemTimer", 2, "wS", zero),
        rust("KillSystemTimer", 3, "w", zero),
        rust("EnableSystemTimers", 4, "", zero),
        rust("DisableSystemTimers", 5, "", zero),
        win32("GetSystemMSecCount", 6, "", K, "GetTickCount", b'w'),
        rust("Get80x87SaveSize", 7, "", fpu_save_size),
        rust("Save80x87State", 8, "p", zero),
        rust("Restore80x87State", 9, "p", zero),
        rust("A20_Proc", 20, "w", zero),
    ],
    library: true,
};

/// WIN87EM: the coprocessor emulator's interface. The processor has a
/// coprocessor, so there is nothing to emulate: __fpMath's functions
/// (BX) all succeed.
pub static WIN87EM: Def = Def {
    name: "WIN87EM",
    file: "WIN87EM.DLL",
    apis: &[
        register("__fpMath", 1, fp_math),
        rust("__WinEm87Info", 3, "pw", zero),
        rust("__WinEm87Restore", 4, "pw", zero),
        rust("__WinEm87Save", 5, "pw", zero),
    ],
    library: true,
};

pub static TOOLHELP: Def = Def {
    name: "TOOLHELP",
    file: "TOOLHELP.DLL",
    apis: &[
        rust("GlobalHandleToSel", 50, "w", global_handle_to_sel),
        rust("GlobalFirst", 51, "pw", zero),
        rust("GlobalNext", 52, "pw", zero),
        rust("GlobalInfo", 53, "p", zero),
        rust("GlobalEntryHandle", 54, "pw", zero),
        rust("GlobalEntryModule", 55, "pww", zero),
        rust("LocalInfo", 56, "pw", zero),
        rust("LocalFirst", 57, "pw", zero),
        rust("LocalNext", 58, "p", zero),
        rust("ModuleFirst", 59, "p", zero),
        rust("ModuleNext", 60, "p", zero),
        rust("ModuleFindName", 61, "pp", zero),
        rust("ModuleFindHandle", 62, "pw", zero),
        rust("TaskFirst", 63, "p", zero),
        rust("TaskNext", 64, "p", zero),
        rust("TaskFindHandle", 65, "pw", zero),
        rust("StackTraceFirst", 66, "pw", zero),
        rust("StackTraceCSIPFirst", 67, "pwwww", zero),
        rust("StackTraceNext", 68, "p", zero),
        rust("SystemHeapInfo", 71, "p", system_heap_info),
        rust("MemManInfo", 72, "p", mem_man_info),
        rust("NotifyRegister", 73, "wSw", one),
        rust("NotifyUnregister", 74, "w", one),
        rust("InterruptRegister", 75, "wS", one),
        rust("InterruptUnRegister", 76, "w", one),
        rust("TerminateApp", 77, "ww", zero),
        rust("MemoryRead", 78, "wlpl", memory_read),
        rust("MemoryWrite", 79, "wlpl", memory_write),
        rust("TimerCount", 80, "p", timer_count),
    ],
    library: true,
};

pub static SHELL: Def = Def {
    name: "SHELL",
    file: "SHELL.DLL",
    apis: &[
        rust("RegOpenKey", 1, "lpp", super::kernel::reg_fail),
        rust("RegCreateKey", 2, "lpp", super::kernel::reg_fail),
        rust("RegCloseKey", 3, "l", zero),
        rust("RegDeleteKey", 4, "lp", super::kernel::reg_fail),
        rust("RegSetValue", 5, "lplpl", super::kernel::reg_fail),
        rust("RegQueryValue", 6, "lppp", super::kernel::reg_fail),
        rust("RegEnumKey", 7, "llpl", super::kernel::reg_fail),
        win32("DragAcceptFiles", 9, "ww", SH, "DragAcceptFiles", b'w'),
        win32("DragQueryFile", 11, "wspw", SH, "DragQueryFileA", b'w'),
        win32("DragFinish", 12, "w", SH, "DragFinish", b'w'),
        win32("DragQueryPoint", 13, "wP", SH, "DragQueryPoint", b'b'),
        rust("ShellExecute", 20, "wpppps", shell_execute),
        win32("FindExecutable", 21, "ppp", SH, "FindExecutableA", b'w'),
        win32("ShellAbout", 22, "wppi", SH, "ShellAboutA", b'b'),
        win32("ExtractIcon", 34, "npw", SH, "ExtractIconA", b'i'),
        rust("RegisterShellHook", 102, "ww", zero),
        rust("DriveType", 262, "l", drive_type),
    ],
    library: true,
};

pub static COMMDLG: Def = Def {
    name: "COMMDLG",
    file: "COMMDLG.DLL",
    apis: &[
        rust("GetOpenFileName", 1, "p", get_open_file_name),
        rust("GetSaveFileName", 2, "p", get_save_file_name),
        rust("ChooseColor", 5, "p", choose_color),
        rust("FindText", 11, "p", no_dialog),
        rust("ReplaceText", 12, "p", no_dialog),
        rust("ChooseFont", 15, "p", no_dialog),
        rust("PrintDlg", 20, "p", no_dialog),
        rust("CommDlgExtendedError", 26, "", zero),
        win32("GetFileTitle", 27, "ppw", CD, "GetFileTitleA", b'w'),
    ],
    library: true,
};

pub static MMSYSTEM: Def = Def {
    name: "MMSYSTEM",
    file: "MMSYSTEM.DLL",
    apis: &[
        win32("sndPlaySound", 2, "pw", MM, "sndPlaySoundA", b'b'),
        rust("PlaySound", 3, "pwl", play_sound),
        rust("mmsystemGetVersion", 5, "", mmsystem_version),
        win32("joyGetNumDevs", 101, "", MM, "joyGetNumDevs", b'w'),
        rust("joyGetDevCaps", 102, "wpw", joy_unplugged),
        rust("joyGetPos", 103, "wp", joy_get_pos),
        rust("joyGetThreshold", 104, "wp", joy_unplugged),
        rust("joyReleaseCapture", 105, "w", joy_unplugged),
        rust("joySetCapture", 106, "wwww", joy_unplugged),
        rust("joySetThreshold", 107, "ww", joy_unplugged),
        rust("joySetCalibration", 109, "w", joy_unplugged),
        rust("midiOutGetNumDevs", 201, "", zero),
        rust("midiOutGetDevCaps", 202, "wpw", no_driver),
        rust("midiOutOpen", 204, "pwlll", no_driver),
        rust("midiOutClose", 205, "w", no_driver),
        rust("midiOutShortMsg", 208, "wl", no_driver),
        rust("midiOutReset", 210, "w", no_driver),
        rust("auxGetNumDevs", 350, "", zero),
        win32("waveOutGetNumDevs", 401, "", MM, "waveOutGetNumDevs", b'w'),
        rust("waveOutGetDevCaps", 402, "wpw", wave_out_get_dev_caps),
        win32(
            "waveOutGetErrorText",
            403,
            "wpw",
            MM,
            "waveOutGetErrorTextA",
            b'w',
        ),
        rust("waveOutOpen", 404, "pwplll", wave_out_open),
        win32("waveOutClose", 405, "w", MM, "waveOutClose", b'w'),
        rust("waveOutPrepareHeader", 406, "wSw", wave_out_prepare_header),
        rust(
            "waveOutUnprepareHeader",
            407,
            "wSw",
            wave_out_unprepare_header,
        ),
        rust("waveOutWrite", 408, "wSw", wave_out_write),
        win32("waveOutPause", 409, "w", MM, "waveOutPause", b'w'),
        win32("waveOutRestart", 410, "w", MM, "waveOutRestart", b'w'),
        win32("waveOutReset", 411, "w", MM, "waveOutReset", b'w'),
        rust("waveOutGetPosition", 412, "wpw", wave_out_get_position),
        win32("waveOutGetVolume", 415, "wp", MM, "waveOutGetVolume", b'w'),
        win32("waveOutSetVolume", 416, "wl", MM, "waveOutSetVolume", b'w'),
        win32("waveOutBreakLoop", 419, "w", MM, "waveOutBreakLoop", b'w'),
        rust("timeGetSystemTime", 601, "pw", time_get_system_time),
        rust("timeSetEvent", 602, "wwSlw", time_set_event),
        rust("timeKillEvent", 603, "w", zero),
        rust("timeGetDevCaps", 604, "pw", time_get_dev_caps),
        win32("timeBeginPeriod", 605, "w", MM, "timeBeginPeriod", b'w'),
        win32("timeEndPeriod", 606, "w", MM, "timeEndPeriod", b'w'),
        win32("timeGetTime", 607, "", MM, "timeGetTime", b'w'),
        rust("mmioOpen", 1210, "pSl", mmio_open),
        rust("mmioClose", 1211, "ww", mmio_close),
        win32("mmioRead", 1212, "wpl", MM, "mmioRead", b'w'),
        win32("mmioWrite", 1213, "wpl", MM, "mmioWrite", b'w'),
        win32("mmioSeek", 1214, "wlw", MM, "mmioSeek", b'w'),
        rust("mmioGetInfo", 1215, "wSw", mmio_get_info),
        rust("mmioSetInfo", 1216, "wSw", mmio_set_info),
        rust("mmioSetBuffer", 1217, "wSlw", mmio_set_buffer),
        win32("mmioFlush", 1218, "ww", MM, "mmioFlush", b'w'),
        rust("mmioAdvance", 1219, "wSw", mmio_advance),
        win32(
            "mmioStringToFOURCC",
            1220,
            "pw",
            MM,
            "mmioStringToFOURCCA",
            b'w',
        ),
        win32("mmioDescend", 1223, "wppw", MM, "mmioDescend", b'w'),
        win32("mmioAscend", 1224, "wpw", MM, "mmioAscend", b'w'),
        win32("mmioCreateChunk", 1225, "wpw", MM, "mmioCreateChunk", b'w'),
        rust("mciSendCommand", 701, "wwll", mci_send_command),
        win32("mciSendString", 702, "ppww", MM, "mciSendStringA", b'w'),
        win32("mciGetDeviceID", 703, "p", MM, "mciGetDeviceIDA", b'w'),
        win32(
            "mciGetErrorString",
            706,
            "lpw",
            MM,
            "mciGetErrorStringA",
            b'b',
        ),
    ],
    library: true,
};

/// LZEXPAND: compressed files are read as they are (the programs that
/// ship them have them expanded by their setup programs).
pub static LZEXPAND: Def = Def {
    name: "LZEXPAND",
    file: "LZEXPAND.DLL",
    apis: &[
        rust("LZCopy", 1, "ww", lz_copy),
        rust("LZOpenFile", 2, "ppw", super::kernel::open_file),
        rust("LZInit", 3, "w", first),
        rust("LZSeek", 4, "wlw", super::kernel::llseek),
        rust("LZRead", 5, "wpw", super::kernel::lread),
        rust("LZClose", 6, "w", super::kernel::lclose),
        rust("LZStart", 7, "", one),
        rust("CopyLZFile", 8, "ww", lz_copy),
        rust("LZDone", 9, "", zero),
        rust("GetExpandedName", 10, "pp", get_expanded_name),
    ],
    library: true,
};

pub static VER: Def = Def {
    name: "VER",
    file: "VER.DLL",
    apis: &[
        win32(
            "GetFileVersionInfoSize",
            6,
            "pp",
            V,
            "GetFileVersionInfoSizeA",
            b'w',
        ),
        win32(
            "GetFileVersionInfo",
            7,
            "pllp",
            V,
            "GetFileVersionInfoA",
            b'b',
        ),
        win32("VerFindFile", 8, "wppppppp", V, "VerFindFileA", b'w'),
        win32("VerInstallFile", 9, "wppppppp", V, "VerInstallFileA", b'w'),
        win32("VerLanguageName", 10, "wpw", V, "VerLanguageNameA", b'w'),
        rust("VerQueryValue", 11, "Sppp", ver_query_value),
    ],
    library: true,
};

pub static WING: Def = Def {
    name: "WING",
    file: "WING.DLL",
    apis: &[
        rust("WinGCreateDC", 1001, "", wing_create_dc),
        rust(
            "WinGRecommendDIBFormat",
            1002,
            "p",
            wing_recommend_dib_format,
        ),
        rust("WinGCreateBitmap", 1003, "wpp", wing_create_bitmap),
        rust("WinGGetDIBPointer", 1004, "wp", wing_get_dib_pointer),
        win32(
            "WinGGetDIBColorTable",
            1005,
            "wwwp",
            G,
            "GetDIBColorTable",
            b'w',
        ),
        win32(
            "WinGSetDIBColorTable",
            1006,
            "wwwp",
            G,
            "SetDIBColorTable",
            b'w',
        ),
        rust("WinGCreateHalfTonePalette", 1007, "", wing_halftone_palette),
        rust("WinGCreateHalfToneBrush", 1008, "wlw", wing_halftone_brush),
        rust("WinGStretchBlt", 1009, "wsssswssss", wing_stretch_blt),
        rust("WinGBitBlt", 1010, "wsssswss", wing_bit_blt),
    ],
    library: true,
};

/// DDEML: there are no DDE servers to talk to; DdeInitialize fails as it
/// does when DDEML can't start.
pub static DDEML: Def = Def {
    name: "DDEML",
    file: "DDEML.DLL",
    apis: &[
        rust("DdeInitialize", 2, "pSll", dde_initialize),
        rust("DdeUninitialize", 3, "l", one),
        rust("DdeGetLastError", 20, "l", dde_error),
    ],
    library: true,
};

/// DISPLAY: the display driver, which programs import for its resources
/// (the system's cursors and icons) more than its functions.
pub static DISPLAY: Def = Def {
    name: "DISPLAY",
    file: "DISPLAY.DRV",
    apis: &[],
    library: true,
};

fn zero(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn one(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

fn minus_one(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0xFFFF)
}

fn first(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0])
}

fn code_page(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(437)
}

fn r16(cpu: &Cpu, at: u32) -> u32 {
    super::read_16(cpu, at) as u32
}

fn r32(cpu: &Cpu, at: u32) -> u32 {
    super::read_32(cpu, at)
}

fn w32(cpu: &mut Cpu, at: u32, value: u32) {
    cpu.bus.guest_write_32(at, value);
}

/// The address of the built-in DLL `dll`'s function `name`.
fn api(dll: &str, name: &str) -> u32 {
    super::builtin::win32_address(dll, name).unwrap_or_else(|| panic!("{} has no {}", dll, name))
}

/// Go on to `dll`'s `name` with `args`, the 32-bit stack below `top`,
/// `post` seeing to what it returns with `data` when there is any.
fn go_at(
    cpu: &mut Cpu,
    top: u32,
    dll: &str,
    name: &str,
    args: &[u32],
    data: Vec<u32>,
) -> Option<u32> {
    let ctx = Context::of(cpu);
    let post = if data.is_empty() {
        thunk::POST_PLAIN
    } else {
        thunk::POST_USER
    };
    thunk::enter32_at(cpu, ctx, top, api(dll, name), args, post, data);
    None
}

fn go(cpu: &mut Cpu, dll: &str, name: &str, args: &[u32], data: Vec<u32>) -> Option<u32> {
    let top = thunk::scratch32(cpu, 0);
    go_at(cpu, top, dll, name, args, data)
}

/// What `post` does: `data[0]`.
const P_OFN: u32 = P_MISC;
const P_COLOR: u32 = P_MISC + 1;
const P_TIMER: u32 = P_MISC + 2;
const P_WAVE_CAPS: u32 = P_MISC + 3;
const P_WAVE_OPEN: u32 = P_MISC + 4;
const P_WAVE_UNPREPARE: u32 = P_MISC + 5;
const P_MMTIME: u32 = P_MISC + 6;
const P_WING_BITMAP: u32 = P_MISC + 7;
const P_SEGPTR: u32 = P_MISC + 8;

/// A 32-bit function returned `value`: what 16-bit code gets.
pub fn post(cpu: &mut Cpu, value: u32, data: &[u32]) -> Option<u32> {
    let d = |i: usize| data.get(i).copied().unwrap_or(0);
    match d(0) {
        P_OFN => {
            let (ofn, ofn16) = (d(1), d(2));
            // nFilterIndex, Flags, nFileOffset and nFileExtension.
            let index = r32(cpu, ofn + 24);
            super::write_32(cpu, ofn16 + 20, index);
            let flags = r32(cpu, ofn + 52) & !HOOK_FLAGS | d(3);
            super::write_32(cpu, ofn16 + 48, flags);
            let offsets = r32(cpu, ofn + 56);
            super::write_32(cpu, ofn16 + 52, offsets);
            Some((value != 0) as u32)
        }
        P_COLOR => {
            let rgb = r32(cpu, d(1) + 12);
            super::write_32(cpu, d(2) + 8, rgb);
            Some((value != 0) as u32)
        }
        P_TIMER => {
            // TIMERINFO: its size, then the milliseconds since Windows
            // started and since the virtual machine did.
            super::write_32(cpu, d(1) + 4, value);
            super::write_32(cpu, d(1) + 8, value);
            Some(1)
        }
        P_WAVE_CAPS => {
            if value == 0 {
                wave_caps16(cpu, d(1), d(2), d(3));
            }
            Some(value)
        }
        P_WAVE_OPEN => {
            if value == 0 && d(2) != 0 {
                let handle = r32(cpu, d(1));
                super::write_16(cpu, d(2), handle as u16);
            }
            Some(value)
        }
        P_WAVE_UNPREPARE => {
            if value == 0 {
                unprepared(cpu, d(1));
            }
            Some(value)
        }
        P_MMTIME => {
            if value == 0 {
                // MMTIME16: wType, then the union as it is.
                let kind = r32(cpu, d(1));
                super::write_16(cpu, d(2), kind as u16);
                let rest = super::peek(cpu, d(1) + 4, 8).unwrap_or(vec![0; 8]);
                super::poke(cpu, d(2) + 2, &rest);
            }
            Some(value)
        }
        P_WING_BITMAP => Some(wing_bitmap_made(cpu, value, d(1), d(2), d(3))),
        P_SEGPTR => {
            // VerQueryValue: the pointer it wrote at d(1), flat, as a
            // 16:16 one into the block at d(2) (flat) and d(3) (16:16).
            if value != 0 {
                let flat = r32(cpu, d(1));
                let segptr = d(3).wrapping_add(flat.wrapping_sub(d(2)));
                super::write_32(cpu, d(4), segptr);
                let len = r32(cpu, d(1) + 4);
                super::write_16(cpu, d(5), len as u16);
            }
            Some((value != 0) as u32)
        }
        _ => Some(value),
    }
}

// SYSTEM, WIN87EM, TOOLHELP.

fn inquire_system(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match args[0] {
        // The timer's period in microseconds.
        0 => 54925,
        // The drive's kind: A: and B: removable, the others fixed.
        1 => {
            if args[1] < 2 {
                2
            } else {
                3
            }
        }
        // Whether the system has a coprocessor... and the rest.
        _ => 1,
    })
}

fn fpu_save_size(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(94)
}

fn fp_math(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn global_handle_to_sel(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::global::handle_to_sel(args[0] as u16) as u32)
}

fn system_heap_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    // SYSHEAPINFO: dwSize, wUserFreePercent, wGDIFreePercent,
    // hUserSegment, hGDISegment.
    let at = args[0];
    for (i, v) in [80u16, 80, 0, 0].into_iter().enumerate() {
        super::write_16(cpu, at + 4 + 2 * i as u32, v);
    }
    Some(1)
}

fn mem_man_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    // MEMMANINFO: dwSize and nine counts of bytes and pages, then the
    // page's size.
    let at = args[0];
    let free = super::kernel::free_space(cpu);
    let pages = free / 4096;
    let values = [free, pages, pages, pages, pages, 0, pages * 2, 0, 0];
    for (i, v) in values.into_iter().enumerate() {
        super::write_32(cpu, at + 4 + 4 * i as u32, v);
    }
    super::write_16(cpu, at + 40, 4096);
    Some(1)
}

fn memory_read(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let from = super::lin(cpu, args[0] as u16, 0).wrapping_add(args[1]);
    let len = args[3].min(0x10_0000) as usize;
    let Some(bytes) = super::peek(cpu, from, len) else {
        return Some(0);
    };
    super::poke(cpu, args[2], &bytes);
    Some(len as u32)
}

fn memory_write(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let to = super::lin(cpu, args[0] as u16, 0).wrapping_add(args[1]);
    let len = args[3].min(0x10_0000) as usize;
    let Some(bytes) = super::peek(cpu, args[2], len) else {
        return Some(0);
    };
    super::poke(cpu, to, &bytes);
    Some(len as u32)
}

fn timer_count(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    go(cpu, K, "GetTickCount", &[], vec![P_TIMER, args[0]])
}

// SHELL.

fn shell_execute(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    go(
        cpu,
        SH,
        "ShellExecuteA",
        &[args[0], args[1], args[2], args[3], args[4], args[5]],
        Vec::new(),
    )
}

fn drive_type(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if args[0] < 2 { 2 } else { 3 })
}

// COMMDLG: the 16-bit structures as the 32-bit ones; hooks and templates
// of 16-bit code are left out.

/// OFN_ENABLEHOOK, OFN_ENABLETEMPLATE and OFN_ENABLETEMPLATEHANDLE.
const HOOK_FLAGS: u32 = 0xE0;

fn open_file_name(cpu: &mut Cpu, name: &str, ofn16: u32) -> Option<u32> {
    let ofn = thunk::scratch32(cpu, 76);
    let p = |cpu: &Cpu, off: u32| super::linear(cpu, r32(cpu, ofn16 + off));
    let instance = super::handles::instance32(cpu, r16(cpu, ofn16 + 6) as u16);
    let flags = r32(cpu, ofn16 + 48);
    let fields = [
        76,
        r16(cpu, ofn16 + 4),
        instance,
        p(cpu, 8),
        p(cpu, 12),
        r32(cpu, ofn16 + 16),
        r32(cpu, ofn16 + 20),
        p(cpu, 24),
        r32(cpu, ofn16 + 28),
        p(cpu, 32),
        r32(cpu, ofn16 + 36),
        p(cpu, 40),
        p(cpu, 44),
        flags & !HOOK_FLAGS,
        r32(cpu, ofn16 + 52),
        p(cpu, 56),
        r32(cpu, ofn16 + 60),
        0,
        0,
    ];
    for (i, f) in fields.into_iter().enumerate() {
        w32(cpu, ofn + 4 * i as u32, f);
    }
    go_at(
        cpu,
        ofn,
        CD,
        name,
        &[ofn],
        vec![P_OFN, ofn, ofn16, flags & HOOK_FLAGS],
    )
}

fn get_open_file_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    open_file_name(cpu, "GetOpenFileName16", args[0])
}

fn get_save_file_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    open_file_name(cpu, "GetSaveFileName16", args[0])
}

fn choose_color(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let cc16 = args[0];
    let cc = thunk::scratch32(cpu, 36);
    let instance = super::handles::instance32(cpu, r16(cpu, cc16 + 6) as u16);
    let custom = super::linear(cpu, r32(cpu, cc16 + 12));
    let fields = [
        36,
        r16(cpu, cc16 + 4),
        instance,
        r32(cpu, cc16 + 8),
        custom,
        // CC_ENABLEHOOK, CC_ENABLETEMPLATE and CC_ENABLETEMPLATEHANDLE.
        r32(cpu, cc16 + 16) & !0x70,
        r32(cpu, cc16 + 20),
        0,
        0,
    ];
    for (i, f) in fields.into_iter().enumerate() {
        w32(cpu, cc + 4 * i as u32, f);
    }
    go_at(cpu, cc, CD, "ChooseColorA", &[cc], vec![P_COLOR, cc, cc16])
}

/// The dialogs whose structures aren't converted: cancelled.
fn no_dialog(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    cpu.bus
        .log_string("[W16] A common dialog that isn't there was cancelled");
    Some(0)
}

// MMSYSTEM.

fn mmsystem_version(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0x030A)
}

fn play_sound(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    // SND_RESOURCE (0x40004) names a resource of the module; there are
    // no 16-bit ones in WINMM's reach, so those play nothing.
    if args[2] & 0x0004_0004 == 0x0004_0004 {
        return Some(0);
    }
    go(cpu, MM, "PlaySoundA", &[args[0], 0, args[2]], Vec::new())
}

/// JOYERR_UNPLUGGED.
fn joy_unplugged(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(167)
}

fn joy_get_pos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let info = thunk::scratch32(cpu, 16);
    go_at(
        cpu,
        info,
        MM,
        "joyGetPos",
        &[args[0], info],
        vec![P_WORDS, info, args[1], 4],
    )
}

/// MMSYSERR_NODRIVER.
fn no_driver(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(6)
}

/// WAVEOUTCAPSA at `caps` as WAVEOUTCAPS16 at `caps16`, `size` bytes of
/// it at most.
fn wave_caps16(cpu: &mut Cpu, caps: u32, caps16: u32, size: u32) {
    let mut b = Vec::new();
    b.extend_from_slice(&(r16(cpu, caps) as u16).to_le_bytes());
    b.extend_from_slice(&(r16(cpu, caps + 2) as u16).to_le_bytes());
    b.extend_from_slice(&(r32(cpu, caps + 4) as u16).to_le_bytes());
    b.extend(super::peek(cpu, caps + 8, 32).unwrap_or(vec![0; 32]));
    b.extend_from_slice(&r32(cpu, caps + 40).to_le_bytes());
    b.extend_from_slice(&(r16(cpu, caps + 44) as u16).to_le_bytes());
    b.extend_from_slice(&r32(cpu, caps + 48).to_le_bytes());
    b.truncate(size as usize);
    super::poke(cpu, caps16, &b);
}

fn wave_out_get_dev_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let caps = thunk::scratch32(cpu, 52);
    let device = if args[0] == 0xFFFF { u32::MAX } else { args[0] };
    go_at(
        cpu,
        caps,
        MM,
        "waveOutGetDevCapsA",
        &[device, caps, 52],
        vec![P_WAVE_CAPS, caps, args[1], args[2]],
    )
}

/// The callback kinds of waveOutOpen's flags.
const CALLBACK_TYPEMASK: u32 = 0x0007_0000;
const CALLBACK_WINDOW: u32 = 0x0001_0000;
const CALLBACK_TASK: u32 = 0x0002_0000;

/// waveOutOpen(handle *, device, format, callback, instance, flags): a
/// window or a task is told about the buffers played, as in Win32; a
/// 16-bit function isn't called (programs that ask for one poll the
/// headers' WHDR_DONE as well).
fn wave_out_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = thunk::scratch32(cpu, 4);
    w32(cpu, out, 0);
    let device = if args[1] == 0xFFFF { u32::MAX } else { args[1] };
    let mut flags = args[5];
    let callback = match flags & CALLBACK_TYPEMASK {
        CALLBACK_WINDOW => args[3] & 0xFFFF,
        CALLBACK_TASK => {
            let task = args[3] as u16;
            cpu.bus
                .win32
                .win16
                .tasks
                .iter()
                .find(|t| t.handle == task)
                .map_or(0, |t| t.tid)
        }
        _ => {
            flags &= !CALLBACK_TYPEMASK;
            0
        }
    };
    let target = if args[0] == 0 { 0 } else { out };
    go_at(
        cpu,
        out,
        MM,
        "waveOutOpen",
        &[target, device, args[2], callback, args[4], flags],
        vec![P_WAVE_OPEN, out, args[0]],
    )
}

/// The WAVEHDR at 16:16 `segptr` made one WINMM can use where it is: the
/// two have the same layout but for the data's pointer, which is made
/// flat while WINMM has the header. Its linear address.
fn wave_header(cpu: &mut Cpu, segptr: u32) -> u32 {
    let at = super::linear(cpu, segptr);
    if at == 0 {
        return 0;
    }
    let data = r32(cpu, at);
    let headers = &mut cpu.bus.win32.win16.wave_headers;
    let known = headers.iter().position(|h| h[0] == at);
    // The pointer is the flat one put there before: the program didn't
    // change it.
    if let Some(i) = known
        && headers[i][3] == data
    {
        return at;
    }
    let flat = super::linear(cpu, data);
    let entry = [at, segptr, data, flat];
    let headers = &mut cpu.bus.win32.win16.wave_headers;
    match known {
        Some(i) => headers[i] = entry,
        None => headers.push(entry),
    }
    w32(cpu, at, flat);
    at
}

/// WINMM let go of the header at `at`: its data's 16:16 pointer back.
fn unprepared(cpu: &mut Cpu, at: u32) {
    let headers = &mut cpu.bus.win32.win16.wave_headers;
    if let Some(i) = headers.iter().position(|h| h[0] == at) {
        let h = headers.remove(i);
        w32(cpu, at, h[2]);
    }
}

/// The 16:16 address of the wave header at the linear `at`, for the
/// MM_WOM_DONE message's lParam.
pub fn wave_header16(cpu: &Cpu, at: u32) -> u32 {
    cpu.bus
        .win32
        .win16
        .wave_headers
        .iter()
        .find(|h| h[0] == at)
        .map_or(at, |h| h[1])
}

fn wave_out_prepare_header(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = wave_header(cpu, args[1]);
    go(
        cpu,
        MM,
        "waveOutPrepareHeader",
        &[args[0], at, 32],
        Vec::new(),
    )
}

fn wave_out_unprepare_header(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = wave_header(cpu, args[1]);
    go(
        cpu,
        MM,
        "waveOutUnprepareHeader",
        &[args[0], at, 32],
        vec![P_WAVE_UNPREPARE, at],
    )
}

fn wave_out_write(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = wave_header(cpu, args[1]);
    go(cpu, MM, "waveOutWrite", &[args[0], at, 32], Vec::new())
}

fn wave_out_get_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let time = thunk::scratch32(cpu, 12);
    let kind = r16(cpu, args[1]);
    w32(cpu, time, kind);
    go_at(
        cpu,
        time,
        MM,
        "waveOutGetPosition",
        &[args[0], time, 12],
        vec![P_MMTIME, time, args[1]],
    )
}

fn time_get_system_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let time = thunk::scratch32(cpu, 12);
    go_at(
        cpu,
        time,
        MM,
        "timeGetSystemTime",
        &[time, 12],
        vec![P_MMTIME, time, args[0]],
    )
}

fn time_get_dev_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    // TIMECAPS16: the shortest and the longest period.
    super::write_16(cpu, args[0], 1);
    super::write_16(cpu, args[0] + 2, 0xFFFF);
    Some(0)
}

/// timeSetEvent: Windows 3.1 calls the callback at interrupt time, which
/// tasks taking turns have no room for; programs are told there is no
/// timer, as when they are all taken.
fn time_set_event(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    cpu.bus
        .log_string("[W16] timeSetEvent's callbacks aren't called");
    Some(0)
}

// MMIO: WINMM's functions, called here with MMIOINFO16 translated to and
// from MMIOINFO, which has dwords where it has words. A file's buffer is
// the program's or a block of the global heap, so that its pointers can
// be 16:16 ones for 16-bit code: `mmio_buffers` has each buffer's 16:16
// and flat addresses.

use crate::win32::winmm::mmio as wmmio;

const MMIO_ALLOCBUF: u32 = 0x0001_0000;
const MMIO_DEFAULTBUFFER: u32 = 8192;
const MMIOERR_CANNOTOPEN: u32 = 259;
const MMIOERR_OUTOFMEMORY: u32 = 258;
const MMSYSERR_INVALHANDLE: u32 = 5;

/// A block of the global heap of `size` bytes for a file's buffer: its
/// 16:16 and flat addresses and its handle.
fn mmio_alloc(cpu: &mut Cpu, size: u32) -> Option<[u32; 3]> {
    let handle = super::global::alloc(cpu, 0, size.max(1), 0, false);
    if handle == 0 {
        return None;
    }
    let sel = super::global::handle_to_sel(handle);
    let flat = super::lin(cpu, sel, 0);
    Some([(sel as u32) << 16, flat, handle as u32])
}

/// The file `hmmio`'s buffer: [hmmio, 16:16, flat, global handle].
fn mmio_buffer(cpu: &Cpu, hmmio: u32) -> Option<[u32; 4]> {
    let list = &cpu.bus.win32.win16.mmio_buffers;
    list.iter().find(|b| b[0] == hmmio).copied()
}

/// The file `hmmio` has the buffer `entry` from now on, or none; the
/// global block it had is freed.
fn mmio_keep_buffer(cpu: &mut Cpu, hmmio: u32, entry: Option<[u32; 4]>) {
    if let Some(old) = mmio_buffer(cpu, hmmio) {
        if old[3] != 0 {
            super::global::free(cpu, old[3] as u16);
        }
        cpu.bus.win32.win16.mmio_buffers.retain(|b| b[0] != hmmio);
    }
    if let Some(e) = entry {
        cpu.bus.win32.win16.mmio_buffers.push(e);
    }
}

/// A flat pointer into the file's buffer as a 16:16 one.
fn mmio_to16(buffer: Option<[u32; 4]>, flat: u32) -> u32 {
    match buffer {
        Some([_, segptr, base, _]) if flat >= base && flat - base <= 0xFFFF => {
            segptr + (flat - base)
        }
        _ => 0,
    }
}

/// A 16:16 pointer into the file's buffer as a flat one.
fn mmio_to32(cpu: &Cpu, buffer: Option<[u32; 4]>, segptr: u32) -> u32 {
    match buffer {
        Some([_, seg, base, _]) if segptr >> 16 == seg >> 16 => {
            base + (segptr & 0xFFFF) - (seg & 0xFFFF)
        }
        _ => super::linear(cpu, segptr),
    }
}

/// Write the MMIOINFO at `at` as the MMIOINFO16 at `at16`: dwFlags,
/// fccIOProc, pIOProc, wErrorRet, hTask, cchBuffer, the four buffer
/// pointers, lBufOffset, lDiskOffset, adwInfo[3], two reserved dwords
/// and hmmio, a word.
fn mmio_info16(cpu: &mut Cpu, hmmio: u32, at: u32, at16: u32) {
    let buffer = mmio_buffer(cpu, hmmio);
    let f = |i: u32| r32(cpu, at + 4 * i);
    let words = [
        f(0),
        f(1),
        0,
        f(3) & 0xFFFF,
        f(5),
        mmio_to16(buffer, f(6)),
        mmio_to16(buffer, f(7)),
        mmio_to16(buffer, f(8)),
        mmio_to16(buffer, f(9)),
        f(10),
        f(11),
        f(12),
        f(13),
        f(14),
        0,
        0,
    ];
    let mut bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    bytes.extend_from_slice(&(hmmio as u16).to_le_bytes());
    super::poke(cpu, at16, &bytes);
}

/// The file's MMIOINFO in 32-bit scratch space, with the flags and the
/// buffer pointers of the MMIOINFO16 at `at16`, for mmioSetInfo and
/// mmioAdvance. None if `hmmio` isn't a file.
fn mmio_info32(cpu: &mut Cpu, hmmio: u32, at16: u32) -> Option<u32> {
    let at = thunk::scratch32(cpu, 72);
    if wmmio::mmio_get_info(cpu, &[hmmio, at, 0])? != 0 {
        return None;
    }
    let buffer = mmio_buffer(cpu, hmmio);
    w32(cpu, at, r32(cpu, at16));
    for (o16, o32) in [(24, 28), (28, 32), (32, 36)] {
        let p = mmio_to32(cpu, buffer, r32(cpu, at16 + o16));
        w32(cpu, at + o32, p);
    }
    Some(at)
}

/// mmioOpen(name, info, flags): with a 32-bit copy of the MMIOINFO16 if
/// there is one, whose error and handle go back to it. A buffer that
/// MMIO_ALLOCBUF, or a size without a buffer, asks for is a block of the
/// global heap. A 16-bit I/O procedure isn't called: the file fails to
/// open.
fn mmio_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (name, info16, mut flags) = (args[0], args[1], args[2]);
    let at16 = super::linear(cpu, info16);
    let r = |cpu: &Cpu, o: u32| if at16 == 0 { 0 } else { r32(cpu, at16 + o) };
    if r(cpu, 8) != 0 {
        super::write_16(cpu, at16 + 12, MMIOERR_CANNOTOPEN as u16);
        return Some(0);
    }
    // MMIOINFO: dwFlags, fccIOProc, pIOProc, wErrorRet, htask,
    // cchBuffer, pchBuffer, the other pointers, the offsets, adwInfo.
    let info = thunk::scratch32(cpu, 72);
    super::poke(cpu, info, &[0; 72]);
    let mut cch = r(cpu, 16);
    let mut buffer = None;
    if at16 != 0 {
        w32(cpu, info, r(cpu, 0));
        w32(cpu, info + 4, r(cpu, 4));
        for i in 0..3 {
            w32(cpu, info + 48 + 4 * i, r(cpu, 44 + 4 * i));
        }
        let segptr = r(cpu, 20);
        if segptr != 0 {
            buffer = Some([segptr, super::linear(cpu, segptr), 0]);
        }
    }
    if buffer.is_none() && (flags & MMIO_ALLOCBUF != 0 || cch != 0) {
        if cch == 0 {
            cch = MMIO_DEFAULTBUFFER;
        }
        let Some(block) = mmio_alloc(cpu, cch) else {
            if at16 != 0 {
                super::write_16(cpu, at16 + 12, MMIOERR_OUTOFMEMORY as u16);
            }
            return Some(0);
        };
        buffer = Some(block);
    }
    flags &= !MMIO_ALLOCBUF;
    if let Some([_, flat, _]) = buffer {
        w32(cpu, info + 20, cch);
        w32(cpu, info + 24, flat);
    }
    let hmmio = wmmio::mmio_open(cpu, &[name, info, flags])?;
    if at16 != 0 {
        let error = r32(cpu, info + 12);
        super::write_16(cpu, at16 + 12, error as u16);
        super::write_16(cpu, at16 + 64, hmmio as u16);
    }
    match buffer {
        Some([segptr, flat, handle]) if hmmio != 0 => {
            mmio_keep_buffer(cpu, hmmio, Some([hmmio, segptr, flat, handle]));
        }
        Some([_, _, handle]) if handle != 0 => {
            super::global::free(cpu, handle as u16);
        }
        _ => {}
    }
    Some(hmmio)
}

/// mmioClose(hmmio, flags): and the global block of its buffer is freed.
fn mmio_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let result = wmmio::mmio_close(cpu, &[args[0], args[1]])?;
    if result == 0 {
        mmio_keep_buffer(cpu, args[0], None);
    }
    Some(result)
}

/// mmioGetInfo(hmmio, info, flags).
fn mmio_get_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at16 = super::linear(cpu, args[1]);
    let at = if at16 == 0 {
        0
    } else {
        thunk::scratch32(cpu, 72)
    };
    let result = wmmio::mmio_get_info(cpu, &[args[0], at, 0])?;
    if result == 0 {
        mmio_info16(cpu, args[0], at, at16);
    }
    Some(result)
}

/// mmioSetInfo(hmmio, info, flags).
fn mmio_set_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at16 = super::linear(cpu, args[1]);
    if at16 == 0 {
        return wmmio::mmio_set_info(cpu, &[args[0], 0, 0]);
    }
    match mmio_info32(cpu, args[0], at16) {
        Some(at) => wmmio::mmio_set_info(cpu, &[args[0], at, 0]),
        None => Some(MMSYSERR_INVALHANDLE),
    }
}

/// mmioAdvance(hmmio, info, flags).
fn mmio_advance(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at16 = super::linear(cpu, args[1]);
    if at16 == 0 {
        return wmmio::mmio_advance(cpu, &[args[0], 0, args[2]]);
    }
    let Some(at) = mmio_info32(cpu, args[0], at16) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    let result = wmmio::mmio_advance(cpu, &[args[0], at, args[2]])?;
    if result == 0 {
        mmio_info16(cpu, args[0], at, at16);
    }
    Some(result)
}

/// mmioSetBuffer(hmmio, buffer, size, flags): the program's buffer, or
/// without one a block of the global heap of `size` bytes.
fn mmio_set_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hmmio, segptr, cch) = (args[0], args[1], args[2]);
    // With no MMIOINFO, mmioGetInfo tells a file from a handle that isn't.
    if wmmio::mmio_get_info(cpu, &[hmmio, 0, 0])? == MMSYSERR_INVALHANDLE {
        return Some(MMSYSERR_INVALHANDLE);
    }
    let entry = if segptr != 0 {
        Some([hmmio, segptr, super::linear(cpu, segptr), 0])
    } else if cch != 0 {
        let Some([seg, flat, handle]) = mmio_alloc(cpu, cch) else {
            return Some(MMIOERR_OUTOFMEMORY);
        };
        Some([hmmio, seg, flat, handle])
    } else {
        None
    };
    let flat = entry.map_or(0, |e| e[2]);
    let result = wmmio::mmio_set_buffer(cpu, &[hmmio, flat, cch, 0])?;
    if result == 0 {
        mmio_keep_buffer(cpu, hmmio, entry);
    } else if let Some(e) = entry
        && e[3] != 0
    {
        super::global::free(cpu, e[3] as u16);
    }
    Some(result)
}

/// mciSendCommand: the 16-bit parameter blocks aren't converted;
/// MCIERR_UNSUPPORTED_FUNCTION.
fn mci_send_command(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    cpu.bus.log_string(&format!(
        "[W16] mciSendCommand({:X}, {:X}) isn't done",
        args[0], args[1]
    ));
    Some(274)
}

// LZEXPAND.

/// LZCopy(from, to): the file copied as it is. The bytes copied, or
/// LZERROR_READ.
fn lz_copy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (from, to) = (args[0] as u16, args[1] as u16);
    let mut total = 0u32;
    let chunk = super::global::alloc(cpu, 0, 0x8000, 0, false);
    if chunk == 0 {
        return Some(0xFFFF_FFFE);
    }
    let at = super::lin(cpu, super::global::handle_to_sel(chunk), 0);
    loop {
        let Ok(n) = super::dos::transfer(cpu, from, at, 0x8000, false) else {
            super::global::free(cpu, chunk);
            return Some(0xFFFF_FFFE);
        };
        if n == 0 {
            break;
        }
        if super::dos::transfer(cpu, to, at, n, true).is_err() {
            super::global::free(cpu, chunk);
            return Some(0xFFFF_FFFB);
        }
        total += n;
        if n < 0x8000 {
            break;
        }
    }
    super::global::free(cpu, chunk);
    Some(total)
}

/// GetExpandedName(from, to): the name as it is.
fn get_expanded_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut name = super::string_bytes(cpu, args[0], 0x100);
    name.push(0);
    super::poke(cpu, args[1], &name);
    Some(1)
}

// VER.

/// VerQueryValue(block, sub-block, buffer *, length *): VERSION's, its
/// pointer into the block made a 16:16 one into the program's.
fn ver_query_value(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let block = super::linear(cpu, args[0]);
    let out = thunk::scratch32(cpu, 8);
    w32(cpu, out, 0);
    w32(cpu, out + 4, 0);
    go_at(
        cpu,
        out,
        V,
        "VerQueryValueA",
        &[block, args[1], out, out + 4],
        vec![P_SEGPTR, out, block, args[0], args[2], args[3]],
    )
}

// WING: its DCs and bitmaps are GDI's; a WinG bitmap's bits are a DIB
// section's, which 16-bit code gets selectors for.

fn wing_create_dc(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    go(cpu, G, "CreateCompatibleDC", &[0], Vec::new())
}

/// The format WinG's bitmaps are fastest in: 8 bits a pixel, top down.
fn wing_recommend_dib_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 {
        return Some(0);
    }
    let header: [u32; 10] = [40, 1, (-1i32) as u32, 0x0008_0001, 0, 0, 0, 0, 0, 0];
    for (i, v) in header.into_iter().enumerate() {
        super::write_32(cpu, at + 4 * i as u32, v);
    }
    Some(1)
}

fn wing_create_bitmap(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, bmi, bits16) = (args[0], args[1], args[2]);
    let width = r32(cpu, bmi + 4) as i32;
    let height = r32(cpu, bmi + 8) as i32;
    let bpp = r16(cpu, bmi + 14);
    let stride = (width.unsigned_abs() * bpp).div_ceil(32) * 4;
    let size = stride * height.unsigned_abs();
    let out = thunk::scratch32(cpu, 4);
    w32(cpu, out, 0);
    go_at(
        cpu,
        out,
        G,
        "CreateDIBSection",
        &[hdc, bmi, 0, out, 0, 0],
        vec![P_WING_BITMAP, out, size, bits16],
    )
}

/// CreateDIBSection made the bitmap `bitmap`, its bits' address at `out`
/// and `size` bytes long: selectors for them, written at `bits16` as a
/// 16:16 pointer when that isn't null. The bitmap's handle.
fn wing_bitmap_made(cpu: &mut Cpu, bitmap: u32, out: u32, size: u32, bits16: u32) -> u32 {
    if bitmap == 0 {
        return 0;
    }
    let bits = r32(cpu, out);
    let owner = super::current_task(cpu).map_or(0, |t| cpu.bus.win32.win16.tasks[t].psp);
    let segptr = super::global::create_block(cpu, 0, bits, size, owner, false, true)
        .map_or(0, |h| (super::global::handle_to_sel(h) as u32) << 16);
    cpu.bus.win32.win16.wing_bitmaps.push((bitmap, segptr));
    if bits16 != 0 {
        super::write_32(cpu, bits16, segptr);
    }
    bitmap
}

fn wing_get_dib_pointer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let bitmap = args[0];
    Some(
        cpu.bus
            .win32
            .win16
            .wing_bitmaps
            .iter()
            .rev()
            .find(|b| b.0 & 0xFFFF == bitmap & 0xFFFF)
            .map_or(0, |b| b.1),
    )
}

fn wing_halftone_palette(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    // DEFAULT_PALETTE.
    go(cpu, G, "GetStockObject", &[15], Vec::new())
}

fn wing_halftone_brush(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    go(cpu, G, "CreateSolidBrush", &[args[1]], Vec::new())
}

/// SRCCOPY.
const SRCCOPY: u32 = 0x00CC_0020;

fn wing_stretch_blt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut all = args[..10].to_vec();
    all.push(SRCCOPY);
    go(cpu, G, "StretchBlt", &all, Vec::new())
}

fn wing_bit_blt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut all = args[..8].to_vec();
    all.push(SRCCOPY);
    go(cpu, G, "BitBlt", &all, Vec::new())
}

// DDEML.

/// DMLERR_DLL_USAGE.
fn dde_initialize(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0x4004)
}

fn dde_error(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0x4004)
}
