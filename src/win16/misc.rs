//! The smaller system modules: KEYBOARD, SOUND, SYSTEM, WIN87EM,
//! TOOLHELP, SHELL, COMMDLG, MMSYSTEM, LZEXPAND, VER, WING and DISPLAY
//! (DDEML has a file of its own, ddeml.rs). Most of their functions go
//! on to the built-in DLL that has them in Win32 (USER32, WINMM,
//! COMDLG32, VERSION); the rest are done here, or answer as Windows 3.1
//! does when there is no such device.

use super::builtin::{Def, register, rust, win32};
use super::global::{HeapItem, HeapPlace};
use super::lz;
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
        rust("__Win87EmInfo", 3, "pw", zero),
        rust("__Win87EmRestore", 4, "pw", zero),
        rust("__Win87EmSave", 5, "pw", zero),
    ],
    library: true,
};

pub static TOOLHELP: Def = Def {
    name: "TOOLHELP",
    file: "TOOLHELP.DLL",
    apis: &[
        rust("GlobalHandleToSel", 50, "w", global_handle_to_sel),
        rust("GlobalFirst", 51, "pw", global_first),
        rust("GlobalNext", 52, "pw", global_next),
        rust("GlobalInfo", 53, "p", global_info),
        rust("GlobalEntryHandle", 54, "pw", global_entry_handle),
        rust("GlobalEntryModule", 55, "pww", global_entry_module),
        rust("LocalInfo", 56, "pw", local_info),
        rust("LocalFirst", 57, "pw", local_first),
        rust("LocalNext", 58, "p", local_next),
        rust("ModuleFirst", 59, "p", module_first),
        rust("ModuleNext", 60, "p", module_next),
        rust("ModuleFindName", 61, "pp", module_find_name),
        rust("ModuleFindHandle", 62, "pw", module_find_handle),
        rust("TaskFirst", 63, "p", task_first),
        rust("TaskNext", 64, "p", task_next),
        rust("TaskFindHandle", 65, "pw", task_find_handle),
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
        rust("RegOpenKey", 1, "lpp", super::kernel::reg_open_key),
        rust("RegCreateKey", 2, "lpp", super::kernel::reg_create_key),
        rust("RegCloseKey", 3, "l", super::kernel::reg_close_key),
        rust("RegDeleteKey", 4, "lp", super::kernel::reg_delete_key),
        rust("RegSetValue", 5, "lplpl", super::kernel::reg_set_value),
        rust("RegQueryValue", 6, "lppp", super::kernel::reg_query_value),
        rust("RegEnumKey", 7, "llpl", super::kernel::reg_enum_key),
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
        win32("auxGetNumDevs", 350, "", MM, "auxGetNumDevs", b'w'),
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
        rust("waveOutClose", 405, "w", wave_out_close),
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
        rust("waveOutReset", 411, "w", wave_out_reset),
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
        // Saved states keep the stubs in this order: new functions go
        // after the others (src/win32/stub_tables.rs).
        rust("midiOutGetVolume", 211, "wp", no_driver),
        rust("midiOutSetVolume", 212, "wl", no_driver),
        rust("midiOutGetID", 215, "wp", invalid_handle),
        rust("auxGetDevCaps", 351, "wpw", aux_get_dev_caps),
        rust("auxGetVolume", 352, "wp", aux_get_volume),
        rust("auxSetVolume", 353, "wl", aux_set_volume),
        rust("waveOutGetID", 420, "wp", wave_out_get_id),
    ],
    library: true,
};

