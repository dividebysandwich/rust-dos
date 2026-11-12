//! The render states and stage states of level 1: their numbers, the
//! values each takes and its value after a reset (chapter 9 of
//! `docs/toadforce-spec.md`).
//!
//! The numbers and values are Direct3D 8's where Direct3D has the state,
//! so a Direct3D driver passes most of them through. The states from 256
//! (render) and 32 (stage) up are the card's own, for what OpenGL has
//! and Direct3D lacks.

use crate::toadforce::caps;

/// What values a state takes. A write of another value is rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// 0 or 1.
    Bool,
    /// A whole number from the first to the second.
    Range(u32, u32),
    /// Any of these bits.
    Mask(u32),
    /// Any word: a colour, a reference value.
    Word,
    /// A finite single-precision number.
    Float,
    /// A stage operation: `OP_DISABLE` to `OP_LERP` but
    /// `OP_PREMODULATE`.
    Op,
    /// A stage argument: a source in bits 0 to 3 with `ARG_COMPLEMENT`
    /// and `ARG_ALPHA_REPLICATE`.
    Arg,
    /// Where a stage's result goes: `ARG_CURRENT` or `ARG_TEMP`.
    Result,
    /// A coordinate set in bits 0 to 15 and how it is generated in bits
    /// 16 to 31.
    TexCoord,
    /// The count of transformed coordinates, 0 to 4, with
    /// `TRANSFORM_PROJECTED`.
    TexTransform,
    /// A result scale: 1, 2 or 4.
    Scale,
}

impl Kind {
    pub fn takes(self, value: u32) -> bool {
        match self {
            Kind::Bool => value <= 1,
            Kind::Range(low, high) => (low..=high).contains(&value),
            Kind::Mask(mask) => value & !mask == 0,
            Kind::Word => true,
            Kind::Float => f32::from_bits(value).is_finite(),
            Kind::Op => (OP_DISABLE..=OP_LERP).contains(&value) && value != OP_PREMODULATE,
            Kind::Arg => value & !(0xF | ARG_FLAGS) == 0 && value & 0xF <= ARG_TEMP,
            Kind::Result => value == ARG_CURRENT || value == ARG_TEMP,
            Kind::TexCoord => value & 0xFFFF < COORD_SETS && value >> 16 <= GEN_SPHERE_MAP,
            Kind::TexTransform => value & !TRANSFORM_PROJECTED <= 4,
            Kind::Scale => matches!(value, 1 | 2 | 4),
        }
    }
}

/// A state: its number, its name in the specification, the values it
/// takes and its value after a reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Def {
    pub id: u32,
    pub name: &'static str,
    pub kind: Kind,
    pub reset: u32,
}

const fn def(id: u32, name: &'static str, kind: Kind, reset: u32) -> Def {
    Def {
        id,
        name,
        kind,
        reset,
    }
}

const fn float(id: u32, name: &'static str, reset: f32) -> Def {
    def(id, name, Kind::Float, reset.to_bits())
}

// Stage operations.
pub const OP_DISABLE: u32 = 1;
pub const OP_SELECT_ARG1: u32 = 2;
pub const OP_MODULATE: u32 = 4;
pub const OP_PREMODULATE: u32 = 17;
pub const OP_LERP: u32 = 26;

// Stage arguments.
pub const ARG_DIFFUSE: u32 = 0;
pub const ARG_CURRENT: u32 = 1;
pub const ARG_TEXTURE: u32 = 2;
pub const ARG_TFACTOR: u32 = 3;
pub const ARG_SPECULAR: u32 = 4;
pub const ARG_TEMP: u32 = 5;
pub const ARG_COMPLEMENT: u32 = 0x10;
pub const ARG_ALPHA_REPLICATE: u32 = 0x20;
const ARG_FLAGS: u32 = ARG_COMPLEMENT | ARG_ALPHA_REPLICATE;

/// The texture coordinate sets a vertex can have.
pub const COORD_SETS: u32 = caps::COORD_SETS;
/// The last way of generating coordinates (bits 16 to 31 of
/// `TEXCOORDINDEX`): 0 the vertex's own, 1 the normal, 2 the position
/// and 3 the reflection vector in camera space, 4 a sphere map.
pub const GEN_SPHERE_MAP: u32 = 4;
pub const TRANSFORM_PROJECTED: u32 = 256;

