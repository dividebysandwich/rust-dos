//! IMM32.DLL: the Input Method Manager of a Windows 95 or 98 that isn't
//! the Chinese, Japanese or Korean edition. Those have no input methods,
//! and their IMM32 is a stub: every function returns 0 at once (no input
//! context, no IME window, nothing done), except ImmGetCompositionStringA
//! and W, which return -1 (IMM_ERROR_GENERAL), and ImmGetVirtualKey,
//! which returns VK_PROCESSKEY. Games import it statically to turn the
//! IME off for their windows (Age of Empires calls ImmAssociateContext
//! and ImmGetContext), so they don't start without it. The table, its
//! ordinals and argument counts are Windows 98 SE's IMM32 4.10.1998 code,
//! one `xor eax, eax` or `mov eax` and one `ret` a function.

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "IMM32.DLL",
    base: 0xBF5E_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
};

const fn stdcall(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: super::stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// The functions, with Windows 98 SE's ordinals. Append only (see
/// `stubs`).
static APIS: &[Api] = &[
    stdcall("ImmAssociateContext", 1, 2, none),
    stdcall("ImmAssociateContextEx", 2, 3, none),
    stdcall("ImmConfigureIMEA", 3, 4, none),
    stdcall("ImmConfigureIMEW", 4, 4, none),
    stdcall("ImmCreateContext", 5, 0, none),
    stdcall("ImmCreateIMCC", 6, 1, none),
    stdcall("ImmCreateSoftKeyboard", 7, 4, none),
    stdcall("ImmDestroyContext", 8, 1, none),
    stdcall("ImmDestroyIMCC", 9, 1, none),
    stdcall("ImmDestroySoftKeyboard", 10, 1, none),
    stdcall("ImmDisableIME", 11, 1, none),
    stdcall("ImmEnumRegisterWordA", 12, 6, none),
    stdcall("ImmEnumRegisterWordW", 13, 6, none),
    stdcall("ImmEscapeA", 14, 4, none),
    stdcall("ImmEscapeW", 15, 4, none),
    stdcall("ImmGenerateMessage", 16, 1, none),
    stdcall("ImmGetCandidateListA", 17, 4, none),
    stdcall("ImmGetCandidateListCountA", 18, 2, none),
    stdcall("ImmGetCandidateListCountW", 19, 2, none),
    stdcall("ImmGetCandidateListW", 20, 4, none),
    stdcall("ImmGetCandidateWindow", 21, 3, none),
    stdcall("ImmGetCompositionFontA", 22, 2, none),
    stdcall("ImmGetCompositionFontW", 23, 2, none),
    stdcall("ImmGetCompositionStringA", 24, 4, error),
    stdcall("ImmGetCompositionStringW", 25, 4, error),
    stdcall("ImmGetCompositionWindow", 26, 2, none),
    stdcall("ImmGetContext", 27, 1, none),
    stdcall("ImmGetConversionListA", 28, 6, none),
    stdcall("ImmGetConversionListW", 29, 6, none),
    stdcall("ImmGetConversionStatus", 30, 3, none),
    stdcall("ImmGetDefaultIMEWnd", 31, 1, none),
    stdcall("ImmGetDescriptionA", 32, 3, none),
    stdcall("ImmGetDescriptionW", 33, 3, none),
    stdcall("ImmGetGuideLineA", 34, 4, none),
    stdcall("ImmGetGuideLineW", 35, 4, none),
    stdcall("ImmGetHotKey", 36, 4, none),
    stdcall("ImmGetIMCCLockCount", 37, 1, none),
    stdcall("ImmGetIMCCSize", 38, 1, none),
    stdcall("ImmGetIMCLockCount", 39, 1, none),
    stdcall("ImmGetIMEFileNameA", 40, 3, none),
    stdcall("ImmGetIMEFileNameW", 41, 3, none),
    stdcall("ImmGetImeMenuItemsA", 42, 6, none),
    stdcall("ImmGetImeMenuItemsW", 43, 6, none),
    stdcall("ImmGetOpenStatus", 44, 1, none),
    stdcall("ImmGetProperty", 45, 2, none),
    stdcall("ImmGetRegisterWordStyleA", 46, 3, none),
    stdcall("ImmGetRegisterWordStyleW", 47, 3, none),
    stdcall("ImmGetStatusWindowPos", 48, 2, none),
    stdcall("ImmGetVirtualKey", 49, 1, process_key),
    stdcall("ImmInstallIMEA", 50, 2, none),
    stdcall("ImmInstallIMEW", 51, 2, none),
    stdcall("ImmIsIME", 52, 1, none),
    stdcall("ImmIsUIMessageA", 53, 4, none),
    stdcall("ImmIsUIMessageW", 54, 4, none),
    stdcall("ImmLockIMC", 55, 1, none),
    stdcall("ImmLockIMCC", 56, 1, none),
    stdcall("ImmNotifyIME", 57, 4, none),
    stdcall("ImmReSizeIMCC", 58, 2, none),
    stdcall("ImmRegisterWordA", 59, 4, none),
    stdcall("ImmRegisterWordW", 60, 4, none),
    stdcall("ImmReleaseContext", 61, 2, none),
    stdcall("ImmRequestMessageA", 62, 3, none),
    stdcall("ImmRequestMessageW", 63, 3, none),
    stdcall("ImmSetCandidateWindow", 64, 2, none),
    stdcall("ImmSetCompositionFontA", 65, 2, none),
    stdcall("ImmSetCompositionFontW", 66, 2, none),
    stdcall("ImmSetCompositionStringA", 67, 6, none),
    stdcall("ImmSetCompositionStringW", 68, 6, none),
    stdcall("ImmSetCompositionWindow", 69, 2, none),
    stdcall("ImmSetConversionStatus", 70, 3, none),
    stdcall("ImmSetHotKey", 71, 4, none),
    stdcall("ImmSetOpenStatus", 72, 2, none),
    stdcall("ImmSetStatusWindowPos", 73, 2, none),
    stdcall("ImmShowSoftKeyboard", 74, 2, none),
    stdcall("ImmSimulateHotKey", 75, 2, none),
    stdcall("ImmUnlockIMC", 76, 1, none),
    stdcall("ImmUnlockIMCC", 77, 1, none),
    stdcall("ImmUnregisterWordA", 78, 4, none),
    stdcall("ImmUnregisterWordW", 79, 4, none),
];

/// What every function but three returns: 0, which is FALSE, a null
/// handle or no bytes, as Windows' stub does.
fn none(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// ImmGetCompositionStringA and W: IMM_ERROR_GENERAL.
fn error(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0xFFFF_FFFF)
}

/// ImmGetVirtualKey: VK_PROCESSKEY.
fn process_key(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0xE5)
}
