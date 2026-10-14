//! The `Host` trait: what the settings window asks of the emulator.

use super::{BootView, SlotView};
use crate::config::Settings;
use crate::disk::DriveInfo;
use crate::games::{GameEntry, NewGame};
use crate::mount::MountSpec;
use std::path::{Path, PathBuf};

/// What the window needs the emulator to do.
pub trait Host {
    /// Take on changed settings. Returns a note on when they take effect,
    /// or a problem.
    fn apply(&mut self, settings: &Settings) -> Result<Option<String>, String>;
    fn mount(&mut self, spec: MountSpec, replace: bool) -> Result<PathBuf, String>;
    fn unmount(&mut self, drive: u8) -> Result<(), String>;
    fn drives(&self) -> Vec<DriveInfo>;
    /// Save the settings and the drives to the configuration file.
    fn save(&mut self, settings: &Settings) -> Result<(), String>;
    /// Center the VR headset's view where the head is now, as
    /// Ctrl+Shift+Home does. Returns what to tell the user.
    fn center_vr(&mut self) -> Result<String, String> {
        Err("No VR headset shows the scene".to_string())
    }
    /// Start marking the real keyboard with a controller in the headset.
    /// Returns what to tell the user.
    fn mark_vr_keyboard(&mut self) -> Result<String, String> {
        Err("No VR headset shows the scene".to_string())
    }
    /// Boot the system on `drive`'s disk image now, as BOOT -l does.
    /// Returns what to tell the user.
    fn boot(&mut self, drive: u8) -> Result<String, String> {
        let _ = drive;
        Err("Disk images can't be booted here".to_string())
    }
    /// What a system booted from a disk image has of the drives, while
    /// one runs.
    fn booted(&self) -> Option<BootView> {
        None
    }
    /// Reset the machine, as Ctrl+Alt+Del does.
    fn reset_machine(&mut self) {}
    /// Put `text` on the host's clipboard.
    fn copy_text(&mut self, _text: &str) -> Result<(), String> {
        Err("there is no clipboard".to_string())
    }
    /// Copy what the booted system changed on the disk of the host
    /// directory `drive` into it. Returns what to tell the user.
    fn sync_shared(&mut self, drive: u8) -> Result<String, String> {
        let _ = drive;
        Err("No system shares this drive".to_string())
    }
    /// Make the CD of the host folder on `drive` again, with what the
    /// folder holds now, as a new disc in the booted system's drive.
    fn reinsert(&mut self, drive: u8) -> Result<String, String> {
        let _ = drive;
        Err("No system has this drive as a CD".to_string())
    }
    /// Have `drive` boot when Rust-DOS starts, or not; the drives' saving
    /// keeps it.
    fn set_boot(&mut self, drive: u8, boot: bool) -> Result<(), String> {
        let _ = (drive, boot);
        Err("Disk images can't be booted here".to_string())
    }
    /// Without the host's files: have the user pick a disk or CD image for
    /// `drive`, or for whichever drive suits it (None). The frontend mounts
    /// it once it is picked, and tells the window (`drives_changed`).
    fn choose_image(&mut self, drive: Option<u8>) -> Result<(), String> {
        let _ = drive;
        Err("Disk images are mounted from the host's files here".to_string())
    }
    /// The game profiles (games.rs), by name.
    fn games(&self) -> Vec<GameEntry> {
        Vec::new()
    }
    /// The game launched and not ended yet, by its id.
    fn active_game(&self) -> Option<String> {
        None
    }
    /// Whether a DOS program is running instead of waiting at the prompt.
    /// `launch_game` and `launch_variant` still start it; the settings
    /// window asks first and calls `close_program` if the user agrees.
    fn program_running(&self) -> bool {
        false
    }
    /// Close the running program, returning the machine to its DOS shell.
    fn close_program(&mut self) {}
    /// Launch the game `id` at the prompt: its settings, its drives and the
    /// commands that start it. Returns what to tell the user.
    fn launch_game(&mut self, id: &str) -> Result<String, String> {
        let _ = id;
        Err("There are no game profiles here".to_string())
    }
    /// The ways the game `id` can start, if its package has launch
    /// configurations to choose from.
    fn launch_choices(&self, id: &str) -> Option<crate::games::LaunchChoices> {
        let _ = id;
        None
    }
    /// Launch the game `id` with its launch configuration `variant` (None:
    /// the default); a `tool`'s end offers the choice again.
    fn launch_variant(&mut self, id: &str, variant: Option<&str>, tool: bool) -> Result<String, String> {
        let _ = (variant, tool);
        self.launch_game(id)
    }
    /// The game `id`'s gamepad mapping, for the player: each input, and
    /// what it does.
    fn game_pad(&self, id: &str) -> Vec<(String, String)> {
        let _ = id;
        Vec::new()
    }
    /// Make a profile of `game` with `settings`. Returns its id.
    fn create_game(&mut self, game: &NewGame, settings: &Settings) -> Result<String, String> {
        let _ = (game, settings);
        Err("There are no game profiles here".to_string())
    }
    fn delete_game(&mut self, id: &str) -> Result<(), String> {
        let _ = id;
        Err("There are no game profiles here".to_string())
    }
    /// Take the game `id` back to how it was installed: the changes its
    /// drives keep apart (games.rs's `reset`) go.
    fn reset_game(&mut self, id: &str) -> Result<(), String> {
        let _ = id;
        Err("There are no game profiles here".to_string())
    }
    /// Make a profile of the game set up for DOSBox at `source` (games.rs's
    /// `import`). Returns its id, and what to tell the user.
    fn import_game(&mut self, source: &Path) -> Result<(String, String), String> {
        let _ = source;
        Err("Games are imported from the host's files".to_string())
    }
    /// The DOS directory the prompt is in, where a new game likely is.
    fn current_directory(&self) -> String {
        "C:\\".to_string()
    }
    /// The machine's RAM, for the Cheats page to search.
    fn memory(&self) -> &[u8] {
        &[]
    }
    /// Write `bytes` to the machine's memory at `addr`.
    fn poke(&mut self, addr: usize, bytes: &[u8]) {
        let _ = (addr, bytes);
    }
    /// Whether cheats may be used now: not in RetroAchievements' hardcore
    /// mode.
    fn cheats_allowed(&self) -> Result<(), String> {
        Ok(())
    }
    /// The values the machine keeps frozen, and new ones.
    fn freezes(&self) -> Vec<crate::cheats::Freeze> {
        Vec::new()
    }
    fn set_freezes(&mut self, freezes: Vec<crate::cheats::Freeze>) {
        let _ = freezes;
    }
    /// RetroAchievements, as the Achievements page shows it; None where
    /// there is none.
    fn achievements(&self) -> Option<crate::achievements::AchievementsView> {
        None
    }
    /// Log in to RetroAchievements; how it goes shows in `achievements`.
    fn achievements_login(&mut self, username: &str, password: &str) -> Result<(), String> {
        let _ = (username, password);
        Err("There is no RetroAchievements here".to_string())
    }
    fn achievements_logout(&mut self) {}
    /// Tell RetroAchievements which version the game playing is: the zip
    /// or .dosz `archive` it came in, whose hash its profile keeps.
    /// Returns what to tell the user.
    fn identify_game(&mut self, archive: &Path) -> Result<String, String> {
        let _ = archive;
        Err("There is no RetroAchievements here".to_string())
    }
    /// The manuals and extras of the game `id` (games.rs's `manuals`).
    fn manuals(&self, id: &str) -> Vec<crate::manuals::Manual> {
        let _ = id;
        Vec::new()
    }
    /// Make the document or picture `path` one of the game `id`'s manuals.
    fn add_manual(&mut self, id: &str, path: &Path) -> Result<(), String> {
        let _ = (id, path);
        Err("There are no game profiles here".to_string())
    }
    /// Whether save states can be kept (there is somewhere to keep them).
    fn states_available(&self) -> bool {
        false
    }
    /// The save state slots with a state in them, of the game playing or of
    /// the machine without one.
    fn states(&self) -> Vec<SlotView> {
        Vec::new()
    }
    /// The slot the hotkeys save to and load (Ctrl+F1, Ctrl+F2).
    fn current_slot(&self) -> u8 {
        1
    }
    /// Save the machine to `slot`, which the hotkeys use from then on.
    /// Returns what to tell the user.
    fn save_state(&mut self, slot: u8) -> Result<String, String> {
        let _ = slot;
        Err("There are no save states here".to_string())
    }
    /// Load the state in `slot`, which the hotkeys use from then on.
    /// Returns what to tell the user.
    fn load_state(&mut self, slot: u8) -> Result<String, String> {
        let _ = slot;
        Err("There are no save states here".to_string())
    }
    fn delete_state(&mut self, slot: u8) -> Result<(), String> {
        let _ = slot;
        Err("There are no save states here".to_string())
    }
    /// The lines of the `[autoexec]` section of the file `save` writes to,
    /// comments and blank lines too.
    fn autoexec(&self) -> Result<Vec<String>, String> {
        Err("There is no configuration file here".to_string())
    }
    /// Make `lines` that file's `[autoexec]` section.
    fn save_autoexec(&mut self, lines: &[String]) -> Result<(), String> {
        let _ = lines;
        Err("There is no configuration file here".to_string())
    }
    /// The LAN, as the room browser shows it, if there is a network.
    fn lan(&self) -> Option<crate::net::LanView> {
        None
    }
    /// Ask `relay` (`host[:port]`, None for the first that answers on this
    /// network) for its rooms with `filter` in their names; `lan` has them
    /// once they come.
    fn browse_rooms(&mut self, relay: Option<&str>, filter: &str) -> Result<(), String> {
        let _ = (relay, filter);
        Err("There is no network here".to_string())
    }
    /// Join `room` at `relay` with `password` (empty for none), making the
    /// room if it isn't there.
    fn join_room(&mut self, relay: Option<&str>, room: &str, password: &str) -> Result<(), String> {
        let _ = (relay, room, password);
        Err("There is no network here".to_string())
    }
    fn leave_room(&mut self) {}
    /// End the room this instance hosts for everyone in it, and leave it.
    fn disband_room(&mut self) {}
    /// Make `room` on this network, on a relay this instance runs, and
    /// join it.
    fn host_room(&mut self, room: &str, password: &str) -> Result<(), String> {
        let _ = (room, password);
        Err("There is no network here".to_string())
    }
}
