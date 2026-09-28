//! The settings window's room browser, opened from the Network page: the
//! rooms of the relay the settings name (the public one unless set),
//! narrowed down as a search is typed, joined with Enter and made with Ins,
//! with a password or open to all. The frontend hands it the LAN every
//! frame (`ConfigUi::poll`), and it asks the relay again as the search
//! changes and every few seconds.

use super::dialog::TextField;
use super::draw::{self, Grid};
use super::{ConfigUi, Hit, Host, Target, UiKey, fit};
use crate::net::tunnel::wire::{self, RoomInfo};
use crate::net::{LanView, RoomList};
use web_time::{Duration, Instant};

/// How often the rooms are asked for again, and how soon after the last
/// time while the search is typed.
const REFRESH: Duration = Duration::from_secs(3);
const TYPING: Duration = Duration::from_millis(300);

/// The controls of the room being made, or of the password being given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomField {
    Name,
    Password,
    Ok,
    Cancel,
}

/// Making a room, or joining one that wants a password.
pub struct RoomPrompt {
    pub making: bool,
    pub name: TextField,
    pub password: TextField,
    pub focus: RoomField,
}

impl RoomPrompt {
    fn fields(&self) -> &'static [RoomField] {
        if self.making {
            &[RoomField::Name, RoomField::Password, RoomField::Ok, RoomField::Cancel]
        } else {
            &[RoomField::Password, RoomField::Ok, RoomField::Cancel]
        }
    }

    fn field(&mut self) -> Option<&mut TextField> {
        match self.focus {
            RoomField::Name => Some(&mut self.name),
            RoomField::Password => Some(&mut self.password),
            _ => None,
        }
    }

    fn step_focus(&mut self, dir: isize) {
        let fields = self.fields();
        let at = fields.iter().position(|&f| f == self.focus).unwrap_or(0) as isize;
        self.focus = fields[(at + dir).rem_euclid(fields.len() as isize) as usize];
    }
}

/// A row of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Row {
    Room(RoomInfo),
    /// Making a room: the one searched for, if it may be one.
    Make(Option<String>),
    /// Leaving the room this instance is in.
    Leave(String),
}

pub struct RoomBrowser {
    /// The relay, as the settings name it (None: the first that answers on
    /// this network).
    relay: Option<String>,
    search: TextField,
    /// The row selected and the first one shown.
    pub(super) selected: usize,
    scroll: usize,
    /// The LAN as the frontend showed it last, and the last rooms that
    /// came, which show while the next are asked for.
    view: LanView,
    list: Option<Result<RoomList, String>>,
    /// The search last asked for, when, and whether its answer is still
    /// to come.
    asked: Option<(String, Instant)>,
    waiting: bool,
    /// The room joined from here, until it is joined or can't be.
    joining: Option<String>,
    pub(super) prompt: Option<RoomPrompt>,
}

impl RoomBrowser {
    fn new(relay: Option<String>) -> Self {
        Self {
            relay,
            search: TextField::default(),
            selected: 0,
            scroll: 0,
            view: LanView::default(),
            list: None,
            asked: None,
            waiting: false,
            joining: None,
            prompt: None,
        }
    }

    /// The rooms the search finds, then making one, then leaving the one
    /// this instance is in.
    pub(super) fn rows(&self) -> Vec<Row> {
        let search = self.search.text();
        let wanted = search.to_lowercase();
        let mut rows: Vec<Row> = match &self.list {
            Some(Ok(list)) => list
                .rooms
                .iter()
                .filter(|r| r.name.to_lowercase().contains(&wanted))
                .map(|r| Row::Room(r.clone()))
                .collect(),
            _ => Vec::new(),
        };
        let name = search.trim();
        let listed = rows.iter().any(|r| matches!(r, Row::Room(room) if room.name == name));
        rows.push(Row::Make((wire::valid_room(name) && !listed).then(|| name.to_string())));
        if let Some((_, room)) = &self.view.joined {
            rows.push(Row::Leave(room.clone()));
        }
        rows
    }

    /// Whether this instance is in `room` of the relay listed.
    fn in_room(&self, room: &str) -> bool {
        match (&self.view.joined, &self.list) {
            (Some((relay, joined)), Some(Ok(list))) => joined == room && *relay == list.relay,
            _ => false,
        }
    }

