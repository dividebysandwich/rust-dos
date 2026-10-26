//! The version of Windows the built-in Windows is: Windows 95 (the
//! default), Windows 98 or Windows 98 Second Edition, by the
//! `win32_version` setting.
//!
//! The setting is read when the first program starts (`start`), and the
//! processes see that version until the last of them ends, so a change
//! of the setting doesn't reach a program halfway. It is kept in saved
//! states with the rest of the built-in Windows.
//!
//! The version shows in GetVersion and GetVersionEx, in MSVCRT's
//! `_osver` and `_winver`, and in the registry's version values. Windows
//! 98 also has functions Windows 95 doesn't: `ONLY_98` lists those of
//! the built-in DLLs, which their export tables have only under 98, so
//! GetProcAddress doesn't find them under 95 and a program that imports
//! one doesn't load, as on Windows 95.

use crate::cpu::Cpu;

pub use crate::config::WindowsVersion;

crate::state_enum!(WindowsVersion {
    WindowsVersion::Win95,
    WindowsVersion::Win98,
    WindowsVersion::Win98Se,
});

/// The version the running programs see.
pub fn current(cpu: &Cpu) -> WindowsVersion {
    cpu.bus.win32.os
}

/// Whether the running programs see Windows 98 (either edition).
pub fn is_98(cpu: &Cpu) -> bool {
    current(cpu).is_98()
}

crate::state_enum!(crate::config::DesktopDepth {
    crate::config::DesktopDepth::Adapter,
    crate::config::DesktopDepth::Palette,
});

/// The first program starts: the settings become the version the
/// processes see and the depth of their desktop.
pub fn start(win32: &mut super::Win32) {
    if win32.processes.is_empty() {
        win32.os = win32.version;
        win32.desktop_depth = win32.depth;
    }
}

/// Whether the desktop has 256 colours from a palette (`win32_depth=8`).
pub fn palette_desktop(cpu: &Cpu) -> bool {
    cpu.bus.win32.desktop_depth == crate::config::DesktopDepth::Palette
}

/// What a version of Windows reports about itself.
pub struct Info {
    /// GetVersion's value.
    pub get_version: u32,
    pub major: u32,
    pub minor: u32,
    /// GetVersionEx's dwBuildNumber: the version in the high word and
    /// the build in the low.
    pub build: u32,
    /// GetVersionEx's szCSDVersion.
    pub csd: &'static str,
    /// The registry's values in
    /// HKLM\Software\Microsoft\Windows\CurrentVersion.
    pub version_number: &'static str,
    pub version_name: &'static str,
    pub product_name: &'static str,
    pub sub_version: &'static str,
    /// HKLM\Software\Microsoft\DirectX's Version and InstalledVersion;
    /// Windows 98 has no InstalledVersion.
    pub directx: &'static str,
    pub directx_installed: Option<[u8; 8]>,
    /// MSVCRT's _osver: GetVersion's high word, which Windows 98's
    /// MSVCRT keeps with its top bit.
    pub osver: u32,
}

static WIN95: Info = Info {
    get_version: 0xC000_0004,
    major: 4,
    minor: 0,
    build: 0x0400_03B6,
    csd: "",
    version_number: "4.00.950",
    version_name: "Windows 95",
    product_name: "Microsoft Windows 95",
    sub_version: "",
    directx: "4.06.00.0318",
    directx_installed: Some([0, 0, 0, 6, 0, 0, 0, 0]),
    osver: 0x4000,
};

static WIN98: Info = Info {
    get_version: 0xC000_0A04,
    major: 4,
    minor: 10,
    build: 0x040A_07CE,
    csd: " ",
    version_number: "4.10.1998",
    version_name: "Windows 98",
    product_name: "Microsoft Windows 98",
    sub_version: " ",
    directx: "4.06.02.0436",
    directx_installed: None,
    osver: 0xC000,
};

static WIN98SE: Info = Info {
    get_version: 0xC000_0A04,
    major: 4,
    minor: 10,
    build: 0x040A_08AE,
    csd: " A ",
    version_number: "4.10.2222",
    version_name: "Windows 98",
    product_name: "Microsoft Windows 98",
    sub_version: " A ",
    directx: "4.06.03.0518",
    directx_installed: None,
    osver: 0xC000,
};

pub fn info(version: WindowsVersion) -> &'static Info {
    match version {
        WindowsVersion::Win95 => &WIN95,
        WindowsVersion::Win98 => &WIN98,
        WindowsVersion::Win98Se => &WIN98SE,
    }
}

/// The version values of the registry, as a .reg file to put over the
/// seed and the registry file: they are the system's, so a registry
/// saved under another version doesn't keep its own.
pub fn registry_values(version: WindowsVersion) -> String {
    let i = info(version);
    let installed = match i.directx_installed {
        Some(bytes) => {
            let hex: Vec<String> = bytes.iter().map(|b| format!("{:02x}", b)).collect();
            format!("hex:{}", hex.join(","))
        }
        None => "-".to_string(),
    };
    format!(
        "REGEDIT4\r\n\r\n\
         [HKEY_LOCAL_MACHINE\\Software\\Microsoft\\DirectX]\r\n\
         \"Version\"=\"{}\"\r\n\
         \"InstalledVersion\"={}\r\n\r\n\
         [HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows\\CurrentVersion]\r\n\
         \"VersionNumber\"=\"{}\"\r\n\
         \"Version\"=\"{}\"\r\n\
         \"ProductName\"=\"{}\"\r\n\
         \"SubVersionNumber\"=\"{}\"\r\n",
        i.directx, installed, i.version_number, i.version_name, i.product_name, i.sub_version
    )
}

/// The functions of the built-in DLLs that Windows 98 has and Windows
/// 95 doesn't, by DLL. Microsoft's documentation of each names Windows
/// 98 as the first of the 9x line to have it.
pub static ONLY_98: &[(&str, &[&str])] = &[
    (
        "USER32.DLL",
        &[
            "GetAncestor",
            "SendInput",
            "TrackMouseEvent",
            "EnumDisplayDevicesA",
            "GetMenuInfo",
            "SetMenuInfo",
            "GetAltTabInfo",
            "GetComboBoxInfo",
            "GetTitleBarInfo",
            "GetWindowInfo",
            "GetGUIThreadInfo",
            "MonitorFromWindow",
            "MonitorFromPoint",
            "MonitorFromRect",
            "GetMonitorInfoA",
            "GetMonitorInfoW",
            "EnumDisplayMonitors",
            "AnimateWindow",
            "GetCursorInfo",
        ],
    ),
    (
        "KERNEL32.DLL",
        &[
            "IsDebuggerPresent",
            "InterlockedCompareExchange",
            "InterlockedExchangeAdd",
            "GetFileAttributesExA",
            "GetLongPathNameA",
        ],
    ),
];

/// Whether the DLL `dll` exports `function` under `version`.
pub fn exports(version: WindowsVersion, dll: &str, function: &str) -> bool {
    version.is_98()
        || !ONLY_98
            .iter()
            .any(|(d, names)| d.eq_ignore_ascii_case(dll) && names.contains(&function))
}
