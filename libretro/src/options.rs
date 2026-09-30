//! The core's options, which the frontend shows in its menu. Most are the
//! configuration file's settings: their first value, `default`, leaves the
//! setting to rust-dos.conf (and its default), and the others go over it
//! as lines of configuration text. The rest are the core's own.

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::ffi::{CString, c_char, c_void};
use std::ptr;

use crate::ffi::*;
use rust_dos::keylayout::LAYOUTS;

/// Where an option's value goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// A setting of the configuration file: its section and key.
    Config(&'static str, &'static str),
    /// The core's own.
    Core,
}

struct Definition {
    key: &'static str,
    desc: &'static str,
    info: &'static str,
    category: &'static str,
    target: Target,
    /// The values and what the menu calls them; the first is the default.
    values: Vec<(String, String)>,
}

const CATEGORIES: [(&str, &str, &str); 4] = [
    ("system", "System", "The processor, memory and DOS."),
    ("video", "Video", "The display adapter and the picture."),
    ("audio", "Audio", "The sound cards."),
    ("input", "Input", "The keyboard, mouse and joysticks."),
];

/// The prefix of the options' keys, as the frontend keeps them.
const PREFIX: &str = "rust_dos_";

fn values(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|&(v, l)| (v.to_string(), l.to_string())).collect()
}

/// `default` (the configuration file's), then `list`.
fn configured(list: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut all = values(&[("default", "rust-dos.conf's")]);
    all.extend(values(list));
    all
}

const ON_OFF: [(&str, &str); 2] = [("true", "On"), ("false", "Off")];

