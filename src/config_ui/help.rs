//! The settings window's help (F1): a short text on the setting under the
//! cursor, or on the page or dialog open, from the Markdown files in
//! `help/`, which are built into the program.
//!
//! Each topic starts with a heading and its id, `# Window scale {#scale}`,
//! and runs to the next. What the text shows of Markdown: paragraphs,
//! `## subheadings`, `- lists`, `> tips`, fenced code blocks, `` `commands` ``,
//! `**keys and strong words**` and `*names of things on the screen*`.

use super::draw::{self, Rgb};
use super::{Item, Page, Pick};
use crate::mixer::Channel;

const FILES: [&str; 8] = [
    include_str!("help/display.md"),
    include_str!("help/emulator.md"),
    include_str!("help/sound.md"),
    include_str!("help/mixer.md"),
    include_str!("help/network.md"),
    include_str!("help/serial.md"),
    include_str!("help/pages.md"),
    include_str!("help/vr.md"),
];

/// A topic of the help: its title, and its text in Markdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Topic {
    pub id: &'static str,
    pub title: &'static str,
    pub body: &'static str,
}

/// The topics of `text`, in order. A heading without an id starts
/// nothing: what follows it, to the next topic, is left out.
fn sections(text: &'static str) -> Vec<Topic> {
    let mut topics = Vec::new();
    let mut current: Option<(&'static str, &'static str, usize)> = None;
    let (mut offset, mut fenced) = (0, false);
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        let Some(heading) = line.trim_end().strip_prefix("# ").filter(|_| !fenced) else { continue };
        if let Some((id, title, body)) = current.take() {
            topics.push(Topic { id, title, body: &text[body..start] });
        }
        current = heading
            .rsplit_once("{#")
            .and_then(|(title, id)| Some((id.strip_suffix('}')?, title.trim(), offset)));
    }
    if let Some((id, title, body)) = current {
        topics.push(Topic { id, title, body: &text[body..] });
    }
    topics
}

/// Every topic.
pub fn topics() -> impl Iterator<Item = Topic> {
    FILES.iter().flat_map(|file| sections(file))
}

pub fn topic(id: &str) -> Option<Topic> {
    topics().find(|t| t.id == id)
}

/// A piece of a line of text in one colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub color: Rgb,
}

pub type Line = Vec<Span>;

/// The colours of the text: keys and values in the key hints' colour,
/// names of things on the screen in light blue.
const PLAIN: Rgb = draw::TEXT;
const STRONG: Rgb = draw::KEY;
const NAME: Rgb = Rgb(0x9C, 0xC4, 0xFF);
const CODE: Rgb = draw::CODE;
const HEADING: Rgb = draw::BRIGHT;
const MARK: Rgb = draw::BORDER;

/// A block of text to wrap: what comes before its first line and before
/// the others, and the text with the colour it has without markup.
struct Block {
    first: (String, Rgb),
    rest: String,
    text: String,
    color: Rgb,
}

