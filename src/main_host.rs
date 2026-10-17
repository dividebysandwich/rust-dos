//! The settings window's way to the machine, the display and the speed:
//! `MainHost` and its `Host` implementation.

use std::path::PathBuf;

use crate::config::Settings;
use crate::config_file::{Saved, config_dir, config_warning, drive_changes, games_dir, save_config};
use crate::config_ui::Host;
use crate::cpu::Cpu;
use crate::describe_state;
use crate::disk::{DriveInfo, DriveKind, LASTDRIVE};
use crate::display::Display;
use crate::input::apply_keyboard_layout;
use crate::mount::MountSpec;
use crate::timer::{self, CpuSpeed};
use crate::{capture, config, config_ui, disk, games, keyboard, video};
use rust_dos::achievements::Achievements;
use rust_dos::games::{ActiveGame, GameEntry, NewGame};
use rust_dos::hardware::Hardware;
use rust_dos::savestate::{self, slots};

/// The settings window's way to the machine, the display and the speed.
pub(crate) struct MainHost<'m, 'd> {
    pub(crate) cpu: &'m mut Cpu,
    pub(crate) display: &'m mut Display<'d>,
    pub(crate) pacer: &'m mut timer::Pacer,
    /// The settings in effect (or waiting, see `Machine`).
    pub(crate) settings: &'m mut Settings,
    pub(crate) machine: &'m mut Hardware,
    pub(crate) saved: &'m mut Saved,
    /// The game launched and not ended yet.
    pub(crate) game: &'m mut Option<ActiveGame>,
    /// A game whose ways to start the window is to offer.
    pub(crate) choose: &'m mut Option<String>,
    pub(crate) confirm_launch: &'m mut Option<String>,
    pub(crate) autoinput: &'m mut Option<rust_dos::autoinput::AutoInput>,
    pub(crate) autoinput_fast: &'m mut bool,
    pub(crate) padmap: &'m mut Option<rust_dos::padmap::PadMapper>,
    /// The save states: their folder, the picture the machine shows, for
    /// theirs, and where the thread writing one says it is done.
    pub(crate) states: &'m Option<PathBuf>,
    pub(crate) picture: &'m video::Frame,
    pub(crate) state_done: &'m std::sync::mpsc::Sender<Result<String, String>>,
    /// The slot the hotkeys use, and whether a state was loaded (the
    /// machine's keys and a video recording go).
    pub(crate) slot: &'m mut u8,
    pub(crate) state_loaded: &'m mut bool,
    pub(crate) achievements: &'m mut Achievements,
    /// Deterministic mode: the date and time its clock started at. The
    /// speed stays fixed, and save states record the start.
    pub(crate) deterministic: Option<chrono::NaiveDateTime>,
    pub(crate) clipboard: &'m sdl2::clipboard::ClipboardUtil,
}

impl MainHost<'_, '_> {
    /// The profile of the game `id`, and the games folder.
    fn game_profile(&self, id: &str) -> Result<(String, PathBuf), String> {
        let dir = games_dir(self.saved.file.as_deref()).ok_or("There is no configuration file for the games folder")?;
        let path = dir.join(format!("{}.conf", id));
        let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
        Ok((text, dir))
    }

    /// The file the settings window saves to: the game's profile while one
    /// plays, else the configuration file.
    fn config_path(&self) -> Result<PathBuf, String> {
        let file = self.saved.file.as_deref().ok_or("No configuration file")?;
        Ok(match self.game.as_ref() {
            Some(game) => games_dir(Some(file)).ok_or("No games folder")?.join(format!("{}.conf", game.id)),
            None => file.to_path_buf(),
        })
    }

