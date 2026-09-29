//! A session with retroachievements.org: logging in, finding the game by
//! its hash, its achievements, leaderboards and rich presence, the unlocks
//! the player has, checking them every frame, and telling the site of
//! unlocks, leaderboard entries and what the player is doing (a ping every
//! two minutes). Requests go through a `Transport`, which the desktop has
//! as HTTPS on a thread of its own (http.rs).
//!
//! Hardcore mode is fixed when a game's session starts; the frontend
//! keeps save states, rewind and cheats from being used while it is on.

use super::memory::MemoryMap;
use super::runtime::{Event, Format, Runtime};
use md5::{Digest, Md5};
use serde_json::Value;
use std::collections::HashMap;
use web_time::{Duration, Instant};

/// MS-DOS, as RetroAchievements numbers consoles.
pub const CONSOLE_MS_DOS: u32 = 26;
/// Achievements from this id on are the site's warnings (an emulator it
/// doesn't know, say), not the game's.
const WARNING_ID: u32 = 101_000_001;
/// Achievement flags: official (core) and unofficial ones.
const CORE: u64 = 3;
/// How often the site hears what the player is doing.
const PING_INTERVAL: Duration = Duration::from_secs(120);

/// The `[achievements]` settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AchievementSettings {
    pub enabled: bool,
    pub username: String,
    /// What logging in gave, which logs in without the password.
    pub token: String,
    pub hardcore: bool,
}

impl AchievementSettings {
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let bool_of = |v: &str| match v.trim().to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok(true),
            "false" | "off" | "no" | "0" => Ok(false),
            _ => Err(format!("invalid {} '{}' (true or false)", key, v)),
        };
        match key.to_ascii_lowercase().as_str() {
            "enabled" => self.enabled = bool_of(value)?,
            "hardcore" => self.hardcore = bool_of(value)?,
            "username" => self.username = value.trim().to_string(),
            "token" => self.token = value.trim().to_string(),
            _ => return Err(format!("unknown setting '{}'", key)),
        }
        Ok(())
    }

    pub fn entries(&self) -> Vec<(&'static str, Option<String>)> {
        vec![
            ("enabled", Some(self.enabled.to_string())),
            ("hardcore", Some(self.hardcore.to_string())),
            (
                "username",
                (!self.username.is_empty()).then(|| self.username.clone()),
            ),
            (
                "token",
                (!self.token.is_empty()).then(|| self.token.clone()),
            ),
        ]
    }
}

/// A request to the site: form fields for dorequest.php.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub fields: Vec<(String, String)>,
}

impl Request {
    /// The fields URL-encoded, as the body of a POST.
    pub fn body(&self) -> String {
        let encode = |s: &str| {
            s.bytes()
                .map(|b| match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                        (b as char).to_string()
                    }
                    _ => format!("%{:02X}", b),
                })
                .collect::<String>()
        };
        self.fields
            .iter()
            .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    }
}

/// How requests reach the site. `poll` gives the answers that came: the
/// body, or why there is none.
pub trait Transport {
    fn send(&mut self, request: Request);
    fn poll(&mut self) -> Vec<(u64, Result<String, String>)>;
}

/// Something to tell the player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub detail: String,
    /// An achievement unlocked or mastery: shown longer.
    pub big: bool,
}

