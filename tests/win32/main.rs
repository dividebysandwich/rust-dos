//! The built-in Windows: Win32 programs, their DLLs, and the conformance
//! programs (tests/conformance).
//!
//! One test program for the files in this folder (Cargo.toml lists the
//! groups): each file is a module, and the helpers they share are
//! declared here, once.

#[path = "../common/cdimage/mod.rs"]
mod cdimage;
#[path = "../common/dyndiff/mod.rs"]
mod dyndiff;
#[path = "../common/win16/mod.rs"]
mod win16;

mod conformance_tests;
mod win32_audio_tests;
mod win32_comctl32_tests;
mod win32_d3d8_tests;
mod win32_d3d_tests;
mod win32_ddraw_tests;
mod win32_dinput_tests;
mod win32_dll_files_tests;
mod win32_dmusic_tests;
mod win32_dshow_tests;
mod win32_glide_tests;
mod win32_imagehlp_tests;
mod win32_opengl_tests;
mod win32_screen_tests;
mod win32_shell_tests;
mod win32_tests;
mod win32_win98_tests;
mod win32_winmm_tests;
mod win32_zorder_tests;
mod wine_dlls;
