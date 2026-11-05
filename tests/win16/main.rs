//! The built-in Windows 3.1: Win16 programs, their DLLs, and the
//! conformance programs (tests/conformance16).
//!
//! One test program for the files in this folder (Cargo.toml lists the
//! groups): each file is a module, and the helpers they share are
//! declared here, once.

#[path = "../common/win16/mod.rs"]
mod win16;

mod win16_conformance_tests;
mod win16_drive_tests;
mod win16_exec_tests;
mod win16_hook_tests;
mod win16_mmsystem_tests;
mod win16_tests;
mod win16_user_tests;
