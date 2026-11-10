//! The names the 3D trace writes for the numbers programs pass: Direct3D's
//! render and texture stage states and their values, formats and caps,
//! and OpenGL's enums. A number without a name here is written as it is.

/// D3DRENDERSTATETYPE, Direct3D 2 to 8 (D3DRENDERSTATE_ and D3DRS_).
pub fn render_state(s: u32) -> Option<&'static str> {
    Some(match s {
        1 => "TEXTUREHANDLE",
        2 => "ANTIALIAS",
        3 => "TEXTUREADDRESS",
        4 => "TEXTUREPERSPECTIVE",
        5 => "WRAPU",
        6 => "WRAPV",
        7 => "ZENABLE",
        8 => "FILLMODE",
        9 => "SHADEMODE",
        10 => "LINEPATTERN",
        11 => "MONOENABLE",
        12 => "ROP2",
        13 => "PLANEMASK",
        14 => "ZWRITEENABLE",
        15 => "ALPHATESTENABLE",
        16 => "LASTPIXEL",
        17 => "TEXTUREMAG",
        18 => "TEXTUREMIN",
        19 => "SRCBLEND",
        20 => "DESTBLEND",
        21 => "TEXTUREMAPBLEND",
        22 => "CULLMODE",
        23 => "ZFUNC",
        24 => "ALPHAREF",
        25 => "ALPHAFUNC",
        26 => "DITHERENABLE",
        27 => "ALPHABLENDENABLE",
        28 => "FOGENABLE",
        29 => "SPECULARENABLE",
        30 => "ZVISIBLE",
        31 => "SUBPIXEL",
        32 => "SUBPIXELX",
        33 => "STIPPLEDALPHA",
        34 => "FOGCOLOR",
        35 => "FOGTABLEMODE",
        36 => "FOGSTART",
        37 => "FOGEND",
        38 => "FOGDENSITY",
        39 => "STIPPLEENABLE",
        40 => "EDGEANTIALIAS",
        41 => "COLORKEYENABLE",
        42 => "OLDALPHABLENDENABLE",
        43 => "BORDERCOLOR",
        44 => "TEXTUREADDRESSU",
        45 => "TEXTUREADDRESSV",
        46 => "MIPMAPLODBIAS",
        47 => "ZBIAS",
        48 => "RANGEFOGENABLE",
        49 => "ANISOTROPY",
        50 => "FLUSHBATCH",
        51 => "TRANSLUCENTSORTINDEPENDENT",
        52 => "STENCILENABLE",
        53 => "STENCILFAIL",
        54 => "STENCILZFAIL",
        55 => "STENCILPASS",
        56 => "STENCILFUNC",
        57 => "STENCILREF",
        58 => "STENCILMASK",
        59 => "STENCILWRITEMASK",
        60 => "TEXTUREFACTOR",
        128 => "WRAP0",
        129 => "WRAP1",
        130 => "WRAP2",
        131 => "WRAP3",
        132 => "WRAP4",
        133 => "WRAP5",
        134 => "WRAP6",
        135 => "WRAP7",
        136 => "CLIPPING",
        137 => "LIGHTING",
        138 => "EXTENTS",
        139 => "AMBIENT",
        140 => "FOGVERTEXMODE",
        141 => "COLORVERTEX",
        142 => "LOCALVIEWER",
        143 => "NORMALIZENORMALS",
        144 => "COLORKEYBLENDENABLE",
        145 => "DIFFUSEMATERIALSOURCE",
        146 => "SPECULARMATERIALSOURCE",
        147 => "AMBIENTMATERIALSOURCE",
        148 => "EMISSIVEMATERIALSOURCE",
        151 => "VERTEXBLEND",
        152 => "CLIPPLANEENABLE",
        153 => "SOFTWAREVERTEXPROCESSING",
        154 => "POINTSIZE",
        155 => "POINTSIZE_MIN",
        156 => "POINTSPRITEENABLE",
        157 => "POINTSCALEENABLE",
        158 => "POINTSCALE_A",
        159 => "POINTSCALE_B",
        160 => "POINTSCALE_C",
        161 => "MULTISAMPLEANTIALIAS",
        162 => "MULTISAMPLEMASK",
        163 => "PATCHEDGESTYLE",
        164 => "PATCHSEGMENTS",
        165 => "DEBUGMONITORTOKEN",
        166 => "POINTSIZE_MAX",
        167 => "INDEXEDVERTEXBLENDENABLE",
        168 => "COLORWRITEENABLE",
        170 => "TWEENFACTOR",
        171 => "BLENDOP",
        172 => "POSITIONORDER",
        173 => "NORMALORDER",
        _ => return None,
    })
}

