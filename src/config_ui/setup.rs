//! The BIOS setup (Del during the POST, `post`): the settings as a blue
//! CMOS setup utility of the time, full screen. Its main menu has pages of
//! the settings a BIOS would have had, changed with PgUp/PgDn or +/-;
//! nothing takes effect until "Save & Exit", which applies and saves them
//! and resets the machine, as setting up a PC did.

use super::draw::{self, Grid, Layout, Rgb};
use super::{ConfigUi, Host, Input, Item, UiKey, fit};
use crate::config::Settings;
use crate::mixer::Channel;
use crate::video::Frame;

const BLUE: Rgb = Rgb(0x00, 0x00, 0xAA);
const WHITE: Rgb = Rgb(0xFF, 0xFF, 0xFF);
const GREY: Rgb = Rgb(0xAA, 0xAA, 0xAA);
const YELLOW: Rgb = Rgb(0xFF, 0xFF, 0x55);
const CYAN: Rgb = Rgb(0x55, 0xFF, 0xFF);
const RED: Rgb = Rgb(0xAA, 0x00, 0x00);

/// A page of the main menu: its title, what the menu says of it, and its
/// settings.
struct Category {
    title: &'static str,
    about: &'static str,
    items: &'static [Item],
}

const CATEGORIES: [Category; 4] = [
    Category {
        title: "STANDARD CMOS SETUP",
        about: "Memory, the DOS memory managers and the drives",
        items: &[
            Item::Memsize,
            Item::Ems,
            Item::Umb,
            Item::DosHigh,
            Item::IdeHardDisks,
            Item::BootCdrom,
            Item::HardDiskSpeed,
            Item::FloppyDiskSpeed,
        ],
    },
    Category {
        title: "BIOS FEATURES SETUP",
        about: "The processor, its speed, and how the machine starts",
        items: &[Item::Cpu, Item::Cycles, Item::Core, Item::Fpu, Item::Dpmi, Item::DosVersion, Item::Post, Item::KeyboardLayout],
    },
    Category {
        title: "INTEGRATED PERIPHERALS",
        about: "The video card, the sound cards and the ports",
        items: &[
            Item::Machine,
            Item::SbType,
            Item::SbBase,
            Item::SbIrq,
            Item::SbDma,
            Item::SbHdma,
            Item::Opl,
            Item::Gus,
            Item::GusBase,
            Item::GusIrq,
            Item::GusDma,
            Item::Midi,
            Item::LptDac,
            Item::SerialPort(0),
            Item::SerialPort(1),
            Item::MouseType,
            Item::Joystick,
        ],
    },
    Category {
        title: "MONITOR SETUP",
        about: "The monitor: its tube, its colours and its effects",
        items: &[Item::Shader, Item::CrtCurvature, Item::CrtGlow, Item::Monochrome, Item::MonitorEffects, Item::Volume(Channel::Monitor)],
    },
];

/// The main menu's other entries, after the pages.
const LOAD_PREVIOUS: usize = CATEGORIES.len();
const SAVE: usize = LOAD_PREVIOUS + 1;
const QUIT: usize = LOAD_PREVIOUS + 2;
const MENU: usize = QUIT + 1;

/// A question the setup asks before it goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Confirm {
    Save,
    Quit,
}

/// The setup while it shows.
pub(super) struct Setup {
    /// The settings as changed here, and as they were.
    pub(super) settings: Settings,
    before: Settings,
    menu: usize,
    /// The page open, and the setting selected on it.
    page: Option<usize>,
    row: usize,
    confirm: Option<Confirm>,
}

impl ConfigUi {
    /// Open the settings as the BIOS setup, over everything.
    pub fn open_setup(&mut self, settings: &Settings, config_file: Option<std::path::PathBuf>, host: &dyn Host) {
        self.open(settings, config_file, host);
        self.setup = Some(Setup { settings: settings.clone(), before: settings.clone(), menu: 0, page: None, row: 0, confirm: None });
    }

    pub fn setup_shown(&self) -> bool {
        self.open && self.setup.is_some()
    }

    /// The settings of page `page` that this frontend has and that apply
    /// with `s`, and that a step changes.
    fn setup_items(&self, page: usize, s: &Settings) -> Vec<Item> {
        CATEGORIES[page]
            .items
            .iter()
            .copied()
            .filter(|item| {
                item.available(self.frontend)
                    && item.shown(s)
                    && matches!(item.input(), Input::Choice | Input::Slider | Input::Presets)
                    && (*item != Item::SbHdma || s.sound.sb.model.is_sb16())
            })
            .collect()
    }

