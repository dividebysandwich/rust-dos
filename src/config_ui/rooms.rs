//! The settings window's room browser, opened from the Network page: the
//! rooms on this network (those of every relay that answers there), or
//! online at the relay the settings name, which Tab switches between. They
//! are narrowed down as a search is typed, joined with Enter and made with
//! Ins, with a password or open to all; a room made on this network is
//! hosted in this instance. While this instance hosts the room it is in,
//! the browser shows that room instead: who is in it, and buttons to leave
//! it or end it for everyone. The frontend hands it the LAN every frame
//! (`ConfigUi::poll`), and it asks for the rooms again as the search
//! changes and every few seconds.

use super::dialog::TextField;
use super::draw::{self, Grid};
use super::{ConfigUi, Hit, Host, Target, UiKey, fit};
use crate::net::tunnel::relay::MAX_ROOM;
use crate::net::tunnel::wire::{self, RoomInfo};
use crate::net::{LanView, RoomList};
use std::net::SocketAddr;
use web_time::{Duration, Instant};

/// How often the rooms are asked for again, and how soon after the last
/// time while the search is typed.
const REFRESH: Duration = Duration::from_secs(3);
const TYPING: Duration = Duration::from_millis(300);

/// The buttons of the room this instance hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomButton {
    Leave,
    Disband,
}

/// The controls of the room being made, or of the password being given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomField {
    Name,
    Password,
    Ok,
    Cancel,
}

/// Making a room, or joining one that wants a password, at its relay.
pub struct RoomPrompt {
    pub making: bool,
    pub name: TextField,
    pub password: TextField,
    pub focus: RoomField,
    relay: Option<SocketAddr>,
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
    /// A room, and the relay it is at.
    Room(RoomInfo, SocketAddr),
    /// Making a room: the one searched for, if it may be one.
    Make(Option<String>),
    /// Leaving the room this instance is in.
    Leave(String),
}

pub struct RoomBrowser {
    /// Whether the rooms are online, at the relay the settings name, or
    /// on this network.
    pub(super) online: bool,
    relay: String,
    search: TextField,
    /// The row selected and the first one shown.
    pub(super) selected: usize,
    scroll: usize,
    /// The LAN as the frontend showed it last, and the last rooms that
    /// came, which show while the next are asked for.
    view: LanView,
    list: Option<Result<Vec<RoomList>, String>>,
    /// The search last asked for, when, and whether its answer is still
    /// to come.
    asked: Option<(String, Instant)>,
    waiting: bool,
    /// The room joined from here, until it is joined or can't be.
    joining: Option<String>,
    pub(super) prompt: Option<RoomPrompt>,
    /// The button of the room this instance hosts that Enter presses, and
    /// whether ending the room waits for Enter once more.
    pub(super) button: RoomButton,
    pub(super) confirm_disband: bool,
}

impl RoomBrowser {
    fn new(online: bool, relay: String) -> Self {
        Self {
            online,
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
            button: RoomButton::Leave,
            confirm_disband: false,
        }
    }

    /// The relay the rooms are at: the one online, or None for those on
    /// this network.
    fn relay(&self) -> Option<&str> {
        self.online.then_some(self.relay.as_str())
    }

    /// The relays that answered.
    fn lists(&self) -> &[RoomList] {
        match &self.list {
            Some(Ok(lists)) => lists,
            _ => &[],
        }
    }

    /// The rooms the search finds, the fullest first, then making one,
    /// then leaving the one this instance is in.
    pub(super) fn rows(&self) -> Vec<Row> {
        let search = self.search.text();
        let wanted = search.to_lowercase();
        let mut rooms: Vec<(RoomInfo, SocketAddr)> = self
            .lists()
            .iter()
            .flat_map(|list| list.rooms.iter().map(|r| (r.clone(), list.relay)))
            .filter(|(r, _)| r.name.to_lowercase().contains(&wanted))
            .collect();
        rooms.sort_by(|a, b| b.0.members.cmp(&a.0.members).then_with(|| a.0.name.cmp(&b.0.name)));
        let mut rows: Vec<Row> = rooms.into_iter().map(|(room, relay)| Row::Room(room, relay)).collect();
        let name = search.trim();
        let listed = rows.iter().any(|r| matches!(r, Row::Room(room, _) if room.name == name));
        rows.push(Row::Make((wire::valid_room(name) && !listed).then(|| name.to_string())));
        if let Some((_, room)) = &self.view.joined {
            rows.push(Row::Leave(room.clone()));
        }
        rows
    }