/// A render state's value: its name where the state takes one of a
/// list, a float for the states that hold one, else the number.
pub fn render_value(s: u32, x: u32) -> String {
    let named = match s {
        7 => pick(x, &["FALSE", "TRUE", "USEW"]),
        8 => pick(x, &["", "POINT", "WIREFRAME", "SOLID"]),
        9 => pick(x, &["", "FLAT", "GOURAUD", "PHONG"]),
        17 => pick(x, &["", "NEAREST", "LINEAR"]),
        18 => pick(
            x,
            &[
                "",
                "NEAREST",
                "LINEAR",
                "MIPNEAREST",
                "MIPLINEAR",
                "LINEARMIPNEAREST",
                "LINEARMIPLINEAR",
            ],
        ),
        19 | 20 => blend(x),
        21 => pick(
            x,
            &[
                "",
                "DECAL",
                "MODULATE",
                "DECALALPHA",
                "MODULATEALPHA",
                "DECALMASK",
                "MODULATEMASK",
                "COPY",
                "ADD",
            ],
        ),
        22 => pick(x, &["", "NONE", "CW", "CCW"]),
        23 | 25 | 56 => cmp(x),
        35 | 140 => pick(x, &["NONE", "EXP", "EXP2", "LINEAR"]),
        53..=55 => pick(
            x,
            &[
                "", "KEEP", "ZERO", "REPLACE", "INCRSAT", "DECRSAT", "INVERT", "INCR", "DECR",
            ],
        ),
        3 | 44 | 45 => address(x),
        145..=148 => pick(x, &["MATERIAL", "COLOR1", "COLOR2"]),
        151 => match x {
            255 => Some("TWEENING"),
            256 => Some("0WEIGHTS"),
            _ => pick(x, &["DISABLE", "1WEIGHTS", "2WEIGHTS", "3WEIGHTS"]),
        },
        171 => pick(x, &["", "ADD", "SUBTRACT", "REVSUBTRACT", "MIN", "MAX"]),
        _ => None,
    };
    if let Some(n) = named {
        return n.to_string();
    }
    match s {
        36..=38 | 154 | 155 | 158..=160 | 166 | 170 => float(x),
        34 | 60 | 139 | 43 | 162 => format!("0x{:X}", x),
        _ => number(x),
    }
}

/// D3DLIGHTSTATETYPE, Direct3D 2 and 3.
pub fn light_state(s: u32) -> Option<&'static str> {
    pick(
        s,
        &[
            "",
            "MATERIAL",
            "AMBIENT",
            "COLORMODEL",
            "FOGMODE",
            "FOGSTART",
            "FOGEND",
            "FOGDENSITY",
            "COLORVERTEX",
        ],
    )
}

pub fn light_value(s: u32, x: u32) -> String {
    match s {
        3 => pick(x, &["", "MONO", "RGB"]).map_or_else(|| number(x), str::to_string),
        4 => pick(x, &["NONE", "EXP", "EXP2", "LINEAR"]).map_or_else(|| number(x), str::to_string),
        5..=7 => float(x),
        2 => format!("0x{:X}", x),
        _ => number(x),
    }
}

/// D3DTEXTURESTAGESTATETYPE, Direct3D 6 to 8.
pub fn stage_state(s: u32) -> Option<&'static str> {
    pick(
        s,
        &[
            "",
            "COLOROP",
            "COLORARG1",
            "COLORARG2",
            "ALPHAOP",
            "ALPHAARG1",
            "ALPHAARG2",
            "BUMPENVMAT00",
            "BUMPENVMAT01",
            "BUMPENVMAT10",
            "BUMPENVMAT11",
            "TEXCOORDINDEX",
            "ADDRESS",
            "ADDRESSU",
            "ADDRESSV",
            "BORDERCOLOR",
            "MAGFILTER",
            "MINFILTER",
            "MIPFILTER",
            "MIPMAPLODBIAS",
            "MAXMIPLEVEL",
            "MAXANISOTROPY",
            "BUMPENVLSCALE",
            "BUMPENVLOFFSET",
            "TEXTURETRANSFORMFLAGS",
            "ADDRESSW",
            "COLORARG0",
            "ALPHAARG0",
            "RESULTARG",
        ],
    )
}

/// A texture stage state's value. The filters' numbers changed in
/// Direct3D 8 (`d8`).
pub fn stage_value(s: u32, x: u32, d8: bool) -> String {
    let named = match s {
        1 | 4 => stage_op(x),
        2 | 3 | 5 | 6 | 26..=28 => return stage_arg(x),
        12..=14 | 25 => address(x),
        16..=18 if d8 => pick(
            x,
            &[
                "NONE",
                "POINT",
                "LINEAR",
                "ANISOTROPIC",
                "FLATCUBIC",
                "GAUSSIANCUBIC",
            ],
        ),
        16 => pick(
            x,
            &[
                "",
                "POINT",
                "LINEAR",
                "FLATCUBIC",
                "GAUSSIANCUBIC",
                "ANISOTROPIC",
            ],
        ),
        17 => pick(x, &["", "POINT", "LINEAR", "ANISOTROPIC"]),
        18 => pick(x, &["", "NONE", "POINT", "LINEAR"]),
        11 => return tex_coord_index(x),
        24 => return texture_transform_flags(x),
        _ => None,
    };
    if let Some(n) = named {
        return n.to_string();
    }
    match s {
        7..=10 | 19 | 22 | 23 => float(x),
        15 => format!("0x{:X}", x),
        _ => number(x),
    }
}