/// LZEXPAND: compressed files read expanded (lz.rs).
pub static LZEXPAND: Def = Def {
    name: "LZEXPAND",
    file: "LZEXPAND.DLL",
    apis: &[
        rust("LZCopy", 1, "ww", lz::lz_copy),
        rust("LZOpenFile", 2, "ppw", lz::lz_open_file),
        rust("LZInit", 3, "w", lz::lz_init),
        rust("LZSeek", 4, "wlw", lz::lz_seek),
        rust("LZRead", 5, "wpw", lz::lz_read),
        rust("LZClose", 6, "w", lz::lz_close),
        rust("LZStart", 7, "", one),
        rust("CopyLZFile", 8, "ww", lz::lz_copy),
        rust("LZDone", 9, "", zero),
        rust("GetExpandedName", 10, "pp", lz::get_expanded_name),
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

/// DISPLAY: the display driver, which programs import for its resources
/// (the system's cursors and icons) more than its functions.
pub static DISPLAY: Def = Def {
    name: "DISPLAY",
    file: "DISPLAY.DRV",
    apis: &[],
    library: true,
};

/// SETUPX: Windows 95's setup engine, which DirectX's setup DLL and
/// other installers' 16-bit DLLs import. None of its functions is there
/// yet: a call of one stops the program and names it.
pub static SETUPX: Def = Def {
    name: "SETUPX",
    file: "SETUPX.DLL",
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

/// GetKBCodePage: the system's OEM code page, as the 32-bit one says.
fn code_page(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(crate::win32::nls::oem(cpu).number())
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
fn api(cpu: &Cpu, dll: &str, name: &str) -> u32 {
    super::builtin::win32_address(cpu, dll, name).unwrap_or_else(|| panic!("{} has no {}", dll, name))
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
    let target = api(cpu, dll, name);
    thunk::enter32_at(cpu, ctx, top, target, args, post, data);
    None
}

pub(super) fn go(
    cpu: &mut Cpu,
    dll: &str,
    name: &str,
    args: &[u32],
    data: Vec<u32>,
) -> Option<u32> {
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
const P_MCI: u32 = P_MISC + 9;
const P_WAVE_CALLS: u32 = P_MISC + 10;
const P_UINT: u32 = P_MISC + 11;
const P_DWORD: u32 = P_MISC + 12;
const P_AUX_CAPS: u32 = P_MISC + 13;

/// The size of Windows 3.1's MMTIME: wType and a union of 6 bytes, its
/// SMPTE time being hour, minute, second, frame, fps and a pad byte
/// (Win32's has two, and a DWORD wType).
const MMTIME16_SIZE: u32 = 8;
/// MMTIME's kinds whose union is SMPTE time; the others are a DWORD.
const TIME_SMPTE: u32 = 8;
/// What a short structure gets from the wave functions, and from
/// timeGetDevCaps.
const MMSYSERR_INVALPARAM: u32 = 11;
const TIMERR_NOCANDO: u32 = 97;

/// A 32-bit function returned `value`: what 16-bit code gets, None
/// when 16-bit code is called before the function returns (`ctx`).
pub fn post(cpu: &mut Cpu, value: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
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
                if d(3) != 0 {
                    let tid = super::task(cpu).tid;
                    crate::win32::winmm::wave::call16_on(cpu, handle, tid, d(3));
                }
            }
            return_after_calls(cpu, value, ctx)
        }
        P_WAVE_CALLS => return_after_calls(cpu, value, ctx),
        P_UINT => {
            // A UINT the 32-bit function wrote at d(1), as Windows 3.1's
            // 16-bit one at d(2); nothing when it failed.
            if value == 0 {
                let v = r32(cpu, d(1));
                super::write_16(cpu, d(2), v as u16);
            }
            Some(value)
        }
        P_DWORD => {
            if value == 0 {
                let v = r32(cpu, d(1));
                super::write_32(cpu, d(2), v);
            }
            Some(value)
        }
        P_AUX_CAPS => {
            if value == 0 {
                aux_caps16(cpu, d(1), d(2), d(3));
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
                mmtime16(cpu, d(1), d(2));
            }
            Some(value)
        }
        P_WING_BITMAP => Some(wing_bitmap_made(cpu, value, d(1), d(2), d(3))),
        P_MCI => {
            if value == 0 {
                mci_returned(cpu, d(1), d(2), d(3));
            }
            // A status's answer, copied to the program's block, and the
            // error code in DX:AX as it is: WINMM's function already
            // said whether an idle proof goes through the command.
            if d(3) == wmci::MCI_STATUS {
                crate::idle::pure_call(cpu, None);
                crate::idle::reads_arguments(cpu);
                crate::idle::reads_also(cpu, d(1) + 4, 4);
                crate::idle::resumed16(cpu, true);
            }
            Some(value)
        }
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

/// GlobalFirst's and GlobalNext's flags: every block and free piece, the
/// blocks of the LRU list, or the free pieces.
const GLOBAL_ALL: u32 = 0;
const GLOBAL_LRU: u32 = 1;
const GLOBAL_FREE: u32 = 2;

/// GLOBALENTRY's size, which its dwSize must be, and its wFlags' bit
/// for a block a task owns. The low byte of wFlags is KERNEL's flags of
/// the block.
const GLOBALENTRY_SIZE: u32 = 36;
const GF_PDB_OWNER: u16 = 0x0100;

/// Whether `place` is on the list GlobalFirst's `flags` walk.
fn on_list(place: &HeapPlace, flags: u32) -> bool {
    match flags {
        GLOBAL_ALL => true,
        GLOBAL_LRU => place.lru,
        GLOBAL_FREE => place.is_free(),
        _ => false,
    }
}

/// Whether `at` is a GLOBALENTRY whose dwSize is set, as TOOLHELP checks
/// before it writes one.
fn global_entry_at(cpu: &Cpu, at: u32) -> bool {
    at != 0 && super::read_32(cpu, at) == GLOBALENTRY_SIZE
}

/// Fill the GLOBALENTRY at `at` with `item`. dwNext is the address of
/// the item after it in the heap (`places`), and dwNextAlt that of the
/// item after it on the LRU or free list it is on, 0 at a list's end
/// and for a discarded block, which isn't in the heap; GlobalNext goes
/// on from there.
fn fill_global_entry(cpu: &mut Cpu, at: u32, places: &[HeapPlace], item: &HeapItem) {
    let after = |list: Option<u32>| {
        if item.address == 0 {
            return 0;
        }
        places
            .iter()
            .find(|p| p.address > item.address && list.is_none_or(|f| on_list(p, f)))
            .map_or(0, |p| p.address)
    };
    let alt = if item.lru {
        Some(GLOBAL_LRU)
    } else if item.kind == super::global::GT_FREE {
        Some(GLOBAL_FREE)
    } else {
        None
    };
    let next = after(None);
    let next_alt = alt.map_or(0, |f| after(Some(f)));
    super::write_32(cpu, at + 4, item.address);
    super::write_32(cpu, at + 8, item.size);
    let words = [
        item.handle,
        item.lock,
        item.page_lock,
        item.flags as u16 | if item.pdb_owner { GF_PDB_OWNER } else { 0 },
        item.heap_present as u16,
        item.owner,
        item.kind,
        item.data,
    ];
    for (i, w) in words.into_iter().enumerate() {
        super::write_16(cpu, at + 12 + 2 * i as u32, w);
    }
    super::write_32(cpu, at + 28, next);
    super::write_32(cpu, at + 32, next_alt);
}

/// GlobalFirst(lpGlobal, wFlags): the first item of the list `wFlags`
/// names.
fn global_first(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (at, flags) = (args[0], args[1] & 0xFFFF);
    if !global_entry_at(cpu, at) {
        return Some(0);
    }
    let places = super::global::heap_places(cpu);
    let Some(place) = places.iter().find(|p| on_list(p, flags)) else {
        return Some(0);
    };
    let item = super::global::place_item(cpu, place);
    fill_global_entry(cpu, at, &places, &item);
    Some(1)
}

/// GlobalNext(lpGlobal, wFlags): the item the entry's dwNext (for
/// GLOBAL_ALL) or dwNextAlt (for the LRU and free lists) points to, or
/// the first one after it if that block is gone.
fn global_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (at, flags) = (args[0], args[1] & 0xFFFF);
    if !global_entry_at(cpu, at) {
        return Some(0);
    }
    let cursor = super::read_32(cpu, at + if flags == GLOBAL_ALL { 28 } else { 32 });
    if cursor == 0 {
        return Some(0);
    }
    let places = super::global::heap_places(cpu);
    let found = places
        .iter()
        .find(|p| p.address >= cursor && on_list(p, flags));
    let Some(place) = found else {
        return Some(0);
    };
    let item = super::global::place_item(cpu, place);
    fill_global_entry(cpu, at, &places, &item);
    Some(1)
}

/// GlobalInfo(lpGlobalInfo): GLOBALINFO's counts of the heap's items,
/// its free pieces and the LRU list's blocks.
fn global_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 {
        return Some(0);
    }
    let places = super::global::heap_places(cpu);
    let count = |flags| places.iter().filter(|p| on_list(p, flags)).count() as u16;
    let counts = [count(GLOBAL_ALL), count(GLOBAL_FREE), count(GLOBAL_LRU)];
    for (i, v) in counts.into_iter().enumerate() {
        super::write_16(cpu, at + 4 + 2 * i as u32, v);
    }
    Some(1)
}

/// Fill the GLOBALENTRY at `at` with the block whose handle or selector
/// is `handle`: whether there is one.
fn entry_of_block(cpu: &mut Cpu, at: u32, handle: u16) -> u32 {
    if !global_entry_at(cpu, at) {
        return 0;
    }
    let Some(item) = super::global::heap_item(cpu, handle) else {
        return 0;
    };
    let places = if item.address == 0 {
        Vec::new()
    } else {
        super::global::heap_places(cpu)
    };
    fill_global_entry(cpu, at, &places, &item);
    1
}

/// GlobalEntryHandle(lpGlobal, hItem): the block whose handle or
/// selector is `hItem`.
fn global_entry_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(entry_of_block(cpu, args[0], args[1] as u16))
}

/// GlobalEntryModule(lpGlobal, hModule, wSeg): the block of segment
/// `wSeg` (from 1) of the module `hModule` (or its instance).
fn global_entry_module(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (at, module, seg) = (args[0], args[1] as u16, args[2] as u16);
    let sel = super::module_index(cpu, module).and_then(|i| {
        let m = &cpu.bus.win32.win16.modules[i];
        let n = u8::try_from(seg).ok()?;
        super::module::segment_selector(cpu, m, n)
    });
    Some(sel.map_or(0, |sel| entry_of_block(cpu, at, sel)))
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

/// Whether the structure at `at` has `size` in its dwSize, as TOOLHELP
/// checks before it fills one.
fn sized(cpu: &Cpu, at: u32, size: u32) -> bool {
    at != 0 && super::read_32(cpu, at) == size
}

/// Write `name` into the `len` bytes at `at`, cut short and ended with
/// a NUL, the rest zeroed.
fn put_name(cpu: &mut Cpu, at: u32, len: usize, name: &[u8]) {
    let mut bytes = name[..name.len().min(len - 1)].to_vec();
    bytes.resize(len, 0);
    super::poke(cpu, at, &bytes);
}

/// TASKENTRY's size: dwSize, hTask, hTaskParent, hInst, hModule, wSS,
/// wSP, wStackTop, wStackMinimum, wStackBottom, wcEvents, hQueue,
/// szModule[10], wPSPOffset, hNext.
const TASKENTRY_SIZE: u32 = 0x28;

/// The tasks TaskFirst and TaskNext walk: those that haven't ended, in
/// the order they started, the first the one GetCurrentTask gives in DX.
fn live_tasks(cpu: &Cpu) -> Vec<usize> {
    let tasks = &cpu.bus.win32.win16.tasks;
    (0..tasks.len())
        .filter(|&t| tasks[t].exit_code.is_none())
        .collect()
}

/// Fill the TASKENTRY at `at` with the task whose handle is `handle`:
/// whether there is one. hNext is the next task's handle, 0 after the
/// last.
fn fill_task_entry(cpu: &mut Cpu, at: u32, handle: u16) -> u32 {
    if handle == 0 || !sized(cpu, at, TASKENTRY_SIZE) {
        return 0;
    }
    let live = live_tasks(cpu);
    let Some(place) = live
        .iter()
        .position(|&t| cpu.bus.win32.win16.tasks[t].handle == handle)
    else {
        return 0;
    };
    let t = live[place];
    let task = cpu.bus.win32.win16.tasks[t].clone();
    let next = live
        .get(place + 1)
        .map_or(0, |&n| cpu.bus.win32.win16.tasks[n].handle);
    let (ss, sp) = thunk::task_stack16(cpu, t);
    // INSTANCEDATA at the start of the automatic data segment: the
    // stack's top, its lowest SP so far and its bottom, as InitTask set
    // them.
    let data = super::lin(cpu, super::global::handle_to_sel(task.instance), 0);
    let (top, min, bottom) = if task.instance == 0 {
        (0, 0, 0)
    } else {
        (
            super::read_16(cpu, data + 0x0A),
            super::read_16(cpu, data + 0x0C),
            super::read_16(cpu, data + 0x0E),
        )
    };
    let name = super::module_index(cpu, task.module)
        .map(|i| cpu.bus.win32.win16.modules[i].name.clone())
        .unwrap_or_default();
    // hQueue and wcEvents: the message queue is USER32's thread queue,
    // which has no 16-bit handle, so both are 0.
    let words = [
        task.handle,
        task.parent,
        task.instance,
        task.module,
        ss,
        sp,
        top,
        min,
        bottom,
        0,
        0,
    ];
    for (i, w) in words.into_iter().enumerate() {
        super::write_16(cpu, at + 4 + 2 * i as u32, w);
    }
    put_name(cpu, at + 0x1A, 10, name.as_bytes());
    // The PDB is at 100h in the task database.
    super::write_16(cpu, at + 0x24, 0x100);
    super::write_16(cpu, at + 0x26, next);
    1
}

/// TaskFirst(lpte): the first task.
fn task_first(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let first = live_tasks(cpu)
        .first()
        .map_or(0, |&t| cpu.bus.win32.win16.tasks[t].handle);
    Some(fill_task_entry(cpu, args[0], first))
}

/// TaskNext(lpte): the task the entry's hNext names.
fn task_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if !sized(cpu, at, TASKENTRY_SIZE) {
        return Some(0);
    }
    let next = super::read_16(cpu, at + 0x26);
    Some(fill_task_entry(cpu, at, next))
}

/// TaskFindHandle(lpte, hTask): the task `hTask`.
fn task_find_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(fill_task_entry(cpu, args[0], args[1] as u16))
}

