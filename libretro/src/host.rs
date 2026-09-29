//! The settings window's way to the machine, its drives and the files the
//! core keeps: rust-dos.conf and the game profiles.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use rust_dos::config::{self, Settings};
use rust_dos::config_ui::{self, Host};
use rust_dos::cpu::Cpu;
use rust_dos::disk::{self, DriveInfo, DriveKind};
use rust_dos::games::{self, ActiveGame, GameEntry, NewGame};
use rust_dos::hardware::Hardware;
use rust_dos::keylayout::Layout;
use rust_dos::mount::MountSpec;
use rust_dos::timer::{CpuSpeed, Pacer};

use crate::content::{self, Base, Dirs, Overlay, Profile};
use crate::options::Values;

/// The machine and what it was set up from.
pub struct Machine {
    /// Boxed, as it is big: frontends may run the core on a small stack.
    pub cpu: Box<Cpu>,
    /// The settings in effect (or waiting, see `Hardware`).
    pub settings: Settings,
    pub hardware: Hardware,
    pub pacer: Pacer,
    pub dirs: Dirs,
    pub base: Base,
    pub options: Values,
    pub overlay: Option<Overlay>,
    /// The content's own profile, if it is one or comes with one.
    pub profile: Option<Profile>,
    /// The game launched and not ended yet.
    pub game: Option<ActiveGame>,
    /// The settings and drives as rust-dos.conf has them, which saving
    /// writes the changes from.
    pub saved: Settings,
    pub saved_drives: BTreeMap<u8, MountSpec>,
    /// Messages for the frontend to show.
    pub notices: Vec<String>,
}

/// The drives the configuration file can hold, by letter: mounts of host
/// directories and images, not the drives held in memory.
pub fn mounted_drives(cpu: &Cpu) -> BTreeMap<u8, MountSpec> {
    cpu.bus
        .disk
        .mounted_drives()
        .into_iter()
        .filter(|info| info.kind != DriveKind::Virtual)
        .filter_map(|info| info.mount.map(|spec| (info.drive, spec)))
        .collect()
}

impl Machine {
    pub fn warn(&mut self, message: &str) {
        self.cpu.bus.log_string(&format!("[CONFIG] Warning: {}", message));
    }

    /// The settings the options and the files make now, below the game's.
    pub fn base_settings(&self) -> (Settings, Vec<String>) {
        content::settings(&self.base, &self.options, self.overlay.as_ref())
    }

    /// The file of the game profile `id`: the content's, or one in the
    /// games folder.
    fn profile_file(&self, id: &str) -> PathBuf {
        match &self.profile {
            Some(profile) if profile.id == id => profile.file.clone(),
            _ => self.dirs.games().join(format!("{}.conf", id)),
        }
    }

    /// The file the settings window saves to: the game's profile while one
    /// plays, else rust-dos.conf.
    pub fn config_path(&self) -> PathBuf {
        match &self.game {
            Some(game) => self.profile_file(&game.id),
            None => self.base.file.clone(),
        }
    }

    /// Launch the game `id` from its profile `text`, whose paths are
    /// relative to `dir`: its settings over these, its drives over theirs,
    /// and the commands that start it.
    pub fn start_game(&mut self, id: &str, text: &str, dir: &Path) -> Result<String, String> {
        if let Some(previous) = self.game.take() {
            self.end_game(previous);
        }
        let base = self.settings.clone();
        let prepared = games::prepare(id, &base, text, dir, content::home().as_deref())?;
        for warning in &prepared.warnings {
            self.warn(&format!("{}.conf: {}", id, warning));
        }
        let settings = content::frontend_settings(prepared.settings);
        if let Err(e) = self.apply(&settings) {
            self.warn(&e);
        }
        let mut replaced = Vec::new();
        for spec in &prepared.drives {
            let before = self.cpu.bus.disk.drive_info(spec.drive).and_then(|d| d.mount);
            match self.cpu.bus.mount_drive(spec.drive, &spec.path, spec.opts.clone(), true) {
                Ok(_) => replaced.push((spec.drive, before)),
                Err(e) => self.warn(&format!("{}.conf: drive {}: {}", id, disk::drive_key(spec.drive), e)),
            }
        }
        self.cpu.bus.config_dir = std::path::absolute(dir).ok();
        self.cpu.queue_batch_lines(&prepared.autoexec);
        self.cpu.bus.log_string(&format!("[CONFIG] Launching the game {} ({}.conf)", prepared.name, id));
        let message = format!("Starting {}", prepared.name);
        self.game = Some(ActiveGame { id: id.to_string(), name: prepared.name, base, saved: settings, replaced });
        Ok(message)
    }

