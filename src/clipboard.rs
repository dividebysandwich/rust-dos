//! The host's clipboard and the text screen: a block of character cells
//! selected by dragging with the right mouse button while the mouse isn't
//! captured, copied with Ctrl+Shift+C (the whole screen when nothing is
//! selected), and the clipboard's text typed on the machine's keyboard with
//! Ctrl+Shift+V.

use std::collections::VecDeque;

use crate::cpu::Cpu;
use crate::debug::keys::PcKey;
use crate::debug::{LowInput, keys_for_char};
use crate::keyboard;
use crate::video::{self, Frame};
use rust_dos::bus::Bus;

/// Frames a pasted keystroke waits for the program to take the one before
/// from the keyboard controller, and from the BIOS's buffer when that is
/// full, before it goes anyway.
const KBC_STALL_FRAMES: u32 = 2;
const BUFFER_STALL_FRAMES: u32 = 60;

/// Character cells selected: the one the drag started on and the one the
/// pointer is over, as (column, row).
struct Selection {
    anchor: (usize, usize),
    end: (usize, usize),
    /// The screen's columns and rows: a screen of another shape ends it.
    size: (usize, usize),
    dragging: bool,
    /// Whether the pointer left the cell it started on: a right click
    /// without a drag selects nothing.
    moved: bool,
}

impl Selection {
    /// The first and the last column and row.
    fn rect(&self) -> ((usize, usize), (usize, usize)) {
        let (c0, c1) = (self.anchor.0.min(self.end.0), self.anchor.0.max(self.end.0));
        let (r0, r1) = (self.anchor.1.min(self.end.1), self.anchor.1.max(self.end.1));
        ((c0, r0), (c1, r1))
    }
}

pub struct Clipboard {
    selection: Option<Selection>,
    /// The pasted text's keys still to press and release, the keys pressed
    /// and not released yet, and the frames the next one waited.
    typing: VecDeque<LowInput>,
    held: Vec<PcKey>,
    stall: u32,
}

/// The text screen's geometry, unless it shows graphics.
fn text_geometry(bus: &Bus) -> Option<video::text::TextGeometry> {
    if bus.voodoo_output() {
        return None;
    }
    video::text::geometry(bus).filter(|g| g.cols > 0 && g.rows > 0)
}

/// The cell under the frame pixel `(x, y)`, and the screen's size in cells.
fn cell_at(bus: &Bus, (x, y): (i32, i32)) -> Option<((usize, usize), (usize, usize))> {
    let g = text_geometry(bus)?;
    let col = (x.max(0) as usize / g.cell_w()).min(g.cols - 1);
    let row = (y.max(0) as usize / g.cell_h()).min(g.rows - 1);
    Some(((col, row), (g.cols, g.rows)))
}

impl Clipboard {
    pub fn new() -> Self {
        Self { selection: None, typing: VecDeque::new(), held: Vec::new(), stall: 0 }
    }

    /// Start selecting at the frame pixel `at`; false on a graphics screen.
    pub fn start(&mut self, bus: &Bus, at: (i32, i32)) -> bool {
        let Some((cell, size)) = cell_at(bus, at) else { return false };
        self.selection = Some(Selection { anchor: cell, end: cell, size, dragging: true, moved: false });
        true
    }

    pub fn dragging(&self) -> bool {
        self.selection.as_ref().is_some_and(|s| s.dragging)
    }

    /// The pointer moved to the frame pixel `at` during the drag.
    pub fn drag(&mut self, bus: &Bus, at: (i32, i32)) {
        let Some(sel) = &mut self.selection else { return };
        match cell_at(bus, at) {
            Some((cell, size)) if size == sel.size => {
                sel.moved |= cell != sel.anchor;
                sel.end = cell;
            }
            _ => self.selection = None,
        }
    }

    /// The button came up at the frame pixel `at`: whether cells are
    /// selected, or it was a click.
    pub fn finish(&mut self, bus: &Bus, at: (i32, i32)) -> bool {
        self.drag(bus, at);
        match &mut self.selection {
            Some(sel) if sel.moved => {
                sel.dragging = false;
                true
            }
            _ => {
                self.selection = None;
                false
            }
        }
    }

    pub fn clear(&mut self) {
        self.selection = None;
    }

    /// The selection, while the screen still has its shape.
    fn current(&self, bus: &Bus) -> Option<(&Selection, video::text::TextGeometry)> {
        let sel = self.selection.as_ref().filter(|s| s.moved)?;
        let g = text_geometry(bus).filter(|g| (g.cols, g.rows) == sel.size)?;
        Some((sel, g))
    }

    /// The selected cells' text, or the whole screen's with nothing
    /// selected, a line per row without the spaces at the end; None on a
    /// graphics screen.
    pub fn text(&self, bus: &Bus) -> Option<String> {
        let (first, last) = match self.current(bus) {
            Some((sel, _)) => sel.rect(),
            None => {
                let g = text_geometry(bus)?;
                ((0, 0), (g.cols - 1, g.rows - 1))
            }
        };
        let g = text_geometry(bus)?;
        let vram = bus.display_mem();
        let lines: Vec<String> = (first.1..=last.1)
            .map(|row| {
                let line: String = (first.0..=last.0)
                    .map(|col| {
                        let off = (g.start + row * g.row_bytes + col * 2) & g.wrap;
                        video::CP437[vram.get(off).copied().unwrap_or(b' ') as usize]
                    })
                    .collect();
                line.trim_end().to_string()
            })
            .collect();
        let text = lines.join("\n");
        Some(text.trim_end_matches('\n').to_string())
    }

