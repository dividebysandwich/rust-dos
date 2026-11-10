//! The video adapters and the picture: the text and graphics modes, the
//! SVGA and 3D cards, the BIOS, and frame timing.
//!
//! One test program for the files in this folder (Cargo.toml lists the
//! groups): each file is a module, and the helpers they share are
//! declared here, once.

#[path = "../common/dyndiff/mod.rs"]
mod dyndiff;
#[path = "../common/pmrig/mod.rs"]
mod pmrig;

mod adapter_tests;
mod cga_tests;
mod crt_timing_tests;
mod cursor_tests;
mod ega_tests;
mod et4000_tests;
mod frame_rate_tests;
mod frame_tests;
mod hercules_tests;
mod lfb_speed;
mod mach64_tests;
mod mono_tests;
mod powervr_tests;
mod render_dirty_tests;
mod tandy_tests;
mod test_vga_integration;
mod toadforce_tests;
mod vbe_tests;
mod verite_tests;
mod vga_page_flip_tests;
mod video_bios_tests;
mod voodoo_tests;
