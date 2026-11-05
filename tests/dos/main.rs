//! The built-in DOS and its shell: INT 21h, files and drives, memory
//! (MCBs, EMS, UMBs), programs and extenders, and the commands.
//!
//! One test program for the files in this folder (Cargo.toml lists the
//! groups): each file is a module, and the helpers they share are
//! declared here, once.

#[path = "../common/cdimage/mod.rs"]
mod cdimage;
mod fatimage;

mod batch_tests;
mod bios_drive_tests;
mod boot_tests;
mod command_tests;
mod disk_speed_tests;
mod dos_features_test;
mod dpmi_tests;
mod drive_int21_tests;
mod edit_tests;
mod ems_tests;
mod exec_tests;
mod extender_support_tests;
mod fat_image_tests;
mod file_table_tests;
mod int21_reproduction;
mod int21_test;
mod load_exe_test;
mod makeimg_tests;
mod mcb_tests;
mod mem_tests;
mod mscdex_tests;
mod rc_tests;
mod reboot_tests;
mod shell_tests;
mod tsr_tests;
mod umb_tests;
mod wildcard_tests;
