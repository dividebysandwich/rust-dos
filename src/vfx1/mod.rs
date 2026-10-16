//! The Forte VFX1 head-mounted display (`[vfx1]`): its VIP card on the ISA
//! bus, and the Sense driver (VFX1.COM) that games find on INT 33h, with the
//! head tracker and the CyberPuck behind it.
//!
//! What games do with them is in the VFX1 SDK and in what the
//! forte-vfx1-vr project (github.com/LeonG-ZA/forte-vfx1-vr) learnt from
//! VFX1.COM 2.14 and the games: the card is little more than a page select
//! register, which tells the headset which page of a 640x480 frame each eye
//! shows, and everything else goes through the driver (sense.rs). The
//! pictures each eye gets are taken from the screen on the host's side
//! (stereo.rs), as some games never tell the card what they draw.

pub mod bios3d;
pub mod host;
pub mod pose;
pub mod sense;
pub mod stereo;

use crate::savestate::{Reader, Result, State, Writer};

/// The VIP card's ports: 16 from its base.
pub const VIP_PORTS: u16 = 16;
/// The base ports Forte's installer offered.
pub const VIP_BASES: [u16; 4] = [0x260, 0x280, 0x2A0, 0x2C0];
/// Where some games write the card whatever `VIPPORT` says (System Shock's
/// CD version, Magic Carpet).
pub const VIP_ALIAS: u16 = 0x300;

/// The card's port offsets.
const AIR: u16 = 0x0;
const CSR: u16 = 0x1;
/// The indexed registers behind AIR and CSR that matter: the page select,
/// with the left eye's page in its high nibble and the right's in its low,
/// and the processor-ready register, which reads 01h.
pub const PSR: usize = 4;
const VRR: usize = 5;

/// The narrowest and widest the picture can be made, in degrees.
pub const VIEW_SIZE_MIN: u32 = 20;
pub const VIEW_SIZE_MAX: u32 = 110;

/// How the eyes' pictures are taken from the screen (`stereo`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vfx1Stereo {
    /// As the game tells the driver, or else pages while it uses the
    /// head tracker on a 640x480 screen, and the same picture for both
    /// eyes otherwise.
    Auto,
    /// The VFX1's multiple-buffer mode: four 320x240 pages in a 640x480
    /// frame, the left eye's on the left, each on the even or odd lines.
    Pages,
    /// Line-sequential: the even lines the left eye, the odd the right.
    Lines,
    /// The left half of the screen the left eye, the right half the right.
    Side,
    /// The whole screen for both eyes.
    Mono,
}

impl Vfx1Stereo {
    pub const ALL: [Vfx1Stereo; 5] = [
        Vfx1Stereo::Auto,
        Vfx1Stereo::Pages,
        Vfx1Stereo::Lines,
        Vfx1Stereo::Side,
        Vfx1Stereo::Mono,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.name().eq_ignore_ascii_case(s.trim()))
    }

    pub fn name(self) -> &'static str {
        match self {
            Vfx1Stereo::Auto => "auto",
            Vfx1Stereo::Pages => "pages",
            Vfx1Stereo::Lines => "lines",
            Vfx1Stereo::Side => "side",
            Vfx1Stereo::Mono => "mono",
        }
    }

    /// As the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            Vfx1Stereo::Auto => "auto (as the game says)",
            Vfx1Stereo::Pages => "VFX1 pages",
            Vfx1Stereo::Lines => "interlaced lines",
            Vfx1Stereo::Side => "side by side",
            Vfx1Stereo::Mono => "off (both eyes alike)",
        }
    }
}

/// The `[vfx1]` settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vfx1Settings {
    /// The card and its driver are there, and the headset is on the desk
    /// of the 3D scene.
    pub enabled: bool,
    /// The card's base port, which `VIPPORT` tells games.
    pub port: u16,
    pub stereo: Vfx1Stereo,
    /// The left eye's picture to the right eye and the other way round.
    pub swap_eyes: bool,
    /// How wide the picture looks, in degrees: the VFX1's was 35.5.
    pub view_size: u32,
    /// The head's turns made larger as the game's frame rate is higher
    /// than 30 per second, for games that steer by the head's movement
    /// between frames (Descent).
    pub steer: bool,
    /// The 3D BIOS of shutter glasses answers too (bios3d.rs), for games
    /// that draw interlaced stereo for it in modes larger than the VFX1's.
    pub stereo_bios: bool,
}

impl Default for Vfx1Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 0x260,
            stereo: Vfx1Stereo::Auto,
            swap_eyes: false,
            view_size: 36,
            steer: false,
            stereo_bios: true,
        }
    }
}