impl Notice {
    fn new(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            big: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserState {
    LoggedOut,
    LoggingIn,
    LoggedIn {
        name: String,
        score: u32,
        softcore_score: u32,
    },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AchievementInfo {
    pub id: u32,
    pub title: String,
    pub description: String,
    pub points: u32,
    pub badge: String,
    /// Official, or one being worked on (which can't be unlocked).
    pub official: bool,
    pub unlocked: bool,
    /// The site's type: progression, missable, win_condition, or none.
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LeaderboardInfo {
    pub id: u32,
    pub title: String,
    pub description: String,
    pub format: Format,
    pub lower_is_better: bool,
    pub hidden: bool,
}

/// The game playing, as the site knows it.
pub struct Game {
    pub id: u32,
    pub title: String,
    pub hash: String,
    /// Fixed when the session started.
    pub hardcore: bool,
    pub achievements: Vec<AchievementInfo>,
    pub leaderboards: Vec<LeaderboardInfo>,
    runtime: Runtime,
    /// The session started: unlocks are known and the logic runs.
    started: bool,
    next_ping: Instant,
    /// When the rich presence line is worked out again for the settings
    /// window.
    next_presence: Instant,
    /// Leaderboards being attempted, and their values.
    trackers: Vec<(u32, i32)>,
    pub rich_presence: String,
}

impl Game {
    pub fn measured(&self, id: u32) -> Option<(u32, u32)> {
        self.runtime.measured(id)
    }

    pub fn is_primed(&self, id: u32) -> bool {
        self.runtime.is_primed(id)
    }

    /// Official achievements unlocked and all, and their points.
    pub fn progress(&self) -> (usize, usize, u32, u32) {
        let official = || {
            self.achievements
                .iter()
                .filter(|a| a.official && a.id < WARNING_ID)
        };
        let unlocked = || official().filter(|a| a.unlocked);
        (
            unlocked().count(),
            official().count(),
            unlocked().map(|a| a.points).sum(),
            official().map(|a| a.points).sum(),
        )
    }

    /// The leaderboards running, with their values shown.
    pub fn trackers(&self) -> Vec<String> {
        self.trackers
            .iter()
            .filter_map(|&(id, value)| {
                let lb = self.leaderboards.iter().find(|l| l.id == id)?;
                Some(lb.format.show(super::runtime::typed::Typed::signed(value)))
            })
            .collect()
    }
}

/// What the settings window's Achievements page shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AchievementsView {
    pub status: String,
    /// Logged in as, and the points.
    pub user: Option<(String, u32, u32)>,
    pub logging_in: bool,
    pub game: Option<GameView>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameView {
    pub title: String,
    pub hardcore: bool,
    pub rich_presence: String,
    pub achievements: Vec<AchievementRow>,
    /// The leaderboards: title and description.
    pub leaderboards: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AchievementRow {
    pub title: String,
    pub description: String,
    pub points: u32,
    pub unlocked: bool,
    pub official: bool,
    /// Measured progress: value and target.
    pub progress: Option<(u32, u32)>,
    /// A challenge on: all but its last conditions hold.
    pub primed: bool,
}

/// What the session knows of the game.
pub enum GameState {
    None,
    /// No hash to know it by.
    NoHash,
    /// Waiting to log in, or for the site.
    Loading {
        hash: String,
        name: String,
    },
    /// The site doesn't know the hash.
    Unknown {
        hash: String,
        name: String,
    },
    Failed(String),
    Playing(Box<Game>),
}

/// What an answer is to.
enum Pending {
    Login {
        token: bool,
    },
    Game {
        hash: String,
        name: String,
    },
    Session,
    Award {
        id: u32,
        hardcore: bool,
        attempt: u32,
    },
    Submit {
        id: u32,
        value: i32,
        attempt: u32,
    },
    Ping,
}

/// An unlock or entry to send again after the network failed.
struct Retry {
    at: Instant,
    pending: Pending,
}

pub struct Achievements {
    transport: Box<dyn Transport>,
    pub settings: AchievementSettings,
    pub user: UserState,
    pub game: GameState,
    pending: HashMap<u64, Pending>,
    retries: Vec<Retry>,
    notices: Vec<Notice>,
    /// A token logging in gave, for the frontend to save.
    new_token: Option<String>,
    next_id: u64,
    /// Whether the frontend lets hardcore mode on (the debug server off).
    pub hardcore_allowed: bool,
}

/// The JSON of an answer, or why it failed.
fn parse_answer(body: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(body).map_err(|_| {
        let text: String = body.chars().take(80).collect();
        format!("the site answered: {}", text.trim())
    })?;
    if value.get("Success").and_then(Value::as_bool) == Some(false) {
        let error = value
            .get("Error")
            .and_then(Value::as_str)
            .unwrap_or("the site refused");
        return Err(error.to_string());
    }
    Ok(value)
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn num_of(v: &Value, key: &str) -> u64 {
    match v.get(key) {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn md5_hex(text: &str) -> String {
    Md5::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

impl Achievements {
    pub fn new(transport: Box<dyn Transport>, settings: AchievementSettings) -> Self {
        let mut achievements = Self {
            transport,
            settings,
            user: UserState::LoggedOut,
            game: GameState::None,
            pending: HashMap::new(),
            retries: Vec::new(),
            notices: Vec::new(),
            new_token: None,
            next_id: 1,
            hardcore_allowed: true,
        };
        achievements.login_with_token();
        achievements
    }

    fn send(&mut self, api: &str, fields: &[(&str, String)], pending: Pending) {
        let id = self.next_id;
        self.next_id += 1;
        let mut all = vec![("r".to_string(), api.to_string())];
        if let UserState::LoggedIn { name, .. } = &self.user
            && api != "login2"
        {
            all.push(("u".to_string(), name.clone()));
            all.push(("t".to_string(), self.settings.token.clone()));
        }
        all.extend(fields.iter().map(|(k, v)| (k.to_string(), v.clone())));
        self.pending.insert(id, pending);
        self.transport.send(Request { id, fields: all });
    }

    fn login_with_token(&mut self) {
        if self.settings.enabled
            && !self.settings.username.is_empty()
            && !self.settings.token.is_empty()
        {
            self.user = UserState::LoggingIn;
            let fields = [
                ("u", self.settings.username.clone()),
                ("t", self.settings.token.clone()),
            ];
            self.send("login2", &fields, Pending::Login { token: true });
        }
    }

    /// Log in with the password, which isn't kept: the token the site gives
    /// is.
    pub fn login(&mut self, username: &str, password: &str) {
        self.settings.username = username.trim().to_string();
        self.user = UserState::LoggingIn;
        let fields = [
            ("u", self.settings.username.clone()),
            ("p", password.to_string()),
        ];
        self.send("login2", &fields, Pending::Login { token: false });
    }

    pub fn logout(&mut self) {
        self.settings.token.clear();
        self.new_token = Some(String::new());
        self.user = UserState::LoggedOut;
        if let GameState::Playing(game) = &self.game {
            let (hash, name) = (game.hash.clone(), game.title.clone());
            self.game = GameState::Loading { hash, name };
        }
    }

    /// Settings changed: turned on or off, or hardcore mode for the next
    /// game.
    pub fn apply(&mut self, settings: &AchievementSettings) {
        let was = self.settings.enabled;
        self.settings.enabled = settings.enabled;
        self.settings.hardcore = settings.hardcore;
        if settings.enabled && !was {
            if settings.username != self.settings.username || settings.token != self.settings.token
            {
                self.settings.username = settings.username.clone();
                self.settings.token = settings.token.clone();
            }
            self.login_with_token();
        } else if !settings.enabled && was {
            self.user = UserState::LoggedOut;
            if let GameState::Playing(game) = &self.game {
                let (hash, name) = (game.hash.clone(), game.title.clone());
                self.game = GameState::Loading { hash, name };
            }
        }
    }

    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// A token to save (empty: logged out), after logging in or out.
    pub fn take_new_token(&mut self) -> Option<(String, String)> {
        self.new_token
            .take()
            .map(|token| (self.settings.username.clone(), token))
    }

    pub fn logged_in(&self) -> bool {
        matches!(self.user, UserState::LoggedIn { .. })
    }

    /// Whether a game's logic runs, to be checked every frame.
    pub fn checking(&self) -> bool {
        self.settings.enabled && matches!(&self.game, GameState::Playing(g) if g.started)
    }

    /// The values of the leaderboards being attempted.
    pub fn trackers(&self) -> Vec<String> {
        match &self.game {
            GameState::Playing(game) if self.settings.enabled => game.trackers(),
            _ => Vec::new(),
        }
    }

    /// Whether hardcore mode is on for the game playing: no states, no
    /// rewind, no cheats.
    pub fn hardcore_active(&self) -> bool {
        matches!(&self.game, GameState::Playing(g) if g.hardcore)
    }

    /// A game started: `hash` is what the site knows it by, if it has one.
    pub fn game_started(&mut self, hash: Option<String>, name: &str) {
        self.game = match hash {
            None => GameState::NoHash,
            Some(hash) => GameState::Loading {
                hash,
                name: name.to_string(),
            },
        };
        self.load_game();
    }

    pub fn game_ended(&mut self) {
        self.game = GameState::None;
        self.retries
            .retain(|r| matches!(r.pending, Pending::Award { .. } | Pending::Submit { .. }));
    }

    /// Ask the site for the game waiting to load, when logged in.
    fn load_game(&mut self) {
        if !self.settings.enabled || !self.logged_in() {
            return;
        }
        if let GameState::Loading { hash, name } = &self.game {
            let (hash, name) = (hash.clone(), name.clone());
            self.send(
                "achievementsets",
                &[("m", hash.clone()), ("v", "2".to_string())],
                Pending::Game { hash, name },
            );
        }
    }

    /// A state was loaded or the machine started over: the logic starts
    /// again from waiting, as the progress it had is gone.
    pub fn reset(&mut self) {
        if let GameState::Playing(game) = &mut self.game {
            game.runtime.reset();
            game.trackers.clear();
        }
    }

    /// Answers from the site, and retries due.
    pub fn poll(&mut self) {
        for (id, answer) in self.transport.poll() {
            if let Some(pending) = self.pending.remove(&id) {
                self.answer(pending, answer);
            }
        }
        let now = Instant::now();
        let due: Vec<Retry> = {
            let (due, later) = std::mem::take(&mut self.retries)
                .into_iter()
                .partition(|r| r.at <= now);
            self.retries = later;
            due
        };
        for retry in due {
            match retry.pending {
                Pending::Award {
                    id,
                    hardcore,
                    attempt,
                } => self.send_award(id, hardcore, attempt),
                Pending::Submit { id, value, attempt } => self.send_submit(id, value, attempt),
                _ => {}
            }
        }
        // What the player is doing, for the settings window every second
        // and the site every two minutes.
        if let GameState::Playing(game) = &mut self.game
            && game.started
            && now >= game.next_presence
        {
            game.next_presence = now + Duration::from_secs(1);
            game.rich_presence = game.runtime.richpresence().unwrap_or_default();
        }
        if let GameState::Playing(game) = &mut self.game
            && game.started
            && now >= game.next_ping
        {
            game.next_ping = now + PING_INTERVAL;
            let presence = game.runtime.richpresence().unwrap_or_default();
            game.rich_presence = presence.clone();
            let mut fields = vec![("g", game.id.to_string())];
            if !presence.is_empty() {
                fields.push(("m", presence));
            }
            fields.push(("h", (game.hardcore as u8).to_string()));
            fields.push(("x", game.hash.clone()));
            self.send("ping", &fields, Pending::Ping);
        }
    }

    fn answer(&mut self, pending: Pending, answer: Result<String, String>) {
        let parsed = answer.clone().and_then(|body| parse_answer(&body));
        match pending {
            Pending::Login { token } => match parsed {
                Ok(v) => {
                    let name = str_of(&v, "User");
                    let new_token = str_of(&v, "Token");
                    if !new_token.is_empty() && new_token != self.settings.token {
                        self.settings.token = new_token.clone();
                        self.new_token = Some(new_token);
                    }
                    if !name.is_empty() {
                        self.settings.username = name.clone();
                    }
                    let (score, softcore_score) = (
                        num_of(&v, "Score") as u32,
                        num_of(&v, "SoftcoreScore") as u32,
                    );
                    self.notices.push(Notice::new(
                        format!("Logged in to RetroAchievements as {}", name),
                        format!("{} points ({} softcore)", score, softcore_score),
                    ));
                    self.user = UserState::LoggedIn {
                        name,
                        score,
                        softcore_score,
                    };
                    self.load_game();
                }
                Err(e) => {
                    // A token the site refuses is forgotten.
                    if token && refused(&answer) {
                        self.settings.token.clear();
                        self.new_token = Some(String::new());
                    }
                    self.notices
                        .push(Notice::new("RetroAchievements: can't log in", e.clone()));
                    self.user = UserState::Failed(e);
                }
            },
            Pending::Game { hash, name } => {
                // Only for the game still waiting for it.
                if !matches!(&self.game, GameState::Loading { hash: h, .. } if *h == hash) {
                    return;
                }
                let not_found = answer
                    .as_ref()
                    .ok()
                    .and_then(|b| serde_json::from_str::<Value>(b).ok())
                    .is_some_and(|v| {
                        str_of(&v, "Code") == "not_found"
                            || str_of(&v, "Error").to_ascii_lowercase().contains("unknown")
                    });
                match parsed {
                    Ok(v) if num_of(&v, "GameId") != 0 => self.game_loaded(&v, hash),
                    Ok(_) => self.unknown_game(hash, name),
                    Err(_) if not_found => self.unknown_game(hash, name),
                    Err(e) => {
                        self.notices.push(Notice::new(
                            "RetroAchievements: the game can't be loaded",
                            e.clone(),
                        ));
                        self.game = GameState::Failed(e);
                    }
                }
            }
            Pending::Session => match parsed {
                Ok(v) => self.session_started(&v),
                Err(e) => {
                    self.notices.push(Notice::new(
                        "RetroAchievements: the session didn't start",
                        e.clone(),
                    ));
                    self.game = GameState::Failed(e);
                }
            },
            Pending::Award {
                id,
                hardcore,
                attempt,
            } => match (refused(&answer), parsed) {
                (_, Ok(v)) => self.awarded(&v),
                (_, Err(e)) if e.starts_with("User already has") => {}
                (false, Err(_)) if attempt < 10 => self.retry(
                    Pending::Award {
                        id,
                        hardcore,
                        attempt: attempt + 1,
                    },
                    attempt,
                ),
                (_, Err(e)) => {
                    let title = self.achievement_title(id);
                    self.notices.push(Notice::new(
                        format!("The unlock of {} didn't reach the site", title),
                        e,
                    ));
                }
            },
            Pending::Submit { id, value, attempt } => match (refused(&answer), parsed) {
                (_, Ok(v)) => self.submitted(id, &v),
                (false, Err(_)) if attempt < 10 => self.retry(
                    Pending::Submit {
                        id,
                        value,
                        attempt: attempt + 1,
                    },
                    attempt,
                ),
                (_, Err(e)) => self.notices.push(Notice::new(
                    "The leaderboard entry didn't reach the site",
                    e,
                )),
            },
            Pending::Ping => {}
        }
    }

    fn retry(&mut self, pending: Pending, attempt: u32) {
        // 1, 2, 4... seconds, up to two minutes.
        let delay = Duration::from_secs((1u64 << attempt.min(7)).min(120));
        self.retries.push(Retry {
            at: Instant::now() + delay,
            pending,
        });
    }

    fn unknown_game(&mut self, hash: String, name: String) {
        self.notices.push(Notice::new(
            format!("RetroAchievements doesn't know {}", name),
            format!(
                "No achievements for this version of the game (hash {})",
                hash
            ),
        ));
        self.game = GameState::Unknown { hash, name };
    }

    fn game_loaded(&mut self, v: &Value, hash: String) {
        let mut runtime = Runtime::new();
        let mut achievements = Vec::new();
        let mut leaderboards = Vec::new();
        let mut broken = 0;
        let sets: Vec<&Value> = v
            .get("Sets")
            .and_then(Value::as_array)
            .map(|s| s.iter().collect())
            .unwrap_or_default();
        for set in sets {
            for a in set
                .get("Achievements")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let id = num_of(a, "ID") as u32;
                let official = num_of(a, "Flags") == CORE;
                let mut title = str_of(a, "Title");
                let mut kind = str_of(a, "Type");
                // The old way of marking missable ones.
                if let Some(rest) = title.strip_prefix("[m]") {
                    title = rest.trim_start().to_string();
                    kind = "missable".to_string();
                } else if let Some(rest) = title.strip_suffix("[m]") {
                    title = rest.trim_end().to_string();
                    kind = "missable".to_string();
                }
                if runtime.add_achievement(id, &str_of(a, "MemAddr")).is_err() {
                    broken += 1;
                    continue;
                }
                achievements.push(AchievementInfo {
                    id,
                    title,
                    description: str_of(a, "Description"),
                    points: num_of(a, "Points") as u32,
                    badge: str_of(a, "BadgeName"),
                    official,
                    unlocked: false,
                    kind,
                });
            }
            for l in set
                .get("Leaderboards")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let id = num_of(l, "ID") as u32;
                if runtime.add_leaderboard(id, &str_of(l, "Mem")).is_err() {
                    broken += 1;
                    continue;
                }
                leaderboards.push(LeaderboardInfo {
                    id,
                    title: str_of(l, "Title"),
                    description: str_of(l, "Description"),
                    format: Format::parse(&str_of(l, "Format")),
                    lower_is_better: l
                        .get("LowerIsBetter")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    hidden: l.get("Hidden").and_then(Value::as_bool).unwrap_or(false),
                });
            }
        }
        let script = str_of(v, "RichPresencePatch");
        if !script.is_empty() && runtime.set_richpresence(&script).is_err() {
            broken += 1;
        }
        if broken > 0 {
            self.notices.push(Notice::new(
                "RetroAchievements",
                format!("{} definitions of this game can't be read", broken),
            ));
        }
        let id = num_of(v, "GameId") as u32;
        let hardcore = self.settings.hardcore && self.hardcore_allowed;
        if self.settings.hardcore && !self.hardcore_allowed {
            self.notices.push(Notice::new(
                "Hardcore mode is off",
                "It needs the debug server off",
            ));
        }
        let game = Game {
            id,
            title: str_of(v, "Title"),
            hash: hash.clone(),
            hardcore,
            achievements,
            leaderboards,
            runtime,
            started: false,
            next_ping: Instant::now(),
            next_presence: Instant::now(),
            trackers: Vec::new(),
            rich_presence: String::new(),
        };
        self.game = GameState::Playing(Box::new(game));
        self.send(
            "startsession",
            &[
                ("g", id.to_string()),
                ("h", (hardcore as u8).to_string()),
                ("m", hash),
            ],
            Pending::Session,
        );
    }

    fn session_started(&mut self, v: &Value) {
        let GameState::Playing(game) = &mut self.game else {
            return;
        };
        let ids = |key: &str| -> Vec<u32> {
            v.get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|u| num_of(u, "ID") as u32)
                .collect()
        };
        // Softcore counts hardcore unlocks too.
        let mut unlocked = ids("HardcoreUnlocks");
        if !game.hardcore {
            unlocked.extend(ids("Unlocks"));
        }
        for a in &mut game.achievements {
            if unlocked.contains(&a.id) {
                a.unlocked = true;
                game.runtime.deactivate(a.id);
            }
        }
        game.started = true;
        game.next_ping = Instant::now() + PING_INTERVAL;
        let (have, of, points, all_points) = game.progress();
        let mode = if game.hardcore {
            "hardcore"
        } else {
            "softcore"
        };
        let detail = if of == 0 {
            "This game has no achievements yet".to_string()
        } else {
            format!(
                "{} of {} achievements unlocked, {} of {} points ({})",
                have, of, points, all_points, mode
            )
        };
        self.notices.push(Notice::new(game.title.clone(), detail));
    }

    fn achievement_title(&self, id: u32) -> String {
        match &self.game {
            GameState::Playing(game) => game
                .achievements
                .iter()
                .find(|a| a.id == id)
                .map_or(id.to_string(), |a| a.title.clone()),
            _ => id.to_string(),
        }
    }

    fn awarded(&mut self, v: &Value) {
        if let UserState::LoggedIn {
            score,
            softcore_score,
            ..
        } = &mut self.user
        {
            if v.get("Score").is_some() {
                *score = num_of(v, "Score") as u32;
            }
            if v.get("SoftcoreScore").is_some() {
                *softcore_score = num_of(v, "SoftcoreScore") as u32;
            }
        }
    }

    fn submitted(&mut self, id: u32, v: &Value) {
        let Some(response) = v.get("Response") else {
            return;
        };
        let rank = response
            .get("RankInfo")
            .map(|r| (num_of(r, "Rank"), num_of(r, "NumEntries")));
        let title = match &self.game {
            GameState::Playing(game) => game
                .leaderboards
                .iter()
                .find(|l| l.id == id)
                .map_or(String::new(), |l| l.title.clone()),
            _ => String::new(),
        };
        if let Some((rank, entries)) = rank {
            self.notices.push(Notice::new(
                format!("Leaderboard: {}", title),
                format!("Rank {} of {}", rank, entries),
            ));
        }
    }

    fn send_award(&mut self, id: u32, hardcore: bool, attempt: u32) {
        let (GameState::Playing(game), UserState::LoggedIn { name, .. }) = (&self.game, &self.user)
        else {
            return;
        };
        let h = (hardcore as u8).to_string();
        let signature = md5_hex(&format!("{}{}{}", id, name, h));
        let fields = [
            ("a", id.to_string()),
            ("h", h),
            ("m", game.hash.clone()),
            ("v", signature),
        ];
        self.send(
            "awardachievement",
            &fields,
            Pending::Award {
                id,
                hardcore,
                attempt,
            },
        );
    }

    fn send_submit(&mut self, id: u32, value: i32, attempt: u32) {
        let (GameState::Playing(game), UserState::LoggedIn { name, .. }) = (&self.game, &self.user)
        else {
            return;
        };
        let signature = md5_hex(&format!("{}{}{}", id, name, value));
        let fields = [
            ("i", id.to_string()),
            ("s", value.to_string()),
            ("m", game.hash.clone()),
            ("v", signature),
        ];
        self.send(
            "submitlbentry",
            &fields,
            Pending::Submit { id, value, attempt },
        );
    }

    /// Check the game's logic against memory for a frame the machine ran.
    pub fn do_frame(&mut self, ram: &[u8], booted: bool) {
        let GameState::Playing(game) = &mut self.game else {
            return;
        };
        if !game.started || !self.settings.enabled {
            return;
        }
        let map = MemoryMap::of(ram, booted);
        let peek = |address: u32| map.peek(ram, address);
        let events = game.runtime.do_frame(&peek, game.hardcore);
        let mut awards = Vec::new();
        let mut submits = Vec::new();
        for event in events {
            match event {
                Event::Triggered(id) => {
                    let Some(a) = game.achievements.iter_mut().find(|a| a.id == id) else {
                        continue;
                    };
                    if id >= WARNING_ID {
                        // The site's warning, shown as it is.
                        self.notices
                            .push(Notice::new(a.title.clone(), a.description.clone()));
                        continue;
                    }
                    a.unlocked = true;
                    let mut notice = Notice::new(
                        format!("Achievement unlocked: {}", a.title),
                        format!("{} ({} points)", a.description, a.points),
                    );
                    notice.big = true;
                    if !a.official {
                        notice.title = format!("Unofficial achievement: {}", a.title);
                    } else {
                        awards.push(id);
                    }
                    self.notices.push(notice);
                    let (have, of, points, _) = game.progress();
                    if of > 0 && have == of && awards.contains(&id) {
                        let mode = if game.hardcore {
                            "Mastered"
                        } else {
                            "Completed"
                        };
                        let mut notice = Notice::new(
                            format!("{} {}", mode, game.title),
                            format!("All {} achievements, {} points", of, points),
                        );
                        notice.big = true;
                        self.notices.push(notice);
                    }
                }
                Event::Progress {
                    id,
                    value,
                    target,
                    percent,
                } => {
                    if let Some(a) = game.achievements.iter().find(|a| a.id == id) {
                        let shown = if percent {
                            format!("{}%", value as u64 * 100 / target.max(1) as u64)
                        } else {
                            format!("{}/{}", value, target)
                        };
                        self.notices.push(Notice::new(a.title.clone(), shown));
                    }
                }
                Event::Primed(id) => {
                    if let Some(a) = game.achievements.iter().find(|a| a.id == id) {
                        self.notices.push(Notice::new(
                            format!("Challenge: {}", a.title),
                            a.description.clone(),
                        ));
                    }
                }
                Event::Unprimed(_) => {}
                Event::LeaderboardStarted(id) => {
                    if let Some(l) = game
                        .leaderboards
                        .iter()
                        .find(|l| l.id == id)
                        .filter(|l| !l.hidden)
                    {
                        self.notices.push(Notice::new(
                            format!("Leaderboard started: {}", l.title),
                            l.description.clone(),
                        ));
                    }
                    let value = game.runtime.leaderboard_value(id).unwrap_or(0);
                    game.trackers.retain(|t| t.0 != id);
                    game.trackers.push((id, value));
                }
                Event::LeaderboardUpdated { id, value } => {
                    if let Some(t) = game.trackers.iter_mut().find(|t| t.0 == id) {
                        t.1 = value;
                    }
                }
                Event::LeaderboardCanceled(id) => {
                    game.trackers.retain(|t| t.0 != id);
                    if let Some(l) = game
                        .leaderboards
                        .iter()
                        .find(|l| l.id == id)
                        .filter(|l| !l.hidden)
                    {
                        self.notices.push(Notice::new(
                            format!("Leaderboard failed: {}", l.title),
                            l.description.clone(),
                        ));
                    }
                }
                Event::LeaderboardSubmitted { id, value } => {
                    game.trackers.retain(|t| t.0 != id);
                    if let Some(l) = game.leaderboards.iter().find(|l| l.id == id) {
                        if !l.hidden {
                            let shown = l.format.show(super::runtime::typed::Typed::signed(value));
                            self.notices.push(Notice::new(
                                format!("Leaderboard: {}", l.title),
                                format!("Submitted {}", shown),
                            ));
                        }
                        submits.push((id, value));
                    }
                }
            }
        }
        let hardcore = game.hardcore;
        for id in awards {
            self.send_award(id, hardcore, 0);
        }
        for (id, value) in submits {
            self.send_submit(id, value, 0);
        }
    }

    /// What the settings window shows.
    pub fn view(&self) -> AchievementsView {
        let user = match &self.user {
            UserState::LoggedIn {
                name,
                score,
                softcore_score,
            } if self.settings.enabled => Some((name.clone(), *score, *softcore_score)),
            _ => None,
        };
        let game = match &self.game {
            GameState::Playing(game) if self.settings.enabled && game.started => Some(GameView {
                title: game.title.clone(),
                hardcore: game.hardcore,
                rich_presence: game.rich_presence.clone(),
                achievements: game
                    .achievements
                    .iter()
                    .filter(|a| a.id < WARNING_ID)
                    .map(|a| AchievementRow {
                        title: a.title.clone(),
                        description: a.description.clone(),
                        points: a.points,
                        unlocked: a.unlocked,
                        official: a.official,
                        progress: if a.unlocked {
                            None
                        } else {
                            game.measured(a.id)
                        },
                        primed: !a.unlocked && game.is_primed(a.id),
                    })
                    .collect(),
                leaderboards: game
                    .leaderboards
                    .iter()
                    .filter(|l| !l.hidden)
                    .map(|l| (l.title.clone(), l.description.clone()))
                    .collect(),
            }),
            _ => None,
        };
        AchievementsView {
            status: self.status(),
            user,
            logging_in: self.user == UserState::LoggingIn,
            game,
        }
    }

    /// A status line for the settings window.
    pub fn status(&self) -> String {
        if !self.settings.enabled {
            return "Off".to_string();
        }
        match &self.user {
            UserState::LoggedOut => return "Not logged in".to_string(),
            UserState::LoggingIn => return "Logging in...".to_string(),
            UserState::Failed(e) => return format!("Can't log in: {}", e),
            UserState::LoggedIn { .. } => {}
        }
        match &self.game {
            GameState::None => {
                "No game: achievements are for games launched from their profiles".to_string()
            }
            GameState::NoHash => {
                "This game's profile doesn't say which version it is (achievements=)".to_string()
            }
            GameState::Loading { name, .. } => format!("Loading {}...", name),
            GameState::Unknown { hash, .. } => format!(
                "RetroAchievements doesn't know this version (hash {})",
                hash
            ),
            GameState::Failed(e) => format!("The game can't be loaded: {}", e),
            GameState::Playing(game) if !game.started => format!("Starting {}...", game.title),
            GameState::Playing(game) => {
                let (have, of, points, all) = game.progress();
                let mode = if game.hardcore {
                    "hardcore"
                } else {
                    "softcore"
                };
                format!(
                    "{}: {} of {} unlocked, {}/{} points ({})",
                    game.title, have, of, points, all, mode
                )
            }
        }
    }
}

/// Whether the site answered, and said no (not an error page or a
/// network failure).
fn refused(answer: &Result<String, String>) -> bool {
    answer
        .as_ref()
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(body).ok())
        .is_some_and(|v| v.get("Success").and_then(Value::as_bool) == Some(false))
}

#[cfg(test)]
mod tests;
