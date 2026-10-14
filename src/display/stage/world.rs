//! What changes in the room while it is shown, shared by the window's
//! view, the headset's thread and the sound: which lamps are on, the time
//! of day, the weather and its lightning, a camera's flash. Each view
//! works out from it what it draws at a moment (`Ambience`).

use super::super::room_sound::Effect;
use super::scene::Scene;
use super::sky::{self, SkyLook};
use glam::Vec3;
use rust_dos::vr::{VrSettings, VrTime, VrWeather};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a lamp takes to come on or go off.
const FADE: f32 = 0.12;

/// How long a camera's flash lights the room.
pub const FLASH: Duration = Duration::from_millis(90);

/// What a view draws at a moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Ambience {
    pub sky: SkyLook,
    /// How bright each of the scene's lamps is, 0 (off) to 1.
    pub lamps: Vec<f32>,
    pub weather: Weather,
    /// Seconds since the room was first shown, for what moves by itself
    /// (rain, snow); wrapped every hour.
    pub seconds: f32,
    /// A camera's flash: where, which way, and how bright (0 to 1).
    pub flash: Option<(Vec3, Vec3, f32)>,
}

impl Ambience {
    /// The scene as it was before there were times of day: the test room's
    /// sunset, the lamps as the scene has them.
    pub fn of(scene: &Scene) -> Self {
        Ambience {
            sky: sky::look(sky::SUNSET_HOURS, 48.0, scene.heading),
            lamps: scene.lamps.iter().map(|l| if l.on { 1.0 } else { 0.0 }).collect(),
            weather: Weather::default(),
            seconds: 0.0,
            flash: None,
        }
    }
}

/// How much rain or snow falls (0 to 1 each).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Weather {
    pub rain: f32,
    pub snow: f32,
}

/// A lightning strike: when (seconds of the room's clock), how far, in
/// kilometres (its thunder comes about 3 s a km after), and which way
/// across the sky it is (radians around).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strike {
    pub at: f64,
    pub distance: f32,
    pub around: f32,
}

impl Strike {
    pub fn thunder_at(&self) -> f64 {
        self.at + self.distance as f64 * 2.9
    }

    /// Its flash's brightness `t` seconds after it: a few flickers.
    pub fn flash(&self, t: f64) -> f32 {
        if !(0.0..0.45).contains(&t) {
            return 0.0;
        }
        let near = (1.0 - self.distance / 8.0).clamp(0.25, 1.0);
        let flicker = |start: f64, length: f64| if (start..start + length).contains(&t) { 1.0 - (t - start) / length } else { 0.0 };
        (flicker(0.0, 0.08).max(flicker(0.13, 0.06) * 0.7).max(flicker(0.24, 0.2) * 0.9) as f32) * near
    }
}

/// The storm's strikes: one in every second's slot or none, by a hash of
/// the slot, about one in `EVERY` seconds.
pub fn strikes_between(from: f64, to: f64) -> Vec<Strike> {
    const EVERY: u64 = 14;
    let mut out = Vec::new();
    let first = from.floor().max(0.0) as u64;
    let last = to.ceil().max(0.0) as u64;
    for slot in first..=last {
        let h = hash(slot);
        if h % EVERY != 0 {
            continue;
        }
        let at = slot as f64 + (h >> 8 & 0xFF) as f64 / 256.0;
        if (from..to).contains(&at) {
            let distance = 0.4 + (h >> 16 & 0xFF) as f32 / 255.0 * 5.0;
            let around = (h >> 24 & 0xFFFF) as f32 / 65535.0 * std::f32::consts::TAU;
            out.push(Strike { at, distance, around });
        }
    }
    out
}

fn hash(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// The settings the room follows.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    time: VrTime,
    speed: u32,
    latitude: i32,
    weather: VrWeather,
    sound: u32,
    lightning: bool,
}

impl Settings {
    fn of(vr: &VrSettings) -> Self {
        Settings {
            time: vr.time,
            speed: vr.time_speed,
            latitude: vr.latitude,
            weather: vr.weather,
            sound: vr.weather_sound,
            lightning: vr.lightning,
        }
    }
}

