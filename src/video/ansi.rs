//! ANSI.SYS's escape sequences in DOS's console output, as DOSBox's CON
//! device has them: the colours of the text (`ESC[...m`, with MS-DOS
//! ANSI.SYS's attributes), moving the cursor (`ESC[r;cH`, `ESC[nA` to
//! `ESC[nD`, `ESC[s` and `ESC[u`), and erasing the screen (`ESC[2J`) and
//! the rest of a line (`ESC[K`). Set mode (`ESC[=nh`, `ESC[=nl`), key
//! reassignment (`ESC[...p`) and the cursor position report (`ESC[6n`)
//! are taken off the output and do nothing, as in DOSBox (#1706).
//! `console_write` in video/mod.rs carries them out, while the `ansi_sys`
//! setting is on.

/// What a byte of console output asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Feed {
    /// A character to show, in `Ansi::attr`.
    Text(u8),
    /// Part of an escape sequence, or one that only changed the colours.
    Held,
    /// The end of a sequence that moves the cursor or erases.
    Command(Command),
}

/// A sequence that moves the cursor or erases.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// The cursor to the row and column, from 1, kept on the screen.
    Position(u16, u16),
    Up(u16),
    Down(u16),
    Forward(u16),
    Back(u16),
    /// Keep the cursor's position for `Restore`.
    Save,
    Restore,
    /// Clear the screen in `Ansi::attr` and home the cursor.
    EraseScreen,
    /// Clear from the cursor to the end of its line in `Ansi::attr`, the
    /// cursor where it is.
    EraseLine,
}

/// Where the console is in an escape sequence, and the attribute it
/// writes text in.
#[derive(Clone, Debug, PartialEq)]
pub struct Ansi {
    /// The attribute of the text written, light grey on black until a
    /// sequence changes it.
    pub attr: u8,
    /// 0 in text, 1 after ESC, 2 after ESC [, 3 in a quoted string of a
    /// sequence (key reassignment's).
    stage: u8,
    /// The numbers of the sequence so far.
    args: Vec<u16>,
    /// The cursor position `ESC[s` kept: row, column.
    pub saved: (u8, u8),
}

/// The most numbers a sequence keeps; later ones are dropped.
const MAX_ARGS: usize = 16;

impl Default for Ansi {
    fn default() -> Self {
        Ansi {
            attr: 0x07,
            stage: 0,
            args: Vec::new(),
            saved: (0, 0),
        }
    }
}

crate::state_fields!(Ansi {
    attr,
    stage,
    args,
    saved
});

impl Ansi {
    /// Take the next byte of console output.
    pub fn feed(&mut self, b: u8) -> Feed {
        match self.stage {
            0 if b == 0x1B => {
                self.stage = 1;
                self.args.clear();
                Feed::Held
            }
            0 => Feed::Text(b),
            // ESC and a character other than [ are dropped, as DOSBox does.
            1 => {
                self.stage = if b == b'[' { 2 } else { 0 };
                Feed::Held
            }
            3 => {
                if b == b'"' {
                    self.stage = 2;
                }
                Feed::Held
            }
            _ => self.sequence(b),
        }
    }

    /// A byte after ESC [.
    fn sequence(&mut self, b: u8) -> Feed {
        match b {
            b'0'..=b'9' => {
                if self.args.is_empty() {
                    self.args.push(0);
                }
                if let Some(n) = self.args.last_mut() {
                    *n = n.saturating_mul(10).saturating_add((b - b'0') as u16);
                }
                return Feed::Held;
            }
            b';' => {
                if self.args.is_empty() {
                    self.args.push(0);
                }
                if self.args.len() < MAX_ARGS {
                    self.args.push(0);
                }
                return Feed::Held;
            }
            // Set mode's = and ?, which change nothing here.
            b'=' | b'?' => return Feed::Held,
            b'"' => {
                self.stage = 3;
                return Feed::Held;
            }
            _ => {}
        }
        self.stage = 0;
        let arg = |i: usize| self.args.get(i).copied().unwrap_or(0);
        let count = arg(0).max(1);
        Feed::Command(match b {
            b'm' => {
                if self.args.is_empty() {
                    self.args.push(0);
                }
                for i in 0..self.args.len() {
                    self.attr = sgr(self.attr, self.args[i]);
                }
                return Feed::Held;
            }
            b'H' | b'f' => Command::Position(arg(0).max(1), arg(1).max(1)),
            b'A' => Command::Up(count),
            b'B' => Command::Down(count),
            b'C' => Command::Forward(count),
            b'D' => Command::Back(count),
            b's' => Command::Save,
            b'u' => Command::Restore,
            // ANSI.SYS knows only 2J; DOSBox takes the others as it.
            b'J' => Command::EraseScreen,
            b'K' => Command::EraseLine,
            _ => return Feed::Held,
        })
    }
}