/// D3DTEXTUREOP.
pub fn stage_op(x: u32) -> Option<&'static str> {
    pick(
        x,
        &[
            "",
            "DISABLE",
            "SELECTARG1",
            "SELECTARG2",
            "MODULATE",
            "MODULATE2X",
            "MODULATE4X",
            "ADD",
            "ADDSIGNED",
            "ADDSIGNED2X",
            "SUBTRACT",
            "ADDSMOOTH",
            "BLENDDIFFUSEALPHA",
            "BLENDTEXTUREALPHA",
            "BLENDFACTORALPHA",
            "BLENDTEXTUREALPHAPM",
            "BLENDCURRENTALPHA",
            "PREMODULATE",
            "MODULATEALPHA_ADDCOLOR",
            "MODULATECOLOR_ADDALPHA",
            "MODULATEINVALPHA_ADDCOLOR",
            "MODULATEINVCOLOR_ADDALPHA",
            "BUMPENVMAP",
            "BUMPENVMAPLUMINANCE",
            "DOTPRODUCT3",
            "MULTIPLYADD",
            "LERP",
        ],
    )
}

/// A D3DTA_ argument: the source, with 1- for D3DTA_COMPLEMENT and .a
/// for D3DTA_ALPHAREPLICATE.
pub fn stage_arg(x: u32) -> String {
    let source = pick(
        x & 0xF,
        &[
            "DIFFUSE", "CURRENT", "TEXTURE", "TFACTOR", "SPECULAR", "TEMP",
        ],
    )
    .map_or_else(|| number(x & 0xF), str::to_string);
    let mut s = String::new();
    if x & 0x10 != 0 {
        s.push_str("1-");
    }
    s.push_str(&source);
    if x & 0x20 != 0 {
        s.push_str(".a");
    }
    if x & !0x3F != 0 {
        s.push_str(&format!("|0x{:X}", x & !0x3F));
    }
    s
}

fn tex_coord_index(x: u32) -> String {
    let how = match x >> 16 {
        0 => return number(x),
        1 => "CAMERASPACENORMAL",
        2 => "CAMERASPACEPOSITION",
        3 => "CAMERASPACEREFLECTIONVECTOR",
        _ => return format!("0x{:X}", x),
    };
    format!("{}|{}", x & 0xFFFF, how)
}

fn texture_transform_flags(x: u32) -> String {
    let count = pick(
        x & 0xFF,
        &["DISABLE", "COUNT1", "COUNT2", "COUNT3", "COUNT4"],
    )
    .map_or_else(|| number(x & 0xFF), str::to_string);
    if x & 0x100 != 0 {
        format!("{}|PROJECTED", count)
    } else {
        count
    }
}

fn address(x: u32) -> Option<&'static str> {
    pick(x, &["", "WRAP", "MIRROR", "CLAMP", "BORDER", "MIRRORONCE"])
}

fn blend(x: u32) -> Option<&'static str> {
    pick(
        x,
        &[
            "",
            "ZERO",
            "ONE",
            "SRCCOLOR",
            "INVSRCCOLOR",
            "SRCALPHA",
            "INVSRCALPHA",
            "DESTALPHA",
            "INVDESTALPHA",
            "DESTCOLOR",
            "INVDESTCOLOR",
            "SRCALPHASAT",
            "BOTHSRCALPHA",
            "BOTHINVSRCALPHA",
        ],
    )
}

fn cmp(x: u32) -> Option<&'static str> {
    pick(
        x,
        &[
            "",
            "NEVER",
            "LESS",
            "EQUAL",
            "LESSEQUAL",
            "GREATER",
            "NOTEQUAL",
            "GREATEREQUAL",
            "ALWAYS",
        ],
    )
}

/// D3DTRANSFORMSTATETYPE: Direct3D 7's numbers, and Direct3D 8's world
/// matrices from 256 on.
pub fn transform(x: u32) -> String {
    match x {
        1 => "WORLD".to_string(),
        2 => "VIEW".to_string(),
        3 => "PROJECTION".to_string(),
        4..=6 => format!("WORLD{}", x - 3),
        16..=23 => format!("TEXTURE{}", x - 16),
        256..=511 => format!("WORLDMATRIX({})", x - 256),
        _ => number(x),
    }
}

/// D3DPRIMITIVETYPE.
pub fn primitive(x: u32) -> String {
    pick(
        x,
        &[
            "",
            "POINTLIST",
            "LINELIST",
            "LINESTRIP",
            "TRIANGLELIST",
            "TRIANGLESTRIP",
            "TRIANGLEFAN",
        ],
    )
    .map_or_else(|| number(x), str::to_string)
}