fn definitions() -> Vec<Definition> {
    let config = |section, key| Target::Config(section, key);
    let mut cycles = configured(&[
        ("auto", "Auto (as much as the game's frames show it needs)"),
        ("max", "Max (as fast as the host goes)"),
    ]);
    cycles.extend(
        [300, 1000, 3000, 5000, 8000, 10_000, 15_000, 20_000, 30_000, 40_000, 50_000, 60_000, 80_000, 100_000, 150_000, 200_000]
            .map(|n| (n.to_string(), format!("{} cycles", n))),
    );
    let mut layouts = configured(&[("auto", "Auto (US)")]);
    layouts.extend(LAYOUTS.iter().map(|l| (l.code.to_string(), format!("{} ({})", l.name, l.code))));
    vec![
        Definition {
            key: "machine",
            desc: "Display adapter",
            info: "The graphics card programs see. Changes when no program runs.",
            category: "video",
            target: config("emulator", "machine"),
            values: configured(&[
                ("svga", "SVGA (VGA with VESA modes)"),
                ("svga_s3", "S3 Trio64 (for booted Windows)"),
                ("vga", "VGA"),
                ("ega", "EGA"),
                ("cga", "CGA"),
                ("tandy", "Tandy 1000"),
                ("pcjr", "PCjr"),
                ("hercules", "Hercules"),
            ]),
        },
        Definition {
            key: "aspect",
            desc: "4:3 aspect ratio",
            info: "Show the picture at a CRT's 4:3, as DOS games were drawn for, rather than with square pixels.",
            category: "video",
            target: config("emulator", "aspect"),
            values: configured(&ON_OFF),
        },
        Definition {
            key: "voodoo",
            desc: "3dfx Voodoo",
            info: "A 3dfx Voodoo Graphics card for Glide games, drawn in software.",
            category: "video",
            target: config("emulator", "voodoo"),
            values: configured(&ON_OFF),
        },
        Definition {
            key: "cycles",
            desc: "CPU speed",
            info: "Instructions a millisecond. Max runs as fast as the host goes; auto gives a game as much as makes its frames come faster, up to the display's refresh rate.",
            category: "system",
            target: config("emulator", "cycles"),
            values: cycles,
        },
        Definition {
            key: "cpu",
            desc: "Processor",
            info: "The processor programs see.",
            category: "system",
            target: config("emulator", "cpu"),
            values: configured(&[
                ("386", "386"),
                ("486", "486DX"),
                ("pentium", "Pentium"),
                ("pentium_mmx", "Pentium MMX"),
            ]),
        },
        Definition {
            key: "core",
            desc: "CPU core",
            info: "Auto runs protected-mode programs on the dynamic recompiler and the rest on the interpreter.",
            category: "system",
            target: config("emulator", "core"),
            values: configured(&[
                ("auto", "Auto"),
                ("dynamic", "Dynamic recompiler"),
                ("normal", "Interpreter"),
            ]),
        },
        Definition {
            key: "memsize",
            desc: "Memory (restart)",
            info: "RAM in MB. Takes effect when the content is started again.",
            category: "system",
            target: config("emulator", "memsize"),
            values: configured(&[
                ("4", "4 MB"),
                ("8", "8 MB"),
                ("16", "16 MB"),
                ("32", "32 MB"),
                ("64", "64 MB"),
            ]),
        },
        Definition {
            key: "ems",
            desc: "Expanded memory (EMS)",
            info: "Changes when no program runs.",
            category: "system",
            target: config("emulator", "ems"),
            values: configured(&ON_OFF),
        },
        Definition {
            key: "umb",
            desc: "Upper memory blocks",
            info: "Changes when no program runs.",
            category: "system",
            target: config("emulator", "umb"),
            values: configured(&ON_OFF),
        },
        Definition {
            key: "dpmi",
            desc: "DPMI host",
            info: "The DOS Protected Mode Interface that DOS extenders use.",
            category: "system",
            target: config("emulator", "dpmi"),
            values: configured(&ON_OFF),
        },
        Definition {
            key: "dos_version",
            desc: "DOS version",
            info: "The version of DOS programs are told.",
            category: "system",
            target: config("emulator", "dos_version"),
            values: configured(&[("5.00", "5.00"), ("6.22", "6.22"), ("7.00", "7.00"), ("7.10", "7.10")]),
        },
        Definition {
            key: "sbtype",
            desc: "Sound Blaster",
            info: "Changes when no program runs.",
            category: "audio",
            target: config("sound", "sbtype"),
            values: configured(&[
                ("sb16", "Sound Blaster 16"),
                ("sbpro2", "Sound Blaster Pro 2"),
                ("sb2", "Sound Blaster 2.0"),
                ("none", "None"),
            ]),
        },
        Definition {
            key: "opl",
            desc: "FM synthesizer",
            info: "The OPL chip of the AdLib and the Sound Blasters.",
            category: "audio",
            target: config("sound", "opl"),
            values: configured(&[("opl3", "OPL3"), ("opl2", "OPL2 (AdLib)")]),
        },
        Definition {
            key: "gus",
            desc: "Gravis Ultrasound",
            info: "Changes when no program runs.",
            category: "audio",
            target: config("sound", "gus"),
            values: configured(&ON_OFF),
        },
        Definition {
            key: "midisynth",
            desc: "MIDI synthesizer",
            info: "What plays the music sent to the MPU-401. A SoundFont and MT-32 ROMs are set in rust-dos.conf, \
                   in the rust-dos folder of the frontend's system directory.",
            category: "audio",
            target: config("sound", "midisynth"),
            values: configured(&[
                ("auto", "Auto"),
                ("soundfont", "SoundFont"),
                ("gus", "Ultrasound patches"),
                ("mt32", "Roland MT-32 (munt)"),
                ("none", "None"),
            ]),
        },
        Definition {
            key: "keyboard_layout",
            desc: "Keyboard layout",
            info: "The layout DOS types with, as KEYB would set it.",
            category: "input",
            target: config("emulator", "keyboard_layout"),
            values: layouts,
        },
        Definition {
            key: "joysticktype",
            desc: "Game port",
            info: "What the game port has: the gamepads of ports 1 and 2 as joysticks.",
            category: "input",
            target: config("joystick", "joysticktype"),
            values: configured(&[
                ("auto", "Auto"),
                ("4axis", "One joystick with 4 axes"),
                ("2axis", "Two joysticks"),
                ("none", "None"),
            ]),
        },
        Definition {
            key: "mouse_speed",
            desc: "Mouse speed",
            info: "How far the DOS mouse moves for the host's.",
            category: "input",
            target: Target::Core,
            values: values(&[
                ("1.0", "1x"),
                ("1.5", "1.5x"),
                ("2.0", "2x"),
                ("3.0", "3x"),
                ("0.25", "0.25x"),
                ("0.5", "0.5x"),
                ("0.75", "0.75x"),
            ]),
        },
        Definition {
            key: "analog_mouse",
            desc: "Right stick moves the mouse",
            info: "The right analog stick of the gamepad in port 1 moves the mouse, with L2 and R2 its buttons.",
            category: "input",
            target: Target::Core,
            values: values(&[("false", "Off"), ("true", "On")]),
        },
        Definition {
            key: "boot",
            desc: "Boot disk images",
            info: "Start a floppy or hard disk image given as content from its boot sector (BOOT) \
                   rather than at the DOS prompt with it as a drive.",
            category: "system",
            target: Target::Core,
            values: values(&[("false", "Off"), ("true", "On")]),
        },
    ]
}