/// `body` laid out in lines of up to `width` characters.
pub fn layout(body: &str, width: usize) -> Vec<Line> {
    let width = width.max(8);
    let mut lines: Vec<Line> = Vec::new();
    let mut block: Option<Block> = None;
    let mut fenced = false;
    let blank = |lines: &mut Vec<Line>| {
        if lines.last().is_some_and(|l| !l.is_empty()) {
            lines.push(Vec::new());
        }
    };
    let flush = |block: &mut Option<Block>, lines: &mut Vec<Line>| {
        if let Some(b) = block.take() {
            lines.extend(wrap(&b, width));
        }
    };
    for raw in body.lines() {
        let line = raw.trim_end();
        let text = line.trim_start();
        if text.starts_with("```") {
            flush(&mut block, &mut lines);
            if !fenced {
                blank(&mut lines);
            }
            fenced = !fenced;
            continue;
        }
        if fenced {
            let code: String = line.chars().take(width - 2).collect();
            lines.push(vec![Span { text: format!("  {}", code), color: CODE }]);
            continue;
        }
        if text.is_empty() {
            flush(&mut block, &mut lines);
            blank(&mut lines);
            continue;
        }
        let start = |first: (&str, Rgb), rest: &str, text: &str, color: Rgb| Block {
            first: (first.0.to_string(), first.1),
            rest: rest.to_string(),
            text: text.to_string(),
            color,
        };
        if let Some(heading) = text.strip_prefix("## ") {
            flush(&mut block, &mut lines);
            block = Some(start(("", PLAIN), "", heading, HEADING));
            flush(&mut block, &mut lines);
        } else if let Some(item) = text.strip_prefix("- ").or_else(|| text.strip_prefix("* ")) {
            flush(&mut block, &mut lines);
            block = Some(start(("\u{2022} ", MARK), "  ", item, PLAIN));
        } else if let Some(tip) = text.strip_prefix('>') {
            let tip = tip.trim_start();
            match &mut block {
                Some(b) if b.first.0 == "\u{2502} " => {
                    b.text.push(' ');
                    b.text.push_str(tip);
                }
                _ => {
                    flush(&mut block, &mut lines);
                    block = Some(start(("\u{2502} ", MARK), "\u{2502} ", tip, PLAIN));
                }
            }
        } else if let Some(b) = &mut block {
            b.text.push(' ');
            b.text.push_str(text);
        } else {
            block = Some(start(("", PLAIN), "", text, PLAIN));
        }
    }
    flush(&mut block, &mut lines);
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// The characters of `text` with their colours, its inline Markdown taken
/// out: `code`, **strong** and *names*, and `\` before a character that
/// would be one of those.
fn styled(text: &str, color: Rgb) -> Vec<(char, Rgb)> {
    let mut out = Vec::new();
    let (mut code, mut strong, mut name) = (false, false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' => code = !code,
            _ if code => out.push((c, CODE)),
            '\\' if chars.peek().is_some_and(|n| "`*\\".contains(*n)) => {
                let n = chars.next().unwrap_or(c);
                out.push((n, if strong { STRONG } else if name { NAME } else { color }));
            }
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                strong = !strong;
            }
            '*' => name = !name,
            _ => out.push((c, if strong { STRONG } else if name { NAME } else { color })),
        }
    }
    out
}

/// A block's lines, broken between words to fit `width`, and within a word
/// longer than a line.
fn wrap(block: &Block, width: usize) -> Vec<Line> {
    let chars = styled(&block.text, block.color);
    let words: Vec<&[(char, Rgb)]> = chars.split(|&(c, _)| c == ' ').filter(|w| !w.is_empty()).collect();
    let mut lines = Vec::new();
    let mut line: Vec<(char, Rgb)> = Vec::new();
    let mut prefix = (block.first.0.clone(), block.first.1);
    let room = |prefix: &(String, Rgb)| width.saturating_sub(prefix.0.chars().count()).max(1);
    let mut finish = |line: &mut Vec<(char, Rgb)>, prefix: &mut (String, Rgb)| {
        let mut spans = Vec::new();
        if !prefix.0.is_empty() {
            spans.push(Span { text: prefix.0.clone(), color: prefix.1 });
        }
        for &(c, color) in line.iter() {
            match spans.last_mut() {
                Some(span) if span.color == color => span.text.push(c),
                _ => spans.push(Span { text: c.to_string(), color }),
            }
        }
        lines.push(spans);
        line.clear();
        *prefix = (block.rest.clone(), MARK);
    };
    for word in words {
        let fits = |line: &Vec<(char, Rgb)>, prefix: &(String, Rgb)| {
            line.len() + usize::from(!line.is_empty()) + word.len() <= room(prefix)
        };
        if !line.is_empty() && !fits(&line, &prefix) {
            finish(&mut line, &mut prefix);
        }
        if !line.is_empty() {
            // The space takes the colour of the text around it.
            let color = if line.last().map(|l| l.1) == Some(word[0].1) { word[0].1 } else { block.color };
            line.push((' ', color));
        }
        let mut rest = word;
        while line.len() + rest.len() > room(&prefix) {
            let take = room(&prefix) - line.len();
            line.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            finish(&mut line, &mut prefix);
        }
        line.extend_from_slice(rest);
    }
    if !line.is_empty() {
        finish(&mut line, &mut prefix);
    }
    lines
}

/// The help open over the window: the topic, and how far it is scrolled.
pub struct HelpView {
    pub topic: Topic,
    /// A text of its own instead of the topic's: a title and a body.
    pub text: Option<(String, String)>,
    pub scroll: usize,
    /// The lines the text took and how many showed, in the last frame.
    pub lines: usize,
    pub visible: usize,
}

impl HelpView {
    pub fn new(topic: Topic) -> Self {
        Self { topic, text: None, scroll: 0, lines: 0, visible: 0 }
    }