/// A flexible vertex format: the position, the other parts, and the
/// texture coordinate sets, with the size of each set that isn't two.
pub fn fvf(x: u32) -> String {
    let mut parts: Vec<String> = Vec::new();
    let position = match x & 0xE {
        0 => None,
        0x2 => Some("XYZ"),
        0x4 => Some("XYZRHW"),
        0x6 => Some("XYZB1"),
        0x8 => Some("XYZB2"),
        0xA => Some("XYZB3"),
        0xC => Some("XYZB4"),
        _ => Some("XYZB5"),
    };
    parts.extend(position.map(str::to_string));
    for (bit, name) in [
        (0x1000, "LASTBETA_UBYTE4"),
        (0x10, "NORMAL"),
        (0x20, "PSIZE"),
        (0x40, "DIFFUSE"),
        (0x80, "SPECULAR"),
    ] {
        if x & bit != 0 {
            parts.push(name.to_string());
        }
    }
    let sets = (x >> 8) & 0xF;
    if sets > 0 {
        let mut tex = format!("TEX{}", sets);
        for set in 0..sets {
            let size = match (x >> (16 + 2 * set)) & 3 {
                0 => 2,
                1 => 3,
                2 => 4,
                _ => 1,
            };
            if size != 2 {
                tex.push_str(&format!(":{}={}", set, size));
            }
        }
        parts.push(tex);
    }
    // D3DFVF_RESERVED0 and the bits no flag has.
    let rest = x & 0xE001;
    if rest != 0 {
        parts.push(format!("0x{:X}", rest));
    }
    if parts.is_empty() {
        return "0".to_string();
    }
    parts.join("|")
}

/// D3DFORMAT, Direct3D 8.
pub fn d3d_format(x: u32) -> String {
    let name = match x {
        0 => "UNKNOWN",
        20 => "R8G8B8",
        21 => "A8R8G8B8",
        22 => "X8R8G8B8",
        23 => "R5G6B5",
        24 => "X1R5G5B5",
        25 => "A1R5G5B5",
        26 => "A4R4G4B4",
        27 => "R3G3B2",
        28 => "A8",
        29 => "A8R3G3B2",
        30 => "X4R4G4B4",
        40 => "A8P8",
        41 => "P8",
        50 => "L8",
        51 => "A8L8",
        52 => "A4L4",
        60 => "V8U8",
        61 => "L6V5U5",
        62 => "X8L8V8U8",
        63 => "Q8W8V8U8",
        64 => "V16U16",
        65 => "W11V11U10",
        70 => "D16_LOCKABLE",
        71 => "D32",
        73 => "D15S1",
        75 => "D24S8",
        77 => "D24X8",
        79 => "D24X4S4",
        80 => "D16",
        100 => "VERTEXDATA",
        101 => "INDEX16",
        102 => "INDEX32",
        _ => return fourcc(x),
    };
    name.to_string()
}

/// D3DPOOL.
pub fn pool(x: u32) -> String {
    pick(x, &["DEFAULT", "MANAGED", "SYSTEMMEM", "SCRATCH"])
        .map_or_else(|| number(x), str::to_string)
}

/// A FourCC as its four characters, if they are printable.
pub fn fourcc(x: u32) -> String {
    let b = x.to_le_bytes();
    if x > 0xFF && b.iter().all(|c| c.is_ascii_graphic()) {
        b.iter().map(|&c| c as char).collect()
    } else {
        number(x)
    }
}

/// A DDPIXELFORMAT's flags, FourCC, bits and masks as a name in
/// D3DFORMAT's style: R5G6B5, A1R5G5B5, X8R8G8B8, P8, Z16, DXT1.
pub fn pixel_format(flags: u32, four: u32, bits: u32, masks: [u32; 4]) -> String {
    const ALPHAPIXELS: u32 = 0x1;
    const ALPHA: u32 = 0x2;
    const FOURCC: u32 = 0x4;
    const PALETTEINDEXED4: u32 = 0x8;
    const PALETTEINDEXED8: u32 = 0x20;
    const RGB: u32 = 0x40;
    const ZBUFFER: u32 = 0x400;
    const STENCILBUFFER: u32 = 0x4000;
    const LUMINANCE: u32 = 0x2_0000;
    const BUMPDUDV: u32 = 0x8_0000;
    if flags & FOURCC != 0 {
        return fourcc(four);
    }
    if flags & ZBUFFER != 0 {
        // dwZBufferBitDepth, then dwStencilBitDepth in the first mask.
        let stencil = if flags & STENCILBUFFER != 0 {
            format!("S{}", masks[0])
        } else {
            String::new()
        };
        return format!("Z{}{}", bits, stencil);
    }
    if flags & PALETTEINDEXED8 != 0 {
        return "P8".to_string();
    }
    if flags & PALETTEINDEXED4 != 0 {
        return "P4".to_string();
    }
    let channels: &[(char, u32)] = if flags & RGB != 0 {
        &[('R', masks[0]), ('G', masks[1]), ('B', masks[2])]
    } else if flags & LUMINANCE != 0 {
        &[('L', masks[0])]
    } else if flags & BUMPDUDV != 0 {
        &[('U', masks[0]), ('V', masks[1]), ('L', masks[2])]
    } else if flags & ALPHA != 0 {
        &[]
    } else {
        return format!("fl=0x{:X},bits={}", flags, bits);
    };
    let mut list: Vec<(char, u32)> = channels.to_vec();
    if flags & (ALPHAPIXELS | ALPHA) != 0 {
        list.push(('A', masks[3]));
    }
    let mut used: Vec<(u32, char, u32)> = list
        .iter()
        .filter(|(_, m)| *m != 0)
        .map(|&(c, m)| (m.trailing_zeros(), c, m.count_ones()))
        .collect();
    if flags & ALPHA != 0 && used.is_empty() {
        return format!("A{}", bits);
    }
    let covered: u32 = used.iter().map(|u| u.2).sum();
    if covered < bits {
        let all = list.iter().fold(0u32, |a, (_, m)| a | m);
        let top = 32 - all.leading_zeros();
        used.push((top, 'X', bits - covered));
    }
    used.sort_by_key(|u| std::cmp::Reverse(u.0));
    used.iter().map(|(_, c, n)| format!("{}{}", c, n)).collect()
}