/// The compare functions, `NEVER` to `ALWAYS`.
const COMPARE: Kind = Kind::Range(1, 8);
/// The blend factors, `ZERO` to `BOTHINVSRCALPHA`.
const BLEND: Kind = Kind::Range(1, 13);
/// The stencil operations, `KEEP` to `DECR`.
const STENCIL_OP: Kind = Kind::Range(1, 8);
/// Where a material colour comes from: the material, the vertex's
/// diffuse or its specular colour.
const SOURCE: Kind = Kind::Range(0, 2);
/// A fog formula: none, exponential, exponential squared, linear.
const FOG: Kind = Kind::Range(0, 3);

// Render states the card's own code names.
pub const RS_ZENABLE: u32 = 7;
pub const RS_FILLMODE: u32 = 8;
pub const RS_SHADEMODE: u32 = 9;
pub const RS_ZWRITEENABLE: u32 = 14;
pub const RS_ALPHATESTENABLE: u32 = 15;
pub const RS_LASTPIXEL: u32 = 16;
pub const RS_SRCBLEND: u32 = 19;
pub const RS_DESTBLEND: u32 = 20;
pub const RS_CULLMODE: u32 = 22;
pub const RS_ZFUNC: u32 = 23;
pub const RS_ALPHAREF: u32 = 24;
pub const RS_ALPHAFUNC: u32 = 25;
pub const RS_DITHERENABLE: u32 = 26;
pub const RS_ALPHABLENDENABLE: u32 = 27;
pub const RS_FOGENABLE: u32 = 28;
pub const RS_SPECULARENABLE: u32 = 29;
pub const RS_FOGCOLOR: u32 = 34;
pub const RS_FOGTABLEMODE: u32 = 35;
pub const RS_FOGSTART: u32 = 36;
pub const RS_FOGEND: u32 = 37;
pub const RS_FOGDENSITY: u32 = 38;
pub const RS_COLORKEYENABLE: u32 = 41;
pub const RS_ZBIAS: u32 = 47;
pub const RS_RANGEFOGENABLE: u32 = 48;
pub const RS_STENCILENABLE: u32 = 52;
pub const RS_STENCILFAIL: u32 = 53;
pub const RS_STENCILZFAIL: u32 = 54;
pub const RS_STENCILPASS: u32 = 55;
pub const RS_STENCILFUNC: u32 = 56;
pub const RS_STENCILREF: u32 = 57;
pub const RS_STENCILMASK: u32 = 58;
pub const RS_STENCILWRITEMASK: u32 = 59;
pub const RS_TEXTUREFACTOR: u32 = 60;
pub const RS_WRAP0: u32 = 128;
pub const RS_CLIPPING: u32 = 136;
pub const RS_LIGHTING: u32 = 137;
pub const RS_AMBIENT: u32 = 139;
pub const RS_FOGVERTEXMODE: u32 = 140;
pub const RS_COLORVERTEX: u32 = 141;
pub const RS_LOCALVIEWER: u32 = 142;
pub const RS_NORMALIZENORMALS: u32 = 143;
pub const RS_DIFFUSEMATERIALSOURCE: u32 = 145;
pub const RS_SPECULARMATERIALSOURCE: u32 = 146;
pub const RS_AMBIENTMATERIALSOURCE: u32 = 147;
pub const RS_EMISSIVEMATERIALSOURCE: u32 = 148;
pub const RS_VERTEXBLEND: u32 = 151;
pub const RS_CLIPPLANEENABLE: u32 = 152;
pub const RS_POINTSIZE: u32 = 154;
pub const RS_POINTSIZE_MIN: u32 = 155;
pub const RS_POINTSPRITEENABLE: u32 = 156;
pub const RS_POINTSCALEENABLE: u32 = 157;
pub const RS_POINTSCALE_A: u32 = 158;
pub const RS_POINTSCALE_B: u32 = 159;
pub const RS_POINTSCALE_C: u32 = 160;
pub const RS_POINTSIZE_MAX: u32 = 166;
pub const RS_COLORWRITEENABLE: u32 = 168;
pub const RS_BLENDOP: u32 = 171;
/// The card's own: 0 puts pixel centres at whole coordinates
/// (Direct3D 7 and 8), 1 at half ones (OpenGL).
pub const RS_PIXEL_CENTRE: u32 = 256;
/// The card's own: which vertex gives a flat-shaded primitive its
/// colour, 0 the first (Direct3D), 1 the last (OpenGL).
pub const RS_FLAT_VERTEX: u32 = 257;
/// The card's own: light the back of a triangle with its normal turned
/// round (OpenGL's two-sided lighting).
pub const RS_TWO_SIDED_LIGHTING: u32 = 258;
/// The card's own: table fog goes by a fragment's w, 1, or by its
/// depth, 0.
pub const RS_FOG_BY_W: u32 = 259;