    /// The relay to join a room at: the one listed if it was found on
    /// this network, so the same one is joined.
    fn join_relay(&self) -> Option<String> {
        match (&self.relay, &self.list) {
            (Some(relay), _) => Some(relay.clone()),
            (None, Some(Ok(list))) => Some(list.relay.to_string()),
            (None, _) => None,
        }
    }
}

/// `text` in at most `lines` lines of `width` columns, broken at spaces,
/// the last one shortened if it all doesn't fit.
fn wrap(text: &str, width: usize, lines: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() && out.len() + 1 < lines && rest.chars().count() > width {
        let cut = rest.char_indices().nth(width).map_or(rest.len(), |(i, _)| i);
        let at = rest[..cut].rfind(' ').filter(|&i| i > 0).unwrap_or(cut);
        out.push(rest[..at].to_string());
        rest = rest[at..].trim_start();
    }
    if !rest.is_empty() {
        out.push(fit(rest, width));
    }
    out
}

#[cfg(test)]
impl RoomBrowser {
    /// Make the last question as old as one to ask again.
    pub(super) fn age_last_question(&mut self) {
        if let Some((_, at)) = &mut self.asked {
            *at = Instant::now().checked_sub(REFRESH * 2).unwrap_or(*at);
        }
    }
}

impl ConfigUi {
    pub(super) fn open_rooms(&mut self) {
        self.status = None;
        self.rooms = Some(RoomBrowser::new(self.settings.network.relay.clone()));
    }

    /// What the window keeps up to date while it is open, for the frontend
    /// to call every frame: the room browser's LAN and rooms, asked for
    /// again as the search changes and every few seconds.
    pub fn poll(&mut self, host: &mut dyn Host) {
        let Some(browser) = &mut self.rooms else { return };
        let Some(view) = host.lan() else { return };
        if browser.waiting && !view.listing.asking {
            browser.waiting = false;
            browser.list = view.listing.result.clone();
        }
        let search = browser.search.text();
        let due = match &browser.asked {
            None => true,
            Some((asked, at)) if *asked != search => at.elapsed() >= TYPING,
            Some((_, at)) => !browser.waiting && at.elapsed() >= REFRESH,
        };
        if due {
            match host.browse_rooms(browser.relay.as_deref(), &search) {
                Ok(()) => browser.waiting = true,
                Err(e) => browser.list = Some(Err(e)),
            }
            browser.asked = Some((search, Instant::now()));
        }
        // How the join from here went.
        let mut outcome = None;
        if let Some(room) = &browser.joining {
            if view.joined.as_ref().is_some_and(|(_, joined)| joined == room) {
                outcome = Some(Ok(format!("In room \"{}\": start the game's network play", room)));
            } else if let Some(problem) = view.state.strip_prefix("not joined: ") {
                outcome = Some(Err(format!("Can't join room \"{}\": {}", room, problem)));
            }
        }
        browser.view = view;
        match outcome {
            Some(Ok(text)) => {
                browser.joining = None;
                self.info(text);
            }
            Some(Err(text)) => {
                browser.joining = None;
                self.error(text);
            }
            None => {}
        }
    }