    pub(super) fn setup_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(setup) = &mut self.setup else { return };
        if let Some(confirm) = setup.confirm {
            match key {
                UiKey::Enter | UiKey::Char('y' | 'Y') => match confirm {
                    Confirm::Save => self.setup_save(host),
                    Confirm::Quit => self.close(),
                },
                UiKey::Esc | UiKey::Char('n' | 'N') => setup.confirm = None,
                _ => {}
            }
            return;
        }
        if key == UiKey::F10 {
            setup.confirm = Some(Confirm::Save);
            return;
        }
        let Some(page) = setup.page else {
            match key {
                UiKey::Up => setup.menu = (setup.menu + MENU - 1) % MENU,
                UiKey::Down => setup.menu = (setup.menu + 1) % MENU,
                // The menu's two columns: the pages left, the rest right.
                UiKey::Left | UiKey::Right => {
                    setup.menu = if setup.menu < LOAD_PREVIOUS { LOAD_PREVIOUS + setup.menu.min(2) } else { setup.menu - LOAD_PREVIOUS };
                }
                UiKey::Esc => setup.confirm = Some(Confirm::Quit),
                UiKey::Enter => match setup.menu {
                    LOAD_PREVIOUS => setup.settings = setup.before.clone(),
                    SAVE => setup.confirm = Some(Confirm::Save),
                    QUIT => setup.confirm = Some(Confirm::Quit),
                    menu => (setup.page, setup.row) = (Some(menu), 0),
                },
                _ => {}
            }
            return;
        };
        let items = self.setup_items(page, &self.setup.as_ref().expect("the setup").settings);
        let (drives, frontend) = (self.drives.clone(), self.frontend);
        let Some(setup) = &mut self.setup else { return };
        let row = setup.row.min(items.len().saturating_sub(1));
        let dir = match key {
            UiKey::Up => {
                setup.row = row.checked_sub(1).unwrap_or(items.len().saturating_sub(1));
                0
            }
            UiKey::Down => {
                setup.row = if row + 1 >= items.len() { 0 } else { row + 1 };
                0
            }
            UiKey::PageUp | UiKey::Char('+') | UiKey::Right => 1,
            UiKey::PageDown | UiKey::Char('-') | UiKey::Left => -1,
            UiKey::Esc => {
                setup.page = None;
                0
            }
            _ => 0,
        };
        if dir != 0
            && let Some(&item) = items.get(row)
        {
            item.step(&mut setup.settings, dir, &drives, frontend);
        }
    }

    /// Save & Exit: the settings take effect and are saved, and the
    /// machine starts again.
    fn setup_save(&mut self, host: &mut dyn Host) {
        let Some(setup) = self.setup.take() else { return };
        self.settings = setup.settings;
        let _ = host.apply(&self.settings);
        let _ = host.save(&self.settings);
        self.close();
        host.reset_machine();
    }

    /// Draw the setup over all of `frame`.
    pub(super) fn draw_setup(&mut self, frame: &mut Frame) {
        let Some(setup) = &self.setup else { return };
        let (width, height) = (frame.width as usize, frame.height as usize);
        let cell_h = if height >= 25 * 16 { 16 } else { 8 };
        let (cols, rows) = ((width / 8).min(80), (height / cell_h).min(25));
        if cols < 60 || rows < 22 {
            return;
        }
        let layout = Layout { x: (width - cols * 8) / 2, y: (height - rows * cell_h) / 2, cell_h, cols, rows };
        let mut g = Grid::new(cols, rows);
        for row in 0..rows {
            g.background(0, row, cols, BLUE);
        }
        let center = |g: &mut Grid, row: usize, text: &str, fg: Rgb| {
            g.text(cols.saturating_sub(text.len()) / 2, row, text, fg);
        };
        frame.rgb.fill(0);
        match setup.page {
            None => {
                center(&mut g, 1, "ROM PCI/ISA BIOS (RUST-DOS)", WHITE);
                center(&mut g, 2, "CMOS SETUP UTILITY", WHITE);
                center(&mut g, 3, &format!("RUST-DOS BIOS v{}", env!("CARGO_PKG_VERSION")), WHITE);
                boxed(&mut g, (1, 5), (cols - 2, 12), YELLOW);
                let half = cols / 2;
                for row in 6..16 {
                    g.char(half, row, 0xB3, YELLOW);
                }
                let entries: Vec<(usize, usize, &str)> = CATEGORIES
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (4, 7 + 2 * i, c.title))
                    .chain([(half + 3, 7, "LOAD PREVIOUS VALUES"), (half + 3, 9, "SAVE & EXIT SETUP"), (half + 3, 11, "EXIT WITHOUT SAVING")])
                    .collect();
                for (i, (col, row, text)) in entries.iter().enumerate() {
                    let selected = i == setup.menu;
                    if selected {
                        g.background(col - 1, *row, text.len() + 2, RED);
                    }
                    g.text(*col, *row, text, if selected { WHITE } else { YELLOW });
                }
                g.text(3, 18, "Esc : Quit", CYAN);
                g.text(half + 3, 18, "\u{2191} \u{2193} \u{2192} \u{2190}   : Select Item", CYAN);
                g.text(3, 19, "F10 : Save & Exit Setup", CYAN);
                boxed(&mut g, (1, 20), (cols - 2, 3), WHITE);
                let about = match setup.menu {
                    LOAD_PREVIOUS => "Back to the values the settings had before this setup",
                    SAVE => "Save the settings to the configuration file and start the machine again",
                    QUIT => "Leave the setup without changing anything",
                    menu => CATEGORIES[menu].about,
                };
                center(&mut g, 21, about, WHITE);
            }
            Some(page) => {
                let category = &CATEGORIES[page];
                center(&mut g, 1, &format!("ROM PCI/ISA BIOS (RUST-DOS)  -  {}", category.title), WHITE);
                center(&mut g, 2, "CMOS SETUP UTILITY", WHITE);
                boxed(&mut g, (1, 4), (cols - 2, rows - 8), WHITE);
                let items = self.setup_items(page, &setup.settings);
                let row = setup.row.min(items.len().saturating_sub(1));
                let value_col = cols / 2;
                // The lines in the box, scrolled to the one selected.
                let visible = rows - 10;
                let first = (row + 1).saturating_sub(visible);
                for (i, &item) in items.iter().enumerate().skip(first).take(visible) {
                    let y = 5 + i - first;
                    let selected = i == row;
                    let changed = item.value(&setup.settings, None) != item.value(&setup.before, None);
                    g.text_to(3, y, &setup_label(item), YELLOW, value_col - 2);
                    let value = fit(&item.value(&setup.settings, None), cols - value_col - 6);
                    let text = format!(": {}", value);
                    if selected {
                        g.background(value_col, y, text.len() + 1, RED);
                    }
                    g.text(value_col, y, &text, if selected || changed { WHITE } else { GREY });
                }
                g.text(3, rows - 3, "Esc : Main Menu      \u{2191} \u{2193} : Select Item", CYAN);
                g.text(3, rows - 2, "PU/PD/+/- : Modify   F10 : Save & Exit Setup", CYAN);
            }
        }
        if let Some(confirm) = setup.confirm {
            let text = match confirm {
                Confirm::Save => "SAVE to CMOS and EXIT (Y/N)? Y",
                Confirm::Quit => "Quit Without Saving (Y/N)? N",
            };
            let (w, top) = (text.len() + 6, rows / 2 - 2);
            let left = (cols - w) / 2;
            for row in top..top + 3 {
                g.text(left, row, &" ".repeat(w), WHITE);
                g.background(left, row, w, RED);
            }
            boxed(&mut g, (left, top), (w, 3), WHITE);
            g.text(left + 3, top + 1, text, YELLOW);
        }
        draw::render_blended(&g, &layout, frame, draw::OPAQUE);
        self.layout = None;
        self.hits.clear();
    }
}