/// MODULEENTRY's size: dwSize, szModule[10], hModule, wcUsage,
/// szExePath[256], wNext.
const MODULEENTRY_SIZE: u32 = 0x114;

/// Fill the MODULEENTRY at `at` with the module `modules[index]`. wNext
/// is the next module's handle, 0 after the last; the system's modules
/// come first, KERNEL the first of them, as in Windows' module list.
fn fill_module_entry(cpu: &mut Cpu, at: u32, index: usize) -> u32 {
    let modules = &cpu.bus.win32.win16.modules;
    let m = &modules[index];
    let (name, handle, refs) = (m.name.clone(), m.handle, m.refs);
    let path = crate::dosstr::to_bytes(&m.path);
    let next = modules.get(index + 1).map_or(0, |n| n.handle);
    put_name(cpu, at + 4, 10, name.as_bytes());
    super::write_16(cpu, at + 0x0E, handle);
    super::write_16(cpu, at + 0x10, refs);
    put_name(cpu, at + 0x12, 256, &path);
    super::write_16(cpu, at + 0x112, next);
    1
}

/// Fill the MODULEENTRY at `at` with the module whose handle is exactly
/// `handle`, after checking dwSize.
fn module_entry_of(cpu: &mut Cpu, at: u32, handle: u16) -> u32 {
    if handle == 0 || !sized(cpu, at, MODULEENTRY_SIZE) {
        return 0;
    }
    let modules = &cpu.bus.win32.win16.modules;
    match modules.iter().position(|m| m.handle == handle) {
        Some(i) => fill_module_entry(cpu, at, i),
        None => 0,
    }
}