/// DDSCAPS's first word as its flags' names, the bits without one in
/// hexadecimal.
pub fn ddscaps(caps: u32) -> String {
    const NAMES: [(u32, &str); 22] = [
        (0x2, "ALPHA"),
        (0x4, "BACKBUFFER"),
        (0x8, "COMPLEX"),
        (0x10, "FLIP"),
        (0x20, "FRONTBUFFER"),
        (0x40, "OFFSCREENPLAIN"),
        (0x80, "OVERLAY"),
        (0x100, "PALETTE"),
        (0x200, "PRIMARYSURFACE"),
        (0x800, "SYSTEMMEMORY"),
        (0x1000, "TEXTURE"),
        (0x2000, "3DDEVICE"),
        (0x4000, "VIDEOMEMORY"),
        (0x8000, "VISIBLE"),
        (0x1_0000, "WRITEONLY"),
        (0x2_0000, "ZBUFFER"),
        (0x4_0000, "OWNDC"),
        (0x20_0000, "MODEX"),
        (0x40_0000, "MIPMAP"),
        (0x400_0000, "ALLOCONLOAD"),
        (0x1000_0000, "LOCALVIDMEM"),
        (0x2000_0000, "NONLOCALVIDMEM"),
    ];
    flags(caps, &NAMES)
}

/// The names of the bits of `x` that `names` has, joined by |, and the
/// others in hexadecimal.
pub fn flags(x: u32, names: &[(u32, &str)]) -> String {
    let mut parts = Vec::new();
    let mut rest = x;
    for &(bit, name) in names {
        if x & bit != 0 {
            parts.push(name.to_string());
            rest &= !bit;
        }
    }
    if rest != 0 {
        parts.push(format!("0x{:X}", rest));
    }
    if parts.is_empty() {
        "0".to_string()
    } else {
        parts.join("|")
    }
}

/// D3DCLEAR_ flags.
pub fn clear_flags(x: u32) -> String {
    flags(x, &[(1, "TARGET"), (2, "ZBUFFER"), (4, "STENCIL")])
}

/// What an OpenGL argument is, for its name.
#[derive(Clone, Copy)]
pub enum Gl {
    /// An enum: named from 0x100 on, smaller values are numbers.
    Enum,
    /// A primitive type of glBegin and the draw calls.
    Mode,
    /// A blend factor, where 0 and 1 are GL_ZERO and GL_ONE.
    Factor,
    /// An integer.
    Int,
    /// A float.
    Float,
    /// A float that may hold an enum (glTexParameterf's).
    FloatEnum,
}

pub fn gl(x: u32, kind: Gl) -> String {
    match kind {
        Gl::Enum => gl_enum(x).map_or_else(|| number(x), str::to_string),
        Gl::Mode => pick(
            x,
            &[
                "POINTS",
                "LINES",
                "LINE_LOOP",
                "LINE_STRIP",
                "TRIANGLES",
                "TRIANGLE_STRIP",
                "TRIANGLE_FAN",
                "QUADS",
                "QUAD_STRIP",
                "POLYGON",
            ],
        )
        .map_or_else(|| number(x), str::to_string),
        Gl::Factor => match x {
            0 => "ZERO".to_string(),
            1 => "ONE".to_string(),
            _ => gl(x, Gl::Enum),
        },
        Gl::Int => number(x),
        Gl::Float => float(x),
        Gl::FloatEnum => {
            let f = f32::from_bits(x);
            if f.fract() == 0.0 && (256.0..65536.0).contains(&f) {
                gl(f as u32, Gl::Enum)
            } else {
                float(x)
            }
        }
    }
}