/// A setting's name on a page: the cards' ports, IRQs and DMA channels,
/// which share their card's row in the window, under it.
fn setup_label(item: Item) -> String {
    match item {
        Item::SbBase | Item::GusBase => "  I/O port".to_string(),
        Item::SbIrq | Item::GusIrq => "  IRQ".to_string(),
        Item::SbDma | Item::GusDma => "  DMA".to_string(),
        Item::SbHdma => "  High DMA".to_string(),
        Item::Volume(Channel::Monitor) => "Monitor noise volume".to_string(),
        _ => item.label().trim().to_string(),
    }
}

/// A single-line box at (`col`, `row`), `w` by `h` cells.
fn boxed(g: &mut Grid, (col, row): (usize, usize), (w, h): (usize, usize), fg: Rgb) {
    let (right, bottom) = (col + w - 1, row + h - 1);
    for x in col..=right {
        let (top, low) = match x {
            _ if x == col => (0xDA, 0xC0),
            _ if x == right => (0xBF, 0xD9),
            _ => (0xC4, 0xC4),
        };
        g.char(x, row, top, fg);
        g.char(x, bottom, low, fg);
    }
    for y in row + 1..bottom {
        g.char(col, y, 0xB3, fg);
        g.char(right, y, 0xB3, fg);
    }
}