/// ModuleFirst(lpme): the first module, KERNEL.
fn module_first(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let first = cpu.bus.win32.win16.modules.first().map_or(0, |m| m.handle);
    Some(module_entry_of(cpu, args[0], first))
}

/// ModuleNext(lpme): the module the entry's wNext names.
fn module_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if !sized(cpu, at, MODULEENTRY_SIZE) {
        return Some(0);
    }
    let next = super::read_16(cpu, at + 0x112);
    Some(module_entry_of(cpu, at, next))
}

/// ModuleFindName(lpme, lpszName): the module named `lpszName`, in any
/// case.
fn module_find_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] == 0 {
        return Some(0);
    }
    let name = super::string(cpu, args[1]).to_ascii_uppercase();
    let handle = cpu
        .bus
        .win32
        .win16
        .modules
        .iter()
        .find(|m| m.name == name)
        .map_or(0, |m| m.handle);
    Some(module_entry_of(cpu, args[0], handle))
}

/// ModuleFindHandle(lpme, hModule): the module `hModule`, or the one
/// whose instance or segment it is, as GetExePtr takes them.
fn module_find_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = super::module_index(cpu, args[1] as u16)
        .map_or(0, |i| cpu.bus.win32.win16.modules[i].handle);
    Some(module_entry_of(cpu, args[0], handle))
}

/// LOCALINFO's and LOCALENTRY's sizes. LOCALENTRY: dwSize, hHandle,
/// wAddress, wSize, wFlags, wcLock, wType, hHeap, wHeapType, wNext.
const LOCALINFO_SIZE: u32 = 6;
const LOCALENTRY_SIZE: u32 = 0x16;
/// LOCALENTRY's wFlags: a fixed, free or moveable block. wType is
/// LT_NORMAL for a block and LT_FREE for a free piece, and wHeapType
/// NORMAL_HEAP: USER's and GDI's heaps are USER32's and GDI32's.
const LF_FIXED: u16 = 1;
const LF_FREE: u16 = 2;
const LF_MOVEABLE: u16 = 4;
const LT_NORMAL: u16 = 0;
const LT_FREE: u16 = 0xFF;

/// The items of the local heap in the segment whose handle or selector
/// is `heap`.
fn local_items(cpu: &Cpu, heap: u16) -> Option<Vec<super::local::WalkItem>> {
    super::local::walk(cpu, super::global::handle_to_sel(heap))
}

/// LocalInfo(lpli, hHeap): the number of items in the heap.
fn local_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if !sized(cpu, at, LOCALINFO_SIZE) {
        return Some(0);
    }
    let Some(items) = local_items(cpu, args[1] as u16) else {
        return Some(0);
    };
    super::write_16(cpu, at + 4, items.len() as u16);
    Some(1)
}

/// Fill the LOCALENTRY at `at` with the item of the heap `heap` whose
/// arena starts at `arena`, or the first one for None. wNext is where
/// the next item's arena starts, 0 after the last.
fn fill_local_entry(cpu: &mut Cpu, at: u32, heap: u16, arena: Option<u16>) -> u32 {
    let Some(items) = local_items(cpu, heap) else {
        return 0;
    };
    let place = match arena {
        None => 0,
        Some(a) => match items.iter().position(|i| i.arena == a) {
            Some(p) => p,
            None => return 0,
        },
    };
    let Some(item) = items.get(place) else {
        return 0;
    };
    let next = items.get(place + 1).map_or(0, |n| n.arena);
    let (flags, kind) = if item.free {
        (LF_FREE, LT_FREE)
    } else if item.moveable {
        (LF_MOVEABLE, LT_NORMAL)
    } else {
        (LF_FIXED, LT_NORMAL)
    };
    let words = [
        item.handle,
        item.address,
        item.size,
        flags,
        item.lock as u16,
        kind,
        heap,
        0,
        next,
    ];
    for (i, w) in words.into_iter().enumerate() {
        super::write_16(cpu, at + 4 + 2 * i as u32, w);
    }
    1
}

