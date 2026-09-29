//! RetroAchievements (retroachievements.org): games identified by the
//! hash of their files, achievement sets and leaderboards checked against
//! the machine's memory every frame, unlocks sent to the site, and the
//! rich presence line it shows of what a player is doing.
//!
//! - `runtime`: the logic, a port of rcheevos's runtime.
//! - `memory`: the machine's memory as the MS-DOS sets address it, laid
//!   out as DOSBox Pure lays it out, which the sets are made with.
//! - `hash`: the game's hash, of its zip (or DOSZ) archive.
//! - `client`: the session with the site, through a `Transport`: HTTPS
//!   (`http`) on the desktop.

pub mod client;
pub mod hash;
#[cfg(not(target_arch = "wasm32"))]
pub mod http;
pub mod memory;
pub mod runtime;

pub use client::{
    AchievementRow, AchievementSettings, Achievements, AchievementsView, GameView, Notice,
};