    /// Launch the game `id` from its profile `text` in the folder `dir`:
    /// its settings over these, its drives over theirs, and the commands
    /// that start it.
    pub(crate) fn start_game(&mut self, id: &str, text: &str, dir: &std::path::Path) -> Result<String, String> {
        if let Some(previous) = self.game.take() {
            self.end_game(previous);
        }
        let base = self.saved.settings.clone();
        let mut prepared = games::prepare(id, &base, text, dir, rust_dos::hostdirs::home_dir().as_deref())?;
        for warning in &prepared.warnings {
            config_warning(self.cpu, &format!("games/{}.conf: {}", id, warning));
        }
        if let Some(saves) = self.saved.file.as_deref().and_then(|f| games_dir(Some(f))).map(|g| games::saves_dir(&g)) {
            games::overlay_drives(&mut prepared, id, &saves)?;
        }
        if let Err(e) = self.apply(&prepared.settings) {
            config_warning(self.cpu, &e);
        }
        let programs_before = self.cpu.programs_loaded;
        let replaced = games::drives_before(self.cpu);
        for spec in &prepared.drives {
            match self.cpu.bus.mount_drive(spec.drive, &spec.path, spec.opts.clone(), true) {
                Ok(_) => {}
                Err(e) => config_warning(self.cpu, &format!("games/{}.conf: drive {}: {}", id, disk::drive_key(spec.drive), e)),
            }
        }
        self.cpu.bus.config_dir = std::path::absolute(dir).ok();
        self.cpu.queue_batch_lines(&prepared.autoexec);
        self.cpu.bus.log_string(&format!("[CONFIG] Launching the game {} (games/{}.conf)", prepared.name, id));
        // What RetroAchievements knows the game by.
        let hash = prepared.achievements.as_deref().and_then(|value| {
            games::achievements_hash(value, dir, rust_dos::hostdirs::home_dir().as_deref())
                .map_err(|e| {
                    if self.settings.achievements.enabled {
                        config_warning(self.cpu, &format!("games/{}.conf: achievements: {}", id, e));
                    }
                })
                .ok()
        });
        self.achievements.game_started(hash, &prepared.name);
        let message = format!("Starting {}", prepared.name);
        let (pad, pad_warnings) = rust_dos::padmap::PadMapping::parse(&prepared.pad);
        let (input, warnings) = ActiveGame::input_steps(prepared.input.as_deref());
        let warnings: Vec<String> = warnings.into_iter().chain(pad_warnings).collect();
        for warning in warnings {
            config_warning(self.cpu, &format!("games/{}.conf: {}", id, warning));
        }
        *self.game =
            Some(ActiveGame { id: id.to_string(), name: prepared.name, base, saved: prepared.settings, replaced, programs_before, input, pad, choose_after: false, saves_lock: prepared.saves_lock });
        Ok(message)
    }

    /// A file or folder dropped onto the window: a game set up for DOSBox
    /// is imported and launched, a folder or disk image mounted, a CD image
    /// put in the CD-ROM drive, a floppy in A:, and a program run from its
    /// folder. Returns what to tell the user.
    pub(crate) fn dropped(&mut self, path: &std::path::Path) -> String {
        use rust_dos::import::drop::{DropAction, drop_action};
        let free = |cpu: &Cpu| (3..LASTDRIVE - 1).find(|&d| !cpu.bus.disk.is_mounted(d));
        let mount = |host: &mut Self, drive: Option<u8>, path: &std::path::Path, kind: DriveKind| -> Result<u8, String> {
            let drive = drive.ok_or("There is no free drive letter")?;
            let spec = MountSpec { drive, path: path.to_path_buf(), opts: disk::MountOptions { kind, ..Default::default() } };
            host.mount(spec, true).map(|_| drive)
        };
        let name = |path: &std::path::Path| path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let result = match drop_action(path) {
            DropAction::ImportGame(source) => {
                self.import_game(&source).map(|(id, imported)| self.launch_game(&id).unwrap_or(imported))
            }
            DropAction::MountFolder(dir) => {
                mount(self, free(self.cpu), &dir, DriveKind::HardDisk).map(|d| format!("{}: is {}", disk::drive_letter(d), name(&dir)))
            }
            DropAction::Disc(image) => {
                let drive = self.cpu.bus.disk.drives_of_kind(DriveKind::CdRom).first().copied().or_else(|| free(self.cpu));
                mount(self, drive, &image, DriveKind::CdRom).map(|d| format!("{}: has {}", disk::drive_letter(d), name(&image)))
            }
            DropAction::Floppy(image) => mount(self, Some(0), &image, DriveKind::Floppy).map(|_| format!("A: has {}", name(&image))),
            DropAction::HardDisk(image) => {
                mount(self, free(self.cpu), &image, DriveKind::HardDisk).map(|d| format!("{}: is {}", disk::drive_letter(d), name(&image)))
            }
            DropAction::Run(program) => {
                let dir = program.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
                let mounted = self.cpu.bus.disk.mounted_drives().into_iter().find(|d| d.root.as_deref() == Some(dir.as_path()));
                let drive = match mounted {
                    Some(info) => Ok(info.drive),
                    None => mount(self, free(self.cpu), &dir, DriveKind::HardDisk),
                };
                drive.map(|d| {
                    if !self.cpu.shell_idle() || self.cpu.batch.is_active() || self.cpu.shell_wait.is_some() {
                        return format!("{}: is {}; quit the program running to start {}", disk::drive_letter(d), name(&dir), name(&program));
                    }
                    let letter = disk::drive_letter(d);
                    self.cpu.queue_batch_lines([format!("{}:", letter), "CD \\".to_string(), name(&program)]);
                    format!("Starting {} from {}:", name(&program), letter)
                })
            }
            DropAction::Package(package) => (|| {
                let dir = games_dir(self.saved.file.as_deref())
                    .ok_or("Packages become games in the games folder beside the configuration file, and there is none (--no-config)")?;
                let (id, game, warnings) = games::add_package(&dir, &package)?;
                for warning in &warnings {
                    self.cpu.bus.log_string(&format!("[CONFIG] Import of {}: {}", game, warning));
                }
                Ok(self.launch_game(&id).unwrap_or_else(|e| format!("{} is a game on the Games page: {}", game, e)))
            })(),
            DropAction::Nothing(e) => Err(e),
        };
        result.unwrap_or_else(|e| e)
    }