    /// A game has ended: the settings and drives from before it.
    pub fn end_game(&mut self, game: ActiveGame) {
        self.cpu.bus.log_string(&format!("[CONFIG] The game {} has ended", game.name));
        self.cpu.bus.config_dir = Some(self.base.dir().to_path_buf());
        if let Err(e) = self.apply(&game.base) {
            self.warn(&e);
        }
        for (drive, before) in game.replaced.into_iter().rev() {
            let result = match before {
                Some(spec) => self.cpu.bus.mount_drive(drive, &spec.path, spec.opts, true).map(|_| ()),
                None => self.cpu.bus.unmount_drive(drive),
            };
            if let Err(e) = result {
                self.warn(&format!("drive {}: {}", disk::drive_key(drive), e));
            }
        }
    }

    /// The core options changed: the settings they make, under the game's.
    pub fn options_changed(&mut self, options: Values) {
        let memsize_before = self.options.get("memsize").map(str::to_string);
        self.options = options;
        let (base, warnings) = self.base_settings();
        for warning in warnings {
            self.warn(&warning);
        }
        let new = match self.game.as_ref().map(|g| g.id.clone()) {
            Some(id) => {
                let file = self.profile_file(&id);
                let text = fs::read_to_string(&file).unwrap_or_default();
                let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
                self.game.as_mut().unwrap().base = base.clone();
                match games::prepare(&id, &base, &text, &dir, content::home().as_deref()) {
                    Ok(prepared) => content::frontend_settings(prepared.settings),
                    Err(_) => base,
                }
            }
            None => base,
        };
        match self.apply(&new) {
            Ok(Some(note)) => self.notices.push(note),
            Ok(None) => {}
            Err(e) => self.notices.push(e),
        }
        if self.options.get("memsize").map(str::to_string) != memsize_before {
            self.notices.push("The memory size changes when the content is started again".to_string());
        }
    }
}

impl Host for Machine {
    fn apply(&mut self, new: &Settings) -> Result<Option<String>, String> {
        let new = content::frontend_settings(new.clone());
        let old = std::mem::replace(&mut self.settings, new.clone());
        if new.cycles != old.cycles {
            self.pacer.set_speed(new.cycles);
            // At max, the pacer tunes the speed from the current one.
            if let CpuSpeed::Fixed(n) = new.cycles {
                self.cpu.bus.set_cycles_per_ms(n);
            }
        }
        if new.core != old.core {
            self.cpu.core = new.core;
        }
        // Programs look for the host as they start.
        self.cpu.bus.dpmi.enabled = new.dpmi;
        self.cpu.bus.ide_hard_disks = new.ide_hard_disks;
        if new.dos_version != old.dos_version {
            rust_dos::dos_data::set_version(&mut self.cpu.bus, new.dos_version);
        }
        if new.keyboard_layout != old.keyboard_layout {
            self.cpu.bus.kbd.layout = new.keyboard_layout.layout(Layout::us());
        }
        if new.disk != old.disk {
            self.cpu.bus.set_disk_settings(new.disk);
        }
        if new.mixer != old.mixer {
            self.cpu.bus.set_mixer(new.mixer);
        }
        if new.joystick != old.joystick {
            self.cpu.bus.set_joystick(new.joystick);
        }
        if new.composite != old.composite {
            self.cpu.bus.vga.set_composite(new.composite);
        }
        if !self.hardware.differs(&new) {
            return Ok(None);
        }
        if !self.cpu.shell_idle() {
            return Ok(Some(config_ui::pending_note(self.hardware.video, &new).to_string()));
        }
        match self.hardware.apply(&mut self.cpu, &new).into_iter().next() {
            Some(problem) => Err(problem),
            None => Ok(None),
        }
    }

    fn mount(&mut self, spec: MountSpec, replace: bool) -> Result<PathBuf, String> {
        self.cpu.bus.log_string(&format!(
            "[CONFIG] Settings window: mount {}: {}",
            disk::drive_letter(spec.drive),
            spec.path.display()
        ));
        self.cpu.bus.mount_drive(spec.drive, &spec.path, spec.opts, replace)
    }

    fn unmount(&mut self, drive: u8) -> Result<(), String> {
        self.cpu.bus.unmount_drive(drive)
    }

    fn drives(&self) -> Vec<DriveInfo> {
        self.cpu.bus.disk.mounted_drives()
    }