/// OpenGL 1.1's enums and those of the extensions games of its time
/// use, without GL_. Where two share a value, the one asked for more
/// often is here.
fn gl_enum(x: u32) -> Option<&'static str> {
    if x < 0x100 {
        return None;
    }
    Some(match x {
        0x0104 => "ADD",
        0x0200 => "NEVER",
        0x0201 => "LESS",
        0x0202 => "EQUAL",
        0x0203 => "LEQUAL",
        0x0204 => "GREATER",
        0x0205 => "NOTEQUAL",
        0x0206 => "GEQUAL",
        0x0207 => "ALWAYS",
        0x0300 => "SRC_COLOR",
        0x0301 => "ONE_MINUS_SRC_COLOR",
        0x0302 => "SRC_ALPHA",
        0x0303 => "ONE_MINUS_SRC_ALPHA",
        0x0304 => "DST_ALPHA",
        0x0305 => "ONE_MINUS_DST_ALPHA",
        0x0306 => "DST_COLOR",
        0x0307 => "ONE_MINUS_DST_COLOR",
        0x0308 => "SRC_ALPHA_SATURATE",
        0x0400 => "FRONT_LEFT",
        0x0402 => "BACK_LEFT",
        0x0404 => "FRONT",
        0x0405 => "BACK",
        0x0408 => "FRONT_AND_BACK",
        0x0800 => "EXP",
        0x0801 => "EXP2",
        0x0900 => "CW",
        0x0901 => "CCW",
        0x0B10 => "POINT_SMOOTH",
        0x0B20 => "LINE_SMOOTH",
        0x0B24 => "LINE_STIPPLE",
        0x0B41 => "POLYGON_SMOOTH",
        0x0B42 => "POLYGON_STIPPLE",
        0x0B44 => "CULL_FACE",
        0x0B45 => "CULL_FACE_MODE",
        0x0B50 => "LIGHTING",
        0x0B51 => "LIGHT_MODEL_LOCAL_VIEWER",
        0x0B52 => "LIGHT_MODEL_TWO_SIDE",
        0x0B53 => "LIGHT_MODEL_AMBIENT",
        0x0B57 => "COLOR_MATERIAL",
        0x0B60 => "FOG",
        0x0B62 => "FOG_DENSITY",
        0x0B63 => "FOG_START",
        0x0B64 => "FOG_END",
        0x0B65 => "FOG_MODE",
        0x0B66 => "FOG_COLOR",
        0x0B71 => "DEPTH_TEST",
        0x0B72 => "DEPTH_WRITEMASK",
        0x0B74 => "DEPTH_FUNC",
        0x0B90 => "STENCIL_TEST",
        0x0BA1 => "NORMALIZE",
        0x0BA2 => "VIEWPORT",
        0x0BA6 => "MODELVIEW_MATRIX",
        0x0BA7 => "PROJECTION_MATRIX",
        0x0BA8 => "TEXTURE_MATRIX",
        0x0BC0 => "ALPHA_TEST",
        0x0BD0 => "DITHER",
        0x0BE2 => "BLEND",
        0x0BF2 => "COLOR_LOGIC_OP",
        0x0C11 => "SCISSOR_TEST",
        0x0C32 => "DOUBLEBUFFER",
        0x0C50 => "PERSPECTIVE_CORRECTION_HINT",
        0x0C51 => "POINT_SMOOTH_HINT",
        0x0C52 => "LINE_SMOOTH_HINT",
        0x0C53 => "POLYGON_SMOOTH_HINT",
        0x0C54 => "FOG_HINT",
        0x0C60 => "TEXTURE_GEN_S",
        0x0C61 => "TEXTURE_GEN_T",
        0x0C62 => "TEXTURE_GEN_R",
        0x0C63 => "TEXTURE_GEN_Q",
        0x0CF2 => "UNPACK_ROW_LENGTH",
        0x0CF3 => "UNPACK_SKIP_ROWS",
        0x0CF4 => "UNPACK_SKIP_PIXELS",
        0x0CF5 => "UNPACK_ALIGNMENT",
        0x0D05 => "PACK_ALIGNMENT",
        0x0D31 => "MAX_LIGHTS",
        0x0D32 => "MAX_CLIP_PLANES",
        0x0D33 => "MAX_TEXTURE_SIZE",
        0x0D3A => "MAX_VIEWPORT_DIMS",
        0x0D52 => "RED_BITS",
        0x0D53 => "GREEN_BITS",
        0x0D54 => "BLUE_BITS",
        0x0D55 => "ALPHA_BITS",
        0x0D56 => "DEPTH_BITS",
        0x0D57 => "STENCIL_BITS",
        0x0DE0 => "TEXTURE_1D",
        0x0DE1 => "TEXTURE_2D",
        0x1000 => "TEXTURE_WIDTH",
        0x1001 => "TEXTURE_HEIGHT",
        0x1003 => "TEXTURE_INTERNAL_FORMAT",
        0x1004 => "TEXTURE_BORDER_COLOR",
        0x1100 => "DONT_CARE",
        0x1101 => "FASTEST",
        0x1102 => "NICEST",
        0x1200 => "AMBIENT",
        0x1201 => "DIFFUSE",
        0x1202 => "SPECULAR",
        0x1203 => "POSITION",
        0x1204 => "SPOT_DIRECTION",
        0x1205 => "SPOT_EXPONENT",
        0x1206 => "SPOT_CUTOFF",
        0x1207 => "CONSTANT_ATTENUATION",
        0x1208 => "LINEAR_ATTENUATION",
        0x1209 => "QUADRATIC_ATTENUATION",
        0x1300 => "COMPILE",
        0x1301 => "COMPILE_AND_EXECUTE",
        0x1400 => "BYTE",
        0x1401 => "UNSIGNED_BYTE",
        0x1402 => "SHORT",
        0x1403 => "UNSIGNED_SHORT",
        0x1404 => "INT",
        0x1405 => "UNSIGNED_INT",
        0x1406 => "FLOAT",
        0x140A => "DOUBLE",
        0x1600 => "EMISSION",
        0x1601 => "SHININESS",
        0x1602 => "AMBIENT_AND_DIFFUSE",
        0x1700 => "MODELVIEW",
        0x1701 => "PROJECTION",
        0x1702 => "TEXTURE",
        0x1800 => "COLOR",
        0x1801 => "DEPTH",
        0x1802 => "STENCIL",
        0x1900 => "COLOR_INDEX",
        0x1902 => "DEPTH_COMPONENT",
        0x1903 => "RED",
        0x1904 => "GREEN",
        0x1905 => "BLUE",
        0x1906 => "ALPHA",
        0x1907 => "RGB",
        0x1908 => "RGBA",
        0x1909 => "LUMINANCE",
        0x190A => "LUMINANCE_ALPHA",
        0x1B00 => "POINT",
        0x1B01 => "LINE",
        0x1B02 => "FILL",
        0x1D00 => "FLAT",
        0x1D01 => "SMOOTH",
        0x1E00 => "KEEP",
        0x1E01 => "REPLACE",
        0x1E02 => "INCR",
        0x1E03 => "DECR",
        0x1F00 => "VENDOR",
        0x1F01 => "RENDERER",
        0x1F02 => "VERSION",
        0x1F03 => "EXTENSIONS",
        0x2000 => "S",
        0x2001 => "T",
        0x2002 => "R",
        0x2003 => "Q",
        0x2100 => "MODULATE",
        0x2101 => "DECAL",
        0x2200 => "TEXTURE_ENV_MODE",
        0x2201 => "TEXTURE_ENV_COLOR",
        0x2300 => "TEXTURE_ENV",
        0x2400 => "EYE_LINEAR",
        0x2401 => "OBJECT_LINEAR",
        0x2402 => "SPHERE_MAP",
        0x2500 => "TEXTURE_GEN_MODE",
        0x2501 => "OBJECT_PLANE",
        0x2502 => "EYE_PLANE",
        0x2600 => "NEAREST",
        0x2601 => "LINEAR",
        0x2700 => "NEAREST_MIPMAP_NEAREST",
        0x2701 => "LINEAR_MIPMAP_NEAREST",
        0x2702 => "NEAREST_MIPMAP_LINEAR",
        0x2703 => "LINEAR_MIPMAP_LINEAR",
        0x2800 => "TEXTURE_MAG_FILTER",
        0x2801 => "TEXTURE_MIN_FILTER",
        0x2802 => "TEXTURE_WRAP_S",
        0x2803 => "TEXTURE_WRAP_T",
        0x2900 => "CLAMP",
        0x2901 => "REPEAT",
        0x2A01 => "POLYGON_OFFSET_POINT",
        0x2A02 => "POLYGON_OFFSET_LINE",
        0x2A10 => "R3_G3_B2",
        0x3000 => "CLIP_PLANE0",
        0x3001 => "CLIP_PLANE1",
        0x3002 => "CLIP_PLANE2",
        0x3003 => "CLIP_PLANE3",
        0x3004 => "CLIP_PLANE4",
        0x3005 => "CLIP_PLANE5",
        0x4000 => "LIGHT0",
        0x4001 => "LIGHT1",
        0x4002 => "LIGHT2",
        0x4003 => "LIGHT3",
        0x4004 => "LIGHT4",
        0x4005 => "LIGHT5",
        0x4006 => "LIGHT6",
        0x4007 => "LIGHT7",
        0x8001 => "CONSTANT_COLOR",
        0x8002 => "ONE_MINUS_CONSTANT_COLOR",
        0x8003 => "CONSTANT_ALPHA",
        0x8004 => "ONE_MINUS_CONSTANT_ALPHA",
        0x8006 => "FUNC_ADD",
        0x8007 => "MIN",
        0x8008 => "MAX",
        0x800A => "FUNC_SUBTRACT",
        0x800B => "FUNC_REVERSE_SUBTRACT",
        0x8032 => "UNSIGNED_BYTE_3_3_2",
        0x8033 => "UNSIGNED_SHORT_4_4_4_4",
        0x8034 => "UNSIGNED_SHORT_5_5_5_1",
        0x8037 => "POLYGON_OFFSET_FILL",
        0x803B => "ALPHA4",
        0x803C => "ALPHA8",
        0x803F => "LUMINANCE4",
        0x8040 => "LUMINANCE8",
        0x8043 => "LUMINANCE4_ALPHA4",
        0x8045 => "LUMINANCE8_ALPHA8",
        0x8049 => "INTENSITY",
        0x804A => "INTENSITY4",
        0x804B => "INTENSITY8",
        0x804F => "RGB4",
        0x8050 => "RGB5",
        0x8051 => "RGB8",
        0x8055 => "RGBA2",
        0x8056 => "RGBA4",
        0x8057 => "RGB5_A1",
        0x8058 => "RGBA8",
        0x8074 => "VERTEX_ARRAY",
        0x8075 => "NORMAL_ARRAY",
        0x8076 => "COLOR_ARRAY",
        0x8077 => "INDEX_ARRAY",
        0x8078 => "TEXTURE_COORD_ARRAY",
        0x8079 => "EDGE_FLAG_ARRAY",
        0x80E0 => "BGR_EXT",
        0x80E1 => "BGRA_EXT",
        0x80E5 => "COLOR_INDEX8_EXT",
        0x812F => "CLAMP_TO_EDGE",
        0x8191 => "GENERATE_MIPMAP_SGIS",
        0x81F8 => "LIGHT_MODEL_COLOR_CONTROL",
        0x81F9 => "SINGLE_COLOR",
        0x81FA => "SEPARATE_SPECULAR_COLOR",
        0x8363 => "UNSIGNED_SHORT_5_6_5",
        0x8365 => "UNSIGNED_SHORT_4_4_4_4_REV",
        0x8366 => "UNSIGNED_SHORT_1_5_5_5_REV",
        0x83F0 => "COMPRESSED_RGB_S3TC_DXT1_EXT",
        0x83F1 => "COMPRESSED_RGBA_S3TC_DXT1_EXT",
        0x83F2 => "COMPRESSED_RGBA_S3TC_DXT3_EXT",
        0x83F3 => "COMPRESSED_RGBA_S3TC_DXT5_EXT",
        0x8450 => "FOG_COORDINATE_SOURCE_EXT",
        0x845E => "SECONDARY_COLOR_ARRAY_EXT",
        0x84C0..=0x84DF => return TEXTURE_UNITS.get((x - 0x84C0) as usize).copied(),
        0x84E2 => "MAX_TEXTURE_UNITS_ARB",
        0x84E7 => "SUBTRACT_ARB",
        0x84FE => "TEXTURE_MAX_ANISOTROPY_EXT",
        0x84FF => "MAX_TEXTURE_MAX_ANISOTROPY_EXT",
        0x8513 => "TEXTURE_CUBE_MAP_ARB",
        0x8570 => "COMBINE_ARB",
        0x8571 => "COMBINE_RGB_ARB",
        0x8572 => "COMBINE_ALPHA_ARB",
        0x8573 => "RGB_SCALE_ARB",
        0x8574 => "ADD_SIGNED_ARB",
        0x8575 => "INTERPOLATE_ARB",
        0x8576 => "CONSTANT_ARB",
        0x8577 => "PRIMARY_COLOR_ARB",
        0x8578 => "PREVIOUS_ARB",
        0x8580 => "SOURCE0_RGB_ARB",
        0x8581 => "SOURCE1_RGB_ARB",
        0x8582 => "SOURCE2_RGB_ARB",
        0x8588 => "SOURCE0_ALPHA_ARB",
        0x8589 => "SOURCE1_ALPHA_ARB",
        0x858A => "SOURCE2_ALPHA_ARB",
        0x8590 => "OPERAND0_RGB_ARB",
        0x8591 => "OPERAND1_RGB_ARB",
        0x8592 => "OPERAND2_RGB_ARB",
        0x8598 => "OPERAND0_ALPHA_ARB",
        0x8599 => "OPERAND1_ALPHA_ARB",
        0x859A => "OPERAND2_ALPHA_ARB",
        0x86B0 => "COMPRESSED_RGB_FXT1_3DFX",
        0x86B1 => "COMPRESSED_RGBA_FXT1_3DFX",
        _ => return None,
    })
}

