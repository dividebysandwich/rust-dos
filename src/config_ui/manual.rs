//! The manuals and extras of a game (manuals.rs), shown over it: a list
//! of them in the settings window, then the one picked over the whole
//! picture, page by page, for the copy protection's questions and the
//! rest. Ctrl+Shift+M opens the running game's; M on the Games page the
//! selected game's.
//!
//! Each manual keeps its place (page, zoom and scroll) while Rust-DOS
//! runs, and each game the manual it had open: opened again, the game's
//! manuals open on it, where it was left (`Places`).
//!
//! The page is drawn into the picture, as the rest of the window is, or,
//! where the frontend can (`set_display`), handed to it as a layer of its
//! own (`ConfigUi::layer`), sharp at the window's size. Either way it is
//! as wide and tall as it should be where the picture's pixels aren't
//! square.

use super::draw::{self, Grid, Layout};
use super::{ConfigUi, Hit, Host, Pick, Target, UiKey, fit};
use crate::manuals::{self, Document, Manual};
use crate::video::Frame;
use std::collections::HashMap;
use std::path::PathBuf;

/// How far a page is zoomed: the whole page, as wide as the picture, and
/// half as wide again and twice as wide.
const ZOOMS: [&str; 4] = ["Page", "Width", "150%", "200%"];

/// A picture over the frontend's, `rect` (x, y, width, height) in the
/// picture's pixels: a page, at more pixels than the picture has.
#[derive(Clone, Debug)]
pub struct Layer {
    pub rect: (f32, f32, f32, f32),
    pub picture: Frame,
    /// Changes with each new layer.
    pub generation: u64,
}

/// A game's manuals, and the one open.
pub struct ManualView {
    id: String,
    name: String,
    manuals: Vec<Manual>,
    row: usize,
    scroll: usize,
    open: Option<OpenManual>,
    /// Opened with Ctrl+Shift+M: Esc in the list closes the window.
    alone: bool,
}

/// A manual's place: its page, zoom, and how far down and right.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Place {
    page: usize,
    zoom: usize,
    at: (f32, f32),
}

/// Where the manuals were left: each one's place, the one each game had
/// open, and the last one open, kept as it was so it opens again at once.
#[derive(Default)]
pub(super) struct Places {
    places: HashMap<PathBuf, Place>,
    last: HashMap<String, PathBuf>,
    kept: Option<OpenManual>,
}

pub(super) struct OpenManual {
    path: PathBuf,
    doc: Document,
    title: String,
    page: usize,
    zoom: usize,
    /// How far down and right the view is, of as far as it goes.
    at: (f32, f32),
    /// The page rendered: its number and size, and it.
    rendered: Option<((usize, u32, u32), Frame)>,
    /// What the picture or layer last showed, to know when it changes.
    shown: Option<Shown>,
}

/// A page shown: its number and zoom, the size it was rendered at, and
/// the part of it shown (x, y, width, height).
type Shown = (usize, usize, (u32, u32), (u32, u32, u32, u32));

impl ManualView {
    fn manual(&self) -> Option<&Manual> {
        self.manuals.get(self.row)
    }

    /// A click on row `i` of the list: selects it, or (true) it was.
    pub(super) fn select(&mut self, i: usize) -> bool {
        std::mem::replace(&mut self.row, i) == i
    }
}

impl ConfigUi {
    /// Open the manuals of the game `id` (named `name`): in the list, or
    /// the one there is, open. `alone` when the window opened for them.
    pub(super) fn open_manuals(&mut self, id: &str, name: &str, host: &dyn Host, alone: bool) {
        self.put_away_manual();
        let manuals = host.manuals(id);
        // The one the game had open, else the first.
        let last = self.manual_places.last.get(id).and_then(|path| manuals.iter().position(|m| m.path == *path));
        let row = last.unwrap_or(0);
        self.manual = Some(ManualView { id: id.to_string(), name: name.to_string(), manuals, row, scroll: 0, open: None, alone });
        self.status = None;
        if last.is_some() || self.manual.as_ref().is_some_and(|m| m.manuals.len() == 1) {
            self.open_manual();
        }
    }

