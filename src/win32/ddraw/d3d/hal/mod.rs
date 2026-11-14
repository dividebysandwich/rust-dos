//! The HAL: Direct3D's drawing on the emulated card, whichever card it
//! is. Direct3D 7 and earlier, Direct3D 8 and OpenGL all draw through
//! what is here, and none of them knows the card behind it.
//!
//! Two cards have a HAL:
//!
//! - `voodoo`: a Banshee, a Voodoo3, or a Voodoo Graphics or Voodoo2
//!   add-on card, drawn on through the Glide layer as 3dfx's Direct3D
//!   driver drew.
//! - `toadforce`: the ToadForce (`crate::toadforce`), drawn on with its
//!   level 1 commands.
//!
//! `card` tells which one draws for a DirectDraw object, and with it
//! what the device reports (`Offer`). Before each batch of primitives
//! `bind` puts the device's state on the card, and `Raster` then takes
//! the pipeline's screen vertices. `clear` fills the render target and
//! its depth buffer. `created`, `release` and `process_ended` follow
//! the devices' lives, for the memory a HAL keeps on its card.

pub mod toadforce;
pub mod voodoo;

use super::super::Format;
use super::device::Device;
use super::pipeline::{Layout, Rasterizer, ScreenVertex, Setup, Vertex};
use crate::cpu::Cpu;

pub use toadforce::Source;
pub(in crate::win32::ddraw) use voodoo::card_fill;
pub use voodoo::{
    RS_ALPHABLENDENABLE, RS_ALPHAREF, RS_ALPHATESTENABLE, RS_COLORKEYENABLE, RS_DITHERENABLE,
    RS_FOGCOLOR,
};

/// The card a HAL draws on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Voodoo,
    ToadForce,
}

impl Kind {
    /// The kind as a number, for what holds only numbers.
    pub fn code(self) -> u32 {
        self as u32
    }

    pub fn from_code(code: u32) -> Kind {
        if code == Kind::ToadForce as u32 {
            Kind::ToadForce
        } else {
            Kind::Voodoo
        }
    }
}

/// What a card's HAL offers Direct3D: the card, and the textures it
/// draws with at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    pub kind: Kind,
    pub units: u32,
}

/// The HAL of the card the DirectDraw object `dd` draws on, if Direct3D
/// has one there: the 3dfx device's add-on card, or the display
/// adapter when it is a Banshee, a Voodoo3 or a ToadForce with level 1.
pub fn card(cpu: &mut Cpu, dd: u32) -> Option<Offer> {
    if super::super::voodoo::is_card_object(cpu, dd) {
        return crate::win32::glide::hw::board(cpu).map(|b| Offer {
            kind: Kind::Voodoo,
            units: b.tmus,
        });
    }
    adapter(cpu)
}

/// The HAL of the display adapter, if Direct3D has one there: a
/// Banshee, a Voodoo3 or a ToadForce with level 1. Direct3D 8 has its
/// device here only.
pub fn adapter(cpu: &Cpu) -> Option<Offer> {
    if toadforce::there(cpu) {
        return Some(Offer {
            kind: Kind::ToadForce,
            units: toadforce::UNITS,
        });
    }
    let units = match cpu.bus.vga.adapter.voodoo_board()? {
        crate::voodoo::Board::Banshee => 1,
        _ => 2,
    };
    Some(Offer {
        kind: Kind::Voodoo,
        units,
    })
}

/// The lights a card of `kind` has on at once where it transforms and
/// lights vertices itself, which a card that does has Direct3D 7's T&L
/// HAL device for, beside the HAL one. None on a card that doesn't.
pub fn tnl_lights(kind: Kind) -> Option<u32> {
    match kind {
        Kind::Voodoo => None,
        Kind::ToadForce => Some(toadforce::TNL_LIGHTS),
    }
}

/// Whether a card of `kind` transforms and lights vertices itself, and
/// so has Direct3D 7's T&L HAL device.
pub fn transforms(kind: Kind) -> bool {
    tnl_lights(kind).is_some()
}

/// The card that draws for the device `d`.
pub fn kind_of(cpu: &mut Cpu, d: &Device) -> Kind {
    card(cpu, d.ddraw).map_or(Kind::Voodoo, |offer| offer.kind)
}

/// D3DPTEXTURECAPS_CUBEMAP.
pub const TEXTURECAPS_CUBEMAP: u32 = 0x800;

