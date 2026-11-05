//! The devices on the bus: sound cards and MIDI, disk controllers and CD
//! drives, network cards, ports, input devices, the timer and the mixer.
//!
//! One test program for the files in this folder (Cargo.toml lists the
//! groups): each file is a module, and the helpers they share are
//! declared here, once.

#[path = "../common/cdimage/mod.rs"]
mod cdimage;

mod ata_tests;
mod awe32_tests;
mod bus_tests;
mod cd_audio_tests;
mod cdrom_tests;
mod fdc_tests;
mod gus_patch_tests;
mod gus_tests;
mod ide_tests;
mod ipx_tests;
mod joystick_tests;
mod lpt_dac_tests;
mod mixer_tests;
mod mouse_driver_tests;
mod mt32_tests;
mod nat_tests;
mod ne2000_tests;
mod pci_irq_tests;
mod printer_tests;
mod ps2_mouse_tests;
mod sb_tests;
mod sc55_tests;
mod tandy_sound_tests;
mod timer_tests;
mod uart_tests;