/// LocalFirst(lple, hHeap): the heap's first item.
fn local_first(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if !sized(cpu, at, LOCALENTRY_SIZE) {
        return Some(0);
    }
    Some(fill_local_entry(cpu, at, args[1] as u16, None))
}

/// LocalNext(lple): the item the entry's wNext names, in the heap its
/// hHeap names.
fn local_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if !sized(cpu, at, LOCALENTRY_SIZE) {
        return Some(0);
    }
    let heap = super::read_16(cpu, at + 0x10);
    let next = super::read_16(cpu, at + 0x14);
    if next == 0 {
        return Some(0);
    }
    Some(fill_local_entry(cpu, at, heap, Some(next)))
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

/// MMSYSERR_INVALHANDLE: midiOutGetID, whose handle can't be one, since
/// midiOutOpen opens nothing (#1243).
fn invalid_handle(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(5)
}

/// waveOutGetID(handle, id): WINMM's answer, written as the 16-bit UINT
/// Windows 3.1 has. A null `id` goes to WINMM as null, which refuses it
/// after it has checked the handle, as Windows does.
fn wave_out_get_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = thunk::scratch32(cpu, 4);
    let out = if args[1] == 0 { 0 } else { id };
    go_at(
        cpu,
        id,
        MM,
        "waveOutGetID",
        &[args[0], out],
        vec![P_UINT, id, args[1]],
    )
}

/// The 32-bit device id of a 16-bit one: WAVE_MAPPER and AUX_MAPPER,
/// FFFFh, are -1 in Win32. WINMM has no aux mapper, and answers -1 as a
/// bad id.
fn device32(id: u32) -> u32 {
    if id == 0xFFFF { u32::MAX } else { id }
}

/// The head the 16-bit caps structures share, from a Win32 one at
/// `caps`: wMid, wPid, the driver's version as a WORD (a DWORD in Win32)
/// and the 32 bytes of szPname. Win32's structure goes on at `caps` + 40,
/// the 16-bit one at 38.
fn caps16_head(cpu: &Cpu, caps: u32) -> Vec<u8> {
    let mut b = Vec::with_capacity(52);
    b.extend_from_slice(&(r16(cpu, caps) as u16).to_le_bytes());
    b.extend_from_slice(&(r16(cpu, caps + 2) as u16).to_le_bytes());
    b.extend_from_slice(&(r32(cpu, caps + 4) as u16).to_le_bytes());
    b.extend(super::peek(cpu, caps + 8, 32).unwrap_or(vec![0; 32]));
    b
}

/// AUXCAPSA at `caps` as AUXCAPS16 at `caps16`, `size` bytes of it at
/// most: the driver's version is a WORD, and there is no wReserved1.
fn aux_caps16(cpu: &mut Cpu, caps: u32, caps16: u32, size: u32) {
    let mut b = caps16_head(cpu, caps);
    b.extend_from_slice(&(r16(cpu, caps + 40) as u16).to_le_bytes());
    b.extend_from_slice(&r32(cpu, caps + 44).to_le_bytes());
    b.truncate(size as usize);
    super::poke(cpu, caps16, &b);
}

fn aux_get_dev_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let caps = thunk::scratch32(cpu, 48);
    let out = if args[1] == 0 { 0 } else { caps };
    go_at(
        cpu,
        caps,
        MM,
        "auxGetDevCapsA",
        &[device32(args[0]), out, 48],
        vec![P_AUX_CAPS, caps, args[1], args[2]],
    )
}

fn aux_get_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let volume = thunk::scratch32(cpu, 4);
    let out = if args[1] == 0 { 0 } else { volume };
    go_at(
        cpu,
        volume,
        MM,
        "auxGetVolume",
        &[device32(args[0]), out],
        vec![P_DWORD, volume, args[1]],
    )
}

fn aux_set_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    go(
        cpu,
        MM,
        "auxSetVolume",
        &[device32(args[0]), args[1]],
        Vec::new(),
    )
}

/// WAVEOUTCAPSA at `caps` as WAVEOUTCAPS16 at `caps16`, `size` bytes of
/// it at most.
fn wave_caps16(cpu: &mut Cpu, caps: u32, caps16: u32, size: u32) {
    let mut b = caps16_head(cpu, caps);
    b.extend_from_slice(&r32(cpu, caps + 40).to_le_bytes());
    b.extend_from_slice(&(r16(cpu, caps + 44) as u16).to_le_bytes());
    b.extend_from_slice(&r32(cpu, caps + 48).to_le_bytes());
    b.truncate(size as usize);
    super::poke(cpu, caps16, &b);
}

/// waveOutGetDevCaps(device, caps, size): a null `caps` goes to WINMM as
/// null, which refuses it once it has checked the device.
fn wave_out_get_dev_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let caps = thunk::scratch32(cpu, 52);
    let out = if args[1] == 0 { 0 } else { caps };
    go_at(
        cpu,
        caps,
        MM,
        "waveOutGetDevCapsA",
        &[device32(args[0]), out, 52],
        vec![P_WAVE_CAPS, caps, args[1], args[2]],
    )
}

/// The callback kinds of waveOutOpen's flags.
const CALLBACK_TYPEMASK: u32 = 0x0007_0000;
const CALLBACK_WINDOW: u32 = 0x0001_0000;
const CALLBACK_TASK: u32 = 0x0002_0000;
const CALLBACK_FUNCTION: u32 = 0x0003_0000;