    /// A game has ended: the settings and drives from before it.
    /// The folder of the save states' slots: the game's, or the machine's
    /// without one.
    pub(crate) fn slot_dir(&self) -> Result<PathBuf, String> {
        let root = self.states.as_deref().ok_or("There is no folder for save states")?;
        Ok(slots::slot_dir(root, self.game.as_ref().map(|g| g.id.as_str())))
    }

    /// The machine's state now, with its header and picture.
    fn capture_state(&mut self) -> (slots::Header, video::Frame, Vec<u8>) {
        let game = self.game.as_ref().map(|g| (g.id.as_str(), g.name.as_str()));
        // The hardware in place, not a change waiting for the program to end.
        let hardware = self.machine.settings(self.settings);
        let mut picture = self.picture.clone();
        // The OpenGL renderer draws the 3dfx card's picture: the one in its
        // memory for the thumbnail.
        if let Some(v) = &mut self.cpu.bus.voodoo
            && v.output()
            && !v.picture_wanted()
        {
            v.set_software_picture(true);
            v.prepare_display();
            v.render(&mut picture.rgb, picture.width as usize);
            v.set_software_picture(false);
        }
        let mut header = slots::header(self.cpu, &hardware, game);
        header.deterministic = self.deterministic.map(|start| start.format("%Y-%m-%d %H:%M:%S").to_string());
        (header, picture, savestate::machine::save(self.cpu))
    }

    /// Save the machine to slot `slot`. Its state is taken now; a thread
    /// packs it and writes the file, and says on `state_done` when it has.
    pub(crate) fn save_slot(&mut self, slot: u8) -> Result<(), String> {
        let path = slots::slot_path(&self.slot_dir()?, slot);
        let (header, picture, state) = self.capture_state();
        // A booted system's disks as they are, beside its state.
        savestate::disks::delete_copies(&path);
        savestate::disks::save_copies(self.cpu, &path)?;
        let done = self.state_done.clone();
        self.cpu.bus.log_string(&format!("[STATE] Saving to {}", path.display()));
        std::thread::spawn(move || {
            let data = slots::encode(&header, &slots::thumbnail(&picture), &state);
            let result = slots::write_file(&path, &data).map(|()| format!("Saved to slot {}", slot));
            let _ = done.send(result);
        });
        Ok(())
    }

    /// Save the machine to the file `path`, now. Returns the state's header.
    /// `contents` records what is on the writable drives as well, which
    /// reads every file on them (a `check_files` save, media.rs).
    pub(crate) fn save_file(&mut self, path: &std::path::Path, contents: bool) -> Result<slots::Header, String> {
        let (mut header, picture, state) = self.capture_state();
        savestate::disks::delete_copies(path);
        savestate::disks::save_copies(self.cpu, path)?;
        // After the copies, which make the state's folder.
        if contents && let Some(media) = header.media.as_mut() {
            savestate::media::add_contents(self.cpu, media, path, |_| true);
        }
        slots::write_file(path, &slots::encode(&header, &slots::thumbnail(&picture), &state))?;
        self.cpu.bus.log_string(&format!("[STATE] Saved to {}", path.display()));
        Ok(header)
    }

    /// Load the save state file `path`: the hardware it was saved with
    /// first, as the settings have it, then the machine.
    fn load_file(&mut self, path: &std::path::Path) -> Result<slots::Header, String> {
        let (header, state) = self.read_state(path)?;
        self.load_state(path, header, &state)
    }