impl Vfx1Settings {
    pub fn set(&mut self, key: &str, value: &str) -> std::result::Result<(), String> {
        let bool_of = |key: &str| match value.trim().to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok(true),
            "false" | "off" | "no" | "0" => Ok(false),
            _ => Err(format!("invalid {} '{}' (true or false)", key, value)),
        };
        match key.to_ascii_lowercase().as_str() {
            "enabled" => self.enabled = bool_of("enabled")?,
            "port" => {
                self.port = u16::from_str_radix(value.trim().trim_end_matches(['h', 'H']), 16)
                    .ok()
                    .filter(|port| VIP_BASES.contains(port))
                    .ok_or_else(|| format!("invalid port '{}' (260, 280, 2A0 or 2C0)", value))?;
            }
            "stereo" => {
                self.stereo = Vfx1Stereo::parse(value).ok_or_else(|| {
                    format!(
                        "invalid stereo '{}' (auto, pages, lines, side or mono)",
                        value
                    )
                })?;
            }
            "swap_eyes" => self.swap_eyes = bool_of("swap_eyes")?,
            "view_size" => {
                self.view_size = value
                    .trim()
                    .trim_end_matches('°')
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .map(|degrees| degrees.round() as u32)
                    .filter(|degrees| (VIEW_SIZE_MIN..=VIEW_SIZE_MAX).contains(degrees))
                    .ok_or_else(|| {
                        format!(
                            "invalid view_size '{}' ({} to {} degrees)",
                            value, VIEW_SIZE_MIN, VIEW_SIZE_MAX
                        )
                    })?;
            }
            "steer" => self.steer = bool_of("steer")?,
            "stereo_bios" => self.stereo_bios = bool_of("stereo_bios")?,
            _ => return Err(format!("unknown setting '{}'", key)),
        }
        Ok(())
    }

    pub fn entries(&self) -> Vec<(&'static str, Option<String>)> {
        vec![
            ("enabled", Some(self.enabled.to_string())),
            ("port", Some(format!("{:X}", self.port))),
            ("stereo", Some(self.stereo.name().to_string())),
            ("swap_eyes", Some(self.swap_eyes.to_string())),
            ("view_size", Some(self.view_size.to_string())),
            ("steer", Some(self.steer.to_string())),
            ("stereo_bios", Some(self.stereo_bios.to_string())),
        ]
    }

    /// The environment variable games find the card by, while it is there.
    pub fn environment(&self) -> Vec<crate::env_inject::Rule> {
        vec![(
            "VIPPORT",
            self.enabled.then(|| format!("{:X} I5", self.port)),
        )]
    }
}

/// The head tracker and the CyberPuck as the host has them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tracking {
    /// Yaw (left), pitch (up) and roll (right), 65536 to a turn.
    pub head: [i16; 3],
    /// The puck's pitch and roll, likewise.
    pub puck: [i16; 2],
    /// Its three buttons, bit 0 the first.
    pub buttons: u8,
}

/// What the game has told the headset, for the host to show it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// The driver's video mode: 0 mono, 1 multiple-buffer, 2 line-sequential.
    pub video_mode: u8,
    /// The page select register.
    pub page_select: u8,
    /// How often it was written, and how often the head tracker was read;
    /// they only count up.
    pub page_writes: u32,
    pub polls: u32,
    /// The 3D BIOS's interlaced display or glasses are on.
    pub glasses: bool,
}

/// The card and the driver's state.
#[derive(Clone, Debug, PartialEq)]
pub struct Vfx1 {
    pub base: u16,
    /// The card's index register, its indexed registers, and the last byte
    /// written to each of its other ports.
    air: u8,
    regs: [u8; 16],
    ports: [u8; 16],
    pub(crate) driver: sense::Driver,
    /// From the host; not part of a saved state.
    tracking: Tracking,
    steer: Option<pose::Steer>,
    pub(crate) activity: Activity,
    /// The 3D BIOS answers (`Vfx1Settings::stereo_bios`), and what it has
    /// on (`bios3d::INTERLACE`, `GLASSES`).
    pub stereo_bios: bool,
    pub(crate) glasses: u8,
}

impl Vfx1 {
    pub fn new(base: u16) -> Self {
        let mut regs = [0; 16];
        regs[VRR] = 0x01;
        Self {
            base,
            air: 0,
            regs,
            ports: [0; 16],
            driver: sense::Driver::default(),
            tracking: Tracking::default(),
            steer: None,
            activity: Activity::default(),
            stereo_bios: true,
            glasses: 0,
        }
    }

    /// Whether `port` is the card's: at its base, or at 300h.
    #[inline]
    pub fn claims(&self, port: u16) -> bool {
        port & !(VIP_PORTS - 1) == self.base || port & !(VIP_PORTS - 1) == VIP_ALIAS
    }