/// The render states' numbers are below this.
pub const RENDER_STATES: usize = 260;

/// The render states of level 1.
pub const RENDER: &[Def] = &[
    def(RS_ZENABLE, "ZENABLE", Kind::Bool, 0),
    def(RS_FILLMODE, "FILLMODE", Kind::Range(1, 3), 3),
    def(RS_SHADEMODE, "SHADEMODE", Kind::Range(1, 2), 2),
    def(RS_ZWRITEENABLE, "ZWRITEENABLE", Kind::Bool, 1),
    def(RS_ALPHATESTENABLE, "ALPHATESTENABLE", Kind::Bool, 0),
    def(RS_LASTPIXEL, "LASTPIXEL", Kind::Bool, 1),
    def(RS_SRCBLEND, "SRCBLEND", BLEND, 2),
    def(RS_DESTBLEND, "DESTBLEND", BLEND, 1),
    def(RS_CULLMODE, "CULLMODE", Kind::Range(1, 3), 3),
    def(RS_ZFUNC, "ZFUNC", COMPARE, 4),
    def(RS_ALPHAREF, "ALPHAREF", Kind::Range(0, 255), 0),
    def(RS_ALPHAFUNC, "ALPHAFUNC", COMPARE, 8),
    def(RS_DITHERENABLE, "DITHERENABLE", Kind::Bool, 0),
    def(RS_ALPHABLENDENABLE, "ALPHABLENDENABLE", Kind::Bool, 0),
    def(RS_FOGENABLE, "FOGENABLE", Kind::Bool, 0),
    def(RS_SPECULARENABLE, "SPECULARENABLE", Kind::Bool, 0),
    def(RS_FOGCOLOR, "FOGCOLOR", Kind::Word, 0),
    def(RS_FOGTABLEMODE, "FOGTABLEMODE", FOG, 0),
    float(RS_FOGSTART, "FOGSTART", 0.0),
    float(RS_FOGEND, "FOGEND", 1.0),
    float(RS_FOGDENSITY, "FOGDENSITY", 1.0),
    def(RS_COLORKEYENABLE, "COLORKEYENABLE", Kind::Bool, 0),
    def(RS_ZBIAS, "ZBIAS", Kind::Range(0, 16), 0),
    def(RS_RANGEFOGENABLE, "RANGEFOGENABLE", Kind::Bool, 0),
    def(RS_STENCILENABLE, "STENCILENABLE", Kind::Bool, 0),
    def(RS_STENCILFAIL, "STENCILFAIL", STENCIL_OP, 1),
    def(RS_STENCILZFAIL, "STENCILZFAIL", STENCIL_OP, 1),
    def(RS_STENCILPASS, "STENCILPASS", STENCIL_OP, 1),
    def(RS_STENCILFUNC, "STENCILFUNC", COMPARE, 8),
    def(RS_STENCILREF, "STENCILREF", Kind::Range(0, 255), 0),
    def(RS_STENCILMASK, "STENCILMASK", Kind::Word, 0xFFFF_FFFF),
    def(
        RS_STENCILWRITEMASK,
        "STENCILWRITEMASK",
        Kind::Word,
        0xFFFF_FFFF,
    ),
    def(RS_TEXTUREFACTOR, "TEXTUREFACTOR", Kind::Word, 0xFFFF_FFFF),
    def(RS_WRAP0, "WRAP0", Kind::Mask(0xF), 0),
    def(RS_WRAP0 + 1, "WRAP1", Kind::Mask(0xF), 0),
    def(RS_WRAP0 + 2, "WRAP2", Kind::Mask(0xF), 0),
    def(RS_WRAP0 + 3, "WRAP3", Kind::Mask(0xF), 0),
    def(RS_WRAP0 + 4, "WRAP4", Kind::Mask(0xF), 0),
    def(RS_WRAP0 + 5, "WRAP5", Kind::Mask(0xF), 0),
    def(RS_WRAP0 + 6, "WRAP6", Kind::Mask(0xF), 0),
    def(RS_WRAP0 + 7, "WRAP7", Kind::Mask(0xF), 0),
    def(RS_CLIPPING, "CLIPPING", Kind::Bool, 1),
    def(RS_LIGHTING, "LIGHTING", Kind::Bool, 1),
    def(RS_AMBIENT, "AMBIENT", Kind::Word, 0),
    def(RS_FOGVERTEXMODE, "FOGVERTEXMODE", FOG, 0),
    def(RS_COLORVERTEX, "COLORVERTEX", Kind::Bool, 1),
    def(RS_LOCALVIEWER, "LOCALVIEWER", Kind::Bool, 1),
    def(RS_NORMALIZENORMALS, "NORMALIZENORMALS", Kind::Bool, 0),
    def(RS_DIFFUSEMATERIALSOURCE, "DIFFUSEMATERIALSOURCE", SOURCE, 1),
    def(
        RS_SPECULARMATERIALSOURCE,
        "SPECULARMATERIALSOURCE",
        SOURCE,
        2,
    ),
    def(RS_AMBIENTMATERIALSOURCE, "AMBIENTMATERIALSOURCE", SOURCE, 0),
    def(
        RS_EMISSIVEMATERIALSOURCE,
        "EMISSIVEMATERIALSOURCE",
        SOURCE,
        0,
    ),
    def(RS_VERTEXBLEND, "VERTEXBLEND", Kind::Range(0, 3), 0),
    def(RS_CLIPPLANEENABLE, "CLIPPLANEENABLE", Kind::Mask(0x3F), 0),
    float(RS_POINTSIZE, "POINTSIZE", 1.0),
    float(RS_POINTSIZE_MIN, "POINTSIZE_MIN", 1.0),
    def(RS_POINTSPRITEENABLE, "POINTSPRITEENABLE", Kind::Bool, 0),
    def(RS_POINTSCALEENABLE, "POINTSCALEENABLE", Kind::Bool, 0),
    float(RS_POINTSCALE_A, "POINTSCALE_A", 1.0),
    float(RS_POINTSCALE_B, "POINTSCALE_B", 0.0),
    float(RS_POINTSCALE_C, "POINTSCALE_C", 0.0),
    float(RS_POINTSIZE_MAX, "POINTSIZE_MAX", 64.0),
    def(
        RS_COLORWRITEENABLE,
        "COLORWRITEENABLE",
        Kind::Mask(0xF),
        0xF,
    ),
    def(RS_BLENDOP, "BLENDOP", Kind::Range(1, 5), 1),
    def(RS_PIXEL_CENTRE, "PIXEL_CENTRE", Kind::Bool, 0),
    def(RS_FLAT_VERTEX, "FLAT_VERTEX", Kind::Bool, 0),
    def(
        RS_TWO_SIDED_LIGHTING,
        "TWO_SIDED_LIGHTING",
        Kind::Range(0, 2),
        0,
    ),
    def(RS_FOG_BY_W, "FOG_BY_W", Kind::Bool, 0),
];