    /// The manual open closed: where it was left is kept, and it too.
    pub(super) fn put_away_manual(&mut self) {
        let Some(view) = &mut self.manual else { return };
        let Some(mut open) = view.open.take() else { return };
        let place = Place { page: open.page, zoom: open.zoom, at: open.at };
        self.manual_places.places.insert(open.path.clone(), place);
        self.manual_places.last.insert(view.id.clone(), open.path.clone());
        open.shown = None;
        self.manual_places.kept = Some(open);
    }

    /// Open the running game's manuals (Ctrl+Shift+M), with the window
    /// opened for them. Returns what to tell the user if there are none.
    pub fn show_manuals(&mut self, host: &dyn Host) -> Result<(), String> {
        let Some(id) = host.active_game() else {
            return Err("No game is playing: its manuals are on the Games page (M)".to_string());
        };
        let name = host.games().into_iter().find(|g| g.id == id).map_or_else(|| id.clone(), |g| g.name);
        self.open_manuals(&id, &name, host, true);
        Ok(())
    }

    /// Whether a manual's page is shown over the whole picture, which
    /// screenshots and recordings show as they show the game.
    pub fn manual_shown(&self) -> bool {
        self.open && self.manual_page_shown()
    }

    /// Whether a page is shown over the whole picture.
    pub(super) fn manual_page_shown(&self) -> bool {
        self.help.is_none() && self.browser.is_none() && self.manual.as_ref().is_some_and(|m| m.open.is_some())
    }

    /// How the frontend shows the picture: each of its pixels `scale`
    /// (across, down) of the frontend's, and whether it draws the layer
    /// (`layer`), which has the page at its pixels, rather than having it
    /// drawn into the picture.
    pub fn set_display(&mut self, scale: (f64, f64), layer: bool) {
        self.pixel_scale = (scale.0.max(0.01) as f32, scale.1.max(0.01) as f32);
        self.layered = layer;
    }

    /// The page shown over the picture, for the frontend to draw sharp.
    pub fn layer(&self) -> Option<&Layer> {
        self.layer.as_ref().filter(|_| self.open && self.manual_page_shown())
    }

    /// The picture `screen` with the page over it as the frontend shows
    /// it, for screenshots and recordings: `scale` times its size (a pixel
    /// of `screen` is `scale` of the result's, across and down), the page
    /// at that size. None when no page is shown as a layer.
    pub fn with_layer(&mut self, screen: &Frame, scale: (f64, f64)) -> Option<Frame> {
        let (rect, generation) = self.layer().map(|l| (l.rect, l.generation))?;
        let (sx, sy) = (scale.0.max(0.01), scale.1.max(0.01));
        let size = (((screen.width as f64 * sx).round() as u32).max(1), ((screen.height as f64 * sy).round() as u32).max(1));
        let mut out = if size == (screen.width, screen.height) { screen.clone() } else { manuals::scale(screen, size.0, size.1) };
        let (x, y, w, h) = rect;
        let x0 = ((x as f64 * sx).round() as u32).min(size.0);
        let y0 = ((y as f64 * sy).round() as u32).min(size.1);
        let x1 = (((x + w) as f64 * sx).round() as u32).clamp(x0, size.0);
        let y1 = (((y + h) as f64 * sy).round() as u32).clamp(y0, size.1);
        let page = (x1 - x0, y1 - y0);
        if page.0 == 0 || page.1 == 0 {
            return Some(out);
        }
        // Scaled once for each layer and size.
        let key = (generation, page);
        if self.layer_scaled.as_ref().is_none_or(|(k, _)| *k != key) {
            let picture = &self.layer.as_ref()?.picture;
            let scaled = if (picture.width, picture.height) == page { picture.clone() } else { manuals::scale(picture, page.0, page.1) };
            self.layer_scaled = Some((key, scaled));
        }
        let scaled = &self.layer_scaled.as_ref().expect("the page scaled").1;
        let row = page.0 as usize * 3;
        for r in 0..page.1 as usize {
            let to = ((y0 as usize + r) * size.0 as usize + x0 as usize) * 3;
            out.rgb[to..to + row].copy_from_slice(&scaled.rgb[r * row..(r + 1) * row]);
        }
        Some(out)
    }