    /// Load the save state file `path` for the debug server. Returns the
    /// header and how the state's hardware settings and media differ from
    /// this machine's. A plain load brings the state's own hardware, as
    /// `load_file` does. A `strict_match` load takes only a state of this
    /// machine: one of other hardware or media, or without a record of its
    /// media, is refused (409) with what differs. A `check_files` load of
    /// a state that records what was on the writable drives (a
    /// `check_files` save's) compares them with the files now, and the
    /// changed files are differences like the others. The third list names
    /// the writable drives a `check_files` load couldn't check, as the
    /// state has no record of what was on them. Without `check_files` no
    /// file is read. `folders` takes the state's folder drives it names as
    /// mounted from other host folders: they are compared (and with
    /// `check_files` their files) as if the state had been saved with them,
    /// and the load leaves the drives mounted from them. The header returned
    /// has the folders the state records.
    #[allow(clippy::type_complexity)]
    pub(crate) fn load_for_debugger(
        &mut self,
        path: &std::path::Path,
        strict_match: bool,
        check_files: bool,
        folders: &[(u8, PathBuf)],
    ) -> Result<(slots::Header, Vec<String>, Vec<String>), (u16, serde_json::Value)> {
        // Whatever failed, the machine, its settings and hardware are as they were.
        let failed = |e: String| (400, serde_json::json!({"error": e, "loaded": false, "unchanged": true}));
        let (mut header, state) = self.read_state(path).map_err(failed)?;
        // A `check_files` state's media list every file: kept only when relocated.
        let recorded = (!folders.is_empty()).then(|| header.media.clone());
        if !folders.is_empty() {
            let media = header.media.as_mut().ok_or_else(|| {
                failed(format!("the state doesn't record its media (saved by rust-dos {}), so folders can't be taken", header.version))
            })?;
            savestate::media::relocate(media, folders).map_err(failed)?;
        }
        let mut differences = slots::machine_differences(&header.machine, &self.machine.settings(self.settings));
        // The machine's clock is the mode's start plus emulated time.
        let start = self.deterministic.map(|start| start.format("%Y-%m-%d %H:%M:%S").to_string());
        differences.extend(slots::mode_difference(header.deterministic.as_deref(), start.as_deref()));
        if strict_match {
            savestate::media::forget(self.cpu);
        }
        let mut unchecked = Vec::new();
        match &header.media {
            Some(media) => {
                let mut now = savestate::media::of(self.cpu);
                if check_files {
                    // What is on the drives the state has a record of.
                    let recorded = |m: &savestate::media::Medium| {
                        media.iter().any(|saved| saved.drive == m.drive && savestate::media::has_contents(saved))
                    };
                    savestate::media::add_contents(self.cpu, &mut now, path, recorded);
                }
                // Without `check_files` the machine's media have no record of
                // their files, so the state's isn't compared.
                differences.extend(savestate::media::differences(media, &now));
                if check_files {
                    let copied = |key: &str| savestate::disks::copy_path_of(path, key).exists();
                    unchecked = savestate::media::unrecorded(media, copied);
                }
            }
            None if strict_match => differences.push(format!(
                "the state doesn't record its media (saved by rust-dos {}), so they can't be checked",
                header.version
            )),
            None => {}
        }
        if strict_match && !differences.is_empty() {
            let error = format!("{} doesn't match this machine; nothing was loaded", path.display());
            return Err((409, serde_json::json!({"error": error, "loaded": false, "unchanged": true, "differences": differences})));
        }
        self.cpu.bus.disk.folders_from_state = folders.to_vec();
        let loaded = self.load_state(path, header, &state);
        self.cpu.bus.disk.folders_from_state.clear();
        let mut header = loaded.map_err(failed)?;
        if let Some(recorded) = recorded {
            header.media = recorded;
        }
        Ok((header, differences, unchecked))
    }

    /// The header and machine's state of the save state file `path`, if
    /// this machine can take it.
    fn read_state(&mut self, path: &std::path::Path) -> Result<(slots::Header, Vec<u8>), String> {
        if self.achievements.hardcore_active() {
            return Err("hardcore mode: no save states while RetroAchievements plays".to_string());
        }
        let data = slots::read_file(path)?;
        slots::decode(&data)
    }

    /// Load the state `state` of the file `path`, with its header: the
    /// hardware it was saved with first, then the machine. A load that
    /// fails puts back the settings and hardware from before it, and the
    /// machine as it was.
    fn load_state(&mut self, path: &std::path::Path, header: slots::Header, state: &[u8]) -> Result<slots::Header, String> {
        let (old_settings, old_hardware) = (self.settings.clone(), self.machine.clone());
        let mut hardware = slots::machine_settings(&header.machine, self.settings);
        // The memory the state has, which it brings (bus/state.rs).
        hardware.memsize = header.memsize;
        // A failed `machine::load` puts the machine back by itself; the
        // machine from before is needed only when the hardware changes.
        let changes = hardware != old_settings || self.machine.differs(&hardware);
        let before = changes.then(|| savestate::machine::save(self.cpu));
        if let Err(e) = self.apply(&hardware) {
            config_warning(self.cpu, &e);
        }
        if self.machine.differs(self.settings) {
            for warning in self.machine.apply(self.cpu, self.settings) {
                config_warning(self.cpu, &warning);
            }
        }
        // A booted system's disks come back from their copies, if the
        // journals don't reach back to the state.
        savestate::disks::offer_copies(self.cpu, path);
        let loaded = savestate::machine::load(self.cpu, state).map_err(|e| e.to_string());
        savestate::disks::withdraw(self.cpu);
        if let Err(e) = loaded {
            if let Some(before) = before {
                if let Err(e) = self.apply(&old_settings) {
                    config_warning(self.cpu, &e);
                }
                savestate::machine::roll_back(self.cpu, self.machine, &old_hardware, &old_settings, &before);
            }
            self.cpu.bus.log_string(&format!("[STATE] {} wasn't loaded: {}", path.display(), e));
            return Err(e);
        }
        // Deterministic mode runs at the speed the machine's state brought
        // (a state saved at auto or max brings the speed it had then), so
        // the run from the state is the same whatever this machine ran at.
        if self.deterministic.is_some() {
            let speed = CpuSpeed::Fixed(self.cpu.bus.clock.cycles_per_ms());
            if self.settings.cycles != speed {
                self.settings.cycles = speed;
                self.pacer.set_speed(speed);
            }
        }
        // The state's time stamp counter goes on at this machine's speed.
        self.cpu.set_tsc_speed(self.settings.cycles);
        self.pacer.rebase(&self.cpu.bus.clock, std::time::Instant::now());
        *self.state_loaded = true;
        self.cpu.bus.log_string(&format!("[STATE] Loaded {} (saved {})", path.display(), header.saved));
        Ok(header)
    }