// Stage states.
pub const TSS_COLOROP: u32 = 1;
pub const TSS_COLORARG1: u32 = 2;
pub const TSS_COLORARG2: u32 = 3;
pub const TSS_ALPHAOP: u32 = 4;
pub const TSS_ALPHAARG1: u32 = 5;
pub const TSS_ALPHAARG2: u32 = 6;
pub const TSS_BUMPENVMAT00: u32 = 7;
pub const TSS_TEXCOORDINDEX: u32 = 11;
pub const TSS_ADDRESSU: u32 = 13;
pub const TSS_ADDRESSV: u32 = 14;
pub const TSS_BORDERCOLOR: u32 = 15;
pub const TSS_MAGFILTER: u32 = 16;
pub const TSS_MINFILTER: u32 = 17;
pub const TSS_MIPFILTER: u32 = 18;
pub const TSS_MIPMAPLODBIAS: u32 = 19;
pub const TSS_MAXMIPLEVEL: u32 = 20;
pub const TSS_MAXANISOTROPY: u32 = 21;
pub const TSS_BUMPENVLSCALE: u32 = 22;
pub const TSS_BUMPENVLOFFSET: u32 = 23;
pub const TSS_TEXTURETRANSFORMFLAGS: u32 = 24;
pub const TSS_COLORARG0: u32 = 26;
pub const TSS_ALPHAARG0: u32 = 27;
pub const TSS_RESULTARG: u32 = 28;
/// The card's own: the colour operation's result times 1, 2 or 4,
/// clamped (`ARB_texture_env_combine`'s `RGB_SCALE`).
pub const TSS_COLOR_SCALE: u32 = 32;
/// The card's own: the same for the alpha operation.
pub const TSS_ALPHA_SCALE: u32 = 33;