/// Whether the card the DirectDraw object `dd` draws on samples cube
/// maps: DirectDraw makes them where it does.
pub fn cube_maps(cpu: &mut Cpu, dd: u32) -> bool {
    card(cpu, dd).is_some_and(|offer| description(offer.kind).texture & TEXTURECAPS_CUBEMAP != 0)
}

/// What a card's device description says that differs from card to
/// card (`caps.rs` writes the descriptions).
#[derive(Clone, Debug, PartialEq)]
pub struct Description {
    /// DDBD_ bits: the depths of render targets and of depth buffers.
    pub target_depths: u32,
    pub z_depths: u32,
    /// The largest width and height of a texture, and the largest ratio
    /// of the two.
    pub texture_max: u32,
    pub texture_aspect: u32,
    /// D3DPRASTERCAPS_, D3DPTEXTURECAPS_, D3DPTFILTERCAPS_ and
    /// D3DPTADDRESSCAPS_ bits a card adds to the ones every HAL has.
    pub raster: u32,
    pub texture: u32,
    pub filter: u32,
    pub address: u32,
    /// D3DPBLENDCAPS_ bits it adds, for the source factor and, those
    /// that are destination factors, for the destination.
    pub blend: u32,
    /// The guard band the device reports: left, top, right, bottom.
    pub guard_band: [f32; 4],
    /// D3DSTENCILCAPS_.
    pub stencil: u32,
    /// D3DTEXOPCAPS_: the stage operations it draws.
    pub ops: u32,
    /// The blend stages it has.
    pub stages: u32,
    /// The largest anisotropy.
    pub anisotropy: u32,
    /// The texture formats and the depth buffer formats.
    pub textures: Vec<Format>,
    pub z_formats: Vec<Format>,
}

/// What `kind`'s device description says.
pub fn description(kind: Kind) -> Description {
    match kind {
        Kind::Voodoo => voodoo::description(),
        Kind::ToadForce => toadforce::description(),
    }
}

/// Whether `kind` can do a stage's operation, for ValidateDevice.
pub fn can_do(kind: Kind, op: u32) -> bool {
    match kind {
        Kind::Voodoo => voodoo::can_do(op),
        Kind::ToadForce => toadforce::can_do(op),
    }
}

/// The band on the screen the pipeline clips what `kind` draws to:
/// left, top, right and bottom.
pub fn vertex_band(kind: Kind) -> [f32; 4] {
    match kind {
        Kind::Voodoo => voodoo::VERTEX_BAND,
        Kind::ToadForce => toadforce::VERTEX_BAND,
    }
}

/// Whether a surface of `format` in video memory can be a render target
/// of the card `kind`, and whether one can be its depth buffer.
pub fn takes_target(cpu: &mut Cpu, kind: Kind, format: &Format, video: bool) -> bool {
    match kind {
        Kind::Voodoo => voodoo::takes_target(cpu, format, video),
        Kind::ToadForce => toadforce::takes_target(format, video),
    }
}

pub fn takes_depth(cpu: &mut Cpu, kind: Kind, format: &Format, video: bool) -> bool {
    match kind {
        Kind::Voodoo => voodoo::takes_depth(cpu, format, video),
        Kind::ToadForce => toadforce::takes_depth(format, video),
    }
}

/// How a batch of primitives is drawn, as `bind` set the card up.
#[derive(Clone, Copy, Debug)]
pub enum Card {
    Voodoo(voodoo::Card),
    ToadForce(toadforce::Card),
}

/// Whether the device `d` can be drawn by its card now, and the card's
/// state for it. None draws nothing; the HAL logs why.
pub fn bind(cpu: &mut Cpu, d: &Device, setup: &Setup) -> Option<Card> {
    match kind_of(cpu, d) {
        Kind::Voodoo => voodoo::bind(cpu, d, setup).map(Card::Voodoo),
        Kind::ToadForce => toadforce::bind(cpu, d, setup).map(Card::ToadForce),
    }
}