    /// While a game plays, what changed into its profile; else what
    /// changed, and the drives mounted or unmounted since the start, into
    /// rust-dos.conf.
    fn save(&mut self, settings: &Settings) -> Result<(), String> {
        let home = content::home();
        if let Some(game) = self.game.as_ref() {
            let path = self.profile_file(&game.id);
            config::save(&path, &game.saved, settings, &[], home.as_deref(), config::Saving::Changes)?;
            self.game.as_mut().unwrap().saved = settings.clone();
            return Ok(());
        }
        let current = mounted_drives(&self.cpu);
        let changes: Vec<config::DriveChange> = (0..disk::LASTDRIVE)
            .filter(|d| self.saved_drives.get(d) != current.get(d))
            .map(|d| (d, current.get(&d).cloned()))
            .collect();
        let path = self.base.file.clone();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        }
        config::save(&path, &self.saved, settings, &changes, home.as_deref(), config::Saving::Changes)?;
        self.cpu.bus.log_string(&format!("[CONFIG] Saved the settings to {}", path.display()));
        self.saved = settings.clone();
        self.saved_drives = current;
        self.base.text = fs::read_to_string(&path).unwrap_or_default();
        Ok(())
    }

    fn autoexec(&self) -> Result<Vec<String>, String> {
        let path = self.config_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        config::load_autoexec(&path)
    }

    fn save_autoexec(&mut self, lines: &[String]) -> Result<(), String> {
        let path = self.config_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        }
        config::save_autoexec(&path, lines)?;
        self.cpu.bus.log_string(&format!("[CONFIG] Saved the [autoexec] commands to {}", path.display()));
        Ok(())
    }

    fn games(&self) -> Vec<GameEntry> {
        games::list(&self.dirs.games()).into_iter().map(|(e, _)| e).collect()
    }

    fn active_game(&self) -> Option<String> {
        self.game.as_ref().map(|g| g.id.clone())
    }

    fn launch_game(&mut self, id: &str) -> Result<String, String> {
        if !self.cpu.shell_idle() || self.cpu.batch.is_active() || self.cpu.shell_wait.is_some() {
            return Err("A program is running: quit it to launch a game".to_string());
        }
        let path = self.profile_file(id);
        let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        self.start_game(id, &text, &dir)
    }

    fn create_game(&mut self, new: &NewGame, settings: &Settings) -> Result<String, String> {
        let dir = self.dirs.games();
        let taken: Vec<String> = games::list(&dir).into_iter().map(|(e, _)| e.id).collect();
        let id = games::slug(&new.name, &taken);
        let base = self.game.as_ref().map_or(&self.saved, |g| &g.base);
        let text = games::profile_text(new, base, settings, &[], content::home().as_deref())?;
        let path = dir.join(format!("{}.conf", id));
        fs::create_dir_all(&dir)
            .and_then(|()| fs::write(&path, text))
            .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
        self.cpu.bus.log_string(&format!("[CONFIG] Made the game profile {}", path.display()));
        Ok(id)
    }

    fn delete_game(&mut self, id: &str) -> Result<(), String> {
        let path = self.dirs.games().join(format!("{}.conf", id));
        fs::remove_file(&path).map_err(|e| format!("cannot delete {}: {}", path.display(), e))
    }

    fn import_game(&mut self, source: &Path) -> Result<(String, String), String> {
        let (id, name, warnings) = games::import(&self.dirs.games(), source, content::home().as_deref())?;
        for warning in &warnings {
            self.cpu.bus.log_string(&format!("[CONFIG] Import of {}: {}", name, warning));
        }
        let message = match warnings.len() {
            0 => format!("{} is imported: Enter launches it", name),
            n => format!("{} is imported; {} settings didn't come across (see the log)", name, n),
        };
        Ok((id, message))
    }

    fn current_directory(&self) -> String {
        games::prompt_directory(&self.cpu)
    }

    fn memory(&self) -> &[u8] {
        self.cpu.bus.ram()
    }

    fn poke(&mut self, addr: usize, bytes: &[u8]) {
        for (i, &byte) in bytes.iter().enumerate() {
            self.cpu.bus.write_8(addr + i, byte);
        }
    }

    fn freezes(&self) -> Vec<rust_dos::cheats::Freeze> {
        self.cpu.bus.freezes.clone()
    }

    fn set_freezes(&mut self, freezes: Vec<rust_dos::cheats::Freeze>) {
        self.cpu.bus.freezes = freezes;
        self.cpu.bus.apply_freezes();
    }

    fn lan(&self) -> Option<rust_dos::net::LanView> {
        Some(self.cpu.bus.net.view())
    }

    fn browse_rooms(&mut self, relay: Option<&str>, filter: &str) -> Result<(), String> {
        self.cpu.bus.net.browse(relay, filter)
    }

    fn join_room(&mut self, relay: Option<&str>, room: &str, password: &str) -> Result<(), String> {
        self.cpu.bus.install_ipx();
        self.cpu.bus.net.join(relay, room, password)
    }

    fn leave_room(&mut self) {
        self.cpu.bus.net.leave();
    }

    fn disband_room(&mut self) {
        self.cpu.bus.net.disband();
    }

    fn host_room(&mut self, room: &str, password: &str) -> Result<(), String> {
        self.cpu.bus.install_ipx();
        self.cpu.bus.net.make_room(room, password)
    }
}