struct State {
    settings: Settings,
    /// The time of day (hours) at `since`, from which it goes at the
    /// settings' speed.
    hours: f64,
    since: Instant,
    /// The scene the lamps are of, and each lamp's switch and when it was
    /// last switched.
    scene: u64,
    lamps: Vec<(bool, Option<Instant>)>,
    flash: Option<(Instant, Vec3, Vec3)>,
    /// How much it rains or snows, easing towards the weather's.
    weather: Weather,
    eased: Instant,
    /// Sounds to play once, and up to when on the room's clock the
    /// thunder was heard.
    effects: Vec<Effect>,
    heard: f64,
}

pub struct World {
    start: Instant,
    state: Mutex<State>,
}

impl World {
    pub fn new(settings: &VrSettings) -> Self {
        let now = Instant::now();
        let settings = Settings::of(settings);
        let weather = target_weather(settings.weather);
        World {
            start: now,
            state: Mutex::new(State {
                settings,
                hours: start_hours(settings.time),
                since: now,
                scene: 0,
                lamps: Vec::new(),
                flash: None,
                weather,
                eased: now,
                effects: Vec::new(),
                heard: 0.0,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Follow the `[vr]` settings: a time set starts the day there again;
    /// another speed goes on from the time it is.
    pub fn apply(&self, settings: &VrSettings) {
        let now = Instant::now();
        let new = Settings::of(settings);
        let mut state = self.lock();
        let old = state.settings;
        if new.time != old.time {
            state.hours = start_hours(new.time);
            state.since = now;
        } else if new.speed != old.speed {
            state.hours = hours_at(&state, now);
            state.since = now;
        }
        state.settings = new;
    }

    /// The lamps of `scene` as it has them, if it is another than before.
    pub fn set_scene(&self, scene: &Scene) {
        let mut state = self.lock();
        if state.scene != scene.source || state.lamps.len() != scene.lamps.len() {
            state.scene = scene.source;
            state.lamps = scene.lamps.iter().map(|l| (l.on, None)).collect();
        }
    }

    /// Switch lamp `i` on or off.
    pub fn toggle_lamp(&self, i: usize) {
        let mut state = self.lock();
        if let Some(lamp) = state.lamps.get_mut(i) {
            *lamp = (!lamp.0, Some(Instant::now()));
            state.effects.push(Effect::Click);
        }
    }

    /// A camera flashed at `at`, towards `ahead`.
    pub fn flash(&self, at: Vec3, ahead: Vec3) {
        let mut state = self.lock();
        state.flash = Some((Instant::now(), at, ahead));
        state.effects.push(Effect::Shutter);
    }

    /// Seconds of the room's clock at `now` (for the lightning).
    pub fn clock(&self, now: Instant) -> f64 {
        now.saturating_duration_since(self.start).as_secs_f64()
    }

    /// The room's sound at `now`: how loud the rain is and the thunder
    /// would be, and the sounds to play once since last asked (the
    /// thunder of strikes now heard among them).
    pub fn sound(&self, now: Instant) -> (f32, f32, Vec<Effect>) {
        let clock = self.clock(now);
        let mut state = self.lock();
        let volume = state.settings.sound as f32 / 100.0;
        let mut effects = std::mem::take(&mut state.effects);
        if state.settings.weather == VrWeather::Storm && state.settings.lightning {
            // (Strikes up to half a minute back, whose thunder comes now.)
            let heard = state.heard.max(clock - 1.0);
            for strike in strikes_between(heard - 30.0, clock) {
                if (heard..clock).contains(&strike.thunder_at()) {
                    effects.push(Effect::Thunder(strike.distance));
                }
            }
        }
        state.heard = clock;
        (state.weather.rain * volume, volume, effects)
    }

    /// The time of day at `now`, in hours from midnight.
    #[cfg(test)]
    pub fn hours(&self, now: Instant) -> f64 {
        hours_at(&self.lock(), now)
    }

    /// What `scene` looks like at `now`.
    pub fn ambience(&self, scene: &Scene, now: Instant) -> Ambience {
        let mut state = self.lock();
        let hours = hours_at(&state, now);
        // The rain and snow come and go over some seconds.
        let target = target_weather(state.settings.weather);
        let dt = now.saturating_duration_since(state.eased).as_secs_f32();
        state.eased = now;
        let ease = 1.0 - (-dt / 4.0).exp();
        state.weather.rain += (target.rain - state.weather.rain) * ease;
        state.weather.snow += (target.snow - state.weather.snow) * ease;
        let weather = state.weather;
        let clouds = (weather.rain * 0.9 + weather.snow * 0.75).min(1.0);
        let mut look = sky::look(hours as f32, state.settings.latitude as f32, scene.heading).overcast(clouds);
        let clock = self.clock(now);
        if state.settings.weather == VrWeather::Storm && state.settings.lightning {
            let flash = strikes_between(clock - 0.5, clock).iter().map(|s| s.flash(clock - s.at)).fold(0.0, f32::max);
            if flash > 0.0 {
                look = look.with_flash(flash);
            }
        }
        let lamps = state
            .lamps
            .iter()
            .map(|&(on, since)| {
                let t = since.map_or(1.0, |s| (now.saturating_duration_since(s).as_secs_f32() / FADE).min(1.0));
                if on { t } else { 1.0 - t }
            })
            .collect();
        let flash = state.flash.and_then(|(at, place, ahead)| {
            let t = now.saturating_duration_since(at);
            (t < FLASH).then(|| (place, ahead, 1.0 - t.as_secs_f32() / FLASH.as_secs_f32()))
        });
        Ambience { sky: look, lamps, weather, seconds: (clock % 3600.0) as f32, flash }
    }
}

/// The time of day `time` starts at, in hours.
fn start_hours(time: VrTime) -> f64 {
    match time {
        VrTime::Now => {
            use chrono::Timelike;
            let now = chrono::Local::now();
            now.hour() as f64 + now.minute() as f64 / 60.0 + now.second() as f64 / 3600.0
        }
        VrTime::At(minutes) => minutes as f64 / 60.0,
    }
}

fn hours_at(state: &State, now: Instant) -> f64 {
    let elapsed = now.saturating_duration_since(state.since).as_secs_f64() / 3600.0;
    (state.hours + elapsed * state.settings.speed as f64).rem_euclid(24.0)
}

fn target_weather(weather: VrWeather) -> Weather {
    match weather {
        VrWeather::Clear => Weather::default(),
        VrWeather::Rain => Weather { rain: 0.55, snow: 0.0 },
        VrWeather::Storm => Weather { rain: 1.0, snow: 0.0 },
        VrWeather::Snow => Weather { rain: 0.0, snow: 0.8 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_goes_at_its_speed_and_starts_again_when_set() {
        let settings = VrSettings { time: VrTime::At(6 * 60), time_speed: 3600, ..VrSettings::default() };
        let world = World::new(&settings);
        let start = world.lock().since;
        let at = |s: f64| world.hours(start + Duration::from_secs_f64(s));
        assert!((at(0.0) - 6.0).abs() < 1e-9);
        assert!((at(2.0) - 8.0).abs() < 1e-6, "an hour a second");
        assert!((at(20.0) - 2.0).abs() < 1e-6, "round past midnight");
        world.apply(&VrSettings { time_speed: 0, ..settings.clone() });
        let held = world.hours(Instant::now());
        assert!((world.hours(Instant::now() + Duration::from_secs(100)) - held).abs() < 1e-9, "stands still");
        world.apply(&VrSettings { time: VrTime::At(21 * 60 + 30), time_speed: 0, ..settings });
        assert!((world.hours(Instant::now()) - 21.5).abs() < 1e-9);
    }

    #[test]
    fn lamps_switch_and_fade() {
        let mut scene = Scene::test_room();
        scene.lamps.push(super::super::scene::Lamp::test(true));
        scene.lamps.push(super::super::scene::Lamp::test(false));
        let world = World::new(&VrSettings::default());
        world.set_scene(&scene);
        let now = Instant::now();
        assert_eq!(world.ambience(&scene, now).lamps, [1.0, 0.0]);
        world.toggle_lamp(1);
        let later = Instant::now() + Duration::from_secs(1);
        assert_eq!(world.ambience(&scene, later).lamps, [1.0, 1.0]);
        // The same scene again keeps them.
        world.set_scene(&scene);
        assert_eq!(world.ambience(&scene, later).lamps, [1.0, 1.0]);
    }

    #[test]
    fn strikes_come_now_and_then_and_agree_however_asked() {
        let all = strikes_between(0.0, 3600.0);
        assert!((150..400).contains(&all.len()), "{}", all.len());
        let pieces: Vec<Strike> = (0..3600).flat_map(|s| strikes_between(s as f64, s as f64 + 1.0)).collect();
        assert_eq!(all, pieces);
        let s = all[0];
        assert!(s.flash(0.01) > 0.0 && s.flash(1.0) == 0.0 && s.thunder_at() > s.at);
    }
}