    /// Help made of `body`, under `title`, rather than a topic's.
    pub fn of_text(title: String, body: String) -> Self {
        Self { text: Some((title, body)), ..Self::new(Topic { id: "", title: "", body: "" }) }
    }

    pub fn title(&self) -> &str {
        self.text.as_ref().map_or(self.topic.title, |(t, _)| t)
    }

    pub fn body(&self) -> &str {
        self.text.as_ref().map_or(self.topic.body, |(_, b)| b)
    }

    /// The furthest it scrolls.
    pub fn max_scroll(&self) -> usize {
        self.lines.saturating_sub(self.visible)
    }
}

impl Item {
    /// The help topic on the setting.
    pub(super) fn help(self) -> &'static str {
        use Item::*;
        match self {
            Scale => "scale",
            Fullscreen => "fullscreen",
            Aspect => "aspect",
            Vrr => "vrr",
            VrMode => "vr-mode",
            VrScene => "vr-scene",
            VrControllers => "vr-controllers",
            VrSpatialAudio => "vr-spatial-audio",
            VrScreenFit => "vr-screen-fit",
            Filter => "filter",
            Shader => "shader",
            CrtCurvature => "crt-curvature",
            CrtGlow => "crt-glow",
            Monochrome => "monochrome",
            Composite => "composite",
            CompositeEra => "composite-era",
            Cycles => "cycles",
            Core => "core",
            Fpu => "fpu",
            Cpu => "cpu",
            Machine => "machine",
            Voodoo => "voodoo",
            VoodooMemory => "voodoo-memory",
            VoodooRenderer => "voodoo-renderer",
            VoodooScale => "voodoo-scale",
            Memsize => "memsize",
            Ems => "ems",
            Umb => "umb",
            DosHigh => "dos-high",
            Dpmi => "dpmi",
            DosVersion => "dos-version",
            IdeHardDisks => "ide-hard-disks",
            BootCdrom => "boot-cdrom",
            KeyboardLayout => "keyboard-layout",
            MouseAutocapture => "mouse-autocapture",
            MouseCaptureMessages => "mouse-capture-messages",
            ShellSuggestions => "shell-suggestions",
            ShellColors => "shell-colors",
            SaveShellHistory => "save-shell-history",
            Rewind => "rewind",
            RewindMemory => "rewind-memory",
            SbType => "sb-type",
            SbPorts | SbBase | SbIrq | SbDma | SbHdma => "sb-ports",
            Awe32Rom | Awe32Download => "awe32-rom",
            Awe32Ram => "awe32-ram",
            Opl => "opl",
            Gus => "gus",
            GusPorts | GusBase | GusIrq | GusDma => "gus-ports",
            GusDrive => "gus-drive",
            UltraDir => "ultradir",
            Midi => "midi",
            SoundFont => "soundfont",
            Mt32Roms => "mt32-roms",
            Mt32Model => "mt32-model",
            Sc55Roms | Sc55Download => "sc55-roms",
            Sc55Model => "sc55-model",
            MidiPort => "midi-port",
            HardDiskSpeed => "hard-disk-speed",
            FloppyDiskSpeed => "floppy-disk-speed",
            HardDiskNoise => "hard-disk-noise",
            FloppyDiskNoise => "floppy-disk-noise",
            Volume(Channel::Master) => "master-volume",
            Volume(_) => "volume",
            CaptureDir => "capture-dir",
            RecordUi => "record-ui",
            RecordShader => "record-shader",
            Joystick => "joystick",
            Deadzone => "deadzone",
            SpeakerFilter => "speaker-filter",
            SbFilter => "sb-filter",
            Reverb => "reverb",
            Chorus => "chorus",
            ReverbMix | ChorusMix => "effect-mix",
            LptDac => "lpt-dac",
            TandySound => "tandy-sound",
            Autoexec => "autoexec",
            Ipx => "ipx",
            IpxIrq => "ipx-irq",
            IpxFrame => "ipx-frame",
            Ne2000 => "ne2000",
            NicBase => "nic-base",
            NicIrq => "nic-irq",
            MacAddr => "mac-addr",
            Online => "online",
            Relay => "relay",
            Rooms => "rooms",
            Player => "player",
            Lan => "lan",
            LanHost => "lan-host",
            Room => "room",
            Password => "password",
            SerialPort(_) => "com-port",
            SerialIrq(_) => "com-irq",
            Uart => "uart",
            MouseType => "mouse-type",
            ModemListen => "modem-listen",
            ModemTelnet => "modem-telnet",
            PrinterOutput => "printer",
            PrinterPaper => "printer-paper",
            PrinterDpi => "printer-dpi",
            PrinterMultipage => "printer-multipage",
            PrinterTimeout => "printer-timeout",
        }
    }
}