/// The options' values, by key without the prefix.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Values(BTreeMap<String, String>);

impl Values {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.0.insert(key.to_string(), value.to_string());
    }

    /// The configuration text the values make: a line for each setting not
    /// left to rust-dos.conf.
    pub fn config_text(&self) -> String {
        let mut sections: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for def in definitions() {
            let Target::Config(section, key) = def.target else { continue };
            match self.get(def.key) {
                None | Some("default") => {}
                // Only what the option offers.
                Some(value) if def.values.iter().any(|(v, _)| v == value) => {
                    sections.entry(section).or_default().push(format!("{}={}", key, value))
                }
                Some(_) => {}
            }
        }
        sections.iter().map(|(section, lines)| format!("[{}]\n{}\n", section, lines.join("\n"))).collect()
    }

    pub fn mouse_speed(&self) -> f64 {
        self.get("mouse_speed").and_then(|v| v.parse().ok()).filter(|s: &f64| *s > 0.0).unwrap_or(1.0)
    }

    pub fn analog_mouse(&self) -> bool {
        self.get("analog_mouse") == Some("true")
    }

    pub fn boot(&self) -> bool {
        self.get("boot") == Some("true")
    }
}

/// The C strings of the options, as the frontend is handed them, kept for
/// as long as it may look at them.
struct Declared {
    _strings: Vec<CString>,
    categories: Vec<retro_core_option_v2_category>,
    definitions: Vec<retro_core_option_v2_definition>,
    variables: Vec<retro_variable>,
}

thread_local! {
    static DECLARED: OnceCell<Declared> = const { OnceCell::new() };
}

fn declared() -> Declared {
    let mut strings = Vec::new();
    let mut c = |s: &str| {
        let s = CString::new(s).expect("option text without NUL");
        let p = s.as_ptr();
        strings.push(s);
        p
    };
    let mut categories: Vec<retro_core_option_v2_category> = CATEGORIES
        .iter()
        .map(|&(key, desc, info)| retro_core_option_v2_category { key: c(key), desc: c(desc), info: c(info) })
        .collect();
    categories.push(retro_core_option_v2_category { key: ptr::null(), desc: ptr::null(), info: ptr::null() });
    let mut defs = Vec::new();
    let mut variables = Vec::new();
    for def in definitions() {
        let none = retro_core_option_value { value: ptr::null(), label: ptr::null() };
        let mut values = [none; RETRO_NUM_CORE_OPTION_VALUES_MAX];
        for (slot, (value, label)) in values.iter_mut().zip(def.values.iter()) {
            *slot = retro_core_option_value { value: c(value), label: c(label) };
        }
        let key = format!("{}{}", PREFIX, def.key);
        defs.push(retro_core_option_v2_definition {
            key: c(&key),
            desc: c(def.desc),
            desc_categorized: ptr::null(),
            info: c(def.info),
            info_categorized: ptr::null(),
            category_key: c(def.category),
            values,
            default_value: c(&def.values[0].0),
        });
        // Version 0: "Description; first|second|...", the first the default.
        let list: Vec<&str> = def.values.iter().map(|(v, _)| v.as_str()).collect();
        variables.push(retro_variable { key: c(&key), value: c(&format!("{}; {}", def.desc, list.join("|"))) });
    }
    let none = retro_core_option_value { value: ptr::null(), label: ptr::null() };
    defs.push(retro_core_option_v2_definition {
        key: ptr::null(),
        desc: ptr::null(),
        desc_categorized: ptr::null(),
        info: ptr::null(),
        info_categorized: ptr::null(),
        category_key: ptr::null(),
        values: [none; RETRO_NUM_CORE_OPTION_VALUES_MAX],
        default_value: ptr::null(),
    });
    variables.push(retro_variable { key: ptr::null(), value: ptr::null() });
    Declared { _strings: strings, categories, definitions: defs, variables }
}