/// `attr` after the Select Graphic Rendition parameter `n`, as MS-DOS's
/// ANSI.SYS sets it (its table GRMODE: a mask, then bits to set). Blink
/// and intensity stay through reverse (7) and concealed (8), and
/// underline (4) is blue text, the colour adapter's form of it.
fn sgr(attr: u8, n: u16) -> u8 {
    // ANSI's colour order (black, red, green, yellow, blue, magenta, cyan,
    // white) in the PC's (blue in bit 0, green in 1, red in 2).
    const PC: [u8; 8] = [0, 4, 2, 6, 1, 5, 3, 7];
    match n {
        0 => 0x07,
        1 => attr | 0x08,
        4 => (attr & 0xF8) | 0x01,
        5 => attr | 0x80,
        7 => (attr & 0xF8) | 0x70,
        8 => attr & 0x88,
        30..=37 => (attr & 0xF8) | PC[(n - 30) as usize],
        40..=47 => (attr & 0x8F) | (PC[(n - 40) as usize] << 4),
        _ => attr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ansi: &mut Ansi, text: &[u8]) -> Vec<Feed> {
        text.iter()
            .map(|&b| ansi.feed(b))
            .filter(|f| *f != Feed::Held)
            .collect()
    }

    #[test]
    fn colours_change_the_attribute_and_leave_the_text() {
        let mut ansi = Ansi::default();
        assert_eq!(run(&mut ansi, b"\x1B[1;33m\x1B[42mA"), [Feed::Text(b'A')]);
        assert_eq!(ansi.attr, 0x2E);
        run(&mut ansi, b"\x1B[34;5m");
        assert_eq!(ansi.attr, 0xA9);
        run(&mut ansi, b"\x1B[m");
        assert_eq!(ansi.attr, 0x07);
        run(&mut ansi, b"\x1B[7m");
        assert_eq!(ansi.attr, 0x70);
        // ANSI.SYS keeps blink and intensity through reverse and
        // concealed, and underlines in blue.
        run(&mut ansi, b"\x1B[0;1;5;31;44m\x1B[7m");
        assert_eq!(ansi.attr, 0xF8);
        run(&mut ansi, b"\x1B[0;1;5;31;44m\x1B[8m");
        assert_eq!(ansi.attr, 0x88);
        run(&mut ansi, b"\x1B[0;4m");
        assert_eq!(ansi.attr, 0x01);
    }

    #[test]
    fn cursor_and_erase_sequences_are_commands() {
        let mut ansi = Ansi::default();
        let feeds = run(
            &mut ansi,
            b"\x1B[5;10H\x1B[f\x1B[3A\x1B[B\x1B[2J\x1B[K\x1B[s\x1B[u",
        );
        use Command::*;
        let commands = [
            Position(5, 10),
            Position(1, 1),
            Up(3),
            Down(1),
            EraseScreen,
            EraseLine,
            Save,
            Restore,
        ];
        assert_eq!(feeds, commands.map(Feed::Command));
    }

    #[test]
    fn modes_key_reassignments_and_unknown_sequences_show_nothing() {
        let mut ansi = Ansi::default();
        let feeds = run(
            &mut ansi,
            b"\x1B[=3h\x1B[?7l\x1B[0;59;\"dir\";13p\x1B[6n\x1BxA",
        );
        assert_eq!(feeds, [Feed::Text(b'A')]);
        assert_eq!(ansi.attr, 0x07);
    }
}
