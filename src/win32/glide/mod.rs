//! GLIDE2X.DLL: 3dfx's Glide 2.x, the library Windows games of 1996 to
//! 1999 drew on a Voodoo with, written against the emulated Voodoo's
//! registers as 3dfx's own library was against the card.
//!
//! It has three layers:
//!
//! * `hw`: the card. Which board the machine has, its registers, its
//!   texture memory and linear frame buffer, and its display: a Voodoo
//!   Graphics or Voodoo2 add-on card takes the monitor from the VGA
//!   through its pass-through and gives it back; a Banshee or Voodoo3
//!   switches to a mode of its own, with the 3D engine's buffers in its
//!   memory, and back to the mode it was in.
//! * `api`: Glide's functions with Rust types, which keep Glide's
//!   shadows of the card's registers in `State` and write the card the
//!   way 3dfx's library did: combine units, blending, depth, fog, clip,
//!   triangles through the card's triangle setup registers, texture
//!   downloads, buffer clears and swaps, and the linear frame buffer.
//!   It also has what Direct3D's HAL needs of the card beside Glide's
//!   functions: drawing into a buffer at an address of the Banshee's
//!   memory, a depth buffer at another, and reading back a rectangle of
//!   a buffer.
//! * `exports`: GLIDE2X.DLL's functions by their stdcall names
//!   (`_grDrawTriangle@12`), which read what the program passes and call
//!   `api`, and `gu`, Glide's utility functions (fog tables, 3DF files,
//!   the texture memory manager, clipped triangles).
//!
//! The process that opened the card (grSstWinOpen) owns it until it
//! closes it or ends; its end gives the screen back as grSstWinClose
//! does.
//!
//! (Phase 4 of docs/native-windows.md.)

pub mod api;
mod exports;
mod gu;
pub mod hw;

use super::stubs::Dll;
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "GLIDE2X.DLL",
    base: 0xBFB0_0000,
    apis: exports::APIS,
    vars: &[],
    init: None,
    vtables: &[],
};

/// Glide's state: the system's, as there is one card.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The process that has the Voodoo open (grSstWinOpen), or 0.
    pub owner: u32,
    /// The display the card shows and the buffers it draws into.
    pub screen: hw::Screen,
    /// Glide's shadows of the card's registers and its own settings.
    pub gc: api::Context,
    /// The utility library's texture memory manager.
    pub gu: gu::Gu,
}

crate::state_fields!(State { owner, screen, gc, gu });

pub fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.glide
}

/// A process ended: the Voodoo it had open is closed, and the screen is
/// given back. Its memory, and its views of the card with it, is gone
/// already.
pub fn process_ended(cpu: &mut Cpu, pid: u32, _tids: &[u32]) {
    if state(cpu).owner == pid {
        let s = state(cpu);
        s.screen.view = hw::View::default();
        s.screen.pipe_view = hw::View::default();
        api::close(cpu);
    }
}