    pub(super) fn rooms_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(browser) = &mut self.rooms else { return };
        if browser.prompt.is_some() {
            return self.room_prompt_key(key, host);
        }
        let rows = browser.rows();
        match key {
            UiKey::Up | UiKey::Down | UiKey::PageUp | UiKey::PageDown => {
                browser.selected = Self::navigate(key, browser.selected, rows.len(), self.visible.saturating_sub(4))
                    .unwrap_or(browser.selected);
            }
            UiKey::Esc => {
                self.rooms = None;
                self.status = None;
            }
            UiKey::Insert => self.open_room_prompt(None),
            UiKey::Enter => match rows.get(browser.selected.min(rows.len() - 1)).cloned() {
                Some(Row::Room(room)) if browser.in_room(&room.name) => {
                    self.info(format!("This instance is in room \"{}\"", room.name));
                }
                Some(Row::Room(room)) if room.password => self.open_room_prompt(Some(room.name)),
                Some(Row::Room(room)) => self.join_room(&room.name, "", host),
                Some(Row::Make(name)) => self.open_room_prompt_to_make(name),
                Some(Row::Leave(room)) => {
                    host.leave_room();
                    self.info(format!("Left room \"{}\"", room));
                }
                None => {}
            },
            // Anything else types the search.
            _ => {
                if browser.search.key(key) {
                    browser.selected = 0;
                    browser.scroll = 0;
                }
            }
        }
    }

    /// Ask for the password of `room`, or with None, for a room to make.
    fn open_room_prompt(&mut self, room: Option<String>) {
        let Some(browser) = &mut self.rooms else { return };
        browser.prompt = Some(match room {
            Some(room) => RoomPrompt {
                making: false,
                name: TextField::new(&room),
                password: TextField::default(),
                focus: RoomField::Password,
            },
            None => RoomPrompt {
                making: true,
                name: TextField::default(),
                password: TextField::default(),
                focus: RoomField::Name,
            },
        });
        self.status = None;
    }

    /// Make a room: the one searched for, which then needs only its
    /// password, or one to name.
    fn open_room_prompt_to_make(&mut self, name: Option<String>) {
        self.open_room_prompt(None);
        if let (Some(name), Some(prompt)) = (name, self.rooms.as_mut().and_then(|b| b.prompt.as_mut())) {
            prompt.name = TextField::new(&name);
            prompt.focus = RoomField::Password;
        }
    }

    fn room_prompt_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(prompt) = self.rooms.as_mut().and_then(|b| b.prompt.as_mut()) else { return };
        if let Some(field) = prompt.field()
            && field.key(key)
        {
            return;
        }
        match key {
            UiKey::Esc => self.close_room_prompt(),
            UiKey::Tab | UiKey::Down => prompt.step_focus(1),
            UiKey::BackTab | UiKey::Up => prompt.step_focus(-1),
            UiKey::Enter if prompt.focus == RoomField::Cancel => self.close_room_prompt(),
            UiKey::Enter if prompt.focus == RoomField::Name => prompt.step_focus(1),
            UiKey::Enter => {
                let (name, password) = (prompt.name.text().trim().to_string(), prompt.password.text());
                if !wire::valid_room(&name) {
                    return self.error("A room's name is 1 to 32 printable characters");
                }
                self.close_room_prompt();
                self.join_room(&name, &password, host);
            }
            _ => {}
        }
    }

    fn close_room_prompt(&mut self) {
        if let Some(browser) = &mut self.rooms {
            browser.prompt = None;
        }
        self.status = None;
    }

    /// Join `room` at the relay listed, making it if it isn't there.
    fn join_room(&mut self, room: &str, password: &str, host: &mut dyn Host) {
        let Some(browser) = &mut self.rooms else { return };
        match host.join_room(browser.join_relay().as_deref(), room, password) {
            Ok(()) => {
                browser.joining = Some(room.to_string());
                self.info(format!("Joining room \"{}\"...", room));
            }
            Err(e) => self.error(e),
        }
    }

    /// A click on a row of the list, or on a control of the prompt.
    pub(super) fn room_clicked(&mut self, target: Target, host: &mut dyn Host) {
        let Some(browser) = &mut self.rooms else { return };
        match target {
            Target::RoomRow(i) if i == browser.selected => self.key(UiKey::Enter, host),
            Target::RoomRow(i) => browser.selected = i,
            Target::RoomField(field) => {
                if let Some(prompt) = &mut browser.prompt {
                    prompt.focus = field;
                    if matches!(field, RoomField::Ok | RoomField::Cancel) {
                        self.key(UiKey::Enter, host);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn room_hints(&self) -> Vec<(&'static str, &'static str, UiKey)> {
        match self.rooms.as_ref().and_then(|b| b.prompt.as_ref()) {
            Some(prompt) => {
                let ok = if prompt.making { "Make" } else { "Join" };
                vec![("Tab", "Next", UiKey::Tab), ("Enter", ok, UiKey::Enter), ("Esc", "Cancel", UiKey::Esc)]
            }
            None => vec![("Enter", "Join", UiKey::Enter), ("Ins", "New room", UiKey::Insert), ("Esc", "Back", UiKey::Esc)],
        }
    }

    pub(super) fn draw_rooms(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some(browser) = &mut self.rooms else { return };
        let cols = g.cols;
        let top = content.start;
        let relay = browser.relay.clone().unwrap_or_else(|| "the first relay on this network".to_string());
        let mut title = format!("Rooms at {}", relay);
        if let Some(Ok(list)) = &browser.list
            && !list.name.is_empty()
        {
            title = format!("{} ({})", title, list.name);
        }
        g.text_to(2, top, &fit(&title, cols - 4), draw::BRIGHT, cols - 2);

        // Where this instance is, at the bottom.
        let lan_row = content.end - 1;
        let color = match &browser.view.joined {
            Some(_) => draw::GOOD,
            None if browser.view.state.starts_with("not joined: ") => draw::ERROR,
            None => draw::DIM,
        };
        let state = if browser.view.state.is_empty() { "not joined" } else { &browser.view.state };
        g.text_to(2, lan_row, &fit(&format!("LAN: {}", state), cols - 4), color, cols - 2);

        if browser.prompt.is_some() {
            let hits = Self::draw_room_prompt(browser, g, top + 2..lan_row);
            self.hits.extend(hits);
            return;
        }

        // The search, and how many rooms there are.
        let search_row = top + 1;
        g.text(2, search_row, "Search", draw::TEXT);
        let search = browser.search.text();
        let count = match &browser.list {
            None if browser.waiting => "asking...".to_string(),
            Some(Ok(list)) if !search.is_empty() => format!("{} found", list.total),
            Some(Ok(list)) if list.total == 1 => "1 room".to_string(),
            Some(Ok(list)) => format!("{} rooms", list.total),
            _ => String::new(),
        };
        let count_col = cols.saturating_sub(count.len() + 3);
        let width = count_col.saturating_sub(12).min(40);
        g.background(9, search_row, width, draw::FIELD);
        let (text, cursor) = browser.search.view(width);
        if text.is_empty() {
            g.text_to(9, search_row, "type to find a room", draw::DIM, 9 + width);
        }
        g.text_to(9, search_row, &text, draw::BRIGHT, 9 + width);
        g.background(9 + cursor, search_row, 1, draw::SELECT);
        g.text(count_col, search_row, &count, draw::DIM);

        // The rooms, or why there are none, and the rows after them.
        let rows = browser.rows();
        let mut list_rows = top + 3..lan_row.saturating_sub(1);
        let message = match &browser.list {
            None => Some(("Asking the relay for its rooms...".to_string(), draw::DIM)),
            Some(Err(e)) => Some((format!("Can't list the rooms: {}", e), draw::ERROR)),
            _ if rows.iter().any(|r| matches!(r, Row::Room(_))) => None,
            _ if search.is_empty() => Some(("No rooms yet: Ins makes one".to_string(), draw::DIM)),
            _ => Some((format!("No room has \"{}\" in its name", search), draw::DIM)),
        };
        if let Some((text, color)) = message {
            for line in wrap(&text, cols - 4, 2) {
                if list_rows.is_empty() {
                    break;
                }
                g.text_to(2, list_rows.start, &line, color, cols - 2);
                list_rows.start += 1;
            }
        }
        browser.selected = browser.selected.min(rows.len() - 1);
        Self::keep_visible(&mut browser.scroll, browser.selected, list_rows.len());
        let members_col = cols.saturating_sub(24);
        let mut hits = Vec::new();
        for (i, row) in (browser.scroll..rows.len()).zip(list_rows.clone()) {
            if i == browser.selected {
                g.background(1, row, cols - 2, draw::SELECT);
            }
            hits.push(Hit { row, col: 1, width: cols - 2, target: Target::RoomRow(i) });
            match &rows[i] {
                Row::Room(room) => {
                    let here = browser.in_room(&room.name);
                    let color = if here { draw::GOOD } else { draw::BRIGHT };
                    g.text_to(4, row, &room.name, color, members_col.saturating_sub(1));
                    let members = if room.members == 1 { "1 in it".to_string() } else { format!("{} in it", room.members) };
                    let after = g.text_to(members_col, row, &format!("{:>8}", members), draw::TEXT, cols - 2);
                    if here {
                        g.text_to(after + 2, row, "you", draw::GOOD, cols - 2);
                    } else if room.password {
                        g.text_to(after + 2, row, "password", draw::NOTE, cols - 2);
                    }
                }
                Row::Make(Some(name)) => {
                    g.text_to(2, row, &fit(&format!("+ Make room \"{}\"...", name), cols - 4), draw::KEY, cols - 2);
                }
                Row::Make(None) => {
                    g.text(2, row, "+ Make a room...", draw::KEY);
                }
                Row::Leave(room) => {
                    g.text_to(2, row, &fit(&format!("- Leave room \"{}\"", room), cols - 4), draw::KEY, cols - 2);
                }
            }
        }
        let (scroll, total) = (browser.scroll, rows.len());
        self.hits.extend(hits);
        self.draw_scrollbar(g, list_rows, scroll, total);
    }

    /// The room being made, or the password being given, in `rows`.
    fn draw_room_prompt(browser: &RoomBrowser, g: &mut Grid, rows: std::ops::Range<usize>) -> Vec<Hit> {
        let Some(prompt) = &browser.prompt else { return Vec::new() };
        let cols = g.cols;
        let top = rows.start;
        let value_col = 14.min(cols / 3);
        let width = (cols - 3).saturating_sub(value_col).min(34);
        let title = if prompt.making {
            "A new room, which this instance joins".to_string()
        } else {
            format!("Room \"{}\" wants a password", prompt.name.text())
        };
        g.text_to(2, top, &fit(&title, cols - 4), draw::BRIGHT, cols - 2);
        let mut hits = Vec::new();
        let mut fields = vec![(RoomField::Password, "Password", &prompt.password, "none: open to all")];
        if prompt.making {
            fields.insert(0, (RoomField::Name, "Name", &prompt.name, "Doom II deathmatch"));
        }
        for (i, (field, label, text, example)) in fields.into_iter().enumerate() {
            let row = top + 2 + i;
            if row >= rows.end {
                break;
            }
            let focused = prompt.focus == field;
            g.text(4, row, label, if focused { draw::BRIGHT } else { draw::TEXT });
            g.background(value_col, row, width, draw::FIELD);
            let (shown, cursor) = text.view(width);
            if shown.is_empty() && !focused {
                g.text_to(value_col, row, example, draw::DIM, value_col + width);
            }
            g.text_to(value_col, row, &shown, draw::BRIGHT, value_col + width);
            if focused {
                g.background(value_col + cursor, row, 1, draw::SELECT);
            }
            hits.push(Hit { row, col: value_col, width, target: Target::RoomField(field) });
        }
        let row = top + 5;
        if row < rows.end {
            let ok = if prompt.making { "[ Make ]" } else { "[ Join ]" };
            let mut col = value_col;
            for (field, text) in [(RoomField::Ok, ok), (RoomField::Cancel, "[ Cancel ]")] {
                let focused = prompt.focus == field;
                if focused {
                    g.background(col, row, text.len(), draw::SELECT);
                }
                g.text(col, row, text, if focused { draw::BRIGHT } else { draw::KEY });
                hits.push(Hit { row, col, width: text.len(), target: Target::RoomField(field) });
                col += text.len() + 2;
            }
        }
        let notes = if prompt.making {
            [
                "Everyone who joins after has to know its password. The room goes",
                "when the last one leaves. Nothing crossing the relay is encrypted.",
            ]
        } else {
            ["The one who made the room gave it its password.", ""]
        };
        for (i, note) in notes.iter().enumerate() {
            if top + 7 + i < rows.end {
                g.text_to(4, top + 7 + i, note, draw::DIM, cols - 2);
            }
        }
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn messages_wrap_at_spaces() {
        assert_eq!(wrap("short", 20, 2), ["short"]);
        let text = "can't find relay.rust-dos.com: failed to lookup address information";
        assert_eq!(wrap(text, 32, 2), ["can't find relay.rust-dos.com:", "failed to... address information"]);
        assert_eq!(wrap(&"x".repeat(50), 20, 3), ["x".repeat(20), "x".repeat(20), "x".repeat(10)]);
    }
}
