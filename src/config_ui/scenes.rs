//! The VR page's scene list, opened with Enter on *Scene*: the built-in
//! room and the scenes downloaded, to pick from; picking a file instead;
//! and, once the user asks rust-dos.com, the scenes it lists, which Enter
//! downloads. Nothing is asked for until then. The list and a download
//! outlive the list being closed (`ConfigUi::scene_catalog`,
//! `ConfigUi::scene_download`).

use super::draw::{self, Grid};
use super::rooms::wrap;
use super::{ConfigUi, Hit, Host, Item, Pick, Target, UiKey, fit};
use crate::vr_scenes::{Downloaded, SceneInfo};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

/// rust-dos.com's list of scenes: not asked for yet, being asked for, or
/// what came.
#[derive(Default)]
pub(super) enum Catalog {
    #[default]
    NotAsked,
    Asking(Receiver<Result<Vec<SceneInfo>, String>>),
    Got(Result<Vec<SceneInfo>, String>),
}

/// A scene being downloaded: which, how much has come, and the file to
/// load once it has.
pub(super) struct SceneDownload {
    info: SceneInfo,
    progress: Arc<AtomicU64>,
    result: Receiver<Result<PathBuf, String>>,
}

/// A line of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Line {
    Heading(&'static str),
    TestRoom,
    Downloaded(usize),
    /// The scene chosen, when it is a file of the user's.
    File,
    Browse,
    Ask,
    Online(usize),
    /// Why there are no scenes online to show.
    Note(String),
}

impl Line {
    fn selectable(&self) -> bool {
        !matches!(self, Line::Heading(_) | Line::Note(_))
    }
}

pub struct SceneList {
    downloaded: Vec<Downloaded>,
    pub(super) selected: usize,
    scroll: usize,
    /// The downloaded scene Del deletes once Enter says so.
    confirm_delete: Option<String>,
}

/// Whether this rust-dos downloads.
const DOWNLOADS: bool = cfg!(all(feature = "sdl", not(target_arch = "wasm32")));

impl ConfigUi {
    /// Enter on *Scene*: the list, on the scene chosen now.
    pub(super) fn open_scenes(&mut self) {
        self.status = None;
        self.scenes = Some(SceneList { downloaded: crate::vr_scenes::downloaded(), selected: 0, scroll: 0, confirm_delete: None });
        self.select_chosen_scene();
    }

    /// Select the line of the scene chosen.
    fn select_chosen_scene(&mut self) {
        let Some(list) = &self.scenes else { return };
        let chosen = match &self.settings.vr.scene {
            None => Line::TestRoom,
            Some(path) => list.downloaded.iter().position(|d| d.path == *path).map_or(Line::File, Line::Downloaded),
        };
        let lines = self.scene_lines(list);
        let at = lines.iter().position(|l| *l == chosen).unwrap_or(1);
        if let Some(list) = &mut self.scenes {
            list.selected = at;
        }
    }

    /// The scene chosen when it is a file of the user's, not downloaded.
    fn chosen_file(&self, list: &SceneList) -> Option<PathBuf> {
        let path = self.settings.vr.scene.as_ref()?;
        (!list.downloaded.iter().any(|d| d.path == *path)).then(|| path.clone())
    }

    fn online(&self) -> &[SceneInfo] {
        match &self.scene_catalog {
            Catalog::Got(Ok(scenes)) => scenes,
            _ => &[],
        }
    }

    pub(super) fn scene_lines(&self, list: &SceneList) -> Vec<Line> {
        let mut lines = vec![Line::Heading("On this computer"), Line::TestRoom];
        lines.extend((0..list.downloaded.len()).map(Line::Downloaded));
        if self.chosen_file(list).is_some() {
            lines.push(Line::File);
        }
        lines.push(Line::Browse);
        if DOWNLOADS {
            lines.push(Line::Heading("On rust-dos.com"));
            lines.push(Line::Ask);
            match &self.scene_catalog {
                Catalog::Got(Ok(scenes)) if scenes.is_empty() => lines.push(Line::Note("It lists no scenes yet".into())),
                Catalog::Got(Ok(scenes)) => lines.extend((0..scenes.len()).map(Line::Online)),
                Catalog::Got(Err(e)) => lines.push(Line::Note(e.clone())),
                _ => {}
            }
        }
        lines
    }