/// waveOutOpen(handle *, device, format, callback, instance, flags): a
/// window or a task is told about the buffers played, as in Win32, and a
/// 16-bit function is called in its task (`queue_call16`).
fn wave_out_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = thunk::scratch32(cpu, 4);
    w32(cpu, out, 0);
    let device = device32(args[1]);
    let mut flags = args[5];
    // WINMM opens the device without a callback, and is given the
    // function once it is open (P_WAVE_OPEN).
    let function = match flags & CALLBACK_TYPEMASK {
        CALLBACK_FUNCTION if args[0] != 0 => args[3],
        _ => 0,
    };
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
        vec![P_WAVE_OPEN, out, args[0], function],
    )
}

/// What `Mmsystem` keeps of a call: the task's thread, the function and
/// its five arguments.
pub type MmCall = [u32; 7];

/// Queue the call of the 16-bit function `function` with `args` (the
/// device's handle, the message, the instance and two parameters) for
/// the task whose thread is `tid`. Windows 3.1 calls such a function at
/// interrupt time, which tasks taking turns have no room for: the task
/// makes the call when it next asks for a message (`call_queued16`), or
/// before the MMSYSTEM function it is in returns (`return_after_calls`).
/// A task that waits is given WM_NULL, so that it asks; one that has
/// ended makes no calls.
pub fn queue_call16(cpu: &mut Cpu, tid: u32, function: u32, args: [u32; 5]) {
    let Some((p, t)) = crate::win32::thread::find(&cpu.bus.win32, tid) else {
        return;
    };
    let waits = cpu.bus.win32.processes[p].threads[t].wait.is_some();
    let [a, b, c, d, e] = args;
    let calls = &mut cpu.bus.win32.win16.mmsystem.calls;
    calls.push([tid, function, a, b, c, d, e]);
    // Each call, not only the first: a task that went on to wait in
    // WaitMessage with a call queued would otherwise never be woken.
    if waits {
        crate::win32::user32::message::post_thread_message(cpu, &[tid, 0, 0, 0]);
    }
}

/// Make the first call queued for the running task (`queue_call16`);
/// the ones after it follow when it returns (`then_calls16`), and then
/// the task goes on with `resume`. False when there is none.
fn make_call16(cpu: &mut Cpu, resume: Context) -> bool {
    if cpu.bus.win32.win16.mmsystem.calls.is_empty() {
        return false;
    }
    let Some(t) = super::current_task(cpu) else {
        return false;
    };
    let tid = cpu.bus.win32.win16.tasks[t].tid;
    let calls = &mut cpu.bus.win32.win16.mmsystem.calls;
    let Some(i) = calls.iter().position(|c| c[0] == tid) else {
        return false;
    };
    let [_, function, handle, msg, instance, p1, p2] = calls.remove(i);
    // void CALLBACK f(HANDLE, UINT, DWORD, DWORD, DWORD), Pascal's.
    let words = [
        handle as u16,
        msg as u16,
        (instance >> 16) as u16,
        instance as u16,
        (p1 >> 16) as u16,
        p1 as u16,
        (p2 >> 16) as u16,
        p2 as u16,
    ];
    thunk::call16(
        cpu,
        function,
        &words,
        0,
        resume,
        thunk::THEN_CALLS16,
        Vec::new(),
    );
    true
}

/// A function `make_call16` called returned: the task's next call, or
/// the context it was to go on with.
pub(super) fn then_calls16(cpu: &mut Cpu, _: u32, _: &[u32], ctx: &mut Context) -> Option<u32> {
    if make_call16(cpu, *ctx) {
        None
    } else {
        Some(ctx.gpr[thunk::EAX])
    }
}

/// The running task, as it asks for a message, makes the calls queued
/// for it (`queue_call16`), and asks again when they are made. False
/// when there are none.
pub fn call_queued16(cpu: &mut Cpu) -> bool {
    let mut resume = Context::of(cpu);
    resume.eip = resume.eip.wrapping_sub(4);
    make_call16(cpu, resume)
}

/// An MMSYSTEM function that returns `value` to the 16-bit context
/// `ctx` makes the calls its WINMM function queued for the task before
/// it returns, as Windows 3.1 calls the function within waveOutOpen
/// (MM_WOM_OPEN), waveOutReset (MM_WOM_DONE) and waveOutClose
/// (MM_WOM_CLOSE). A program may free the headers and the function's
/// library once they return.
fn return_after_calls(cpu: &mut Cpu, value: u32, ctx: &Context) -> Option<u32> {
    let mut resume = *ctx;
    resume.gpr[thunk::EAX] = resume.gpr[thunk::EAX] & 0xFFFF_0000 | value & 0xFFFF;
    resume.gpr[thunk::EDX] = resume.gpr[thunk::EDX] & 0xFFFF_0000 | value >> 16;
    if make_call16(cpu, resume) {
        None
    } else {
        Some(value)
    }
}

/// A task ended: the calls queued for it go.
pub fn task_ended(cpu: &mut Cpu, tid: u32) {
    cpu.bus.win32.win16.mmsystem.calls.retain(|c| c[0] != tid);
}

/// What MMSYSTEM keeps for the 16-bit programs: the wave headers WINMM
/// has (`wave_header`), each one's process, linear address and 16:16
/// address, and the calls of callback functions that wait for their
/// tasks (`queue_call16`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mmsystem {
    pub headers: Vec<[u32; 3]>,
    pub calls: Vec<MmCall>,
}

/// The W32 section's version that first keeps `Mmsystem`. An older
/// rust-dos kept the wave headers alone, and put the data's flat address
/// in the program's own header while WINMM had it. Their headers are
/// forgotten on loading: WINMM then tells the program about them by
/// their linear addresses.
const MMSYSTEM_SAVED: u16 = 74;