/// The stage states' numbers are below this.
pub const STAGE_STATES: usize = 34;
/// The blend stages.
pub const STAGES: usize = caps::STAGES as usize;

/// The stage states of level 1, with stage 0's values after a reset.
/// `stage_reset` has the other stages'.
pub const STAGE: &[Def] = &[
    def(TSS_COLOROP, "COLOROP", Kind::Op, OP_MODULATE),
    def(TSS_COLORARG1, "COLORARG1", Kind::Arg, ARG_TEXTURE),
    def(TSS_COLORARG2, "COLORARG2", Kind::Arg, ARG_CURRENT),
    def(TSS_ALPHAOP, "ALPHAOP", Kind::Op, OP_SELECT_ARG1),
    def(TSS_ALPHAARG1, "ALPHAARG1", Kind::Arg, ARG_TEXTURE),
    def(TSS_ALPHAARG2, "ALPHAARG2", Kind::Arg, ARG_CURRENT),
    float(TSS_BUMPENVMAT00, "BUMPENVMAT00", 0.0),
    float(TSS_BUMPENVMAT00 + 1, "BUMPENVMAT01", 0.0),
    float(TSS_BUMPENVMAT00 + 2, "BUMPENVMAT10", 0.0),
    float(TSS_BUMPENVMAT00 + 3, "BUMPENVMAT11", 0.0),
    def(TSS_TEXCOORDINDEX, "TEXCOORDINDEX", Kind::TexCoord, 0),
    def(TSS_ADDRESSU, "ADDRESSU", Kind::Range(1, 4), 1),
    def(TSS_ADDRESSV, "ADDRESSV", Kind::Range(1, 4), 1),
    def(TSS_BORDERCOLOR, "BORDERCOLOR", Kind::Word, 0),
    def(TSS_MAGFILTER, "MAGFILTER", Kind::Range(1, 3), 1),
    def(TSS_MINFILTER, "MINFILTER", Kind::Range(1, 3), 1),
    def(TSS_MIPFILTER, "MIPFILTER", Kind::Range(0, 2), 0),
    float(TSS_MIPMAPLODBIAS, "MIPMAPLODBIAS", 0.0),
    def(TSS_MAXMIPLEVEL, "MAXMIPLEVEL", Kind::Range(0, 11), 0),
    def(TSS_MAXANISOTROPY, "MAXANISOTROPY", Kind::Range(1, 8), 1),
    float(TSS_BUMPENVLSCALE, "BUMPENVLSCALE", 0.0),
    float(TSS_BUMPENVLOFFSET, "BUMPENVLOFFSET", 0.0),
    def(
        TSS_TEXTURETRANSFORMFLAGS,
        "TEXTURETRANSFORMFLAGS",
        Kind::TexTransform,
        0,
    ),
    def(TSS_COLORARG0, "COLORARG0", Kind::Arg, ARG_CURRENT),
    def(TSS_ALPHAARG0, "ALPHAARG0", Kind::Arg, ARG_CURRENT),
    def(TSS_RESULTARG, "RESULTARG", Kind::Result, ARG_CURRENT),
    def(TSS_COLOR_SCALE, "COLOR_SCALE", Kind::Scale, 1),
    def(TSS_ALPHA_SCALE, "ALPHA_SCALE", Kind::Scale, 1),
];