    /// Load slot `slot`. A slot without a file is "empty", as the slot
    /// list calls it.
    pub(crate) fn load_slot(&mut self, slot: u8) -> Result<slots::Header, String> {
        let path = slots::slot_path(&self.slot_dir()?, slot);
        if !path.exists() {
            return Err("empty".to_string());
        }
        self.load_file(&path)
    }

    /// RetroAchievements' login: kept in the settings, and in the
    /// configuration file (not a game's profile), without the password.
    pub(crate) fn keep_token(&mut self, username: &str, token: &str) -> Result<(), String> {
        let set = |s: &mut Settings| {
            s.achievements.username = username.to_string();
            s.achievements.token = token.to_string();
        };
        set(self.settings);
        if let Some(game) = self.game.as_mut() {
            set(&mut game.base);
            set(&mut game.saved);
        }
        let before = self.saved.settings.clone();
        set(&mut self.saved.settings);
        let Some(path) = self.saved.file.clone() else { return Ok(()) };
        let home = rust_dos::hostdirs::home_dir();
        config::save(&path, &before, &self.saved.settings, &[], home.as_deref(), config::Saving::Changes)
    }

    /// The front end's part of closing the program: the pad mapper and
    /// autoinput stop, the game ends and every key comes up. The settings
    /// window's close and the debug server's reboot_shell both do this.
    pub(crate) fn release_program(&mut self) {
        if let Some(mut mapper) = self.padmap.take() {
            mapper.release(&mut self.cpu.bus);
        }
        if let Some(mut input) = self.autoinput.take() {
            input.stop(self.cpu);
        }
        if std::mem::take(self.autoinput_fast) {
            self.pacer.set_fast_forward(false, &self.cpu.bus.clock, std::time::Instant::now());
            self.cpu.bus.mixer.fast_forward = false;
        }
        if let Some(previous) = self.game.take() {
            self.end_game(previous);
        }
        keyboard::release_all(&mut self.cpu.bus);
    }

    pub(crate) fn end_game(&mut self, game: ActiveGame) {
        self.cpu.bus.log_string(&format!("[CONFIG] The game {} has ended", game.name));
        self.achievements.game_ended();
        self.cpu.bus.config_dir = config_dir(self.saved.file.as_deref());
        if let Err(e) = self.apply(&game.base) {
            config_warning(self.cpu, &e);
        }
        for e in games::restore_drives(self.cpu, game.replaced) {
            config_warning(self.cpu, &e);
        }
        games::tell_ended(self.cpu, &game.name);
    }
}

