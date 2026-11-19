//! The dialog that tells the user rust-dos crashed, with the text to
//! report and where (`rust_dos::log::set_crash_dialog`).
//!
//! It runs on the thread that crashed, maybe inside a graphics driver's
//! access violation, so it uses nothing of the emulator's: not the
//! window, its OpenGL context or the settings window. On Windows it is
//! the system's message box, whose text Ctrl+C copies. Elsewhere it is
//! SDL's message box, which SDL makes without a window of its own, with
//! a button that puts the text on the clipboard.

/// The dialog's title.
const TITLE: &str = "rust-dos crashed";

/// Whether a run shows crashes in a dialog: one in a window a user
/// looks at. A hidden window, SDL's dummy or offscreen video and a run
/// driven through the debug server have nobody to read it, and a dialog
/// there would keep the crashed process waiting; their crashes go to
/// the log only.
pub fn wanted(hidden: bool, video_driver: &str, debug_server: bool) -> bool {
    let windowless = ["dummy", "offscreen"]
        .iter()
        .any(|d| video_driver.eq_ignore_ascii_case(d));
    !hidden && !windowless && !debug_server
}

/// Show crashes in the dialog from now on. Called on the main thread.
pub fn enable() {
    #[cfg(target_os = "macos")]
    let _ = MAIN.set(std::thread::current().id());
    rust_dos::log::set_crash_dialog(Some(show));
}

/// The main thread, which alone may show the dialog on macOS: SDL hands
/// a message box from another thread to it and waits, and the main
/// thread may be waiting for the thread that crashed.
#[cfg(target_os = "macos")]
static MAIN: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();

#[cfg(windows)]
fn show(report: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW,
    };
    let text = format!("{report}\n\nCtrl+C copies this text.");
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (text, title) = (wide(&text), wide(TITLE));
    let style = MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST;
    // SAFETY: both strings end with a nul, and the box has no owner.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), style) };
}

#[cfg(not(windows))]
fn show(report: &str) {
    #[cfg(target_os = "macos")]
    if MAIN.get() != Some(&std::thread::current().id()) {
        return;
    }
    use sdl3::messagebox::{
        ButtonData, ClickedButton, MessageBoxButtonFlag, MessageBoxFlag, show_message_box,
    };
    const COPY: i32 = 1;
    let buttons = [
        ButtonData {
            flags: MessageBoxButtonFlag::RETURNKEY_DEFAULT,
            button_id: COPY,
            text: "Copy and close",
        },
        ButtonData {
            flags: MessageBoxButtonFlag::ESCAPEKEY_DEFAULT,
            button_id: 0,
            text: "Close",
        },
    ];
    let text = format!("{report}\n\n\"Copy and close\" puts this text on the clipboard.");
    let clicked = show_message_box(MessageBoxFlag::ERROR, &buttons, TITLE, &text, None, None);
    if let Ok(ClickedButton::CustomButton(button)) = clicked
        && button.button_id == COPY
        && let Ok(report) = std::ffi::CString::new(report)
    {
        // SAFETY: the text ends with a nul. SDL's clipboard needs its
        // video, which a run in a window has.
        unsafe { sdl3::sys::clipboard::SDL_SetClipboardText(report.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::wanted;

    #[test]
    fn only_a_run_in_a_window_nobody_drives_shows_the_dialog() {
        assert!(wanted(false, "windows", false));
        assert!(wanted(false, "x11", false));
        assert!(!wanted(true, "windows", false), "--hidden");
        assert!(!wanted(false, "dummy", false), "SDL_VIDEO_DRIVER=dummy");
        assert!(!wanted(false, "offscreen", false));
        assert!(!wanted(false, "windows", true), "--debug-server");
    }
}