impl Page {
    /// The help topic on a page without settings.
    pub(super) fn help(self) -> Option<&'static str> {
        Some(match self {
            Page::Drives => "drives",
            Page::Games => "games",
            Page::States => "states",
            Page::Cheats => "cheats",
            Page::Achievements => "achievements",
            Page::Vr => "vr",
            Page::Stats => "stats",
            _ => return None,
        })
    }
}

impl Pick {
    /// The help topic on what the file browser picks.
    pub(super) fn help(self) -> &'static str {
        match self {
            Pick::MountPath => "mount",
            Pick::SoundFont => "soundfont",
            Pick::VrScene => "vr-scene",
            Pick::Mt32Roms => "mt32-roms",
            Pick::Awe32Rom => "awe32-rom",
            Pick::Sc55Roms => "sc55-roms",
            Pick::ImportGame => "import",
            Pick::ImagePath => "new-image",
            Pick::AchievementsArchive => "achievements-archive",
            Pick::Manual => "manuals",
            Pick::OverlayPath => "mount",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_are_found_by_id() {
        let t = topic("scale").expect("the scale's topic");
        assert_eq!(t.title, "Window scale");
        assert!(!t.body.contains("{#"), "a topic ends at the next");
        assert!(topic("no-such-topic").is_none());
    }

    #[test]
    fn every_heading_has_an_id_and_ids_are_unique() {
        for file in FILES {
            let headings = file.lines().filter(|l| l.starts_with("# ")).count();
            assert_eq!(sections(file).len(), headings, "a heading without an id in:\n{}", &file[..80]);
        }
        let mut ids: Vec<&str> = topics().map(|t| t.id).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "an id used twice");
    }

    #[test]
    fn the_text_is_in_code_page_437() {
        for t in topics() {
            for c in t.title.chars().chain(t.body.chars()).filter(|&c| c != '\n') {
                assert!(c == '?' || draw::cp437(c) != b'?', "{:?} in topic {}", c, t.id);
            }
        }
    }

    #[test]
    fn topics_are_short_and_fit_a_small_window() {
        for t in topics() {
            let lines = layout(t.body, 40);
            assert!(!lines.is_empty(), "topic {} is empty", t.id);
            for line in &lines {
                let len: usize = line.iter().map(|s| s.text.chars().count()).sum();
                assert!(len <= 40, "a line of {} is {} long", t.id, len);
            }
            assert!(layout(t.body, 70).len() <= 40, "topic {} is long: keep it short", t.id);
        }
    }

    fn text(line: &Line) -> String {
        line.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn markdown_is_laid_out_in_colours() {
        let body = "Type `MOUNT C ~/dos` and\npress **Enter** on *Browse*.\n\n## Tips\n- one two three four\n> a tip\n> more\n\n```\nC:\\> DIR\n```\n";
        let lines = layout(body, 20);
        let texts: Vec<String> = lines.iter().map(text).collect();
        assert_eq!(
            texts,
            [
                "Type MOUNT C ~/dos",
                "and press Enter on",
                "Browse.",
                "",
                "Tips",
                "\u{2022} one two three four",
                "\u{2502} a tip more",
                "",
                "  C:\\> DIR",
            ]
        );
        let color = |line: usize, s: &str| lines[line].iter().find(|span| span.text.contains(s)).map(|span| span.color);
        assert_eq!(color(0, "MOUNT C ~/dos"), Some(CODE), "the command's spaces stay in its colour");
        assert_eq!(color(0, "Type"), Some(PLAIN));
        assert_eq!(color(1, "Enter"), Some(STRONG));
        assert_eq!(color(2, "Browse"), Some(NAME));
        assert_eq!(color(4, "Tips"), Some(HEADING));
        assert_eq!(color(8, "DIR"), Some(CODE));
    }

    #[test]
    fn long_words_and_list_items_wrap_under_themselves() {
        let lines = layout("- abcdefghijklmnop qrs", 10);
        let texts: Vec<String> = lines.iter().map(text).collect();
        assert_eq!(texts, ["\u{2022} abcdefgh", "  ijklmnop", "  qrs"]);
    }
}