impl Host for MainHost<'_, '_> {
    fn apply(&mut self, new: &Settings) -> Result<Option<String>, String> {
        // Deterministic mode keeps the speed it has where the settings (a
        // game's profile, the window) ask for auto or max.
        let pinned;
        let new = match new.cycles {
            CpuSpeed::Auto(_) | CpuSpeed::Max if self.deterministic.is_some() => {
                pinned = Settings { cycles: CpuSpeed::Fixed(self.cpu.bus.clock.cycles_per_ms()), ..new.clone() };
                &pinned
            }
            _ => new,
        };
        let old = std::mem::replace(self.settings, new.clone());
        let shown = |s: &Settings| (s.scale, s.fullscreen, s.aspect, s.filter, s.shader, s.crt, s.monochrome, s.voodoo.scale_shader);
        if shown(new) != shown(&old) {
            self.display.apply(new)?;
        }
        // The 3D scene opened, closed or changed (a scene file is read again
        // once it is written again, an update downloaded in its place); the
        // first of what it has to say is shown, unless the machine has more
        // to say.
        let mut vr_note = None;
        if new.vr != old.vr || new.vr.scene.is_some() {
            for note in self.display.apply_vr(&new.vr) {
                self.cpu.bus.log_string(&note);
                vr_note.get_or_insert_with(|| note.trim_start_matches("[VR] ").to_string());
            }
        }
        if new.cycles != old.cycles {
            self.pacer.set_speed(new.cycles);
            self.cpu.set_tsc_speed(new.cycles);
            // At max, the pacer tunes the speed from the current one, and
            // at auto, it picks the speed at the end of the frame.
            if let CpuSpeed::Fixed(n) = new.cycles {
                self.cpu.bus.set_cycles_per_ms(n);
            }
        }
        if new.auto_fps != old.auto_fps {
            self.pacer.set_auto_fps(new.auto_fps);
        }
        if new.auto_busy_os != old.auto_busy_os {
            self.pacer.set_busy_os(new.auto_busy_os);
        }
        if new.core != old.core {
            self.cpu.core = new.core;
        }
        self.cpu.set_fpu_fast(new.fpu_fast);
        self.cpu.bus.observe.enabled = new.idle_skip;
        self.cpu.bus.idle_hint = new.idle_hint;
        // Programs look for the host as they start.
        self.cpu.bus.dpmi.enabled = new.dpmi;
        // Glide's DOS overlay, once downloaded.
        rust_dos::voodoo::overlay::provide(&mut self.cpu.bus);
        self.cpu.bus.ide_hard_disks = new.ide_hard_disks;
        self.cpu.bus.boot_cdrom = new.boot_cdrom;
        self.cpu.bus.floppy_controller = new.floppy_controller;
        if new.dos_version != old.dos_version {
            rust_dos::dos_data::set_version(&mut self.cpu.bus, new.dos_version);
        }
        if new.disk != old.disk {
            self.cpu.bus.set_disk_settings(new.disk);
        }
        if new.mixer != old.mixer {
            self.cpu.bus.set_mixer(new.mixer);
        }
        if new.audio_buffer != old.audio_buffer
            && let Some(device) = &mut self.cpu.bus.audio_device
        {
            device.set_buffer(new.audio_buffer)?;
        }
        if new.joystick != old.joystick {
            self.cpu.bus.set_joystick(new.joystick);
        }
        if new.composite != old.composite {
            self.cpu.bus.vga.set_composite(new.composite);
        }
        if new.keyboard_layout != old.keyboard_layout {
            apply_keyboard_layout(self.cpu, new.keyboard_layout);
        }
        if new.achievements != old.achievements {
            self.achievements.apply(&new.achievements);
        }
        if new.shell != old.shell {
            rust_dos::cmdline::configure(self.cpu, &new.shell);
        }
        if !self.machine.differs(new) {
            return Ok(vr_note);
        }
        if !self.cpu.shell_idle() {
            return Ok(Some(config_ui::pending_note(self.machine.video, new).to_string()));
        }
        match self.machine.apply(self.cpu, new).into_iter().next() {
            Some(problem) => Err(problem),
            None => Ok(None),
        }
    }

    fn mount(&mut self, spec: MountSpec, replace: bool) -> Result<PathBuf, String> {
        self.cpu.bus.log_string(&format!(
            "[CONFIG] Settings window: mount {} {}",
            disk::drive_name(spec.drive),
            spec.path.display()
        ));
        self.cpu.bus.mount_drive(spec.drive, &spec.path, spec.opts, replace)
    }

    fn unmount(&mut self, drive: u8) -> Result<(), String> {
        self.cpu.bus.unmount_drive(drive)
    }

    fn drives(&self) -> Vec<DriveInfo> {
        self.cpu.bus.disk.all_drives()
    }

    fn booted(&self) -> Option<config_ui::BootView> {
        let bus = &self.cpu.bus;
        bus.boot.as_ref()?;
        let shared =
            bus.disk.shared_drives().into_iter().filter_map(|d| Some((d, rust_dos::boot::drive_unit(bus, d)?))).collect();
        Some(config_ui::BootView { cd_drive: bus.booted_cd_drive(), shared })
    }

    fn reset_machine(&mut self) {
        self.cpu.bus.reset_requested = true;
    }

    fn copy_text(&mut self, text: &str) -> Result<(), String> {
        self.clipboard.set_clipboard_text(text)
    }

    fn sync_shared(&mut self, drive: u8) -> Result<String, String> {
        self.cpu.bus.sync_shared(Some(drive)).into_iter().next().ok_or_else(|| "Nothing is shared".to_string())
    }

    fn reinsert(&mut self, drive: u8) -> Result<String, String> {
        self.cpu.bus.reinsert_cd(drive)
    }

    fn center_vr(&mut self) -> Result<String, String> {
        if !self.display.has_headset() {
            return Err("No VR headset shows the scene".to_string());
        }
        self.display.recenter();
        Ok("View centered where your head is".to_string())
    }

    fn mark_vr_keyboard(&mut self) -> Result<String, String> {
        if !self.display.mark_vr_keyboard() {
            return Err("No VR headset shows the scene".to_string());
        }
        Ok("In the headset: hold the trigger along the keyboard's front edge, then click at its back edge (grip cancels)".to_string())
    }

