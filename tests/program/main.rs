//! The rust-dos program: its command line, the debug server, save states,
//! deterministic runs, game profiles and fixes, the run lock and the game
//! suite.
//!
//! One test program for the files in this folder (Cargo.toml lists the
//! groups): each file is a module, and the helpers they share are
//! declared here, once.

#[path = "../common/dyndiff/mod.rs"]
mod dyndiff;
mod gamesuite;
#[path = "../common/pmrig/mod.rs"]
mod pmrig;

mod cheats_tests;
mod cli_tests;
mod debug_journal_tests;
mod deterministic_tests;
mod exe_exports;
mod game_fixes_tests;
mod game_suite;
mod games_tests;
mod layout_tests;
mod run_lock_tests;
mod savestate_tests;