    /// The list, and the download, as they come.
    pub(super) fn poll_scenes(&mut self, host: &mut dyn Host) {
        if let Catalog::Asking(result) = &self.scene_catalog {
            match result.try_recv() {
                Ok(got) => self.scene_catalog = Catalog::Got(got),
                Err(TryRecvError::Disconnected) => self.scene_catalog = Catalog::Got(Err("the question stopped".into())),
                Err(TryRecvError::Empty) => {}
            }
        }
        let Some(download) = &self.scene_download else { return };
        let outcome = match download.result.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the download stopped".to_string()),
        };
        let name = download.info.name.clone();
        self.scene_download = None;
        match outcome {
            Ok(path) => {
                if let Some(list) = &mut self.scenes {
                    list.downloaded = crate::vr_scenes::downloaded();
                }
                self.settings.vr.scene = Some(path);
                self.changed(Item::VrScene, host);
                self.select_chosen_scene();
                let shown = self.shown_now();
                self.info(format!("{} is downloaded and {}", name, shown));
            }
            Err(e) => self.error(format!("{}: {}", name, e)),
        }
    }

    /// Ask rust-dos.com for its scenes, on a thread of its own.
    fn ask_for_scenes(&mut self) {
        if matches!(self.scene_catalog, Catalog::Asking(_)) {
            return;
        }
        let (done, result) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("vr-scenes".to_string()).spawn(move || {
            let _ = done.send(crate::vr_scenes::fetch_catalog());
        });
        match spawned {
            Ok(_) => {
                self.scene_catalog = Catalog::Asking(result);
                self.status = None;
            }
            Err(e) => self.error(format!("Can't ask rust-dos.com: {}", e)),
        }
    }

    /// Download `info`'s files, on a thread of its own.
    fn download_scene(&mut self, info: SceneInfo) {
        if let Some(download) = &self.scene_download {
            return self.info(format!("{} is downloading...", download.info.name));
        }
        if let Some(version) = info.needs() {
            return self.error(format!("{} needs Rust-DOS {} or later", info.name, version));
        }
        let progress = Arc::new(AtomicU64::new(0));
        let (done, result) = std::sync::mpsc::channel();
        let (scene, counted) = (info.clone(), progress.clone());
        let spawned = std::thread::Builder::new().name("vr-scene-download".to_string()).spawn(move || {
            let _ = done.send(crate::vr_scenes::download(&scene, &counted));
        });
        match spawned {
            Ok(_) => {
                self.info(format!("Downloading {} ({})...", info.name, megabytes(info.size())));
                self.scene_download = Some(SceneDownload { info, progress, result });
            }
            Err(e) => self.error(format!("The download didn't start: {}", e)),
        }
    }

    fn choose_scene(&mut self, path: Option<PathBuf>, name: &str, host: &mut dyn Host) {
        self.scenes = None;
        self.settings.vr.scene = path;
        self.changed(Item::VrScene, host);
        let shown = self.shown_now();
        self.info(format!("{} is {}", name, shown));
    }

    pub(super) fn scenes_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(list) = &self.scenes else { return };
        let lines = self.scene_lines(list);
        let selected = list.selected.min(lines.len() - 1);
        let downloaded = list.downloaded.clone();
        if let Some(id) = self.scenes.as_mut().and_then(|list| list.confirm_delete.take()) {
            if key == UiKey::Enter {
                self.delete_scene(&id, &downloaded, host);
            } else {
                self.status = None;
            }
            return;
        }
        match key {
            UiKey::Up | UiKey::Down | UiKey::PageUp | UiKey::PageDown | UiKey::Home | UiKey::End => {
                let page = self.visible.saturating_sub(8).max(1);
                let last = lines.len() - 1;
                let next = match key {
                    UiKey::Up => (0..selected).rev().find(|&i| lines[i].selectable()),
                    UiKey::Down => (selected + 1..=last).find(|&i| lines[i].selectable()),
                    UiKey::PageUp => (0..=selected.saturating_sub(page)).rev().find(|&i| lines[i].selectable()),
                    UiKey::PageDown => (selected..=(selected + page).min(last)).rev().find(|&i| lines[i].selectable()),
                    UiKey::Home => lines.iter().position(Line::selectable),
                    _ => lines.iter().rposition(Line::selectable),
                };
                if let (Some(next), Some(list)) = (next, &mut self.scenes) {
                    list.selected = next;
                }
            }
            UiKey::Esc => {
                self.scenes = None;
                self.status = None;
            }
            UiKey::Delete => {
                if let Line::Downloaded(i) = lines[selected] {
                    let scene = &downloaded[i];
                    if let Some(list) = &mut self.scenes {
                        list.confirm_delete = Some(scene.info.id.clone());
                    }
                    self.error(format!("Delete {} from this computer?", scene.info.name));
                }
            }
            UiKey::Enter => match &lines[selected] {
                Line::TestRoom => self.choose_scene(None, "The test room", host),
                Line::Downloaded(i) => {
                    let scene = downloaded[*i].clone();
                    self.choose_scene(Some(scene.path), &scene.info.name, host);
                }
                Line::File => {
                    self.scenes = None;
                    self.status = None;
                }
                Line::Browse => {
                    self.scenes = None;
                    self.open_browser(Pick::VrScene);
                }
                Line::Ask => self.ask_for_scenes(),
                Line::Online(i) => {
                    let Some(info) = self.online().get(*i).cloned() else { return };
                    match downloaded.into_iter().find(|d| d.info.id == info.id) {
                        Some(have) if have.info.version == info.version => {
                            self.choose_scene(Some(have.path), &have.info.name, host);
                        }
                        _ => self.download_scene(info),
                    }
                }
                Line::Heading(_) | Line::Note(_) => {}
            },
            _ => {}
        }
    }

    /// Delete the downloaded scene `id`; the test room takes its place if
    /// it was chosen.
    fn delete_scene(&mut self, id: &str, downloaded: &[Downloaded], host: &mut dyn Host) {
        if let Err(e) = crate::vr_scenes::delete(id) {
            return self.error(e);
        }
        if let Some(list) = &mut self.scenes {
            list.downloaded = crate::vr_scenes::downloaded();
        }
        let Some(gone) = downloaded.iter().find(|d| d.info.id == id) else { return self.status = None };
        // Its line is gone: the chosen one's then.
        let chosen = self.settings.vr.scene.as_ref() == Some(&gone.path);
        if chosen {
            self.settings.vr.scene = None;
        }
        self.select_chosen_scene();
        if chosen {
            self.changed(Item::VrScene, host);
            self.info(format!("{} is deleted: the test room shows instead", gone.info.name));
        } else {
            self.info(format!("{} is deleted", gone.info.name));
        }
    }

    /// A click on a line: the selected one is as Enter.
    pub(super) fn scene_clicked(&mut self, target: Target, host: &mut dyn Host) {
        let Target::SceneRow(i) = target else { return };
        let Some(list) = &mut self.scenes else { return };
        if list.selected == i {
            self.key(UiKey::Enter, host);
        } else {
            list.selected = i;
            list.confirm_delete = None;
        }
    }

    pub(super) fn scene_hints(&self) -> Vec<(&'static str, &'static str, UiKey)> {
        let Some(list) = &self.scenes else { return Vec::new() };
        if list.confirm_delete.is_some() {
            return vec![("Enter", "Delete", UiKey::Enter), ("Esc", "Keep", UiKey::Esc)];
        }
        let lines = self.scene_lines(list);
        let mut hints = match lines.get(list.selected) {
            Some(Line::Online(i)) => {
                let have = self.online().get(*i).and_then(|info| {
                    list.downloaded.iter().find(|d| d.info.id == info.id && d.info.version == info.version)
                });
                vec![("Enter", if have.is_some() { "Choose" } else { "Download" }, UiKey::Enter)]
            }
            Some(Line::Ask) => vec![("Enter", "Ask", UiKey::Enter)],
            Some(Line::Browse) => vec![("Enter", "Browse", UiKey::Enter)],
            Some(Line::Downloaded(_)) => vec![("Enter", "Choose", UiKey::Enter), ("Del", "Delete", UiKey::Delete)],
            _ => vec![("Enter", "Choose", UiKey::Enter)],
        };
        hints.push(("Esc", "Back", UiKey::Esc));
        hints
    }

    pub(super) fn draw_scenes(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some(list) = &self.scenes else { return };
        let lines = self.scene_lines(list);
        let cols = g.cols;
        let chosen = self.settings.vr.scene.clone();
        let file = self.chosen_file(list);
        let downloading = self.scene_download.as_ref().map(|d| (d.info.id.clone(), d.progress.load(Ordering::Relaxed), d.info.size()));
        let online = self.online().to_vec();
        let asking = matches!(self.scene_catalog, Catalog::Asking(_));
        let asked = matches!(self.scene_catalog, Catalog::Got(Ok(_)));
        let Some(list) = &mut self.scenes else { return };
        list.selected = list.selected.min(lines.len() - 1);
        let top = content.start;
        g.text_to(2, top, "Choose VR Scene", draw::BRIGHT, cols - 2);

        // What the line selected is, at the bottom.
        let about_rows = 5;
        let about_top = content.end.saturating_sub(about_rows);
        let info = match &lines[list.selected] {
            Line::Downloaded(i) => Some(&list.downloaded[*i].info),
            Line::Online(i) => online.get(*i),
            _ => None,
        };
        let about = match (&lines[list.selected], info) {
            (_, Some(info)) => {
                let parts = [
                    info.description.clone(),
                    (!info.author.is_empty()).then(|| format!("By {}.", info.author)).unwrap_or_default(),
                    (!info.license.is_empty()).then(|| format!("Licence: {}.", info.license)).unwrap_or_default(),
                    info.homepage.clone(),
                ];
                parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join("  ")
            }
            (Line::TestRoom, _) => "Gaming in the void.".to_string(),
            (Line::File, _) => file.as_ref().map_or(String::new(), |path| {
                format!("Your file {}", super::contract_home(path, self.home.as_deref()))
            }),
            (Line::Browse, _) => "A .glb or .gltf file of your own, exported from Blender. See docs/vr.md for instructions.".to_string(),
            (Line::Ask, _) => format!("Fetch a list of scenes from {}. No other information is shared.", crate::vr_scenes::CATALOG_URL),
            _ => String::new(),
        };
        for (line, row) in wrap(about.trim(), cols - 4, about_rows).into_iter().zip(about_top..content.end) {
            g.text_to(2, row, &line, draw::DIM, cols - 2);
        }

        let rows = top + 2..about_top.saturating_sub(1);
        Self::keep_visible(&mut list.scroll, list.selected, rows.len());
        let size_col = cols.saturating_sub(24);
        let mut hits = Vec::new();
        for (i, row) in (list.scroll..lines.len()).zip(rows.clone()) {
            let selected = i == list.selected;
            if selected {
                g.background(1, row, cols - 2, draw::SELECT);
            }
            if lines[i].selectable() {
                hits.push(Hit { row, col: 1, width: cols - 2, target: Target::SceneRow(i) });
            }
            let (text, color, note, note_color) = match &lines[i] {
                Line::Heading(title) => (title.to_string(), draw::NOTE, String::new(), draw::DIM),
                Line::TestRoom => {
                    let note = if chosen.is_none() { "active" } else { "" };
                    ("  The test room".to_string(), draw::BRIGHT, note.to_string(), draw::GOOD)
                }
                Line::Downloaded(d) => {
                    let scene = &list.downloaded[*d];
                    let note = if chosen.as_ref() == Some(&scene.path) { "active" } else { "" };
                    (format!("  {}", scene.info.name), draw::BRIGHT, note.to_string(), draw::GOOD)
                }
                Line::File => {
                    let name = file.as_deref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
                    (format!("  {}", name.unwrap_or_default()), draw::BRIGHT, "active".to_string(), draw::GOOD)
                }
                Line::Browse => ("  Browse for a scene file...".to_string(), draw::KEY, String::new(), draw::DIM),
                Line::Ask if asking => ("  Fetching scenes...".to_string(), draw::DIM, String::new(), draw::DIM),
                Line::Ask if asked => ("  Refresh".to_string(), draw::KEY, String::new(), draw::DIM),
                Line::Ask => ("  Download scenes...".to_string(), draw::KEY, String::new(), draw::DIM),
                Line::Note(text) => (format!("  {}", text), draw::ERROR, String::new(), draw::DIM),
                Line::Online(o) => {
                    let info = &online[*o];
                    let have = list.downloaded.iter().find(|d| d.info.id == info.id);
                    let (note, note_color) = match (&downloading, have, info.needs()) {
                        (Some((id, done, total)), _, _) if *id == info.id => {
                            (format!("{}%", (done * 100 / (*total).max(1)).min(100)), draw::NOTE)
                        }
                        (_, Some(have), _) if have.info.version == info.version => ("downloaded".to_string(), draw::GOOD),
                        (_, Some(_), _) => (format!("update, {}", megabytes(info.size())), draw::NOTE),
                        (_, None, Some(version)) => (format!("needs {}", version), draw::ERROR),
                        (_, None, None) => (megabytes(info.size()), draw::TEXT),
                    };
                    (format!("  {}", info.name), draw::BRIGHT, note, note_color)
                }
            };
            let color = if selected && lines[i].selectable() { draw::BRIGHT } else { color };
            g.text_to(2, row, &fit(&text, size_col.saturating_sub(3)), color, size_col.saturating_sub(1));
            g.text_to(size_col, row, &note, note_color, cols - 2);
        }
        let (scroll, total) = (list.scroll, lines.len());
        self.hits.extend(hits);
        self.draw_scrollbar(g, rows, scroll, total);
    }
}

impl ConfigUi {
    /// Where a scene chosen shows.
    fn shown_now(&self) -> &'static str {
        if self.settings.vr.mode == crate::vr::VrMode::Off {
            "chosen (F2 keeps it)"
        } else {
            "active: This scene is currently being rendered"
        }
    }
}

/// `bytes` as megabytes, to a tenth.
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}