/// Draw `kind` primitives of `vertices`, which aren't in screen space,
/// in the order `order` or as they are, on a card that transforms and
/// lights vertices itself. False when the card of `d` doesn't, or
/// can't for this draw: the pipeline then does, and hands the card
/// screen vertices.
pub fn draw_geometry(
    cpu: &mut Cpu,
    d: &Device,
    setup: &Setup,
    layout: &Layout,
    vertices: &[Vertex],
    kind: u32,
    order: Option<&[u32]>,
) -> bool {
    match kind_of(cpu, d) {
        Kind::Voodoo => false,
        Kind::ToadForce => toadforce::draw_geometry(cpu, d, setup, layout, vertices, kind, order),
    }
}

/// Draw `kind` primitives of the device `d`'s transformed vertices of
/// `layout`, which are at `from` in the program's memory, on a card
/// that takes them from there: the triangles drawn. None when the card
/// of `d` doesn't, or can't for this draw, with nothing drawn: the
/// pipeline then reads them, and hands the card what it makes of them.
pub fn draw_screen(
    cpu: &mut Cpu,
    d: &Device,
    setup: &Setup,
    layout: &Layout,
    kind: u32,
    from: &Source,
) -> Option<u32> {
    match kind_of(cpu, d) {
        Kind::Voodoo => None,
        Kind::ToadForce => toadforce::draw_screen(cpu, d, setup, layout, kind, from),
    }
}

/// The card as the pipeline's rasterizer.
pub enum Raster<'a> {
    Voodoo(voodoo::Raster<'a>),
    ToadForce(toadforce::Raster<'a>),
}

impl Raster<'_> {
    /// The card as the rasterizer of a batch, as `bind` set it up.
    pub fn new(cpu: &mut Cpu, card: Card) -> Raster<'_> {
        match card {
            Card::Voodoo(card) => Raster::Voodoo(voodoo::Raster::new(cpu, card)),
            Card::ToadForce(card) => Raster::ToadForce(toadforce::Raster::new(cpu, card)),
        }
    }

    /// The end of the batch.
    pub fn finish(self) {
        match self {
            Raster::Voodoo(raster) => raster.finish(),
            Raster::ToadForce(raster) => raster.finish(),
        }
    }
}

impl Rasterizer for Raster<'_> {
    fn triangle(&mut self, v: &[ScreenVertex; 3]) {
        match self {
            Raster::Voodoo(raster) => raster.triangle(v),
            Raster::ToadForce(raster) => raster.triangle(v),
        }
    }

    fn line(&mut self, v: &[ScreenVertex; 2]) {
        match self {
            Raster::Voodoo(raster) => raster.line(v),
            Raster::ToadForce(raster) => raster.line(v),
        }
    }

    fn point(&mut self, v: &ScreenVertex) {
        match self {
            Raster::Voodoo(raster) => raster.point(v),
            Raster::ToadForce(raster) => raster.point(v),
        }
    }
}

/// What `clear` fills: the render target with a color (ARGB), the depth
/// buffer with a depth, the stencil buffer with a value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fill {
    pub target: Option<u32>,
    pub depth: Option<f32>,
    pub stencil: Option<u32>,
}

/// Fill rectangles (left, top, right, bottom) of the device `d`'s render
/// target and of its depth buffer as `fill` says, or the whole target
/// with no rectangles. The depth buffer is filled where it is under the
/// target.
pub fn clear(cpu: &mut Cpu, d: &Device, rects: &[[i32; 4]], fill: Fill) {
    match kind_of(cpu, d) {
        Kind::Voodoo => voodoo::clear(cpu, d, rects, fill.target, fill.depth),
        Kind::ToadForce => toadforce::clear(cpu, d, rects, fill),
    }
}

/// A device of the DirectDraw object `dd` is made.
pub fn created(cpu: &mut Cpu, dd: u32) {
    match card(cpu, dd).map(|offer| offer.kind) {
        Some(Kind::ToadForce) => toadforce::created(cpu),
        _ => voodoo::created(cpu),
    }
}

/// The device `gone` goes: what the HALs kept for the devices goes with
/// the last of them.
pub fn release(cpu: &mut Cpu, gone: u32) {
    voodoo::release(cpu, gone);
    toadforce::release(cpu, gone);
}

/// A process ended: the HALs give back the video memory they kept for
/// it.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    voodoo::process_ended(cpu, pid);
    toadforce::process_ended(cpu, pid);
}

/// Video memory was lost (a mode was set): the blocks the HALs took
/// from it are gone, and other surfaces may have them now.
pub fn video_memory_lost(cpu: &mut Cpu) {
    toadforce::video_memory_lost(cpu);
}