impl crate::savestate::State for Mmsystem {
    fn save(&self, w: &mut crate::savestate::Writer) {
        crate::savestate::State::save(&self.headers, w);
        crate::savestate::State::save(&self.calls, w);
    }

    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        // Bytes outside a section are in this rust-dos's own layout.
        if r.version().is_some_and(|v| v < MMSYSTEM_SAVED) {
            let mut old: Vec<[u32; 4]> = Vec::new();
            crate::savestate::State::load(&mut old, r)?;
            *self = Mmsystem::default();
            return Ok(());
        }
        crate::savestate::State::load(&mut self.headers, r)?;
        crate::savestate::State::load(&mut self.calls, r)
    }
}

/// The WAVEHDR at 16:16 `segptr`, given to WINMM where it is, as
/// Windows 98's MMSYSTEM gives a 16-bit program's own header to the
/// driver: its linear address. The two layouts are the same, and the
/// header keeps what the program wrote, its data's 16:16 pointer too;
/// WINMM changes only dwFlags. The header's process and addresses are
/// kept, so that WINMM reads the data's pointer as a 16:16 one
/// (`wave_data`) and gives the program its own pointer to the header in
/// MM_WOM_DONE (`wave_header16`).
fn wave_header(cpu: &mut Cpu, segptr: u32) -> u32 {
    let at = super::linear(cpu, segptr);
    if at == 0 {
        return 0;
    }
    let pid = crate::win32::user32::current_pid(cpu);
    let headers = &mut cpu.bus.win32.win16.mmsystem.headers;
    match headers.iter_mut().find(|h| h[0] == pid && h[1] == at) {
        Some(h) => h[2] = segptr,
        None => headers.push([pid, at, segptr]),
    }
    at
}

/// WINMM let go of the header at `at`.
fn unprepared(cpu: &mut Cpu, at: u32) {
    let pid = crate::win32::user32::current_pid(cpu);
    let headers = &mut cpu.bus.win32.win16.mmsystem.headers;
    headers.retain(|h| h[0] != pid || h[1] != at);
}

/// The 16:16 address of the 16-bit program's wave header at the linear
/// `at` in the process `pid`, if it is one.
fn wave_header_of(cpu: &Cpu, pid: u32, at: u32) -> Option<u32> {
    let headers = &cpu.bus.win32.win16.mmsystem.headers;
    headers
        .iter()
        .find(|h| h[0] == pid && h[1] == at)
        .map(|h| h[2])
}

/// The 16:16 address of the wave header at the linear `at` in the
/// process `pid`, which WINMM puts in MM_WOM_DONE for a 16-bit program;
/// `at` for a header that isn't a 16-bit program's.
pub fn wave_header16(cpu: &Cpu, pid: u32, at: u32) -> u32 {
    wave_header_of(cpu, pid, at).unwrap_or(at)
}

/// The linear address of the data `data` that the wave header at the
/// linear `at` in the process `pid` points to: a 16-bit program's header
/// has a 16:16 pointer there, the others a flat one.
pub fn wave_data(cpu: &Cpu, pid: u32, at: u32, data: u32) -> u32 {
    match wave_header_of(cpu, pid, at) {
        Some(_) => super::linear(cpu, data),
        None => data,
    }
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

fn wave_out_reset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    go(cpu, MM, "waveOutReset", &[args[0]], vec![P_WAVE_CALLS])
}

fn wave_out_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    go(cpu, MM, "waveOutClose", &[args[0]], vec![P_WAVE_CALLS])
}

fn wave_out_get_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] == 0 || args[2] < MMTIME16_SIZE {
        return Some(MMSYSERR_INVALPARAM);
    }
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

/// timeGetSystemTime: with a short MMTIME or none, nothing is written
/// and the answer is still 0, as Windows has it.
fn time_get_system_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 || args[1] < MMTIME16_SIZE {
        return Some(0);
    }
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

/// The MMTIME at `time` as Windows 3.1's at `time16`: wType, and the
/// union as far as its kind fills it (6 bytes of SMPTE time, else a
/// DWORD), as MMSYSTEM leaves the rest of the caller's MMTIME as it was.
/// The caller's size was checked to hold it.
fn mmtime16(cpu: &mut Cpu, time: u32, time16: u32) {
    let kind = r32(cpu, time);
    let len = if kind == TIME_SMPTE { 6 } else { 4 };
    let mut bytes = (kind as u16).to_le_bytes().to_vec();
    bytes.extend(super::peek(cpu, time + 4, len).unwrap_or(vec![0; len]));
    super::poke(cpu, time16, &bytes);
}

fn time_get_dev_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    // TIMECAPS16: the shortest and the longest period, 4 bytes.
    if args[0] == 0 || args[1] < 4 {
        return Some(TIMERR_NOCANDO);
    }
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
const MMIO_PARSE: u32 = 0x0100;
const MMIO_EXIST: u32 = 0x4000;
const MMIO_GETTEMP: u32 = 0x0002_0000;
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
    // MMIO_PARSE, MMIO_EXIST and MMIO_GETTEMP open no file: TRUE or FALSE,
    // with no buffer to give.
    if flags & (MMIO_PARSE | MMIO_EXIST | MMIO_GETTEMP) != 0 {
        return wmmio::mmio_open(cpu, &[name, 0, flags]);
    }
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

// MCI: WINMM's mciSendCommandA, called with the program's parameter
// block made a 32-bit one, as Windows 95's MMSYSTEM thunks to WINMM.
// The blocks of Windows 3.1 have the 32-bit ones' layout but for their
// UINT and HWND fields, which are words (most of them followed by a
// reserved word, so the offsets stay), their RECTs, which have words,
// and their strings, which are 16:16 pointers. A callback window's
// handle is its 32-bit one, as for the wave functions.

use crate::win32::winmm::mci::{self as wmci, video};

/// The longest block WINMM reads: MCI_WAVE_SET_PARMS, 44 bytes.
const MCI_BLOCK: u32 = 48;