    fn boot(&mut self, drive: u8) -> Result<String, String> {
        self.cpu.bus.log_string(&format!("[CONFIG] Settings window: boot {}", disk::drive_name(drive)));
        rust_dos::boot::boot_drive(self.cpu, drive)?;
        Ok(format!("Booting from drive {}", disk::drive_name(drive)))
    }

    fn set_boot(&mut self, drive: u8, boot: bool) -> Result<(), String> {
        self.cpu.bus.disk.set_boots(drive, boot);
        Ok(())
    }

    fn save(&mut self, settings: &Settings) -> Result<(), String> {
        // While a game plays, into its profile.
        if let Some(game) = self.game.as_mut() {
            let dir = games_dir(self.saved.file.as_deref()).ok_or("No configuration file")?;
            let path = dir.join(format!("{}.conf", game.id));
            config::save(&path, &game.saved, settings, &[], rust_dos::hostdirs::home_dir().as_deref(), config::Saving::Changes)?;
            game.saved = settings.clone();
            return Ok(());
        }
        save_config(self.cpu, self.saved, settings)
    }

    fn autoexec(&self) -> Result<Vec<String>, String> {
        config::load_autoexec(&self.config_path()?)
    }

    fn save_autoexec(&mut self, lines: &[String]) -> Result<(), String> {
        let path = self.config_path()?;
        config::save_autoexec(&path, lines)?;
        self.cpu.bus.log_string(&format!("[CONFIG] Saved the [autoexec] commands to {}", path.display()));
        Ok(())
    }

    fn lan(&self) -> Option<rust_dos::net::LanView> {
        Some(self.cpu.bus.net.view())
    }

    fn browse_rooms(&mut self, relay: Option<&str>, filter: &str) -> Result<(), String> {
        self.cpu.bus.net.browse(relay, filter)
    }