    fn open_manual(&mut self) {
        let Some(view) = &mut self.manual else { return };
        let Some(manual) = view.manual().cloned() else { return };
        let place = self.manual_places.places.get(&manual.path).copied().unwrap_or_default();
        // The last one open is as it was; others are opened again.
        let kept = self.manual_places.kept.take_if(|kept| kept.path == manual.path);
        let doc = match kept {
            Some(kept) => Ok(kept),
            None => Document::open(&manual.path).map(|doc| OpenManual {
                path: manual.path.clone(),
                doc,
                title: manual.title.clone(),
                page: 0,
                zoom: 0,
                at: (0.0, 0.0),
                rendered: None,
                shown: None,
            }),
        };
        match doc {
            Ok(mut open) => {
                open.title = manual.title;
                // A document that got shorter opens on its last page.
                open.page = place.page.min(open.doc.pages().saturating_sub(1));
                (open.zoom, open.at) = (place.zoom.min(ZOOMS.len() - 1), place.at);
                view.open = Some(open);
                self.status = None;
            }
            Err(e) => self.error(e),
        }
    }

    pub(super) fn manual_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(view) = &mut self.manual else { return };
        if let Some(open) = &mut view.open {
            let pages = open.doc.pages();
            let step = 0.25;
            match key {
                UiKey::PageDown | UiKey::Right | UiKey::Char(' ') if open.page + 1 < pages => {
                    open.page += 1;
                    open.at.1 = 0.0;
                }
                UiKey::PageUp | UiKey::Left if open.page > 0 => {
                    open.page -= 1;
                    open.at.1 = if key == UiKey::PageUp { 0.0 } else { 1.0 };
                }
                UiKey::Down => open.at.1 = (open.at.1 + step).min(1.0),
                UiKey::Up => open.at.1 = (open.at.1 - step).max(0.0),
                UiKey::Home => (open.page, open.at) = (0, (0.0, 0.0)),
                UiKey::End => (open.page, open.at) = (pages - 1, (0.0, 0.0)),
                UiKey::Char('+' | '=') => open.zoom = (open.zoom + 1).min(ZOOMS.len() - 1),
                UiKey::Char('-') => open.zoom = open.zoom.saturating_sub(1),
                UiKey::Char('[') => open.at.0 = (open.at.0 - step).max(0.0),
                UiKey::Char(']') => open.at.0 = (open.at.0 + step).min(1.0),
                UiKey::Esc | UiKey::Backspace if view.manuals.len() > 1 || !view.alone => self.put_away_manual(),
                UiKey::Esc | UiKey::Backspace => self.close(),
                _ => {}
            }
            return;
        }
        let count = view.manuals.len();
        // The row after the manuals adds one.
        if let Some(row) = Self::navigate(key, view.row, count + 1, self.visible) {
            view.row = row;
            return;
        }
        match key {
            UiKey::Enter if view.row < count => self.open_manual(),
            UiKey::Enter | UiKey::Insert => self.open_browser(Pick::Manual),
            UiKey::Esc if view.alone => self.close(),
            UiKey::Esc => {
                self.put_away_manual();
                self.manual = None;
                self.status = None;
                self.refresh_games(host);
            }
            _ => {}
        }
    }

    /// A document or picture picked to be one of the game's manuals.
    pub(super) fn add_manual(&mut self, path: &std::path::Path, host: &mut dyn Host) {
        let Some(id) = self.manual.as_ref().map(|m| m.id.clone()) else { return };
        match host.add_manual(&id, path) {
            Ok(()) => {
                let view = self.manual.as_mut().expect("the manuals");
                view.manuals = host.manuals(&id);
                view.row = view.manuals.iter().position(|m| m.path == path).unwrap_or(view.row);
                let message = format!("{} is one of {}'s manuals now", manuals::title_of(path), view.name);
                self.info(message);
            }
            Err(e) => self.error(e),
        }
    }

    /// The list of manuals, in the window's rows `content`.
    pub(super) fn draw_manual_list(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some(view) = &mut self.manual else { return };
        let cols = g.cols;
        g.text_to(2, content.start, &format!("Manuals and extras of {}", view.name), draw::BRIGHT, cols - 2);
        let rows = content.start + 2..content.end;
        if view.manuals.is_empty() {
            let text = format!("None yet: Ins adds a PDF or a picture, or put them in games/{}.extras", view.id);
            g.text_to(2, content.start + 1, &fit(&text, cols - 4), draw::DIM, cols - 2);
        }
        Self::keep_visible(&mut view.scroll, view.row, rows.len());
        let path_col = 32.min(cols / 2);
        for (i, row) in (view.scroll..view.manuals.len() + 1).zip(rows) {
            if i == view.row {
                g.background(1, row, cols - 2, draw::SELECT);
            }
            self.hits.push(Hit { row, col: 1, width: cols - 2, target: Target::Row(i) });
            let Some(manual) = view.manuals.get(i) else {
                g.text(2, row, "+ Add a PDF or picture...", draw::KEY);
                continue;
            };
            g.text_to(2, row, &fit(&manual.title, path_col - 3), draw::BRIGHT, path_col - 1);
            let path = super::contract_home(&manual.path, self.home.as_deref());
            g.text_to(path_col, row, &fit(&path, cols - path_col - 2), draw::DIM, cols - 2);
        }
    }

    /// The manual open, over the whole picture: its title and page at the
    /// top, the keys at the bottom, the page between.
    pub(super) fn draw_manual_page(&mut self, frame: &mut Frame) {
        let cell_h = Layout::for_frame(frame.width as usize, frame.height as usize).cell_h;
        let (width, height) = (frame.width as usize, frame.height as usize);
        let cols = width / 8;
        let rows = height / cell_h;
        if cols < 20 || rows < 4 {
            return;
        }
        self.layout = None;
        self.hits.clear();
        let (sx, sy) = self.pixel_scale;
        let layered = self.layered;
        if !layered {
            self.layer = None;
        }
        let Some(open) = self.manual.as_mut().and_then(|m| m.open.as_mut()) else { return };
        let area = (0usize, cell_h, width, (rows - 2) * cell_h);
        for row in frame.rgb.chunks_exact_mut(width * 3).skip(area.1).take(area.3) {
            for pixel in row.as_chunks_mut::<3>().0.iter_mut() {
                pixel.copy_from_slice(&[0x18, 0x1C, 0x24]);
            }
        }

        // The page's size and place in the frontend's pixels, from the
        // top left of the area.
        let (pw, ph) = open.doc.page_size(open.page);
        let (aw, ah) = (area.2 as f32 * sx, area.3 as f32 * sy);
        let fit_width = aw / pw;
        let s = match open.zoom {
            0 => fit_width.min(ah / ph),
            1 => fit_width,
            2 => fit_width * 1.5,
            _ => fit_width * 2.0,
        };
        let (dw, dh) = (pw * s, ph * s);
        let place = |size: f32, room: f32, at: f32| if size <= room { (room - size) / 2.0 } else { -at * (size - room) };
        let (px, py) = (place(dw, aw, open.at.0), place(dh, ah, open.at.1));
        // What of it shows.
        let (vx0, vy0, vx1, vy1) = (px.max(0.0), py.max(0.0), (px + dw).min(aw), (py + dh).min(ah));
        // Rendered at the frontend's pixels for the layer, else at the
        // picture's.
        let (kx, ky) = if layered { (1.0, 1.0) } else { (1.0 / sx, 1.0 / sy) };
        let size = (((dw * kx).round() as u32).max(1), ((dh * ky).round() as u32).max(1));
        if open.rendered.as_ref().is_none_or(|(key, _)| *key != (open.page, size.0, size.1)) {
            open.rendered = Some(((open.page, size.0, size.1), open.doc.render(open.page, size.0, size.1)));
        }
        let picture = &open.rendered.as_ref().expect("the page rendered").1;
        // Pixels of the picture rendered to one of the frontend's.
        let (qx, qy) = (picture.width as f32 / dw, picture.height as f32 / dh);
        let part = (
            ((vx0 - px) * qx).round() as u32,
            ((vy0 - py) * qy).round() as u32,
            ((vx1 - vx0) * qx).round() as u32,
            ((vy1 - vy0) * qy).round() as u32,
        );
        // Where that is in the picture.
        let rect = (area.0 as f32 + vx0 / sx, area.1 as f32 + vy0 / sy, (vx1 - vx0) / sx, (vy1 - vy0) / sy);
        let shown = (open.page, open.zoom, (picture.width, picture.height), part);
        if vx1 > vx0 && vy1 > vy0 {
            if layered {
                if open.shown != Some(shown) || self.layer.as_ref().is_none_or(|l| l.rect != rect) {
                    self.layer_generation += 1;
                    self.layer = Some(Layer {
                        rect,
                        picture: manuals::crop(picture, part.0, part.1, part.2, part.3),
                        generation: self.layer_generation,
                    });
                }
            } else {
                let cropped = manuals::crop(picture, part.0, part.1, part.2, part.3);
                let (x0, y0) = (rect.0.round() as usize, rect.1.round() as usize);
                let (w, h) = (((rect.0 + rect.2).round() as usize).saturating_sub(x0), ((rect.1 + rect.3).round() as usize).saturating_sub(y0));
                let scaled = if (cropped.width as usize, cropped.height as usize) == (w, h) {
                    cropped
                } else {
                    manuals::scale(&cropped, w as u32, h as u32)
                };
                for y in 0..h.min(height.saturating_sub(y0)) {
                    let from = y * w * 3;
                    let to = ((y0 + y) * width + x0) * 3;
                    let n = w.min(width.saturating_sub(x0)) * 3;
                    frame.rgb[to..to + n].copy_from_slice(&scaled.rgb[from..from + n]);
                }
            }
        }
        open.shown = Some(shown);

        // The bars.
        let mut g = Grid::new(cols, rows);
        g.background(0, 0, cols, draw::FIELD);
        g.background(0, rows - 1, cols, draw::FIELD);
        let pages = open.doc.pages();
        let right = format!("Page {} of {}  {}", open.page + 1, pages, ZOOMS[open.zoom]);
        let end = cols.saturating_sub(right.len() + 1);
        g.text_to(1, 0, &fit(&open.title, end.saturating_sub(2)), draw::BRIGHT, end);
        g.text(end, 0, &right, draw::DIM);
        let mut x = 1;
        for (key, what) in [("PgUp/PgDn", "Page"), ("\u{2191}\u{2193}", "Scroll"), ("+/-", "Zoom"), ("Esc", "Back")] {
            if x + key.chars().count() + what.len() + 2 > cols {
                break;
            }
            x = g.text(x, rows - 1, key, draw::KEY);
            x = g.text(x + 1, rows - 1, what, draw::TEXT) + 2;
        }
        let layout = Layout { x: 0, y: 0, cell_h, cols, rows };
        draw::render_area(&g, &layout, frame, draw::OPAQUE, (0..cols, 0..1));
        draw::render_area(&g, &layout, frame, draw::OPAQUE, (0..cols, rows - 1..rows));
    }

    /// The manual open: its title, page and zoom.
    #[cfg(test)]
    pub(super) fn manual_page(&self) -> Option<(String, usize, usize)> {
        let open = self.manual.as_ref()?.open.as_ref()?;
        Some((open.title.clone(), open.page, open.zoom))
    }

    /// The mouse wheel over a page scrolls it.
    pub(super) fn manual_wheel(&mut self, dy: i32) -> bool {
        let Some(open) = self.manual.as_mut().and_then(|m| m.open.as_mut()) else { return false };
        open.at.1 = (open.at.1 - dy as f32 * 0.1).clamp(0.0, 1.0);
        true
    }
}