/// mciSendCommand(id, message, flags, parms).
fn mci_send_command(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, msg, flags, segptr) = (args[0], args[1], args[2], args[3]);
    let at16 = super::linear(cpu, segptr);
    let block = thunk::scratch32(cpu, MCI_BLOCK);
    let target = if at16 == 0 {
        0
    } else {
        mci_block32(cpu, segptr, at16, block, msg, flags);
        block
    };
    // A status's block is copied as it is, into the 32-bit stack's
    // scratch space: WINMM's function says for itself whether an idle
    // proof goes through the command (`mci::send_command`), and `post`
    // whether it goes through what comes back.
    if msg == wmci::MCI_STATUS && at16 != 0 {
        let room = super::segment_room(cpu, segptr).min(MCI_BLOCK);
        crate::idle::pure_call(cpu, None);
        crate::idle::reads_arguments(cpu);
        crate::idle::reads_also(cpu, at16, room);
    }
    // The program's block goes to `post` as its 16:16 pointer: the
    // command can call into USER32, whose messages run the program's
    // window procedures, and a segment they grow can move.
    go_at(
        cpu,
        block,
        MM,
        "mciSendCommandA",
        &[id, msg, flags, target],
        vec![P_MCI, block, segptr, msg],
    )
}

/// The 32-bit block at `block` made from the program's at `segptr`
/// (`at16` linear) for the command `msg`.
fn mci_block32(cpu: &mut Cpu, segptr: u32, at16: u32, block: u32, msg: u32, flags: u32) {
    // The bytes up to the end of the program's segment, at most the
    // block's.
    let room = super::segment_room(cpu, segptr).min(MCI_BLOCK);
    let mut bytes = super::peek_upto(cpu, at16, room as usize);
    bytes.resize(MCI_BLOCK as usize, 0);
    super::poke(cpu, block, &bytes);
    let word = |at: u32| u16::from_le_bytes([bytes[at as usize], bytes[at as usize + 1]]) as u32;
    let signed = |at: u32| word(at) as i16 as u32;
    // The field at `at`, a word, made a DWORD at the same offset.
    let widen = |cpu: &mut Cpu, at: u32| w32(cpu, block + at, word(at));
    // The 16:16 pointer at `at` made flat.
    let pointer = |cpu: &mut Cpu, at: u32| {
        let p = super::linear(cpu, r32(cpu, block + at));
        w32(cpu, block + at, p);
    };
    // The RECT of words at `at` made one of longs.
    let rect = |cpu: &mut Cpu, at: u32| {
        let longs = [0, 2, 4, 6].map(|k| signed(at + k));
        for (k, v) in longs.into_iter().enumerate() {
            w32(cpu, block + at + 4 * k as u32, v);
        }
    };
    // dwCallback's window is its low word.
    widen(cpu, 0);
    match msg {
        wmci::MCI_OPEN => {
            // wDeviceID comes back; the strings are 16:16 unless they
            // are numbers (the _ID flags). MCI_ANIM_OPEN_PARMS's and
            // MCI_DGV_OPEN_PARMS's hWndParent is a word.
            if flags & wmci::MCI_OPEN_TYPE != 0 && flags & wmci::MCI_OPEN_TYPE_ID == 0 {
                pointer(cpu, 8);
            }
            if flags & wmci::MCI_OPEN_ELEMENT != 0 && flags & wmci::MCI_OPEN_ELEMENT_ID == 0 {
                pointer(cpu, 12);
            }
            if flags & wmci::MCI_OPEN_ALIAS != 0 {
                pointer(cpu, 16);
            }
            widen(cpu, 24);
        }
        wmci::MCI_INFO | wmci::MCI_SYSINFO => {
            // lpstrReturn; MCI_SYSINFO_PARMS's wDeviceType is a word.
            pointer(cpu, 4);
            if msg == wmci::MCI_SYSINFO {
                widen(cpu, 16);
            }
        }
        wmci::MCI_LOAD | wmci::MCI_SAVE => {
            // The file, and the RECT of words that MCI_OVLY_LOAD_PARMS,
            // MCI_OVLY_SAVE_PARMS and MCI_DGV_SAVE_PARMS have after it
            // (MCI_OVLY_RECT and MCI_DGV_RECT).
            pointer(cpu, 4);
            rect(cpu, 8);
        }
        video::MCI_WINDOW => {
            // hWnd and nCmdShow are words, lpstrText a 16:16 pointer.
            widen(cpu, 4);
            w32(cpu, block + 8, signed(8));
            if flags & video::MCI_DGV_WINDOW_TEXT != 0 {
                pointer(cpu, 12);
            }
        }
        video::MCI_PUT | video::MCI_WHERE | video::MCI_UPDATE => {
            // A RECT of words, and MCI_UPDATE's hDC after it.
            rect(cpu, 4);
            let hdc = if msg == video::MCI_UPDATE {
                word(12)
            } else {
                0
            };
            w32(cpu, block + 20, hdc);
        }
        wmci::MCI_BREAK => {
            // nVirtKey and hwndBreak.
            w32(cpu, block + 4, signed(4));
            widen(cpu, 8);
        }
        _ => {}
    }
}

/// mciSendCommandA returned 0: what the command gives back, from the
/// 32-bit block to the program's at `segptr`.
fn mci_returned(cpu: &mut Cpu, block: u32, segptr: u32, msg: u32) {
    let at16 = super::linear(cpu, segptr);
    if at16 == 0 {
        return;
    }
    match msg {
        wmci::MCI_OPEN => {
            let id = r32(cpu, block + 4);
            super::write_16(cpu, at16 + 4, id as u16);
        }
        wmci::MCI_STATUS | wmci::MCI_GETDEVCAPS => {
            let value = r32(cpu, block + 4);
            super::write_32(cpu, at16 + 4, value);
        }
        video::MCI_WHERE => {
            for k in 0..4 {
                let v = r32(cpu, block + 4 + 4 * k);
                super::write_16(cpu, at16 + 4 + 2 * k, v as u16);
            }
        }
        _ => {}
    }
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