    fn join_room(&mut self, relay: Option<&str>, room: &str, password: &str) -> Result<(), String> {
        // As LAN JOIN does, for the games started next.
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

    fn games(&self) -> Vec<GameEntry> {
        games_dir(self.saved.file.as_deref()).map_or_else(Vec::new, |dir| games::list(&dir).into_iter().map(|(e, _)| e).collect())
    }

    fn active_game(&self) -> Option<String> {
        self.game.as_ref().map(|g| g.id.clone())
    }

    fn program_running(&self) -> bool {
        !self.cpu.shell_idle() || self.cpu.batch.is_active() || self.cpu.shell_wait.is_some()
    }

    fn close_program(&mut self) {
        self.release_program();
        self.cpu.close_program();
        self.cpu.load_shell();
    }

    fn launch_game(&mut self, id: &str) -> Result<String, String> {
        // The window asks whether to close the program running first.
        if self.program_running() {
            *self.confirm_launch = Some(id.to_string());
            return Ok("Choose whether to close the running program".to_string());
        }
        let (text, dir) = self.game_profile(id)?;
        // One with launch configurations: the window offers them.
        if let Some(choices) = games::launch_choices(&dir, &text) {
            *self.choose = Some(id.to_string());
            return Ok(format!("Choose how to start {}", choices.name));
        }
        self.start_game(id, &text, &dir)
    }

    fn game_pad(&self, id: &str) -> Vec<(String, String)> {
        let Ok((text, dir)) = self.game_profile(id) else { return Vec::new() };
        let lines = rust_dos::config::parse(&text, &dir, None).game_pad;
        rust_dos::padmap::PadMapping::parse(&lines).0.map(|m| m.labels()).unwrap_or_default()
    }

    fn launch_choices(&self, id: &str) -> Option<games::LaunchChoices> {
        let (text, dir) = self.game_profile(id).ok()?;
        games::launch_choices(&dir, &text)
    }

    fn launch_variant(&mut self, id: &str, variant: Option<&str>, tool: bool) -> Result<String, String> {
        let (text, dir) = self.game_profile(id)?;
        let text = match variant {
            Some(variant) => games::variant_profile(&dir, &text, variant)?,
            None => text,
        };
        let message = self.start_game(id, &text, &dir)?;
        if let Some(game) = self.game.as_mut() {
            game.choose_after = tool;
        }
        Ok(message)
    }

    fn create_game(&mut self, new: &NewGame, settings: &Settings) -> Result<String, String> {
        let dir = games_dir(self.saved.file.as_deref())
            .ok_or("Game profiles go beside the configuration file, and there is none (--no-config)")?;
        let taken: Vec<String> = games::list(&dir).into_iter().map(|(e, _)| e.id).collect();
        let id = games::slug(&new.name, &taken);
        let base = self.game.as_ref().map_or(&self.saved.settings, |g| &g.base);
        let drives = drive_changes(self.cpu, self.saved);
        let text = games::profile_text(new, base, settings, &drives, rust_dos::hostdirs::home_dir().as_deref())?;
        let path = dir.join(format!("{}.conf", id));
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&path, text))
            .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
        self.cpu.bus.log_string(&format!("[CONFIG] Made the game profile {}", path.display()));
        Ok(id)
    }

    fn delete_game(&mut self, id: &str) -> Result<(), String> {
        let dir = games_dir(self.saved.file.as_deref()).ok_or("There is no games folder")?;
        let path = dir.join(format!("{}.conf", id));
        std::fs::remove_file(&path).map_err(|e| format!("cannot delete {}: {}", path.display(), e))
    }

    fn reset_game(&mut self, id: &str) -> Result<(), String> {
        if self.game.as_ref().is_some_and(|g| g.id == id) {
            return Err("The game is running: reset it once it has ended".to_string());
        }
        let dir = games_dir(self.saved.file.as_deref()).ok_or("There is no games folder")?;
        games::reset(&games::saves_dir(&dir), id)
    }

    fn import_game(&mut self, source: &std::path::Path) -> Result<(String, String), String> {
        let dir = games_dir(self.saved.file.as_deref())
            .ok_or("Game profiles go beside the configuration file, and there is none (--no-config)")?;
        let (id, name, warnings) = games::import(&dir, source, rust_dos::hostdirs::home_dir().as_deref())?;
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
        games::prompt_directory(self.cpu)
    }

    fn memory(&self) -> &[u8] {
        self.cpu.bus.ram()
    }

    fn cheats_allowed(&self) -> Result<(), String> {
        if self.achievements.hardcore_active() {
            return Err("Hardcore mode: no cheats while RetroAchievements plays".to_string());
        }
        Ok(())
    }

    fn achievements(&self) -> Option<rust_dos::achievements::AchievementsView> {
        Some(self.achievements.view())
    }

    fn achievements_login(&mut self, username: &str, password: &str) -> Result<(), String> {
        self.achievements.login(username, password);
        Ok(())
    }

    fn achievements_logout(&mut self) {
        self.achievements.logout();
    }

    fn identify_game(&mut self, archive: &std::path::Path) -> Result<String, String> {
        let Some(game) = self.game.as_ref() else {
            return Err("Launch the game from its profile first (the Games page)".to_string());
        };
        let hash = rust_dos::achievements::hash::hash_archive(archive)?;
        let dir = games_dir(self.saved.file.as_deref()).ok_or("There is no games folder")?;
        let path = dir.join(format!("{}.conf", game.id));
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
        std::fs::write(&path, games::set_achievements(&text, &hash)).map_err(|e| format!("{}: {}", path.display(), e))?;
        let name = game.name.clone();
        self.achievements.game_started(Some(hash.clone()), &name);
        Ok(format!("{} is known by {} now (hash {})", name, archive.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned()), hash))
    }

    fn manuals(&self, id: &str) -> Vec<rust_dos::manuals::Manual> {
        let Some(dir) = games_dir(self.saved.file.as_deref()) else { return Vec::new() };
        let text = std::fs::read_to_string(dir.join(format!("{}.conf", id))).unwrap_or_default();
        games::manuals(&dir, id, &text, rust_dos::hostdirs::home_dir().as_deref())
    }

    fn add_manual(&mut self, id: &str, path: &std::path::Path) -> Result<(), String> {
        let dir = games_dir(self.saved.file.as_deref()).ok_or("There is no games folder")?;
        games::add_manual_file(&dir, id, path, rust_dos::hostdirs::home_dir().as_deref())
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

    fn states_available(&self) -> bool {
        self.states.is_some()
    }

    fn states(&self) -> Vec<config_ui::SlotView> {
        let Ok(dir) = self.slot_dir() else { return Vec::new() };
        slots::list(&dir)
            .into_iter()
            .map(|(slot, header, picture)| config_ui::SlotView {
                slot,
                header: Some(header),
                picture: capture::png::decode(&picture),
            })
            .collect()
    }

    fn current_slot(&self) -> u8 {
        *self.slot
    }

    fn save_state(&mut self, slot: u8) -> Result<String, String> {
        let path = slots::slot_path(&self.slot_dir()?, slot);
        self.save_file(&path, false)?;
        *self.slot = slot;
        Ok(format!("Saved to slot {}", slot))
    }

    fn load_state(&mut self, slot: u8) -> Result<String, String> {
        let header = self.load_slot(slot)?;
        *self.slot = slot;
        Ok(format!("Loaded slot {}: {}", slot, describe_state(&header)))
    }

    fn delete_state(&mut self, slot: u8) -> Result<(), String> {
        let path = slots::slot_path(&self.slot_dir()?, slot);
        savestate::disks::delete_copies(&path);
        std::fs::remove_file(&path).map_err(|e| format!("{}: {}", path.display(), e))
    }
}