    pub fn read(&self, port: u16) -> u8 {
        match port & (VIP_PORTS - 1) {
            AIR => self.air,
            CSR => self.regs[self.air as usize & 15],
            offset => self.ports[offset as usize],
        }
    }

    pub fn write(&mut self, port: u16, value: u8) {
        let offset = port & (VIP_PORTS - 1);
        self.ports[offset as usize] = value;
        match offset {
            AIR => self.air = value,
            CSR => {
                let index = self.air as usize & 15;
                if index != VRR {
                    self.regs[index] = value;
                }
                if index == PSR {
                    self.page_select(value);
                }
            }
            _ => {}
        }
    }

    /// The page select register written, through the card or the driver.
    pub(crate) fn page_select(&mut self, value: u8) {
        self.regs[PSR] = value;
        self.activity.page_select = value;
        self.activity.page_writes = self.activity.page_writes.wrapping_add(1);
    }

    /// The head and the puck as they are now. `steer` makes the head's
    /// turns larger as the game reads them less often (`Vfx1Settings::steer`).
    pub fn set_tracking(&mut self, tracking: Tracking, steer: bool) {
        self.tracking = tracking;
        if !steer {
            self.steer = None;
        } else if self.steer.is_none() {
            self.steer = Some(pose::Steer::default());
        }
    }

    /// The head as the game reads it at `now_ms` of the machine's time,
    /// before the driver's zero.
    pub(crate) fn poll_head(&mut self, now_ms: f64) -> [i16; 3] {
        self.activity.polls = self.activity.polls.wrapping_add(1);
        match &mut self.steer {
            Some(steer) => steer.update(self.tracking.head, now_ms),
            None => self.tracking.head,
        }
    }

    pub(crate) fn tracking(&self) -> &Tracking {
        &self.tracking
    }

    pub fn activity(&self) -> Activity {
        Activity {
            video_mode: self.driver.video_mode,
            glasses: self.glasses != 0,
            ..self.activity
        }
    }
}

impl State for Vfx1 {
    fn save(&self, w: &mut Writer) {
        let Self {
            base,
            air,
            regs,
            ports,
            driver,
            tracking: _,
            steer: _,
            activity,
            stereo_bios: _,
            glasses,
        } = self;
        base.save(w);
        air.save(w);
        regs.save(w);
        ports.save(w);
        driver.save(w);
        activity.page_select.save(w);
        glasses.save(w);
    }

    fn load(&mut self, r: &mut Reader) -> Result<()> {
        // The base is the settings'; a state's is read past.
        let mut base = 0u16;
        base.load(r)?;
        self.air.load(r)?;
        self.regs.load(r)?;
        self.ports.load(r)?;
        self.driver.load(r)?;
        self.activity.page_select.load(r)?;
        self.glasses.load(r)?;
        self.steer = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let mut settings = Vfx1Settings::default();
        for (key, value) in [
            ("enabled", "true"),
            ("port", "2A0"),
            ("stereo", "lines"),
            ("swap_eyes", "yes"),
            ("view_size", "60"),
            ("steer", "on"),
            ("stereo_bios", "false"),
        ] {
            settings.set(key, value).unwrap();
        }
        let mut again = Vfx1Settings::default();
        for (key, value) in settings.entries() {
            again.set(key, &value.unwrap()).unwrap();
        }
        assert_eq!(again, settings);
        assert_eq!(
            settings.environment(),
            vec![("VIPPORT", Some("2A0 I5".to_string()))]
        );
        assert!(settings.set("port", "300").is_err());
        assert!(settings.set("view_size", "200").is_err());
    }

    #[test]
    fn vip_registers() {
        let mut vip = Vfx1::new(0x260);
        assert!(vip.claims(0x260) && vip.claims(0x26F) && vip.claims(0x303));
        assert!(!vip.claims(0x270) && !vip.claims(0x25F));
        // VRR reads ready and can't be written.
        vip.write(0x260, VRR as u8);
        vip.write(0x261, 0);
        assert_eq!(vip.read(0x261), 0x01);
        // PSR through AIR and CSR, at the alias too.
        vip.write(0x300, PSR as u8);
        vip.write(0x301, 0x01);
        assert_eq!(vip.read(0x261), 0x01);
        assert_eq!(vip.read(0x260), PSR as u8);
        assert_eq!(vip.activity().page_writes, 1);
        assert_eq!(vip.activity().page_select, 0x01);
        // Other ports keep what was written.
        vip.write(0x26C, 0x5A);
        assert_eq!(vip.read(0x26C), 0x5A);
    }
}