    /// Show the selected cells inverted on `frame`, the picture of `bus`'s
    /// screen.
    pub fn draw(&self, frame: &mut Frame, bus: &Bus) {
        let Some((sel, g)) = self.current(bus) else { return };
        let ((c0, r0), (c1, r1)) = sel.rect();
        let (w, h) = (frame.width as usize, frame.height as usize);
        let (x0, x1) = ((c0 * g.cell_w()).min(w), ((c1 + 1) * g.cell_w()).min(w));
        let (y0, y1) = ((r0 * g.cell_h()).min(h), ((r1 + 1) * g.cell_h()).min(h));
        for y in y0..y1 {
            for v in &mut frame.rgb[(y * w + x0) * 3..(y * w + x1) * 3] {
                *v = !*v;
            }
        }
    }

    /// Type `text` on the machine's keyboard, a key a frame (see `feed`),
    /// with the keys the keyboard layout has its characters on: the number
    /// of characters that will be typed and of those no key types.
    pub fn paste(&mut self, bus: &Bus, text: &str) -> (usize, usize) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let (mut typed, mut skipped) = (0, 0);
        let mut keys = Vec::new();
        for c in text.chars() {
            if keys_for_char(c, bus.typing_layout(), &mut keys).is_ok() {
                typed += 1;
            } else {
                skipped += 1;
            }
        }
        self.typing.extend(keys);
        (typed, skipped)
    }

    pub fn pasting(&self) -> bool {
        !self.typing.is_empty()
    }

    /// Press or release the next pasted key, once the program has read the
    /// ones before. Call once a frame while the machine runs.
    pub fn feed(&mut self, cpu: &mut Cpu) {
        let Some(input) = self.typing.front() else { return };
        let bios = keyboard::bios_keystrokes(&cpu.bus);
        let full = !bios && cpu.bus.keyboard_buffer.len() >= keyboard::BIOS_BUFFER_KEYS;
        if (cpu.bus.kbc.pending() > 0 && self.stall < KBC_STALL_FRAMES)
            || (full && matches!(input, LowInput::KeyDown { .. } | LowInput::Char(_)) && self.stall < BUFFER_STALL_FRAMES)
        {
            self.stall += 1;
            return;
        }
        self.stall = 0;
        match self.typing.pop_front() {
            Some(LowInput::KeyDown { key, ascii }) => {
                keyboard::apply_key(&mut cpu.bus, key, ascii, true);
                self.held.push(key);
            }
            Some(LowInput::KeyUp { key }) => self.release(cpu, key),
            // Typed as Alt and the keypad type it: a keystroke with no
            // scan code, for the BIOS's buffer where the host keeps it.
            Some(LowInput::Char(byte)) if !bios && !full => cpu.bus.keyboard_buffer.push_back(byte as u16),
            _ => {}
        }
    }

    fn release(&mut self, cpu: &mut Cpu, key: PcKey) {
        if let Some(i) = self.held.iter().position(|k| k.scan == key.scan && k.extended == key.extended) {
            self.held.remove(i);
            keyboard::apply_key(&mut cpu.bus, key, 0, false);
        }
    }

    /// Stop typing the pasted text, and release its keys.
    pub fn stop_paste(&mut self, cpu: &mut Cpu) {
        self.typing.clear();
        self.stall = 0;
        for key in std::mem::take(&mut self.held) {
            keyboard::apply_key(&mut cpu.bus, key, 0, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Clipboard;
    use crate::cpu::Cpu;

    fn cpu_with(lines: &[&str]) -> Cpu {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        for (row, line) in lines.iter().enumerate() {
            for (col, byte) in line.bytes().enumerate() {
                cpu.bus.write_8(0xB8000 + row * 160 + col * 2, byte);
            }
        }
        cpu
    }

    #[test]
    fn a_drag_selects_a_block_of_cells() {
        let cpu = cpu_with(&["C:\\>DIR", "HELLO WORLD", "  NEXT LINE"]);
        let mut clipboard = Clipboard::new();
        // From row 0, column 3 to row 2, column 6, backwards.
        assert!(clipboard.start(&cpu.bus, (6 * 8 + 4, 2 * 16 + 3)));
        clipboard.drag(&cpu.bus, (100, 100));
        assert!(clipboard.finish(&cpu.bus, (3 * 8, 0)));
        assert_eq!(clipboard.text(&cpu.bus).unwrap(), ">DIR\nLO W\nEXT");
    }

    #[test]
    fn a_click_selects_nothing_and_copies_the_screen() {
        let cpu = cpu_with(&["C:\\>DIR", "", "  NEXT"]);
        let mut clipboard = Clipboard::new();
        assert!(clipboard.start(&cpu.bus, (10, 10)));
        assert!(!clipboard.finish(&cpu.bus, (12, 12)));
        assert!(!clipboard.dragging());
        assert_eq!(clipboard.text(&cpu.bus).unwrap(), "C:\\>DIR\n\n  NEXT");
    }

    #[test]
    fn pasted_text_is_typed() {
        let mut cpu = cpu_with(&[]);
        let mut clipboard = Clipboard::new();
        assert_eq!(clipboard.paste(&cpu.bus, "dir\r\n"), (4, 0));
        for _ in 0..100 {
            clipboard.feed(&mut cpu);
        }
        assert!(!clipboard.pasting());
        let typed: Vec<u8> = cpu.bus.keyboard_buffer.iter().map(|&k| k as u8).collect();
        assert_eq!(typed, b"dir\r");
    }
}
