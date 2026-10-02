//! The line on the screen: its character cells from where it begins (the
//! cursor after the prompt), written with the video BIOS so they show in
//! any mode, and only those that changed since they were last written.

use super::LineEditor;
use crate::cpu::Cpu;
use crate::shell::video_call;

/// The page shown, the columns of a row and the rows of the screen.
fn geometry(cpu: &Cpu) -> (u8, usize, usize) {
    let page = cpu.bus.read_8(0x0462);
    let cols = (cpu.bus.read_16(0x044A) as usize).max(1);
    (page, cols, cpu.bus.text_rows())
}

/// The cursor of the page shown, as a cell from the top left.
pub fn cursor_cell(cpu: &Cpu) -> usize {
    let (page, cols, _) = geometry(cpu);
    let (col, row) = (cpu.bus.read_8(0x0450 + page as usize * 2), cpu.bus.read_8(0x0451 + page as usize * 2));
    row as usize * cols + col as usize
}

/// Put the cursor of the page shown on cell `at`.
pub fn set_cursor(cpu: &mut Cpu, at: usize) {
    let (page, cols, _) = geometry(cpu);
    video_call(cpu, 0x0200, (page as u16) << 8, 0, ((at / cols) as u16) << 8 | (at % cols) as u16);
}

/// The attribute of the cell at the cursor in a text mode, which the line
/// is written in where it has no colour of its own; 07h in a graphics
/// mode.
pub fn attribute_at_cursor(cpu: &mut Cpu) -> u8 {
    if crate::video::pixels::graphics_mode(&cpu.bus) {
        return 0x07;
    }
    let page = cpu.bus.read_8(0x0462);
    video_call(cpu, 0x0800, (page as u16) << 8, 0, 0);
    match cpu.get_ah() {
        0 => 0x07,
        attr => attr,
    }
}

/// Write the line's cells where they differ from what is on the screen,
/// scrolling the screen up first when they run past its last row, blank
/// what is left of a longer line before, and put the cursor in its place.
pub fn draw(cpu: &mut Cpu, ed: &mut LineEditor) {
    let (page, cols, rows) = geometry(cpu);
    let (cells, cursor) = ed.view();
    let mut start = ed.anchor;
    // The cell the cursor is on must be on the screen too.
    let last = start + cells.len().saturating_sub(1).max(cursor);
    let below = (last / cols + 1).saturating_sub(rows);
    if below > 0 {
        let bottom = ((rows - 1) as u16) << 8 | (cols - 1) as u16;
        video_call(cpu, 0x0600 | below.min(rows) as u16, (ed.attr as u16) << 8, 0, bottom);
        start = start.saturating_sub(below * cols);
        ed.anchor = start;
    }
    let graphics = crate::video::pixels::graphics_mode(&cpu.bus);
    for i in 0..cells.len().max(ed.shown.len()) {
        let cell = cells.get(i).copied().unwrap_or((b' ', ed.attr));
        if ed.shown.get(i) == Some(&cell) {
            continue;
        }
        let pos = start + i;
        if pos / cols >= rows {
            break;
        }
        set_cursor(cpu, pos);
        // In a graphics mode the attribute is the colour, which its top
        // bit would XOR in.
        let attr = if graphics { cell.1 & 0x0F } else { cell.1 };
        video_call(cpu, 0x0900 | cell.0 as u16, (page as u16) << 8 | attr as u16, 1, 0);
    }
    ed.shown = cells;
    set_cursor(cpu, start + cursor);
    cursor_shape(cpu, ed);
}

/// A block cursor while typing overwrites, the shape the screen had
/// otherwise.
fn cursor_shape(cpu: &mut Cpu, ed: &mut LineEditor) {
    match (ed.line.overwrite, ed.cursor_shape) {
        (true, None) => {
            let shape = cpu.bus.read_16(0x0460);
            ed.cursor_shape = Some(shape);
            video_call(cpu, 0x0100, 0, shape & 0x00FF, 0);
        }
        (false, Some(_)) => restore_cursor_shape(cpu, ed),
        _ => {}
    }
}

/// The cursor's shape as it was before overwriting.
pub fn restore_cursor_shape(cpu: &mut Cpu, ed: &mut LineEditor) {
    if let Some(shape) = ed.cursor_shape.take() {
        video_call(cpu, 0x0100, 0, shape, 0);
    }
}