const TEXTURE_UNITS: [&str; 32] = [
    "TEXTURE0_ARB",
    "TEXTURE1_ARB",
    "TEXTURE2_ARB",
    "TEXTURE3_ARB",
    "TEXTURE4_ARB",
    "TEXTURE5_ARB",
    "TEXTURE6_ARB",
    "TEXTURE7_ARB",
    "TEXTURE8_ARB",
    "TEXTURE9_ARB",
    "TEXTURE10_ARB",
    "TEXTURE11_ARB",
    "TEXTURE12_ARB",
    "TEXTURE13_ARB",
    "TEXTURE14_ARB",
    "TEXTURE15_ARB",
    "TEXTURE16_ARB",
    "TEXTURE17_ARB",
    "TEXTURE18_ARB",
    "TEXTURE19_ARB",
    "TEXTURE20_ARB",
    "TEXTURE21_ARB",
    "TEXTURE22_ARB",
    "TEXTURE23_ARB",
    "TEXTURE24_ARB",
    "TEXTURE25_ARB",
    "TEXTURE26_ARB",
    "TEXTURE27_ARB",
    "TEXTURE28_ARB",
    "TEXTURE29_ARB",
    "TEXTURE30_ARB",
    "TEXTURE31_ARB",
];

/// The name at `x` in `list`, unless it is empty there.
fn pick(x: u32, list: &[&'static str]) -> Option<&'static str> {
    list.get(x as usize).copied().filter(|n| !n.is_empty())
}

/// A number: small ones in decimal, the rest in hexadecimal.
pub fn number(x: u32) -> String {
    if x < 0x1_0000 {
        x.to_string()
    } else {
        format!("0x{:X}", x)
    }
}

/// A float's bits as the float.
pub fn float(x: u32) -> String {
    let f = f32::from_bits(x);
    if f.fract() == 0.0 && f.abs() < 1e9 {
        format!("{}.0", f as i64)
    } else {
        f.to_string()
    }
}