/// The render state `id`, if level 1 has it. By a table of the
/// numbers, made once: a driver asks for every state it sets before
/// each draw.
pub fn render(id: u32) -> Option<&'static Def> {
    static BY_ID: std::sync::OnceLock<[Option<&'static Def>; RENDER_STATES]> =
        std::sync::OnceLock::new();
    let table = BY_ID.get_or_init(|| {
        let mut table = [None; RENDER_STATES];
        for def in RENDER {
            table[def.id as usize] = Some(def);
        }
        table
    });
    *table.get(id as usize)?
}

/// The stage state `id`, if level 1 has it.
pub fn stage(id: u32) -> Option<&'static Def> {
    STAGE.iter().find(|def| def.id == id)
}

/// The value of stage state `def` after a reset, in `stage`: stage 0
/// modulates its texture with the vertex's colour and takes the
/// texture's alpha, the others are off, and each stage has the
/// coordinate set of its own number.
pub fn stage_reset(def: &Def, stage: usize) -> u32 {
    match def.id {
        TSS_COLOROP | TSS_ALPHAOP if stage > 0 => OP_DISABLE,
        TSS_TEXCOORDINDEX => stage as u32,
        _ => def.reset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_has_one_number_below_the_limit_and_a_reset_value_it_takes() {
        for (table, limit) in [(RENDER, RENDER_STATES), (STAGE, STAGE_STATES)] {
            for (i, def) in table.iter().enumerate() {
                assert!((def.id as usize) < limit, "{}", def.name);
                assert!(def.kind.takes(def.reset), "{}", def.name);
                assert!(
                    table[..i].iter().all(|other| other.id < def.id),
                    "{} is out of order",
                    def.name
                );
            }
        }
        for stage in 0..STAGES {
            for def in STAGE {
                assert!(def.kind.takes(stage_reset(def, stage)), "{}", def.name);
            }
        }
    }

    /// The tables of chapter 9 are these tables: a state added or
    /// renumbered here has to be written there.
    #[test]
    fn the_specification_lists_every_state_by_its_number() {
        let spec = include_str!("../../../docs/toadforce-spec.md");
        for def in RENDER.iter().chain(STAGE) {
            let row = format!("| {} | `{}` |", def.id, def.name);
            assert!(spec.contains(&row), "{row}");
        }
        let rows = |heading: &str| {
            let from = spec.find(heading).unwrap();
            let table = &spec[from..];
            let table = &table[table.find("|---").unwrap()..];
            let end = table.find("\n\n").unwrap();
            table[..end].lines().count() - 1
        };
        assert_eq!(rows("#### Render states"), RENDER.len());
        assert_eq!(rows("#### Stage states"), STAGE.len());
    }

    #[test]
    fn each_kind_takes_its_values_only() {
        assert!(Kind::Bool.takes(1) && !Kind::Bool.takes(2));
        assert!(Kind::Range(1, 8).takes(8) && !Kind::Range(1, 8).takes(0));
        assert!(Kind::Mask(0xF).takes(0xA) && !Kind::Mask(0xF).takes(0x10));
        assert!(Kind::Float.takes(1.5f32.to_bits()));
        assert!(!Kind::Float.takes(f32::NAN.to_bits()));
        assert!(!Kind::Float.takes(f32::INFINITY.to_bits()));
        assert!(Kind::Op.takes(OP_LERP) && Kind::Op.takes(24));
        assert!(!Kind::Op.takes(OP_PREMODULATE) && !Kind::Op.takes(0) && !Kind::Op.takes(27));
        assert!(Kind::Arg.takes(ARG_TEXTURE | ARG_COMPLEMENT | ARG_ALPHA_REPLICATE));
        assert!(!Kind::Arg.takes(6) && !Kind::Arg.takes(ARG_TEXTURE | 0x40));
        assert!(Kind::Result.takes(ARG_TEMP) && !Kind::Result.takes(ARG_TEXTURE));
        assert!(Kind::TexCoord.takes(7 | 3 << 16) && Kind::TexCoord.takes(4 << 16));
        assert!(!Kind::TexCoord.takes(8) && !Kind::TexCoord.takes(5 << 16));
        assert!(Kind::TexTransform.takes(4 | TRANSFORM_PROJECTED));
        assert!(!Kind::TexTransform.takes(5));
        assert!(Kind::Scale.takes(4) && !Kind::Scale.takes(3));
    }
}