/// Tell the frontend the options: version 2 with categories where it has
/// them, else the plain variables.
pub fn declare(env: retro_environment_t) {
    DECLARED.with(|cell| {
        let declared = cell.get_or_init(declared);
        let mut version = 0u32;
        // SAFETY: the calls' data is what libretro.h says they take, and
        // lives in `DECLARED` for as long as the thread does.
        unsafe {
            if !env(RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION, &mut version as *mut u32 as *mut c_void) {
                version = 0;
            }
            if version >= 2 {
                let options = retro_core_options_v2 {
                    categories: declared.categories.as_ptr(),
                    definitions: declared.definitions.as_ptr(),
                };
                if env(RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2, &options as *const _ as *mut c_void) {
                    return;
                }
            }
            env(RETRO_ENVIRONMENT_SET_VARIABLES, declared.variables.as_ptr() as *mut c_void);
        }
    });
}

/// The options' values as the frontend has them now.
pub fn read(env: retro_environment_t) -> Values {
    let mut values = Values::default();
    for def in definitions() {
        let key = CString::new(format!("{}{}", PREFIX, def.key)).unwrap();
        let mut var = retro_variable { key: key.as_ptr(), value: ptr::null() };
        // SAFETY: GET_VARIABLE takes a retro_variable, and gives back a
        // string valid until the next call.
        let found = unsafe { env(RETRO_ENVIRONMENT_GET_VARIABLE, &mut var as *mut _ as *mut c_void) };
        if found && !var.value.is_null() {
            let value = unsafe { std::ffi::CStr::from_ptr(var.value as *const c_char) };
            values.set(def.key, &value.to_string_lossy());
        }
    }
    values
}

/// Whether the user changed an option since the last call.
pub fn updated(env: retro_environment_t) -> bool {
    let mut updated = false;
    // SAFETY: GET_VARIABLE_UPDATE takes a bool.
    unsafe { env(RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE, &mut updated as *mut bool as *mut c_void) && updated }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_options_make_configuration_text() {
        let mut values = Values::default();
        assert_eq!(values.config_text(), "");
        values.set("machine", "default");
        values.set("cycles", "10000");
        values.set("sbtype", "sb2");
        values.set("gus", "false");
        values.set("mouse_speed", "2.0");
        values.set("cpu", "z80");
        assert_eq!(values.config_text(), "[emulator]\ncycles=10000\n[sound]\nsbtype=sb2\ngus=false\n");
        assert_eq!(values.mouse_speed(), 2.0);
        let config = rust_dos::config::parse(&values.config_text(), std::path::Path::new("/"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    }

    #[test]
    fn every_option_fits_and_every_value_is_a_setting() {
        for def in definitions() {
            assert!(def.values.len() <= RETRO_NUM_CORE_OPTION_VALUES_MAX, "{}", def.key);
            assert!(CATEGORIES.iter().any(|c| c.0 == def.category), "{}", def.key);
            let Target::Config(section, key) = def.target else { continue };
            for (value, _) in def.values.iter().skip(1) {
                let text = format!("[{}]\n{}={}\n", section, key, value);
                let config = rust_dos::config::parse(&text, std::path::Path::new("/"), None);
                assert!(config.warnings.is_empty(), "{}: {:?}", text, config.warnings);
            }
        }
    }
}