    /// The room this instance is in, by name.
    fn joined_room(&self) -> String {
        self.view.joined.as_ref().map_or(String::new(), |(_, room)| room.clone())
    }

    /// What the room this instance is in was last listed as.
    fn listed(&self) -> Option<&RoomInfo> {
        let (relay, room) = self.view.joined.as_ref()?;
        let list = self.lists().iter().find(|l| l.relay == *relay)?;
        list.rooms.iter().find(|r| r.name == *room)
    }

    /// Whether this instance is in `room` at `relay`.
    fn in_room(&self, room: &str, relay: SocketAddr) -> bool {
        self.view.joined.as_ref().is_some_and(|(at, joined)| joined == room && *at == relay)
    }

    /// The name of the relay at `relay`, as it gave it.
    fn relay_name(&self, relay: SocketAddr) -> &str {
        self.lists().iter().find(|l| l.relay == relay).map_or("", |l| l.name.as_str())
    }
}

/// `text` in at most `lines` lines of `width` columns, broken at spaces,
/// the last one shortened if it all doesn't fit.
pub(super) fn wrap(text: &str, width: usize, lines: usize) -> Vec<String> {
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
        let network = &self.settings.network;
        self.rooms = Some(RoomBrowser::new(network.online, network.relay.clone()));
    }

    /// Switch between the rooms on this network and those online, which
    /// the settings keep.
    fn switch_rooms(&mut self, online: bool, host: &mut dyn Host) {
        let Some(browser) = &mut self.rooms else { return };
        if browser.online == online {
            return;
        }
        let joining = browser.joining.take();
        *browser = RoomBrowser { joining, ..RoomBrowser::new(online, browser.relay.clone()) };
        self.settings.network.online = online;
        // The LAN commands go by it at the prompt, when it is in place.
        let _ = host.apply(&self.settings);
        self.status = None;
    }

    /// The room browser's LAN and rooms, asked for again as the search
    /// changes and every few seconds.
    pub(super) fn poll_rooms(&mut self, host: &mut dyn Host) {
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
            match host.browse_rooms(browser.relay(), &search) {
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
        if browser.view.hosting() {
            return self.hosted_room_key(key, host);
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
            UiKey::Tab | UiKey::BackTab => {
                let online = !browser.online;
                self.switch_rooms(online, host);
            }
            UiKey::Insert => self.open_room_prompt(None),
            UiKey::Enter => match rows.get(browser.selected.min(rows.len() - 1)).cloned() {
                Some(Row::Room(room, relay)) if browser.in_room(&room.name, relay) => {
                    self.info(format!("This instance is in room \"{}\"", room.name));
                }
                Some(Row::Room(room, relay)) if room.password => self.open_room_prompt(Some((room.name, relay))),
                Some(Row::Room(room, relay)) => self.join_room(&room.name, "", Some(relay), host),
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

    /// The room this instance hosts: Left, Right and Tab go between Leave
    /// and Disband, Enter presses one, and Disband wants Enter once more.
    fn hosted_room_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(browser) = &mut self.rooms else { return };
        let room = browser.joined_room();
        if std::mem::take(&mut browser.confirm_disband) {
            if key == UiKey::Enter {
                host.disband_room();
                self.info(format!("Ended room \"{}\" for everyone in it", room));
            } else {
                self.status = None;
            }
            return;
        }
        match key {
            UiKey::Left | UiKey::Right | UiKey::Up | UiKey::Down | UiKey::Tab | UiKey::BackTab => {
                browser.button = match browser.button {
                    RoomButton::Leave => RoomButton::Disband,
                    RoomButton::Disband => RoomButton::Leave,
                };
            }
            UiKey::Esc => {
                self.rooms = None;
                self.status = None;
            }
            UiKey::Enter if browser.button == RoomButton::Leave => {
                // The one there longest after this instance hosts it next.
                let next = browser.view.roster.as_ref().and_then(|r| r.members.get(1).map(wire::Member::shown));
                host.leave_room();
                match next {
                    Some(next) => self.info(format!("Left room \"{}\": {} hosts it now", room, next)),
                    None => self.info(format!("Left room \"{}\", which went with its last player", room)),
                }
            }
            UiKey::Enter => {
                browser.confirm_disband = true;
                self.error(format!("End room \"{}\" for everyone in it?", room));
            }
            _ => {}
        }
    }

    /// Ask for the password of `room` at its relay, or with None, for a
    /// room to make.
    fn open_room_prompt(&mut self, room: Option<(String, SocketAddr)>) {
        let Some(browser) = &mut self.rooms else { return };
        browser.prompt = Some(match room {
            Some((room, relay)) => RoomPrompt {
                making: false,
                name: TextField::new(&room),
                password: TextField::default(),
                focus: RoomField::Password,
                relay: Some(relay),
            },
            None => RoomPrompt {
                making: true,
                name: TextField::default(),
                password: TextField::default(),
                focus: RoomField::Name,
                relay: None,
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
                let (making, relay) = (prompt.making, prompt.relay);
                self.close_room_prompt();
                match relay {
                    Some(relay) => self.join_room(&name, &password, Some(relay), host),
                    None if making => self.make_room(&name, &password, host),
                    None => self.join_room(&name, &password, None, host),
                }
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

    /// Join `room` at `relay`, or at the relay online.
    fn join_room(&mut self, room: &str, password: &str, relay: Option<SocketAddr>, host: &mut dyn Host) {
        let Some(browser) = &mut self.rooms else { return };
        let relay = relay.map(|r| r.to_string()).or_else(|| browser.relay().map(String::from));
        let joined = host.join_room(relay.as_deref(), room, password);
        self.joined_room(room, joined);
    }

    /// Make `room` and join it: online at the relay, or on this network
    /// on a relay this instance hosts.
    fn make_room(&mut self, room: &str, password: &str, host: &mut dyn Host) {
        let Some(browser) = &self.rooms else { return };
        let made = match browser.relay() {
            Some(relay) => host.join_room(Some(relay), room, password),
            None => host.host_room(room, password),
        };
        self.joined_room(room, made);
    }

    /// Joining `room` went out, or couldn't.
    fn joined_room(&mut self, room: &str, joined: Result<(), String>) {
        let Some(browser) = &mut self.rooms else { return };
        match joined {
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
            Target::RoomButton(button) => {
                browser.button = button;
                self.key(UiKey::Enter, host);
            }
            Target::RoomsOnline(online) if browser.prompt.is_none() => self.switch_rooms(online, host),
            _ => {}
        }
    }

    pub(super) fn room_hints(&self) -> Vec<(&'static str, &'static str, UiKey)> {
        let Some(browser) = &self.rooms else { return Vec::new() };
        match &browser.prompt {
            Some(prompt) => {
                let ok = if prompt.making { "Make" } else { "Join" };
                vec![("Tab", "Next", UiKey::Tab), ("Enter", ok, UiKey::Enter), ("Esc", "Cancel", UiKey::Esc)]
            }
            None if browser.view.hosting() && browser.confirm_disband => {
                vec![("Enter", "End it", UiKey::Enter), ("Esc", "Keep it", UiKey::Esc)]
            }
            None if browser.view.hosting() => {
                let press = if browser.button == RoomButton::Leave { "Leave" } else { "Disband" };
                vec![("Enter", press, UiKey::Enter), ("Tab", "Next", UiKey::Tab), ("Esc", "Back", UiKey::Esc)]
            }
            None => {
                let other = if browser.online { "This network" } else { "Online" };
                vec![
                    ("Enter", "Join", UiKey::Enter),
                    ("Ins", "New room", UiKey::Insert),
                    ("Tab", other, UiKey::Tab),
                    ("Esc", "Back", UiKey::Esc),
                ]
            }
        }
    }

    pub(super) fn draw_rooms(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some(browser) = &mut self.rooms else { return };
        let cols = g.cols;
        let top = content.start;
        let hosting = browser.view.hosting() && browser.prompt.is_none();

        // The rooms on this network and online, as tabs, and where those
        // shown are.
        let mut hits = Vec::new();
        let mut x = 2;
        for (online, label) in [(false, " This network "), (true, " Online ")] {
            let selected = browser.online == online;
            if selected {
                g.background(x, top, label.len().min(cols - 2 - x), draw::SELECT);
            }
            let after = g.text_to(x, top, label, if selected { draw::BRIGHT } else { draw::TEXT }, cols - 2);
            hits.push(Hit { row: top, col: x, width: after - x, target: Target::RoomsOnline(online) });
            x = after + 1;
        }
        let at = match browser.lists() {
            [list] if browser.online && !list.name.is_empty() => format!("at {} ({})", browser.relay, list.name),
            _ if browser.online => format!("at {}", browser.relay),
            _ => String::new(),
        };
        g.text_to(x + 1, top, &fit(&at, cols.saturating_sub(x + 3)), draw::DIM, cols - 2);
        self.hits.extend(hits);

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
        if hosting {
            let hits = Self::draw_hosted_room(browser, g, top + 2..lan_row);
            self.hits.extend(hits);
            return;
        }

        // The search, and how many rooms there are.
        let search_row = top + 1;
        g.text(2, search_row, "Search", draw::TEXT);
        let search = browser.search.text();
        let total: usize = browser.lists().iter().map(|l| l.total).sum();
        let count = match &browser.list {
            None if browser.waiting => "asking...".to_string(),
            Some(Ok(_)) if !search.is_empty() => format!("{} found", total),
            Some(Ok(_)) if total == 1 => "1 room".to_string(),
            Some(Ok(_)) => format!("{} rooms", total),
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
        // On this network, whose each room is: the relay's name.
        let rows = browser.rows();
        let members_col = cols.saturating_sub(24);
        let host_col = (!browser.online).then(|| members_col.saturating_sub(18)).filter(|&c| c > 20);
        g.text(4, top + 2, "Room", draw::DIM);
        if let Some(col) = host_col {
            g.text(col, top + 2, "Host", draw::DIM);
        }
        g.text_to(members_col, top + 2, "Players", draw::DIM, cols - 2);
        let mut list_rows = top + 3..lan_row.saturating_sub(1);
        let message = match &browser.list {
            None if browser.online => Some(("Asking the relay for its rooms...".to_string(), draw::DIM)),
            None => Some(("Looking for rooms on this network...".to_string(), draw::DIM)),
            Some(Err(e)) => Some((format!("Can't list the rooms: {}", e), draw::ERROR)),
            _ if rows.iter().any(|r| matches!(r, Row::Room(..))) => None,
            _ if !search.is_empty() => Some((format!("No room has \"{}\" in its name", search), draw::DIM)),
            _ if browser.online => Some(("No rooms yet: Ins makes one".to_string(), draw::DIM)),
            _ => Some(("No rooms on this network yet: Ins makes one here".to_string(), draw::DIM)),
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
        let mut hits = Vec::new();
        for (i, row) in (browser.scroll..rows.len()).zip(list_rows.clone()) {
            if i == browser.selected {
                g.background(1, row, cols - 2, draw::SELECT);
            }
            hits.push(Hit { row, col: 1, width: cols - 2, target: Target::RoomRow(i) });
            match &rows[i] {
                Row::Room(room, relay) => {
                    let here = browser.in_room(&room.name, *relay);
                    let color = if here { draw::GOOD } else { draw::BRIGHT };
                    let end = host_col.unwrap_or(members_col).saturating_sub(1);
                    g.text_to(4, row, &room.name, color, end);
                    if let Some(col) = host_col {
                        g.text_to(col, row, browser.relay_name(*relay), draw::TEXT, members_col.saturating_sub(1));
                    }
                    let after = g.text_to(members_col, row, &format!("{:>7}", room.members), draw::TEXT, cols - 2);
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

    /// The room this instance hosts, in `rows`: its name, who is in it,
    /// and the buttons to leave it or end it.
    fn draw_hosted_room(browser: &RoomBrowser, g: &mut Grid, rows: std::ops::Range<usize>) -> Vec<Hit> {
        let Some(roster) = &browser.view.roster else { return Vec::new() };
        let cols = g.cols;
        let room = browser.joined_room();
        let top = rows.start;
        let after = g.text_to(2, top, &fit(&room, cols - 20), draw::GOOD, cols - 2);
        match browser.listed() {
            Some(listed) if listed.password => g.text_to(after + 2, top, "password", draw::NOTE, cols - 2),
            Some(_) => g.text_to(after + 2, top, "open to all", draw::DIM, cols - 2),
            None => after,
        };

        // Who is in it, and the buttons below.
        let buttons_row = rows.end.saturating_sub(2).max(top + 3);
        let players = top + 3..buttons_row.saturating_sub(1);
        let role_col = cols.saturating_sub(24);
        g.text(4, top + 2, "Players", draw::DIM);
        g.text_to(role_col, top + 2, &format!("{} of {}", roster.total, MAX_ROOM), draw::DIM, cols - 2);
        let hidden = roster.total as usize - roster.members.len().min(roster.total as usize);
        let fits = if roster.members.len() > players.len() || hidden > 0 {
            players.len().saturating_sub(1)
        } else {
            players.len()
        };
        for (member, row) in roster.members.iter().take(fits).zip(players.clone()) {
            let me = browser.view.index == Some(member.index);
            let name = format!("{:>3}  {}", member.index, member.shown());
            g.text_to(4, row, &name, if me { draw::GOOD } else { draw::BRIGHT }, role_col.saturating_sub(1));
            let mut role = match (member.index == roster.host, me) {
                (true, true) => "host, you",
                (true, false) => "host",
                (false, true) => "you",
                (false, false) => "",
            }
            .to_string();
            // The player the serial link goes to.
            if let Some((_, link)) = browser.view.serial.as_ref().filter(|(peer, _)| *peer == member.index) {
                role = if role.is_empty() { link.clone() } else { format!("{}, {}", role, link) };
            }
            g.text_to(role_col, row, &role, draw::NOTE, cols - 2);
        }
        let more = roster.total as usize - fits.min(roster.total as usize);
        if more > 0 && fits < players.len() {
            g.text(9, players.start + fits, &format!("and {} more", more), draw::DIM);
        }

        let mut hits = Vec::new();
        let mut col = 4;
        for (button, text) in [(RoomButton::Leave, "[ Leave ]"), (RoomButton::Disband, "[ Disband ]")] {
            let focused = browser.button == button;
            if focused {
                g.background(col, buttons_row, text.len(), draw::SELECT);
            }
            g.text(col, buttons_row, text, if focused { draw::BRIGHT } else { draw::KEY });
            hits.push(Hit { row: buttons_row, col, width: text.len(), target: Target::RoomButton(button) });
            col += text.len() + 2;
        }
        let note = match browser.button {
            RoomButton::Leave => "Leave without disbanding",
            RoomButton::Disband => "Disband the room for everyone",
        };
        g.text_to(col, buttons_row, note, draw::DIM, cols - 2);
        hits
    }

    /// The room being made, or the password being given, in `rows`.
    fn draw_room_prompt(browser: &RoomBrowser, g: &mut Grid, rows: std::ops::Range<usize>) -> Vec<Hit> {
        let Some(prompt) = &browser.prompt else { return Vec::new() };
        let cols = g.cols;
        let top = rows.start;
        let value_col = 14.min(cols / 3);
        let width = (cols - 3).saturating_sub(value_col).min(34);
        let title = if prompt.making {
            "Create new room".to_string()
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
                "The room will exist as long as at least one player is inside.",
                "Traffic is not encrypted.",
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
